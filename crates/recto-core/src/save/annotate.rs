//! **Highlights into a save** (SPEC section 6, Annotations; R4.3): each highlight made as an edit becomes one standard `/Highlight` annotation on its
//! page, with `/QuadPoints`, its colour and an appearance stream, so other readers draw it as RectoPDF does. Highlights are additions, so they go into
//! an appended revision like a turn, and a signed file's bytes stay as they were (AGENTS.md rule 5).
//!
//! **Everything written here is made from numbers this module worked out** (coordinator, 2026-11-14): the rectangles come from a selection, held to
//! [`MAX_EDGE`] and finite, and are turned into the page's own coordinates here; no byte of any annotation already in the file is copied into what's
//! written. Nothing names who made it, when, or what text it covers: no `/T`, no dates, no `/Contents`, no `/NM` (coordinator, Q10).
//!
//! **Where a highlight sits.** Its rectangles are in the page's frame as `recto select` gives them: points from the top left, y down, past the crop and
//! turned by the page's own `/Rotate` ([`crate::nav::Frame`], the frame links are drawn in). A save puts them back into the page's own coordinates by
//! the same frame read from the file being written, so a rotated or cropped page gets its highlight where it was shown.

use super::{Dictionary, IndexedReader, Object, SaveError, Sink};
use crate::nav::Frame;
use std::collections::BTreeMap;

/// The highlight's colour, `0xRRGGBB`: the one place it's said (coordinator's default, Q1, 2026-11-14: Preview's yellow), for the file and the screen.
pub const HIGHLIGHT_RGB: u32 = 0xFF_E1_4D;
/// Largest edge a highlight's rectangle may have, in points: past any real page (PDF's own limit is 14,400).
pub const MAX_EDGE: f32 = 200_000.0;
/// Most highlights one save writes on one page, and rectangles each: what an edited list can hold, and no more.
pub const MAX_PER_PAGE: usize = 10_000;
pub const MAX_RECTS: usize = 4096;

/// One highlight to write: the position of its page in the saved document, and its lines, `[x, y, width, height]` in the page's frame.
#[derive(Clone, Copy, Debug)]
pub struct NewHighlight<'a> {
    pub position: u32,
    pub rects: &'a [[f32; 4]],
}

/// What a save writes of highlights, by page position. Owned, so the plan doesn't borrow the request.
#[derive(Default)]
pub(super) struct Highlights {
    by_position: BTreeMap<u32, Vec<Box<[[f32; 4]]>>>,
    /// How long the file being saved is, for a refusal to say.
    file_pages: u32,
}

impl Highlights {
    /// The request's highlights, each checked: on a page the document has, with sound rectangles. One that isn't refuses the save, since an edit
    /// list that says such a thing isn't the one that was made.
    pub(super) fn of(highlights: &[NewHighlight<'_>], pages: usize, file_pages: u32) -> Result<Self, SaveError> {
        Self::within(highlights, pages, MAX_PER_PAGE).map(|out| Self { file_pages, ..out })
    }

    /// [`Self::of`], with the limit a page's highlights have set small for a test.
    fn within(highlights: &[NewHighlight<'_>], pages: usize, per_page: usize) -> Result<Self, SaveError> {
        let mut out = Self::default();
        for highlight in highlights {
            let sound = |rect: &[f32; 4]| rect.iter().all(|edge| edge.is_finite() && edge.abs() <= MAX_EDGE) && rect[2] > 0.0 && rect[3] > 0.0;
            let on_page = (highlight.position as usize) < pages;
            if !on_page || highlight.rects.is_empty() || highlight.rects.len() > MAX_RECTS || !highlight.rects.iter().all(sound) {
                return Err(SaveError::EditsDontMatch {
                    file_pages: u32::try_from(pages).unwrap_or(u32::MAX),
                });
            }
            let page = out.by_position.entry(highlight.position).or_default();
            if page.len() >= per_page {
                return Err(SaveError::TooLarge);
            }
            page.push(highlight.rects.into());
        }
        Ok(out)
    }

    /// Whether the page at this position gets any.
    pub(super) fn on(&self, position: usize) -> bool {
        u32::try_from(position).is_ok_and(|position| self.by_position.contains_key(&position))
    }

    /// **Puts the page's highlights on it:** each annotation and its appearance written as new objects, and the page's `/Annots` a list of what it had
    /// and these after. A list the page names by reference is read through `reader`, whose page it is, and written into the page itself, so a list
    /// another page shares is never changed. Nothing for a page with none.
    ///
    /// **Refuses rather than leaves one out** (review of R4.3): a highlight whose rectangles come to no finite place on this page's frame, as a hostile
    /// box can make them, stops the save, which would otherwise say done with that highlight missing from the file. And the save's bound on what it
    /// appends is asked before each annotation, so one page's thousands of highlights can't write gigabytes before it's looked at.
    pub(super) fn dress<'c>(
        &self,
        position: usize,
        frame: Frame,
        reader: &IndexedReader,
        page: &mut Dictionary,
        (writer, cap): (&mut impl Sink<'c>, u64),
    ) -> Result<(), SaveError> {
        let Some(highlights) = u32::try_from(position).ok().and_then(|position| self.by_position.get(&position)) else {
            return Ok(());
        };
        let mut annots = annots_of(reader, page);
        for rects in highlights {
            if writer.taken() > cap {
                return Err(SaveError::TooLarge);
            }
            let made = annotation(frame, rects).ok_or(SaveError::EditsDontMatch { file_pages: self.file_pages })?;
            let appearance = writer.add(Object::Stream(made.appearance))?;
            let mut annotation = made.annotation;
            let mut ap = Dictionary::new();
            ap.set("N", Object::Reference(appearance));
            annotation.set("AP", Object::Dictionary(ap));
            annots.push(Object::Reference(writer.add(Object::Dictionary(annotation))?));
        }
        page.set("Annots", Object::Array(annots));
        Ok(())
    }
}

/// A page's `/Annots` as a list of its own, whatever form the file gave it: written in place, or named. Anything else is no list, so what's
/// written is the highlights alone.
fn annots_of(reader: &IndexedReader, page: &Dictionary) -> Vec<Object> {
    match page.get(b"Annots") {
        Ok(Object::Array(items)) => items.clone(),
        Ok(Object::Reference(id)) => match reader.resolve_object(*id) {
            Ok(Object::Array(items)) => items,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// A copied page's `/Annots`, if the file names its list, read into the page before it's renumbered, so the highlights can be added to it. The list's
/// own object still comes along, unused.
pub(super) fn inline_annots(source: &IndexedReader, page: &mut Dictionary) {
    if let Ok(Object::Reference(_)) = page.get(b"Annots") {
        let items = annots_of(source, page);
        page.set("Annots", Object::Array(items));
    }
}

/// An annotation and its appearance, not yet written.
struct Made {
    annotation: Dictionary,
    appearance: lopdf::Stream,
}

/// A point of the page's frame (from its top left, y down) in the page's own coordinates: the inverse of [`Frame::to_shown`].
fn to_user(frame: Frame, x: f32, y: f32) -> (f32, f32) {
    let (width, height) = (frame.right - frame.left, frame.top - frame.bottom);
    match frame.turns & 3 {
        0 => (frame.left + x, frame.bottom + height - y),
        1 => (frame.left + y, frame.bottom + x),
        2 => (frame.left + width - x, frame.bottom + y),
        _ => (frame.left + width - y, frame.bottom + height - x),
    }
}

/// A number as a PDF writes it: no exponent, at most three decimals.
fn number(value: f32) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0".into() } else { text.into() }
}

/// The highlight's annotation and appearance, from its rectangles. `None` if what they come to in the page's coordinates isn't finite.
fn annotation(frame: Frame, rects: &[[f32; 4]]) -> Option<Made> {
    let mut quads: Vec<[(f32, f32); 4]> = Vec::with_capacity(rects.len());
    for rect in rects {
        let [x, y, width, height] = *rect;
        // As the line reads: top left, top right, bottom left, bottom right, which is the order `/QuadPoints` takes.
        let corners = [(x, y), (x + width, y), (x, y + height), (x + width, y + height)].map(|(x, y)| to_user(frame, x, y));
        if !corners.iter().all(|(x, y)| x.is_finite() && y.is_finite()) {
            return None;
        }
        quads.push(corners);
    }
    let points = quads.iter().flatten();
    let (left, bottom) = points.clone().fold((f32::MAX, f32::MAX), |(l, b), (x, y)| (l.min(*x), b.min(*y)));
    let (right, top) = points.fold((f32::MIN, f32::MIN), |(r, t), (x, y)| (r.max(*x), t.max(*y)));
    let rect = Object::Array([left, bottom, right, top].map(Object::Real).to_vec());
    let [red, green, blue] = [16, 8, 0].map(|shift| ((HIGHLIGHT_RGB >> shift) & 0xFF) as f32 / 255.0);
    let mut content = format!("/G0 gs {} {} {} rg\n", number(red), number(green), number(blue));
    for [top_left, top_right, bottom_left, bottom_right] in &quads {
        for (at, (x, y)) in [top_left, top_right, bottom_right, bottom_left].into_iter().enumerate() {
            content.push_str(&format!("{} {} {}\n", number(*x), number(*y), if at == 0 { "m" } else { "l" }));
        }
        content.push_str("h\n");
    }
    content.push_str("f\n");
    let mut state = Dictionary::new();
    state.set("Type", Object::Name(b"ExtGState".to_vec()));
    state.set("BM", Object::Name(b"Multiply".to_vec()));
    let mut states = Dictionary::new();
    states.set("G0", Object::Dictionary(state));
    let mut resources = Dictionary::new();
    resources.set("ExtGState", Object::Dictionary(states));
    let mut form = Dictionary::new();
    form.set("Type", Object::Name(b"XObject".to_vec()));
    form.set("Subtype", Object::Name(b"Form".to_vec()));
    form.set("BBox", rect.clone());
    form.set("Resources", Object::Dictionary(resources));
    let mut annotation = Dictionary::new();
    annotation.set("Type", Object::Name(b"Annot".to_vec()));
    annotation.set("Subtype", Object::Name(b"Highlight".to_vec()));
    annotation.set("Rect", rect);
    annotation.set(
        "QuadPoints",
        Object::Array(quads.iter().flatten().flat_map(|(x, y)| [Object::Real(*x), Object::Real(*y)]).collect()),
    );
    annotation.set("C", Object::Array([red, green, blue].map(Object::Real).to_vec()));
    // Printed, as a highlight on paper would be.
    annotation.set("F", Object::Integer(4));
    Some(Made {
        annotation,
        appearance: lopdf::Stream::new(form, content.into_bytes()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(turns: u8) -> Frame {
        Frame {
            turns,
            left: 10.0,
            bottom: 20.0,
            right: 610.0,
            top: 820.0,
        }
    }

    /// Failure: a highlight on a rotated or cropped page is written somewhere other than where it was shown, as the frame's inverse is wrong for a turn.
    #[test]
    fn a_rectangle_goes_back_to_where_it_was_shown_at_every_turn() {
        for turns in 0..4 {
            let frame = frame(turns);
            let shown = [40.0, 100.0, 140.0, 112.0];
            let (x0, y0) = to_user(frame, shown[0], shown[1]);
            let (x1, y1) = to_user(frame, shown[2], shown[3]);
            assert_eq!(frame.to_shown([x0, y0, x1, y1]), Some(shown), "turns {turns}");
        }
    }

    /// Failure: a written highlight's quad isn't in `/QuadPoints`' order (top left, top right, bottom left, bottom right as the line reads), so other
    /// readers draw a bow tie, or its `/Rect` doesn't hold it; or its appearance names anything but what this module made.
    #[test]
    fn a_highlight_is_a_standard_annotation() {
        let made = annotation(frame(0), &[[40.0, 100.0, 100.0, 12.0]]).unwrap();
        let quad: Vec<f32> = made
            .annotation
            .get(b"QuadPoints")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_float().unwrap())
            .collect();
        // Shown from the top: y 100 is 720 points up from the page's bottom edge at 20.
        assert_eq!(quad, vec![50.0, 720.0, 150.0, 720.0, 50.0, 708.0, 150.0, 708.0]);
        let rect: Vec<f32> = made
            .annotation
            .get(b"Rect")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_float().unwrap())
            .collect();
        assert_eq!(rect, vec![50.0, 708.0, 150.0, 720.0]);
        assert_eq!(made.annotation.get(b"Subtype").unwrap().as_name().unwrap(), b"Highlight");
        for key in [&b"T"[..], b"M", b"CreationDate", b"Contents", b"NM"] {
            assert!(!made.annotation.has(key), "nothing about who or when: {}", String::from_utf8_lossy(key));
        }
        let content = String::from_utf8(made.appearance.content.clone()).unwrap();
        assert_eq!(content, "/G0 gs 1 0.882 0.302 rg\n50 720 m\n150 720 l\n150 708 l\n50 708 l\nh\nf\n");
    }

    /// Failure: a request naming a page past the document, an empty highlight, a rectangle with a NaN or an enormous edge, too many rectangles, or too
    /// many highlights on one page (at its limit plus one, the limit set small), reaches the writer.
    #[test]
    fn what_a_save_is_asked_to_write_is_checked() {
        let good = [[1.0, 1.0, 5.0, 5.0]];
        assert!(Highlights::of(&[NewHighlight { position: 0, rects: &good }], 1, 1).is_ok());
        let bad: [&[[f32; 4]]; 4] = [&[], &[[f32::NAN, 1.0, 5.0, 5.0]], &[[1.0, 1.0, MAX_EDGE * 2.0, 5.0]], &[[1.0, 1.0, 0.0, 5.0]]];
        for rects in bad {
            assert!(Highlights::of(&[NewHighlight { position: 0, rects }], 1, 1).is_err(), "{rects:?}");
        }
        assert!(
            Highlights::of(&[NewHighlight { position: 1, rects: &good }], 1, 1).is_err(),
            "a page past the end"
        );
        let many = vec![[1.0, 1.0, 5.0, 5.0]; MAX_RECTS + 1];
        assert!(Highlights::of(&[NewHighlight { position: 0, rects: &many }], 1, 1).is_err());
        let three = [NewHighlight { position: 0, rects: &good }; 3];
        assert!(Highlights::within(&three[..2], 1, 2).is_ok());
        assert!(matches!(Highlights::within(&three, 1, 2), Err(SaveError::TooLarge)));
    }
}
