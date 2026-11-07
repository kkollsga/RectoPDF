//! What signatures a file holds, read through the lazy reader. Facts only: nothing here looks at whether a signature holds up, and every string comes from the file, so it's cleaned and untrusted.
//!
//! The walk is bounded for hostile files: a field tree can loop, nest deeply or name millions of fields, and none of that may hang, overflow a stack or grow without limit.

use crate::{OpenError, clean_text, deref, dict_of, open_reader};
use lopdf::{Dictionary, IndexedReader, Object, ObjectId};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Most signature fields reported for one file.
pub const MAX_SIGNATURES: usize = 1024;
/// Most form fields visited looking for them.
const MAX_FIELD_VISITS: usize = 16_384;
/// Deepest field tree followed.
const MAX_FIELD_DEPTH: u8 = 32;

/// One signature field. Everything but `signed` and the numbers is text from the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignatureEntry {
    /// The field's full name, parents first.
    pub field: String,
    /// Zero-based page its widget says it's on.
    pub page: Option<u32>,
    /// Whether the field holds a signature, rather than waiting for one.
    pub signed: bool,
    pub filter: Option<String>,
    pub sub_filter: Option<String>,
    /// The name the signature dictionary states, which nothing has confirmed.
    pub stated_name: Option<String>,
    pub stated_reason: Option<String>,
    pub stated_location: Option<String>,
    pub stated_time: Option<String>,
    /// The `/ByteRange` as written: start and length, twice.
    pub byte_range: Option<[u64; 4]>,
    /// File bytes past the end of that range, which means something was added after this signature was made. Says nothing about whether what was added matters.
    pub bytes_after_range: Option<u64>,
}

/// Every signature field found. `truncated` says the walk stopped at a limit, so "none" or "these" may not be all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignatureInventory {
    pub entries: Vec<SignatureEntry>,
    pub truncated: bool,
    /// The catalog names a certification signature (`/Perms /DocMDP`).
    pub certified: bool,
}

/// Reads a file's signature fields and closes it. No handle stays open afterwards.
pub fn signatures(path: &Path) -> Result<SignatureInventory, OpenError> {
    let reader = open_reader(path)?;
    Ok(read_inventory(&reader).unwrap_or_default())
}

fn read_inventory(reader: &IndexedReader) -> Option<SignatureInventory> {
    let root = reader.trailer_entry_owned(b"Root").ok()??;
    let root = root.as_dict().ok()?;
    let certified = root.get(b"Perms").ok().and_then(|perms| deref(reader, perms)).is_some_and(|perms| perms.as_dict().is_ok_and(|perms| perms.has(b"DocMDP")));
    let mut inventory = SignatureInventory { certified, ..SignatureInventory::default() };
    let Some(form) = root.get(b"AcroForm").ok().and_then(|form| deref(reader, form)) else { return Some(inventory) };
    let Some(fields) = form.as_dict().ok().and_then(|form| form.get(b"Fields").ok()).and_then(|fields| deref(reader, fields)) else { return Some(inventory) };
    let Ok(fields) = fields.as_array() else { return Some(inventory) };

    let pages: HashMap<ObjectId, u32> = reader.page_map().ok().map_or_else(HashMap::new, |map| map.iter().enumerate().filter_map(|(index, page)| Some((page.id(), u32::try_from(index).ok()?))).collect());
    let file_bytes = reader.source_len();
    let mut seen = HashSet::new();
    let mut visits = 0;
    // An explicit stack, so a deep file can't overflow the thread's. Reversed so the report keeps the file's order.
    let mut stack: Vec<Pending> = fields.iter().rev().filter_map(Object::as_reference_opt).map(|id| Pending { id, depth: 0, name: String::new(), signature_type: false }).collect();
    while let Some(pending) = stack.pop() {
        visits += 1;
        if visits > MAX_FIELD_VISITS || inventory.entries.len() >= MAX_SIGNATURES {
            inventory.truncated = true;
            break;
        }
        // A file can loop its field tree back on itself.
        if !seen.insert(pending.id) {
            continue;
        }
        let Some(node) = dict_of(reader, pending.id) else { continue };
        let name = joined_name(reader, &pending.name, &node);
        let signature_type = node.get(b"FT").ok().and_then(|kind| deref(reader, kind)).map_or(pending.signature_type, |kind| kind.as_name().is_ok_and(|kind| kind == b"Sig"));
        let kids = node.get(b"Kids").ok().and_then(|kids| deref(reader, kids));
        let kids = kids.as_ref().and_then(|kids| kids.as_array().ok());
        let value = signature_type.then(|| node.get(b"V").ok().and_then(|value| deref(reader, value))).flatten();
        // A field holding a value is one signature, however many widgets show it. An empty one with children is reached through them.
        if signature_type && (value.is_some() || kids.is_none_or(|kids| kids.is_empty())) {
            inventory.entries.push(entry(reader, &node, value.as_ref(), kids.map_or(&[], Vec::as_slice), name, &pages, file_bytes));
            continue;
        }
        if pending.depth < MAX_FIELD_DEPTH {
            if let Some(kids) = kids {
                stack.extend(kids.iter().rev().filter_map(Object::as_reference_opt).map(|id| Pending { id, depth: pending.depth + 1, name: name.clone(), signature_type }));
            }
        } else if kids.is_some() {
            inventory.truncated = true;
        }
    }
    Some(inventory)
}

struct Pending {
    id: ObjectId,
    depth: u8,
    name: String,
    signature_type: bool,
}

trait ReferenceOpt {
    fn as_reference_opt(&self) -> Option<ObjectId>;
}

impl ReferenceOpt for Object {
    fn as_reference_opt(&self) -> Option<ObjectId> {
        self.as_reference().ok()
    }
}

fn joined_name(reader: &IndexedReader, parent: &str, node: &Dictionary) -> String {
    let own = node.get(b"T").ok().and_then(|title| deref(reader, title)).and_then(|title| title.as_str().ok().map(clean_text)).unwrap_or_default();
    match (parent.is_empty(), own.is_empty()) {
        (true, _) => own,
        (false, true) => parent.to_string(),
        (false, false) => crate::clean_line(&format!("{parent}.{own}")),
    }
}

fn entry(reader: &IndexedReader, node: &Dictionary, value: Option<&Object>, kids: &[Object], field: String, pages: &HashMap<ObjectId, u32>, file_bytes: u64) -> SignatureEntry {
    let page_of = |dict: &Dictionary| dict.get(b"P").ok().and_then(|page| page.as_reference().ok()).and_then(|id| pages.get(&id).copied());
    let page = page_of(node).or_else(|| kids.iter().filter_map(Object::as_reference_opt).take(8).find_map(|id| dict_of(reader, id).as_ref().and_then(page_of)));
    let mut out = SignatureEntry { field, page, signed: value.is_some(), ..SignatureEntry::default() };
    let Some(value) = value.and_then(|value| value.as_dict().ok()) else { return out };
    out.filter = name_of(reader, value, b"Filter");
    out.sub_filter = name_of(reader, value, b"SubFilter");
    out.stated_name = text_of(reader, value, b"Name");
    out.stated_reason = text_of(reader, value, b"Reason");
    out.stated_location = text_of(reader, value, b"Location");
    out.stated_time = text_of(reader, value, b"M");
    out.byte_range = byte_range(reader, value);
    out.bytes_after_range = out.byte_range.and_then(|range| range[2].checked_add(range[3])).map(|end| file_bytes.saturating_sub(end));
    out
}

fn name_of(reader: &IndexedReader, dict: &Dictionary, key: &[u8]) -> Option<String> {
    let value = deref(reader, dict.get(key).ok()?)?;
    Some(crate::clean_line(&String::from_utf8_lossy(value.as_name().ok()?))).filter(|text| !text.is_empty())
}

fn text_of(reader: &IndexedReader, dict: &Dictionary, key: &[u8]) -> Option<String> {
    let value = deref(reader, dict.get(key).ok()?)?;
    Some(clean_text(value.as_str().ok()?)).filter(|text| !text.is_empty())
}

fn byte_range(reader: &IndexedReader, dict: &Dictionary) -> Option<[u64; 4]> {
    let value = deref(reader, dict.get(b"ByteRange").ok()?)?;
    let [first, second, third, fourth] = value.as_array().ok()?.as_slice() else { return None };
    let number = |object: &Object| object.as_i64().ok().and_then(|number| u64::try_from(number).ok());
    Some([number(first)?, number(second)?, number(third)?, number(fourth)?])
}
