//! What a page uses that PDFium's objects can't show, read from the file's own structure through the lazy reader. The SVG export asks this before it starts, and a yes only means it looks harder or uses a picture: none of it is ever a claim about the file.
//!
//! The walk is bounded for hostile files: forms inside forms, loops and millions of entries all end at a fixed number of visits, and past that, or for anything that can't be read, the answer is `None`, which the caller takes as "may use anything". A stream's dictionary is read without its bytes, so a form on a page with a 300 MiB image costs no more than one without.

use crate::{deref, open_reader};
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
    /// The document has optional content, or something on this page names a layer in its own dictionary. What that hides is PDFium's to say and can't be worked out from here, since a layer's state can go by its usage, or a membership's by a rule, so this only says to look.
    pub optional_content: bool,
    /// A name can't stand for a layer on this page: see [`Walk::look_at_properties`].
    pub layer_names_unsafe: bool,
    /// A JPEG here has a `/Decode` or `/ColorTransform` that changes what its bytes mean.
    pub jpeg_read_differently: bool,
}

/// What this page uses, or `None` if that can't be said. `page` is zero-based.
pub fn page_effects(path: &Path, page: u32) -> Option<PageEffects> {
    let reader = open_reader(path).ok()?;
    let map = reader.page_map().ok()?;
    let entry = map.get(page as usize)?;
    let owner = entry.inherited().resources().unwrap_or(entry.id());
    let page_dict = dict_at(&reader, owner)?;
    let resources = page_dict.get(b"Resources").ok()?;
    let mut walk = Walk { reader: &reader, deadline: Instant::now() + MAX_TIME, visits: 0, effects: PageEffects::default(), waiting: Vec::new(), seen: HashSet::new(), listed: HashSet::new() };
    walk.read_layers()?;
    walk.resources(resources)?;
    while let Some(id) = walk.waiting.pop() {
        walk.tick()?;
        let Some(inner) = stream_or_dict(&reader, &Object::Reference(id)) else { continue };
        walk.effects.optional_content |= inner.has(b"OC");
        walk.look_at_pattern(&inner);
        walk.look_at_image(&inner);
        if let Ok(next) = inner.get(b"Resources") {
            walk.resources(next)?;
        }
    }
    Some(walk.effects)
}

/// Where a look has got to. It holds only ids: a dictionary is read, looked through and dropped, and each is read once, so forms that all name one large dictionary cost one read of it and no copies.
struct Walk<'a> {
    reader: &'a IndexedReader,
    deadline: Instant,
    visits: usize,
    effects: PageEffects,
    waiting: Vec<ObjectId>,
    seen: HashSet<ObjectId>,
    /// The layers whose names can stand for them.
    listed: HashSet<ObjectId>,
}

impl Walk<'_> {
    /// One step closer to the limits. `None` is past them, which is no answer.
    fn tick(&mut self) -> Option<()> {
        self.visits += 1;
        (self.visits <= MAX_VISITS && Instant::now() < self.deadline).then_some(())
    }

    /// Whether the document has layers, and which can be known by name: each listed as a layer by reference, with a plain name no other has. A name that isn't plain may read as another once it's decoded, by rules that aren't this reader's. Anything else just isn't listed, which costs looking at it by its mark.
    fn read_layers(&mut self) -> Option<()> {
        let Some(root) = self.reader.trailer_entry_owned(b"Root").ok().flatten() else {
            self.effects.optional_content = true;
            self.effects.layer_names_unsafe = true;
            return Some(());
        };
        let Ok(root) = root.as_dict() else {
            self.effects.optional_content = true;
            self.effects.layer_names_unsafe = true;
            return Some(());
        };
        let Ok(properties) = root.get(b"OCProperties") else { return Some(()) };
        self.effects.optional_content = true;
        let layers = value_dict(self.reader, properties).and_then(|properties| deref(self.reader, properties.get(b"OCGs").ok()?));
        let Some(Object::Array(layers)) = layers else {
            self.effects.layer_names_unsafe = true;
            return Some(());
        };
        let mut names = HashSet::new();
        for layer in &layers {
            self.tick()?;
            let name = match layer {
                Object::Reference(id) => dict_at(self.reader, *id).filter(is_a_layer).and_then(|dict| plain_name(self.reader, &dict)).map(|name| (*id, name)),
                _ => None,
            };
            if let Some((id, name)) = name
                && names.insert(name)
            {
                self.listed.insert(id);
            } else {
                self.effects.layer_names_unsafe = true;
            }
        }
        Some(())
    }

    /// A marked section says which layer it's in by a name in these, and that needn't be a layer of the document: it can be a membership with a rule of its own, or one the document doesn't list. So a name stands for a layer only if everything here is one listed.
    fn look_at_properties(&mut self, dict: &Dictionary) -> Option<()> {
        let Ok(value) = dict.get(b"Properties") else { return Some(()) };
        let Some(properties) = value_dict(self.reader, value) else {
            self.effects.layer_names_unsafe = true;
            return Some(());
        };
        for (_, value) in properties.iter() {
            self.tick()?;
            if !matches!(value, Object::Reference(id) if self.listed.contains(id)) {
                self.effects.layer_names_unsafe = true;
            }
        }
        Some(())
    }

    fn resources(&mut self, value: &Object) -> Option<()> {
        let dict = match value {
            Object::Reference(id) => {
                if !self.seen.insert(*id) {
                    return Some(());
                }
                self.tick()?;
                let Some(dict) = dict_at(self.reader, *id) else { return Some(()) };
                dict
            }
            Object::Dictionary(dict) => dict.clone(),
            _ => return Some(()),
        };
        self.look_at_properties(&dict)?;
        if let Some(states) = entry_dict(self.reader, &dict, b"ExtGState") {
            for (_, state) in states.iter() {
                self.tick()?;
                if let Some(state) = value_dict(self.reader, state) {
                    self.effects.soft_mask |= has_soft_mask(self.reader, &state);
                    self.effects.blend |= has_blend(&state);
                }
            }
        }
        for key in [&b"XObject"[..], &b"Pattern"[..]] {
            let Some(entries) = entry_dict(self.reader, &dict, key) else { continue };
            for (_, value) in entries.iter() {
                self.tick()?;
                match value {
                    Object::Reference(id) => {
                        if self.seen.insert(*id) {
                            self.waiting.push(*id);
                        }
                    }
                    // Only a pattern can be written in place, and a shading has no resources of its own to go down into.
                    Object::Dictionary(inner) => self.look_at_pattern(inner),
                    _ => {}
                }
            }
        }
        Some(())
    }

    /// A stored JPEG can be passed on as it is only if the page means by it what a reader of the bytes would.
    fn look_at_image(&mut self, dict: &Dictionary) {
        let names_dct = |value: &Object| matches!(value, Object::Name(name) if name == b"DCTDecode" || name == b"DCT");
        let filter = dict.get(b"Filter").ok().and_then(|filter| deref(self.reader, filter));
        let is_jpeg = filter.is_some_and(|filter| match &filter {
            Object::Array(filters) => filters.iter().any(names_dct),
            other => names_dct(other),
        });
        if !is_jpeg {
            return;
        }
        let decode_changes = dict.get(b"Decode").ok().and_then(|decode| deref(self.reader, decode)).is_some_and(|decode| match decode.as_array() {
            Ok(values) => values.iter().enumerate().any(|(at, value)| {
                let number = deref(self.reader, value).and_then(|value| value.as_float().ok().or_else(|| value.as_i64().ok().map(|whole| whole as f32)));
                number != Some(if at % 2 == 0 { 0.0 } else { 1.0 })
            }),
            Err(_) => true,
        });
        let parms = dict.get(b"DecodeParms").ok().and_then(|parms| deref(self.reader, parms));
        let transform_of = |parms: &Dictionary| parms.get(b"ColorTransform").ok().and_then(|value| deref(self.reader, value)).is_some_and(|value| value.as_i64().ok() != Some(1));
        let transform_changes = parms.is_some_and(|parms| match &parms {
            Object::Dictionary(parms) => transform_of(parms),
            Object::Array(list) => list.iter().any(|one| deref(self.reader, one).is_some_and(|one| one.as_dict().is_ok_and(transform_of))),
            _ => false,
        });
        self.effects.jpeg_read_differently |= decode_changes || transform_changes;
    }

    fn look_at_pattern(&mut self, dict: &Dictionary) {
        if dict.get(b"PatternType").and_then(Object::as_i64).is_ok_and(|kind| kind == 2) {
            self.effects.shading_pattern = true;
        }
    }
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

fn is_a_layer(dict: &Dictionary) -> bool {
    matches!(dict.get(b"Type"), Ok(Object::Name(name)) if name == b"OCG")
}

/// A layer's name, if it's plain printable ASCII, which reads the same however a text string is decoded.
fn plain_name(reader: &IndexedReader, dict: &Dictionary) -> Option<Vec<u8>> {
    match deref(reader, dict.get(b"Name").ok()?)? {
        Object::String(bytes, _) if bytes.iter().all(|byte| (0x20..0x7F).contains(byte)) => Some(bytes),
        _ => None,
    }
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

    /// A page with a marked section naming layers through these, in a document listing these.
    fn layered(listed: &str, properties: &str, objects: Vec<Object>) -> PageEffects {
        let bytes = pages_with(&[Page::new(text_lines(&["Words"]))], &format!("/Properties << {properties} >>"), objects);
        let bytes = String::from_utf8_lossy(&bytes).replace("/Type /Catalog", &format!("/Type /Catalog /OCProperties << /OCGs [{listed}] /D << >> >>"));
        page_effects(&TempPdf::new("doc.pdf", bytes.as_bytes()).0, 0).unwrap()
    }

    /// Failure: only layers that start off get looked at, so a membership with a rule, or a layer whose usage says it's off, goes out as though it were on. Having layers at all, or a form or image naming one, is what makes it worth looking.
    #[test]
    fn having_layers_at_all_is_noticed() {
        let group = first_extra_object(1);
        let found = layered(&format!("{group} 0 R"), "", vec![Object::Dict("<< /Type /OCG /Name (L) >>".into())]);
        assert!(found.optional_content, "with nothing off and no rule");
        assert_eq!(page_effects(&TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["Words"]))])).0, 0).unwrap().optional_content, false, "and none costs nothing");
        for subtype in ["/Subtype /Form /BBox [0 0 1 1]", "/Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray"] {
            let object = first_extra_object(1);
            let inside = Object::Stream { dict: format!("/Type /XObject {subtype} /OC << /Type /OCMD >>"), data: vec![0] };
            assert!(effects(&format!("/XObject << /X {object} 0 R >>"), vec![inside]).optional_content, "{subtype}, in a document with no layers listed");
        }
    }

    /// Failure: a name stands for a layer it isn't. A membership named after a layer that's on, or a second layer that reads as the same name once decoded, takes that layer's state, and what the page hides goes out in plain sight. A name is trusted only when everything a section can name is a listed layer with a plain name no other has.
    #[test]
    fn a_name_stands_for_a_layer_only_when_nothing_else_can_share_it() {
        let (first, second) = (first_extra_object(1), first_extra_object(1) + 1);
        let group = |name: &str| Object::Dict(format!("<< /Type /OCG /Name {name} >>"));
        let both = format!("{first} 0 R {second} 0 R");
        let named = format!("/A {first} 0 R /B {second} 0 R");
        assert!(!layered(&both, &named, vec![group("(Roads)"), group("(Rivers)")]).layer_names_unsafe, "the plain case keeps its shortcut");
        assert!(layered(&both, &named, vec![group("(Roads)"), group("(Roads)")]).layer_names_unsafe, "two with one name");
        assert!(layered(&both, &named, vec![group("(Roads)"), group("<FFFE52006F00>")]).layer_names_unsafe, "a name in UTF-16");
        assert!(layered(&both, &named, vec![group("(Roads)"), group("(R\\351gion)")]).layer_names_unsafe, "a name past ASCII");
        assert!(layered(&format!("{first} 0 R"), &named, vec![group("(Roads)"), group("(Rivers)")]).layer_names_unsafe, "one a section names isn't listed");
        assert!(layered(&format!("{first} 0 R"), &format!("/A {first} 0 R /B {second} 0 R"), vec![group("(Roads)"), Object::Dict(format!("<< /Type /OCMD /Name (Roads) /OCGs [{first} 0 R] /P /AllOff >>"))]).layer_names_unsafe, "a membership by the same name");
        assert!(layered(&both, &format!("/A {first} 0 R /B {second} 0 R"), vec![group("(Roads)"), Object::Dict(format!("<< /Type /OCMD /Name (Rivers) /OCGs [{first} 0 R] >>"))]).layer_names_unsafe, "a membership listed as if it were a layer");
        assert!(layered(&both, "/A << /Type /OCG /Name (Roads) >>", vec![group("(Roads)"), group("(Rivers)")]).layer_names_unsafe, "written in place, not by reference");
        let form = first_extra_object(1) + 2;
        let inside = Object::Stream { dict: "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /Properties << /C << /Type /OCMD >> >> >>".into(), data: Vec::new() };
        let page_form = format!("/Properties << /A {first} 0 R /B {second} 0 R >> /XObject << /F {form} 0 R >>");
        let bytes = pages_with(&[Page::new(text_lines(&["Words"]))], &page_form, vec![group("(Roads)"), group("(Rivers)"), inside]);
        let bytes = String::from_utf8_lossy(&bytes).replace("/Type /Catalog", &format!("/Type /Catalog /OCProperties << /OCGs [{both}] /D << >> >>"));
        assert!(page_effects(&TempPdf::new("doc.pdf", bytes.as_bytes()).0, 0).unwrap().layer_names_unsafe, "inside a form");
    }

    /// Failure: a JPEG's bytes pass through though the page reads them differently: inverted by `/Decode`, or in other colours by `/ColorTransform`. Neither shows through PDFium's calls. A `/Decode` that changes nothing, or a transform that's already the default, shouldn't cost a JPEG its original bytes.
    #[test]
    fn jpegs_the_page_reads_differently_are_noticed() {
        let with = |extra: &str| {
            let image = first_extra_object(1);
            let stream = Object::Stream { dict: format!("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode {extra}"), data: vec![0xFF, 0xD8] };
            effects(&format!("/XObject << /I {image} 0 R >>"), vec![stream]).jpeg_read_differently
        };
        assert!(with("/Decode [1 0 1 0 1 0]"));
        assert!(with("/DecodeParms << /ColorTransform 0 >>"));
        assert!(with("/DecodeParms [<< /ColorTransform 0 >>]"));
        assert!(!with(""));
        assert!(!with("/Decode [0 1 0 1 0 1]"));
        assert!(!with("/DecodeParms << /ColorTransform 1 >>"));
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
