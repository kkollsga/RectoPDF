//! What a save copies in from added PDFs: finding what their pages need, renumbering it, and writing it. Out of `save.rs` unchanged (R4.3), so
//! the save had room for highlights.

use super::*;

impl Plan {
    /// What's copied from the added PDFs besides their pages, renumbered. Out of [`Plan::write`] unchanged (R4.3).
    pub(super) fn write_copies<'c>(
        &self,
        sources: &'c [Opened],
        renumbered: &[HashMap<u32, ObjectId>],
        used_copy: &[HashSet<u32>],
        writer: &mut impl Sink<'c>,
    ) -> Result<(), SaveError> {
        // What's copied besides the pages themselves.
        for (at, copy) in self.copies.iter().enumerate() {
            let (Some(copy), Some((source, source_map))) = (copy, sources.get(at).and_then(Option::as_ref)) else {
                continue;
            };
            let pages: HashSet<u32> = source_map.iter().map(|entry| entry.id().0).collect();
            let numbers = &renumbered[at];
            for id in &copy.order {
                if writer.taken() > self.append_cap {
                    return Err(SaveError::TooLarge);
                }
                if pages.contains(&id.0) {
                    if !used_copy[at].contains(&id.0) {
                        // A page found only as something another page names, such as a link's target, isn't in this document: it goes as nothing, as remap has it. Its number was taken, so it's written as null.
                        writer.put(numbers[&id.0], Object::Null)?;
                    }
                    continue;
                }
                let new_id = numbers[&id.0];
                if let Ok(descriptor) = source.resolve_stream_descriptor(*id) {
                    let mut dict = match remap(&Object::Dictionary(descriptor.dictionary().clone()), copy, numbers) {
                        Some(Object::Dictionary(dict)) => dict,
                        _ => return Err(SaveError::Damaged),
                    };
                    dict.remove(b"Length");
                    let length = descriptor.encoded_len().ok_or(SaveError::Damaged)?;
                    drop(descriptor);
                    writer.put_stream(new_id, dict, length, source, *id)?;
                } else {
                    let object = source.resolve_object(*id).map_err(|_| SaveError::Damaged)?;
                    let object = remap_copied(id.0, object, copy, numbers).unwrap_or(Object::Null);
                    writer.put(new_id, object)?;
                }
            }
        }
        Ok(())
    }
}

/// What an annotation names that belongs to its old document: its page, its form field and its place in the structure. Without them it still draws, from its own appearance.
pub(super) fn is_annotation(dict: &Dictionary) -> bool {
    dict.has(b"Rect") && dict.has(b"Subtype")
}

/// Whether this entry of a dictionary is left out of a copy.
pub(super) fn left_behind(annotation: bool, key: &[u8]) -> bool {
    annotation && matches!(key, b"P" | b"Parent" | b"StructParent")
}

/// Everything the wanted pages of an added PDF reach, found before anything's written. Another page of that PDF, its page tree and its catalog aren't followed: a link or a bookmark would otherwise bring its whole document along. What can't be read refuses the save. **Bounded for hostile files:** an id is queued once, when it's first seen, and every value looked at counts against [`MAX_COPY_WORK`], so a page that names millions of references, or objects that each name all the others, is refused with [`SaveError::TooLarge`] rather than held. Nothing is cloned to be looked at.
pub(super) fn discover(
    source: &IndexedReader,
    map: &lopdf::PageMap,
    wanted: &[u32],
    missing: &mut u64,
    room: usize,
    work: &mut u64,
) -> Result<Copy, SaveError> {
    let wanted_ids: HashSet<u32> = wanted.iter().filter_map(|page| map.get(*page as usize)).map(|entry| entry.id().0).collect();
    let all_pages: HashSet<u32> = map.iter().map(|entry| entry.id().0).collect();
    let mut copy = Copy {
        fields: form_fields(source, map, wanted, work)?,
        ..Copy::default()
    };
    let mut queued: HashSet<u32> = HashSet::new();
    let mut stack: Vec<ObjectId> = Vec::new();
    // A signature widget doesn't come along, nor does what only it names, its value with its byte range among them.
    for widget in &copy.fields.signature_widgets {
        copy.map.insert(*widget, None);
        queued.insert(*widget);
    }
    // The fonts the fields' default appearances name come along, for the form's own resources.
    if let Some(fonts) = &copy.fields.fonts {
        references_in(&Object::Dictionary(fonts.clone()), &mut stack, &mut queued, work)?;
    }
    for page in wanted {
        let entry = map.get(*page as usize).ok_or(SaveError::Damaged)?;
        if copy.order.len() >= room {
            return Err(SaveError::TooLarge);
        }
        if copy.map.insert(entry.id().0, Some(())).is_none() {
            queued.insert(entry.id().0);
            copy.order.push(entry.id());
            let dict = copied_page(source, entry)?;
            references_in(&Object::Dictionary(dict), &mut stack, &mut queued, work)?;
        }
    }
    while let Some(id) = stack.pop() {
        if copy.order.len() >= room {
            return Err(SaveError::TooLarge);
        }
        if all_pages.contains(&id.0) && !wanted_ids.contains(&id.0) {
            copy.map.insert(id.0, None);
            continue;
        }
        let object = match source.resolve_stream_descriptor(id) {
            Ok(descriptor) => {
                if descriptor.encoded_len().is_none() {
                    return Err(SaveError::Damaged);
                }
                Object::Dictionary(descriptor.dictionary().clone())
            }
            // A number the file's cross-reference data doesn't have is null, by ISO 32000, as every viewer reads it: it goes as nothing, and is counted. One that's
            // there and can't be read, as one past the 1 MiB limit, refuses the save: leaving it out would change the page unasked.
            Err(_) if source.object_location(id).is_none() => {
                *missing += 1;
                copy.map.insert(id.0, None);
                continue;
            }
            Err(_) => source.resolve_object(id).map_err(|_| SaveError::Damaged)?,
        };
        if let Object::Dictionary(dict) = &object
            && (dict.has_type(b"Pages") || dict.has_type(b"Catalog") || dict.has_type(b"Page"))
        {
            copy.map.insert(id.0, None);
            continue;
        }
        copy.map.insert(id.0, Some(()));
        copy.order.push(id);
        let object = copy.fields.adjusted(id.0, object);
        if copy.fields.keep_parent.contains(&id.0) {
            references_kept_parent(&object, &mut stack, &mut queued, work)?;
        } else {
            references_in(&object, &mut stack, &mut queued, work)?;
        }
    }
    Ok(copy)
}

/// Most values one save looks at and writes in copying pages from other PDFs: every number, name, reference and entry counts. Far past any real page, which has thousands; a hostile one that names millions is refused.
pub const MAX_COPY_WORK: u64 = 16_000_000;

pub(super) fn charge(work: &mut u64, by: u64) -> Result<(), SaveError> {
    *work = work.saturating_add(by);
    if *work > MAX_COPY_WORK { Err(SaveError::TooLarge) } else { Ok(()) }
}

/// The references an object holds, as it will be written, each queued the first time it's seen: an annotation's ties to its old document aren't among them.
pub(super) fn references_in(object: &Object, out: &mut Vec<ObjectId>, queued: &mut HashSet<u32>, work: &mut u64) -> Result<(), SaveError> {
    charge(work, 1)?;
    match object {
        Object::Reference(id) => {
            if queued.insert(id.0) {
                out.push(*id);
            }
        }
        Object::Array(items) => {
            for item in items {
                references_in(item, out, queued, work)?;
            }
        }
        Object::Dictionary(dict) => references_in_dict(dict, out, queued, work)?,
        Object::Stream(stream) => references_in_dict(&stream.dict, out, queued, work)?,
        _ => {}
    }
    Ok(())
}

/// As [`references_in`], for a widget or field whose field comes along: its `/Parent` is followed.
pub(super) fn references_kept_parent(object: &Object, out: &mut Vec<ObjectId>, queued: &mut HashSet<u32>, work: &mut u64) -> Result<(), SaveError> {
    let Object::Dictionary(dict) = object else {
        return references_in(object, out, queued, work);
    };
    let annotation = is_annotation(dict);
    for (key, value) in dict.iter() {
        if key.as_slice() == b"Parent" || !left_behind(annotation, key) {
            references_in(value, out, queued, work)?;
        }
    }
    Ok(())
}

pub(super) fn references_in_dict(dict: &Dictionary, out: &mut Vec<ObjectId>, queued: &mut HashSet<u32>, work: &mut u64) -> Result<(), SaveError> {
    let annotation = is_annotation(dict);
    for (key, value) in dict.iter() {
        if !left_behind(annotation, key) {
            references_in(value, out, queued, work)?;
        }
    }
    Ok(())
}

/// An object as it's written into the new file: references renumbered, and those to what doesn't come along dropped, from a dictionary as an entry and from an array as `null`, keeping the other places. `None` if the object itself is one that goes. Builds the copy once, cloning nothing on the way down.
pub(super) fn remap(object: &Object, copy: &Copy, numbers: &HashMap<u32, ObjectId>) -> Option<Object> {
    match object {
        Object::Reference(id) => match copy.map.get(&id.0) {
            Some(Some(())) => numbers.get(&id.0).map(|id| Object::Reference(*id)),
            _ => None,
        },
        Object::Array(items) => Some(Object::Array(
            items.iter().map(|item| remap(item, copy, numbers).unwrap_or(Object::Null)).collect(),
        )),
        Object::Dictionary(dict) => Some(Object::Dictionary(remap_dict(dict, copy, numbers))),
        other => Some(other.clone()),
    }
}

/// An object copied as itself, `id` in its source: as [`remap`], with its field's changes, and its `/Parent` kept if its field comes along.
pub(super) fn remap_copied(id: u32, object: Object, copy: &Copy, numbers: &HashMap<u32, ObjectId>) -> Option<Object> {
    let object = copy.fields.adjusted(id, object);
    match (&object, copy.fields.keep_parent.contains(&id)) {
        (Object::Dictionary(dict), true) => Some(Object::Dictionary(remap_dict_keeping(dict, copy, numbers, true))),
        _ => remap(&object, copy, numbers),
    }
}

pub(super) fn remap_dict(dict: &Dictionary, copy: &Copy, numbers: &HashMap<u32, ObjectId>) -> Dictionary {
    remap_dict_keeping(dict, copy, numbers, false)
}

pub(super) fn remap_dict_keeping(dict: &Dictionary, copy: &Copy, numbers: &HashMap<u32, ObjectId>, keep_parent: bool) -> Dictionary {
    let annotation = is_annotation(dict);
    let mut out = Dictionary::new();
    for (key, value) in dict.iter() {
        if (!left_behind(annotation, key) || (keep_parent && key.as_slice() == b"Parent"))
            && let Some(value) = remap(value, copy, numbers)
        {
            out.set(key.clone(), value);
        }
    }
    out
}
