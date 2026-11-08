//! What signatures a file holds, read through the lazy reader. Facts only: nothing here looks at whether a signature holds up, and every string comes from the file, so it's cleaned and untrusted.
//!
//! The walk is bounded for hostile files: a field tree can loop, nest deeply or name millions of fields, and none of that may hang, overflow a stack or grow without limit. Memory is held to one frame per
//! level, each with at most as many children as there are visits left, and a name is shared rather than copied for each child. **What couldn't be read is never reported as absent:** an unreadable catalog
//! or form means `structure_read` is false, and an unreadable field counts in `unread`, so "none" is only ever said of a file that was really read.

use crate::{OpenError, clean_text, open_bytes, open_reader};
use lopdf::{Dictionary, IndexedReader, Object, ObjectId};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Most signature fields reported for one file.
pub const MAX_SIGNATURES: usize = 1024;
/// Most form fields visited looking for them.
pub const MAX_FIELD_VISITS: usize = 16_384;
/// Deepest field tree followed.
const MAX_FIELD_DEPTH: u8 = 32;
/// How long one walk may take. A field costs little, but a hostile file's references can be slow to chase, and this runs in the session's own process.
const MAX_TIME: Duration = Duration::from_secs(5);

/// What a signature field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureState {
    /// It holds a signature.
    Signed,
    /// It's waiting for one.
    Empty,
    /// It names a value that couldn't be read, so it can't be said to be either.
    Unread,
}

/// One signature field. Everything but the state and the numbers is text from the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureEntry {
    /// The field's full name, parents first.
    pub field: String,
    /// Zero-based page its widget says it's on.
    pub page: Option<u32>,
    pub state: SignatureState,
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

/// Every signature field found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureInventory {
    pub entries: Vec<SignatureEntry>,
    /// The catalog and the form could be read. If not, nothing can be said about what the file holds.
    pub structure_read: bool,
    /// The walk stopped at a limit, so these may not be all.
    pub truncated: bool,
    /// Objects along the way that couldn't be read, so there may be more.
    pub unread: u32,
    /// The catalog names a certification signature (`/Perms /DocMDP`).
    pub certified: bool,
}

/// Reads a file's signature fields and closes it. No handle stays open afterwards.
pub fn signatures(path: &Path) -> Result<SignatureInventory, OpenError> {
    signatures_within(path, MAX_TIME)
}

/// [`signatures`], with its own bound on time.
pub fn signatures_within(path: &Path, limit: Duration) -> Result<SignatureInventory, OpenError> {
    let reader = open_reader(path)?;
    Ok(read_inventory(&reader, Instant::now() + limit))
}

/// [`signatures`] on bytes in memory. The same walk, for a fuzzer.
pub fn signatures_in_bytes(bytes: Arc<[u8]>) -> Result<SignatureInventory, OpenError> {
    Ok(read_inventory(&open_bytes(bytes)?, Instant::now() + MAX_TIME))
}

/// A key of a dictionary, followed through a reference. Absent and unreadable are different answers, and that difference is the point.
enum Found {
    Absent,
    Unread,
    Value(Object),
}

fn lookup(reader: &IndexedReader, dict: &Dictionary, key: &[u8]) -> Found {
    let Ok(value) = dict.get(key) else { return Found::Absent };
    match value {
        Object::Null => Found::Absent,
        Object::Reference(id) => match reader.resolve_object(*id) {
            Ok(Object::Null) => Found::Absent,
            Ok(resolved) => Found::Value(resolved),
            Err(_) => Found::Unread,
        },
        direct => Found::Value(direct.clone()),
    }
}

/// One level of the field tree: its children, and how far through them the walk is.
struct Frame {
    kids: Vec<Object>,
    next: usize,
    depth: u8,
    name: Arc<str>,
    signature_type: bool,
}

fn read_inventory(reader: &IndexedReader, deadline: Instant) -> SignatureInventory {
    let mut inventory = SignatureInventory {
        entries: Vec::new(),
        structure_read: false,
        truncated: false,
        unread: 0,
        certified: false,
    };
    let Ok(Some(root)) = reader.trailer_entry_owned(b"Root") else {
        return inventory;
    };
    let Ok(root) = root.as_dict() else { return inventory };
    match lookup(reader, root, b"Perms") {
        Found::Absent => {}
        Found::Unread => inventory.unread += 1,
        Found::Value(perms) => inventory.certified = perms.as_dict().is_ok_and(|perms| perms.has(b"DocMDP")),
    }
    let form = match lookup(reader, root, b"AcroForm") {
        Found::Absent => {
            inventory.structure_read = true;
            return inventory;
        }
        Found::Unread => return inventory,
        Found::Value(form) => form,
    };
    let Ok(form) = form.as_dict() else { return inventory };
    let fields = match lookup(reader, form, b"Fields") {
        Found::Absent => {
            inventory.structure_read = true;
            return inventory;
        }
        Found::Unread => return inventory,
        Found::Value(fields) => fields,
    };
    let Object::Array(mut fields) = fields else { return inventory };
    inventory.structure_read = true;
    if fields.len() > MAX_FIELD_VISITS {
        fields.truncate(MAX_FIELD_VISITS);
        inventory.truncated = true;
    }

    // Only when there's a field to place. It costs a walk of the page tree.
    let pages: OnceLock<HashMap<ObjectId, u32>> = OnceLock::new();
    let file_bytes = reader.source_len();
    let mut seen = HashSet::new();
    let mut visits = 0;
    let mut stack = vec![Frame {
        kids: fields,
        next: 0,
        depth: 0,
        name: Arc::from(""),
        signature_type: false,
    }];
    // An explicit stack, so a deep file can't overflow the thread's. Children are left in place and taken one at a time, so a wide array costs itself and nothing per child.
    while let Some(frame) = stack.last_mut() {
        let Some(kid) = frame.kids.get_mut(frame.next) else {
            stack.pop();
            continue;
        };
        let kid = std::mem::replace(kid, Object::Null);
        frame.next += 1;
        let (depth, parent_name, parent_signature) = (frame.depth, Arc::clone(&frame.name), frame.signature_type);
        visits += 1;
        if visits > MAX_FIELD_VISITS || inventory.entries.len() >= MAX_SIGNATURES || Instant::now() >= deadline {
            inventory.truncated = true;
            break;
        }
        let mut node = match kid {
            Object::Reference(id) => {
                // A file can loop its field tree back on itself.
                if !seen.insert(id) {
                    continue;
                }
                match reader.resolve_object(id) {
                    Ok(Object::Dictionary(node)) => node,
                    Ok(Object::Null) => continue,
                    _ => {
                        inventory.unread += 1;
                        continue;
                    }
                }
            }
            Object::Dictionary(node) => node,
            Object::Null => continue,
            _ => {
                inventory.unread += 1;
                continue;
            }
        };
        let name = joined_name(reader, &parent_name, &node);
        let signature_type = match lookup(reader, &node, b"FT") {
            Found::Absent => parent_signature,
            Found::Unread => {
                inventory.unread += 1;
                parent_signature
            }
            Found::Value(kind) => kind.as_name().is_ok_and(|kind| kind == b"Sig"),
        };
        // Taken out of the field rather than copied: this array can be megabytes.
        let kids = match node.remove(b"Kids") {
            None | Some(Object::Null) => None,
            Some(Object::Array(kids)) => Some(kids),
            Some(Object::Reference(id)) => match reader.resolve_object(id) {
                Ok(Object::Array(kids)) => Some(kids),
                Ok(Object::Null) => None,
                _ => {
                    inventory.unread += 1;
                    None
                }
            },
            Some(_) => {
                inventory.unread += 1;
                None
            }
        };
        let value = if signature_type { lookup(reader, &node, b"V") } else { Found::Absent };
        // A field holding a value is one signature, however many widgets show it. An empty one with children is reached through them.
        let holds_value = !matches!(value, Found::Absent);
        if signature_type && (holds_value || kids.as_ref().is_none_or(Vec::is_empty)) {
            let pages = pages.get_or_init(|| page_numbers(reader));
            inventory
                .entries
                .push(entry(reader, &node, &value, kids.as_deref().unwrap_or(&[]), &name, pages, file_bytes));
            if matches!(inventory.entries.last(), Some(found) if found.state == SignatureState::Unread) {
                inventory.unread += 1;
            }
            continue;
        }
        let Some(mut kids) = kids else { continue };
        if depth >= MAX_FIELD_DEPTH {
            inventory.truncated = true;
            continue;
        }
        // Nothing past what's left to visit can ever be looked at, so it isn't kept.
        let left = MAX_FIELD_VISITS.saturating_sub(visits);
        if kids.len() > left {
            kids.truncate(left);
            inventory.truncated = true;
        }
        stack.push(Frame {
            kids,
            next: 0,
            depth: depth + 1,
            name,
            signature_type,
        });
    }
    inventory
}

fn page_numbers(reader: &IndexedReader) -> HashMap<ObjectId, u32> {
    reader.page_map().ok().map_or_else(HashMap::new, |map| {
        map.iter()
            .enumerate()
            .filter_map(|(index, page)| Some((page.id(), u32::try_from(index).ok()?)))
            .collect()
    })
}

fn joined_name(reader: &IndexedReader, parent: &str, node: &Dictionary) -> Arc<str> {
    let own = match lookup(reader, node, b"T") {
        Found::Value(title) => title.as_str().map(clean_text).unwrap_or_default(),
        _ => String::new(),
    };
    match (parent.is_empty(), own.is_empty()) {
        (true, _) => own.into(),
        (false, true) => parent.into(),
        (false, false) => crate::clean_line(&format!("{parent}.{own}")).into(),
    }
}

fn entry(
    reader: &IndexedReader,
    node: &Dictionary,
    value: &Found,
    kids: &[Object],
    field: &str,
    pages: &HashMap<ObjectId, u32>,
    file_bytes: u64,
) -> SignatureEntry {
    let page_of = |dict: &Dictionary| {
        dict.get(b"P")
            .ok()
            .and_then(|page| page.as_reference().ok())
            .and_then(|id| pages.get(&id).copied())
    };
    let page = page_of(node).or_else(|| {
        kids.iter().take(8).find_map(|kid| match kid {
            Object::Reference(id) => match reader.resolve_object(*id) {
                Ok(Object::Dictionary(dict)) => page_of(&dict),
                _ => None,
            },
            Object::Dictionary(dict) => page_of(dict),
            _ => None,
        })
    });
    let state = match value {
        Found::Absent => SignatureState::Empty,
        Found::Value(Object::Dictionary(_)) => SignatureState::Signed,
        // A value that's gone, or isn't what a signature's value is, can't be called signed or empty.
        Found::Unread | Found::Value(_) => SignatureState::Unread,
    };
    let mut out = SignatureEntry {
        field: field.to_string(),
        page,
        state,
        filter: None,
        sub_filter: None,
        stated_name: None,
        stated_reason: None,
        stated_location: None,
        stated_time: None,
        byte_range: None,
        bytes_after_range: None,
    };
    let Found::Value(Object::Dictionary(value)) = value else { return out };
    out.filter = name_of(reader, value, b"Filter");
    out.sub_filter = name_of(reader, value, b"SubFilter");
    out.stated_name = text_of(reader, value, b"Name");
    out.stated_reason = text_of(reader, value, b"Reason");
    out.stated_location = text_of(reader, value, b"Location");
    out.stated_time = text_of(reader, value, b"M");
    out.byte_range = byte_range(reader, value);
    out.bytes_after_range = out
        .byte_range
        .and_then(|range| range[2].checked_add(range[3]))
        .map(|end| file_bytes.saturating_sub(end));
    out
}

fn name_of(reader: &IndexedReader, dict: &Dictionary, key: &[u8]) -> Option<String> {
    let Found::Value(value) = lookup(reader, dict, key) else { return None };
    Some(crate::clean_line(&String::from_utf8_lossy(value.as_name().ok()?))).filter(|text| !text.is_empty())
}

fn text_of(reader: &IndexedReader, dict: &Dictionary, key: &[u8]) -> Option<String> {
    let Found::Value(value) = lookup(reader, dict, key) else { return None };
    Some(clean_text(value.as_str().ok()?)).filter(|text| !text.is_empty())
}

fn byte_range(reader: &IndexedReader, dict: &Dictionary) -> Option<[u64; 4]> {
    let Found::Value(value) = lookup(reader, dict, b"ByteRange") else {
        return None;
    };
    let [first, second, third, fourth] = value.as_array().ok()?.as_slice() else {
        return None;
    };
    let number = |object: &Object| object.as_i64().ok().and_then(|number| u64::try_from(number).ok());
    Some([number(first)?, number(second)?, number(third)?, number(fourth)?])
}
