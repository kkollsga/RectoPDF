//! What a page uses that PDFium's objects can't show, read from the file's own structure through the lazy reader. The SVG export asks this before it starts, and a yes only means it looks harder or uses a picture: none of it is ever a claim about the file.
//!
//! The walk is bounded for hostile files: forms inside forms, loops and millions of entries all end at a fixed number of visits, and past that, or for anything that can't be read, the answer is `None`, which the caller takes as "may use anything". A stream's dictionary is read without its bytes, so a form on a page with a 300 MiB image costs no more than one without.

use crate::open_reader;
use lopdf::{Dictionary, IndexedReader, Object, ObjectId};
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

/// Most dictionaries looked at for one page.
const MAX_VISITS: usize = 8192;
/// How long one look may take. This runs on a job's own thread, but a hostile file's references shouldn't cost more than the page.
const MAX_TIME: Duration = Duration::from_secs(1);

/// What's on a page, as far as its structure says, down through its forms and patterns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageEffects {
    /// A graphics state with a soft mask.
    pub soft_mask: bool,
    /// A graphics state with a blend mode other than Normal.
    pub blend: bool,
    /// A pattern that's a shading.
    pub shading_pattern: bool,
    /// The document has optional content that starts off, or changes by how it's viewed.
    pub hidden_layers: bool,
}

/// What this page uses, or `None` if that can't be said. `page` is zero-based.
pub fn page_effects(path: &Path, page: u32) -> Option<PageEffects> {
    let reader = open_reader(path).ok()?;
    let map = reader.page_map().ok()?;
    let entry = map.get(page as usize)?;
    let owner = entry.inherited().resources().unwrap_or(entry.id());
    let page_dict = dict_at(&reader, owner)?;
    let resources = value_dict(&reader, page_dict.get(b"Resources").ok()?)?;
    let deadline = Instant::now() + MAX_TIME;
    let mut effects = PageEffects { hidden_layers: has_hidden_layers(&reader), ..PageEffects::default() };
    let mut stack = vec![resources];
    let mut seen = HashSet::new();
    let mut visits = 0;
    while let Some(dict) = stack.pop() {
        visits += 1;
        if visits > MAX_VISITS || Instant::now() >= deadline {
            return None;
        }
        if let Some(states) = entry_dict(&reader, &dict, b"ExtGState") {
            for (_, state) in states.iter() {
                visits += 1;
                if let Some(state) = value_dict(&reader, state) {
                    effects.soft_mask |= has_soft_mask(&reader, &state);
                    effects.blend |= has_blend(&state);
                }
            }
        }
        for (key, is_pattern) in [(&b"XObject"[..], false), (&b"Pattern"[..], true)] {
            let Some(entries) = entry_dict(&reader, &dict, key) else { continue };
            for (_, value) in entries.iter() {
                visits += 1;
                if visits > MAX_VISITS {
                    return None;
                }
                if let Object::Reference(id) = value
                    && !seen.insert(*id)
                {
                    continue;
                }
                let Some(inner) = stream_or_dict(&reader, value) else { continue };
                if is_pattern && inner.get(b"PatternType").and_then(Object::as_i64).is_ok_and(|kind| kind == 2) {
                    effects.shading_pattern = true;
                }
                if let Some(next) = entry_dict(&reader, &inner, b"Resources") {
                    stack.push(next);
                }
                // A soft mask's group has resources of its own, and its forms can have soft masks.
            }
        }
    }
    Some(effects)
}

fn dict_at(reader: &IndexedReader, id: ObjectId) -> Option<Dictionary> {
    match reader.resolve_object(id).ok()? {
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

/// A dictionary, or a stream's dictionary without reading its bytes.
fn stream_or_dict(reader: &IndexedReader, value: &Object) -> Option<Dictionary> {
    match value {
        Object::Dictionary(dict) => Some(dict.clone()),
        Object::Reference(id) => match reader.resolve_stream_descriptor(*id) {
            Ok(descriptor) => Some(descriptor.dictionary().clone()),
            Err(_) => dict_at(reader, *id),
        },
        _ => None,
    }
}

fn value_dict(reader: &IndexedReader, value: &Object) -> Option<Dictionary> {
    match value {
        Object::Dictionary(dict) => Some(dict.clone()),
        Object::Reference(id) => dict_at(reader, *id),
        _ => None,
    }
}

fn entry_dict(reader: &IndexedReader, dict: &Dictionary, key: &[u8]) -> Option<Dictionary> {
    value_dict(reader, dict.get(key).ok()?)
}

fn has_soft_mask(reader: &IndexedReader, state: &Dictionary) -> bool {
    match state.get(b"SMask") {
        Ok(Object::Name(name)) => name != b"None",
        Ok(Object::Reference(id)) => !matches!(reader.resolve_object(*id), Ok(Object::Name(name)) if name == b"None"),
        Ok(_) => true,
        Err(_) => false,
    }
}

fn has_blend(state: &Dictionary) -> bool {
    match state.get(b"BM") {
        Ok(Object::Name(name)) => name != b"Normal" && name != b"Compatible",
        // A list of modes to try in turn, or something odd: taken as one that blends.
        Ok(_) => true,
        Err(_) => false,
    }
}

fn has_hidden_layers(reader: &IndexedReader) -> bool {
    let read = || -> Option<bool> {
        let root = reader.trailer_entry_owned(b"Root").ok()??;
        let properties = match root.as_dict().ok()?.get(b"OCProperties") {
            Ok(value) => value_dict(reader, value)?,
            Err(_) => return Some(false),
        };
        let config = entry_dict(reader, &properties, b"D")?;
        let starts_off = config.get(b"BaseState").is_ok_and(|state| matches!(state, Object::Name(name) if name != b"ON"));
        let list = |key: &[u8]| config.get(key).ok().and_then(|value| match value {
            Object::Array(items) => Some(!items.is_empty()),
            Object::Reference(id) => match reader.resolve_object(*id).ok()? {
                Object::Array(items) => Some(!items.is_empty()),
                _ => Some(true),
            },
            _ => Some(true),
        });
        Some(starts_off || list(b"OFF").unwrap_or(false) || list(b"AS").unwrap_or(false))
    };
    // Something there that can't be read may hide things, so it counts.
    read().unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_testpdf::{Object, Page, TempPdf, first_extra_object, pages, pages_with, text_lines};

    fn effects(resources: &str, objects: Vec<Object>) -> PageEffects {
        let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new(text_lines(&["Words"]))], resources, objects));
        page_effects(&file.0, 0).unwrap()
    }

    /// Failure: a plain page reads as using effects, so everything on it with a transparency group turns into a picture for no reason.
    #[test]
    fn a_plain_page_uses_nothing() {
        let file = TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["Words"]))]));
        assert_eq!(page_effects(&file.0, 0), Some(PageEffects::default()));
        assert_eq!(effects("/ExtGState << /G << /BM /Normal /SMask /None /ca 0.5 >> >>", vec![]), PageEffects::default());
    }

    /// Failure: what a page uses is missed, so a soft mask or a blend reaches the SVG as plain paint. Each is found on the page itself, and down through a form's own resources.
    #[test]
    fn what_a_page_uses_is_found_through_its_forms() {
        assert!(effects("/ExtGState << /G << /SMask << /S /Luminosity >> >> >>", vec![]).soft_mask);
        assert!(effects("/ExtGState << /G << /BM /Multiply >> >>", vec![]).blend);
        assert!(effects("/ExtGState << /G << /BM [/Foo /Normal] >> >>", vec![]).blend, "a list of modes counts");
        assert!(effects("/Pattern << /P << /PatternType 2 /Shading << >> >> >>", vec![]).shading_pattern);
        let form = first_extra_object(1);
        let inside = Object::Stream { dict: "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /ExtGState << /G << /BM /Screen >> >> >>".into(), data: Vec::new() };
        assert!(effects(&format!("/XObject << /F {form} 0 R >>"), vec![inside]).blend);
    }

    /// Failure: forms that name each other send the look round for ever, on a job's thread, while a user waits. It ends, and an answer still comes.
    #[test]
    fn forms_that_loop_end() {
        let form = first_extra_object(1);
        let resources = format!("/XObject << /F {form} 0 R >>");
        let looped = Object::Stream { dict: format!("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << {resources} >>"), data: Vec::new() };
        assert_eq!(effects(&resources, vec![looped]), PageEffects::default());
    }

    /// Failure: a layer that starts off goes unnoticed, so what the page doesn't show is handed over. And a file with layers that are all on shouldn't pay for drawing each to see.
    #[test]
    fn layers_that_start_off_are_noticed() {
        let group = first_extra_object(1);
        let doc = |config: &str| {
            let bytes = pages_with(&[Page::new(text_lines(&["Words"]))], "", vec![Object::Dict("<< /Type /OCG /Name (L) >>".into())]);
            let bytes = String::from_utf8_lossy(&bytes).replace("/Type /Catalog", &format!("/Type /Catalog /OCProperties << /OCGs [{group} 0 R] /D << {config} >> >>"));
            TempPdf::new("doc.pdf", bytes.as_bytes())
        };
        assert!(page_effects(&doc(&format!("/OFF [{group} 0 R]")).0, 0).unwrap().hidden_layers);
        assert!(page_effects(&doc("/BaseState /OFF").0, 0).unwrap().hidden_layers);
        assert!(!page_effects(&doc("/OFF [] /BaseState /ON").0, 0).unwrap().hidden_layers);
    }

    /// Failure: something that can't be read reads as "uses nothing", which would be a claim about what it hasn't seen. There's no answer, and the caller assumes anything.
    #[test]
    fn what_cant_be_read_has_no_answer() {
        let file = TempPdf::new("doc.pdf", b"not a pdf at all");
        assert_eq!(page_effects(&file.0, 0), None);
        let good = TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["Words"]))]));
        assert_eq!(page_effects(&good.0, 5), None, "no such page");
    }
}
