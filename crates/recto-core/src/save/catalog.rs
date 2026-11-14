//! What a save writes into the catalog: the added PDFs' layers and form fields, and usage rights gone. Out of `save.rs` unchanged (R4.3), so
//! the save had room for highlights.

use super::*;

/// **What the added PDFs bring to the catalog:** the layers their pages draw with, and which of them their own files hide, so the saved file hides them
/// too; and their top form fields, with the fonts their appearances name. Out of [`Plan::write`] unchanged (R4.3).
pub(super) fn catalog_additions(copies: &[Option<Copy>], sources: &[Opened], renumbered: &[HashMap<u32, ObjectId>]) -> (Vec<(ObjectId, bool)>, FormAdditions) {
    // Layers the added pages draw with, and which of them their own files hide, so the saved file hides them too.
    let mut layers: Vec<(ObjectId, bool)> = Vec::new();
    for (at, copy) in copies.iter().enumerate() {
        let (Some(_), Some((source, _))) = (copy, sources.get(at).and_then(Option::as_ref)) else {
            continue;
        };
        let (all, off) = layer_states(source);
        let numbers = &renumbered[at];
        for id in all {
            if let Some(new_id) = numbers.get(&id) {
                layers.push((*new_id, off.contains(&id)));
            }
        }
    }
    // The added pages' top fields join the form, with the fonts their appearances name.
    let mut form = FormAdditions::default();
    for (at, copy) in copies.iter().enumerate() {
        let Some(copy) = copy else { continue };
        let numbers = &renumbered[at];
        form.fields.extend(copy.fields.tops.iter().filter_map(|top| numbers.get(&top.0).copied()));
        if let Some(Object::Dictionary(fonts)) = copy
            .fields
            .fonts
            .as_ref()
            .and_then(|fonts| remap(&Object::Dictionary(fonts.clone()), copy, numbers))
        {
            for (name, font) in fonts.iter() {
                if !form.fonts.has(name) {
                    form.fonts.set(name.clone(), font.clone());
                }
            }
        }
    }
    (layers, form)
}

/// A file's layers, by object number, and which of them it hides by default: `/OFF`, or all but `/ON` when its base state is off. Nothing for a file with none or one that can't be read, which then hides nothing.
fn layer_states(reader: &IndexedReader) -> (Vec<u32>, HashSet<u32>) {
    let Some(properties) = catalog(reader)
        .and_then(|root| root.get(b"OCProperties").ok().cloned())
        .and_then(|value| dict_value(reader, &value))
    else {
        return (Vec::new(), HashSet::new());
    };
    let ids = |value: Option<&Object>| -> Vec<u32> {
        match value.cloned().map(|value| match value {
            Object::Reference(id) => reader.resolve_object(id).unwrap_or(Object::Null),
            other => other,
        }) {
            Some(Object::Array(items)) => items
                .iter()
                .filter_map(|item| item.as_reference().ok().map(|id| id.0))
                .take(MAX_COPIED_OBJECTS)
                .collect(),
            _ => Vec::new(),
        }
    };
    let all = ids(properties.get(b"OCGs").ok());
    let defaults = properties
        .get(b"D")
        .ok()
        .cloned()
        .and_then(|value| dict_value(reader, &value))
        .unwrap_or_default();
    let base_off = matches!(defaults.get(b"BaseState"), Ok(Object::Name(name)) if name == b"OFF");
    let off: HashSet<u32> = if base_off {
        let on: HashSet<u32> = ids(defaults.get(b"ON").ok()).into_iter().collect();
        all.iter().copied().filter(|id| !on.contains(id)).collect()
    } else {
        ids(defaults.get(b"OFF").ok()).into_iter().collect()
    };
    (all, off)
}

/// **A new version of the catalog,** for what a save changes there. Usage rights go: any page edit ends them, a reader that finds them broken says so, and they aren't a person's signature (owner, 2026-11-08). Layers of added pages join the file's own, with the ones their files hide hidden here too: otherwise a layer a file hides, which can be what it keeps out of sight, would show on its page in this one.
pub(super) fn update_catalog<'c>(
    reader: &IndexedReader,
    writer: &mut impl Sink<'c>,
    drop_rights: bool,
    layers: &[(ObjectId, bool)],
    form: &FormAdditions,
) -> Result<(), SaveError> {
    let root = catalog_id(reader).ok_or(SaveError::Damaged)?;
    let mut catalog = page_dict(reader, root)?;
    if !form.fields.is_empty() {
        // The form is a dictionary of its own or written in the catalog: a new version of whichever it is.
        let own = catalog.get(b"AcroForm").ok().and_then(|form| form.as_reference().ok());
        let mut acro = catalog
            .get(b"AcroForm")
            .ok()
            .cloned()
            .and_then(|form| dict_value(reader, &form))
            .unwrap_or_default();
        // A list that's there and can't be read isn't replaced: the plan has left these fields out already, and this holds to it whatever's happened since.
        let mut fields = match acro.get(b"Fields") {
            Ok(Object::Reference(id)) => match reader.resolve_object(*id) {
                Ok(Object::Array(items)) => items,
                _ => return Err(SaveError::Damaged),
            },
            Ok(Object::Array(items)) => items.clone(),
            Ok(_) => return Err(SaveError::Damaged),
            Err(_) => Vec::new(),
        };
        fields.extend(form.fields.iter().map(|id| Object::Reference(*id)));
        acro.set("Fields", Object::Array(fields));
        if form.fonts.iter().next().is_some() {
            let mut resources = acro.get(b"DR").ok().cloned().and_then(|dr| dict_value(reader, &dr)).unwrap_or_default();
            let mut fonts = resources
                .get(b"Font")
                .ok()
                .cloned()
                .and_then(|fonts| dict_value(reader, &fonts))
                .unwrap_or_default();
            for (name, font) in form.fonts.iter() {
                if !fonts.has(name) {
                    fonts.set(name.clone(), font.clone());
                }
            }
            resources.set("Font", Object::Dictionary(fonts));
            acro.set("DR", Object::Dictionary(resources));
        }
        match own {
            Some(id) => writer.put(id, Object::Dictionary(acro))?,
            None => catalog.set("AcroForm", Object::Dictionary(acro)),
        }
    }
    if drop_rights && let Some(mut perms) = catalog.get(b"Perms").ok().cloned().and_then(|perms| dict_value(reader, &perms)) {
        perms.remove(b"UR3");
        perms.remove(b"UR");
        if perms.iter().next().is_none() {
            catalog.remove(b"Perms");
        } else {
            catalog.set("Perms", Object::Dictionary(perms));
        }
    }
    if !layers.is_empty() {
        let mut properties = catalog
            .get(b"OCProperties")
            .ok()
            .cloned()
            .and_then(|value| dict_value(reader, &value))
            .unwrap_or_default();
        let resolved = |value: Option<&Object>| -> Vec<Object> {
            match value.cloned().map(|value| match value {
                Object::Reference(id) => reader.resolve_object(id).unwrap_or(Object::Null),
                other => other,
            }) {
                Some(Object::Array(items)) => items,
                _ => Vec::new(),
            }
        };
        let mut all = resolved(properties.get(b"OCGs").ok());
        all.extend(layers.iter().map(|(id, _)| Object::Reference(*id)));
        properties.set("OCGs", Object::Array(all));
        let mut defaults = properties
            .get(b"D")
            .ok()
            .cloned()
            .and_then(|value| dict_value(reader, &value))
            .unwrap_or_default();
        let base_off = matches!(defaults.get(b"BaseState"), Ok(Object::Name(name)) if name == b"OFF");
        // With everything off unless it's listed on, it's the shown ones that need listing.
        let (key, wanted) = if base_off { (&b"ON"[..], false) } else { (&b"OFF"[..], true) };
        let mut listed = resolved(defaults.get(key).ok());
        listed.extend(layers.iter().filter(|(_, off)| *off == wanted).map(|(id, _)| Object::Reference(*id)));
        defaults.set(key, Object::Array(listed));
        properties.set("D", Object::Dictionary(defaults));
        catalog.set("OCProperties", Object::Dictionary(properties));
    }
    writer.put(root, Object::Dictionary(catalog))?;
    Ok(())
}
