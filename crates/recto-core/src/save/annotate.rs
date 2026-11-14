//! **Highlights into a save** (SPEC section 6, Annotations; R4.3): each highlight made as an edit becomes one standard `/Highlight` annotation on its
//! page, with `/QuadPoints`, its colour and an appearance stream, so other readers draw it as RectoPDF does. Highlights are additions, so they go into
//! an appended revision like a turn, and a signed file's bytes stay as they were (AGENTS.md rule 5).
//!
//! **Every new object written here is made from numbers this module worked out** (coordinator, 2026-11-14): the rectangles come from a selection,
//! held to [`MAX_EDGE`] and finite, and are turned into the page's own coordinates here. The page's `/Annots` list is written again with its own
//! entries first, as the file had them (a reference stays a reference, and an annotation the file wrote in place in the list is carried as it was):
//! nothing of an existing annotation is read into a new one. Nothing names who made it, when, or what text it covers: no `/T`, no dates, no
//! `/Contents`, no `/NM` (coordinator, Q10).
//!
//! **Where a highlight sits.** Its rectangles are in the page's frame as `recto select` gives them: points from the top left, y down, past the crop and
//! turned by the page's own `/Rotate` ([`crate::nav::Frame`], the frame links are drawn in). A save puts them back into the page's own coordinates by
//! the same frame read from the file being written, so a rotated or cropped page gets its highlight where it was shown.

use super::{Dictionary, IndexedReader, Object, PageRef, SaveError, Sink};
use crate::nav::Frame;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// The highlight's colour, `0xRRGGBB`: the one place it's said (coordinator's default, Q1, 2026-11-14: Preview's yellow), for the file and the screen.
pub const HIGHLIGHT_RGB: u32 = 0xFF_E1_4D;
/// Largest edge a highlight's rectangle may have, in points: past any real page (PDF's own limit is 14,400).
pub const MAX_EDGE: f32 = 200_000.0;
/// Most highlights one save writes on one page, and rectangles each: what an edited list can hold, and no more.
pub const MAX_PER_PAGE: usize = 10_000;
pub const MAX_RECTS: usize = 4096;

/// Most highlights already in a file one save takes out: as many as a document may hold hidden (`recto_session`'s removals of a file's highlights
/// are held to this).
pub const MAX_GONE: usize = 10_000;
/// How far a file highlight's `/Rect` may be from what the removal says it was, in points, and still be that highlight.
pub const RECT_SLACK: f32 = 0.5;

/// One highlight to write: the position of its page in the saved document, and its lines, `[x, y, width, height]` in the page's frame.
#[derive(Clone, Copy, Debug)]
pub struct NewHighlight<'a> {
    pub position: u32,
    pub rects: &'a [[f32; 4]],
}

/// **A highlight already in the file, to take out at this save** (review of R4.3, S2): its page's position in the document saved, which has to be a
/// page of the file itself, its place in that page's `/Annots`, and its `/Rect` as the file has it (`[left, bottom, right, top]`). The save takes it out
/// only if the item there is a `/Highlight` whose `/Rect` is that, within [`RECT_SLACK`]; anything else refuses the save, as the file isn't what the
/// removal was about. Taking something out of a file is a rewrite, so what was removed is gone from it, not just unlisted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GoneHighlight {
    pub position: u32,
    pub index: u32,
    pub rect: [f32; 4],
}

/// **What a save did with highlights, for the history that goes on past it** (review of R4.3, S2 and S3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarksWritten {
    /// For each of the request's highlights, in its order: its place in its page's `/Annots` in the saved file, and the `/Rect` it was written with.
    pub placed: Vec<(u32, [f32; 4])>,
    /// Each page an edit turned, by its position: its width and height as shown before that turn, which the rectangles of highlights on it are in.
    pub turned: Vec<(u32, [f32; 2])>,
    /// Each item taken out of a page's list, by the page's position and its place in the list as the file had it: the highlights asked for and
    /// their popups. What names a later item on that page by its place counts down by those before it.
    pub removed: Vec<(u32, u32)>,
}

/// Every number in it was worked out by the save from finite rectangles, so equality is an equivalence.
impl Eq for MarksWritten {}

/// What a save writes of highlights, and takes out, by page position. Owned, so the plan doesn't borrow the request.
#[derive(Default)]
pub(super) struct Highlights {
    /// Each page's highlights to write, with their place in the request.
    by_position: BTreeMap<u32, Vec<(usize, Box<[[f32; 4]]>)>>,
    /// Each page's file highlights to take out, by their place in its list, ascending.
    gone: BTreeMap<u32, Vec<u32>>,
    /// How long the file being saved is, for a refusal to say.
    file_pages: u32,
    /// What's been written, as the plan writes it.
    written: RefCell<MarksWritten>,
}

impl Highlights {
    /// The request's highlights, each checked: on a page the document has, with sound rectangles. One that isn't refuses the save, since an edit
    /// list that says such a thing isn't the one that was made. And the file highlights it takes out, each checked against the file: see
    /// [`GoneHighlight`].
    pub(super) fn of(request: &super::SaveRequest<'_>, reader: &IndexedReader, map: &lopdf::PageMap, file_pages: u32) -> Result<Self, SaveError> {
        let mut out = Self::within(request.highlights, request.pages.len(), MAX_PER_PAGE)?;
        out.file_pages = file_pages;
        out.gone = gone_checked(request.highlights_gone, request.pages, reader, map, MAX_GONE).map_err(|error| match error {
            SaveError::EditsDontMatch { .. } => SaveError::EditsDontMatch { file_pages },
            other => other,
        })?;
        out.written.borrow_mut().placed = vec![(u32::MAX, [0.0; 4]); request.highlights.len()];
        Ok(out)
    }

    /// What's been written, once the plan has.
    pub(super) fn written(&self) -> MarksWritten {
        self.written.take()
    }

    /// [`Self::of`]'s highlights to write, with the limit a page's highlights have set small for a test.
    fn within(highlights: &[NewHighlight<'_>], pages: usize, per_page: usize) -> Result<Self, SaveError> {
        let mut out = Self::default();
        for (at, highlight) in highlights.iter().enumerate() {
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
            page.push((at, highlight.rects.into()));
        }
        Ok(out)
    }

    /// Whether the page at this position gets any, or has any taken out.
    pub(super) fn on(&self, position: usize) -> bool {
        u32::try_from(position).is_ok_and(|position| self.by_position.contains_key(&position) || self.gone.contains_key(&position))
    }

    /// **Puts the page's highlights on it:** each annotation and its appearance written as new objects, and the page's `/Annots` a list of what it had
    /// and these after. A list the page names by reference is read through `reader`, whose page it is, and written into the page itself, so a list
    /// another page shares is never changed. Nothing for a page with none.
    ///
    /// **Refuses rather than leaves one out** (review of R4.3): a highlight whose rectangles come to no finite place on this page's frame, as a hostile
    /// box can make them, stops the save, which would otherwise say done with that highlight missing from the file. And the save's bound on what it
    /// appends is asked before each annotation, so one page's thousands of highlights can't write gigabytes before it's looked at.
    ///
    /// **Takes out the file's highlights the request names** on this page, first, with any popup that belongs to one, so nothing in the new file
    /// reaches them; and says where each written one went (its place in the list and its `/Rect`) and, for a page an edit turns (`turns`), the
    /// size it was shown at before that turn: [`MarksWritten`].
    pub(super) fn dress<'c>(
        &self,
        (position, turns): (usize, u8),
        frame: Frame,
        reader: &IndexedReader,
        page: &mut Dictionary,
        (writer, cap): (&mut impl Sink<'c>, u64),
    ) -> Result<(), SaveError> {
        let Ok(at) = u32::try_from(position) else { return Ok(()) };
        if turns % 4 != 0 {
            let (width, height) = (frame.right - frame.left, frame.top - frame.bottom);
            let shown = if frame.turns % 2 == 1 { [height, width] } else { [width, height] };
            self.written.borrow_mut().turned.push((at, shown));
        }
        let (highlights, gone) = (self.by_position.get(&at), self.gone.get(&at));
        if highlights.is_none() && gone.is_none() {
            return Ok(());
        }
        let mut annots = annots_of(reader, page);
        if let Some(gone) = gone {
            let removed = take_out(reader, &mut annots, gone);
            self.written.borrow_mut().removed.extend(removed.into_iter().map(|index| (at, index)));
        }
        for (asked, rects) in highlights.into_iter().flatten() {
            if writer.taken() > cap {
                return Err(SaveError::TooLarge);
            }
            let made = annotation(frame, rects).ok_or(SaveError::EditsDontMatch { file_pages: self.file_pages })?;
            let rect = numbers_of(made.annotation.get(b"Rect").ok()).unwrap_or([0.0; 4]);
            let appearance = writer.add(Object::Stream(made.appearance))?;
            let mut annotation = made.annotation;
            let mut ap = Dictionary::new();
            ap.set("N", Object::Reference(appearance));
            annotation.set("AP", Object::Dictionary(ap));
            let index = u32::try_from(annots.len()).map_err(|_| SaveError::TooLarge)?;
            annots.push(Object::Reference(writer.add(Object::Dictionary(annotation))?));
            if let Some(slot) = self.written.borrow_mut().placed.get_mut(*asked) {
                *slot = (index, rect);
            }
        }
        page.set("Annots", Object::Array(annots));
        Ok(())
    }
}

/// A rectangle's four numbers, `[left, bottom, right, top]` as the corners come, if it is four finite numbers.
fn numbers_of(object: Option<&Object>) -> Option<[f32; 4]> {
    let items = object?.as_array().ok()?;
    let mut out = [0.0f32; 4];
    if items.len() != 4 {
        return None;
    }
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = item.as_float().ok().filter(|value| value.is_finite())?;
    }
    Some([out[0].min(out[2]), out[1].min(out[3]), out[0].max(out[2]), out[1].max(out[3])])
}

/// The annotation an item of a list is, read through `reader`.
fn annotation_at(reader: &IndexedReader, item: &Object) -> Option<Dictionary> {
    match item {
        Object::Dictionary(dict) => Some(dict.clone()),
        Object::Reference(id) => match reader.resolve_object(*id).ok()? {
            Object::Dictionary(dict) => Some(dict),
            _ => None,
        },
        _ => None,
    }
}

/// **The file's highlights a save takes out, each checked against the file** (see [`GoneHighlight`]): by position, each page's indexes in order.
/// At most `most`, none named twice, each on a page of the file itself.
fn gone_checked(
    gone: &[GoneHighlight],
    pages: &[PageRef],
    reader: &IndexedReader,
    map: &lopdf::PageMap,
    most: usize,
) -> Result<BTreeMap<u32, Vec<u32>>, SaveError> {
    let refused = SaveError::EditsDontMatch { file_pages: 0 };
    if gone.len() > most {
        return Err(SaveError::TooLarge);
    }
    let mut out: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for asked in gone {
        let page = pages.get(asked.position as usize).filter(|page| page.source == 0).ok_or(refused.clone())?;
        let entry = map.get(page.page as usize).ok_or(refused.clone())?;
        let Ok(Object::Dictionary(dict)) = reader.resolve_object(entry.id()) else {
            return Err(refused);
        };
        let annots = annots_of(reader, &dict);
        let found = annots
            .get(asked.index as usize)
            .and_then(|item| annotation_at(reader, item))
            .ok_or(refused.clone())?;
        let is_highlight = matches!(found.get(b"Subtype"), Ok(Object::Name(kind)) if kind == b"Highlight");
        let rect = numbers_of(found.get(b"Rect").ok()).ok_or(refused.clone())?;
        let close = rect
            .iter()
            .zip(asked.rect)
            .all(|(there, said)| said.is_finite() && (there - said).abs() <= RECT_SLACK);
        let indexes = out.entry(asked.position).or_default();
        if !is_highlight || !close || indexes.contains(&asked.index) {
            return Err(refused);
        }
        indexes.push(asked.index);
    }
    for indexes in out.values_mut() {
        indexes.sort_unstable();
    }
    Ok(out)
}

/// Takes the items at `indexes` out of a page's list, and any popup in the list whose `/Parent` is one of them: a popup is the highlight's own
/// note, and its `/Parent` would keep the highlight reachable in the new file. Gives the places in the list as it was of everything taken out,
/// ascending, for what names a later item by its place to be counted down.
fn take_out(reader: &IndexedReader, annots: &mut Vec<Object>, indexes: &[u32]) -> Vec<u32> {
    let ids: Vec<lopdf::ObjectId> = indexes
        .iter()
        .filter_map(|index| match annots.get(*index as usize) {
            Some(Object::Reference(id)) => Some(*id),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    let mut at = 0u32;
    annots.retain(|item| {
        let popup_of_one = annotation_at(reader, item).is_some_and(|annot| {
            matches!(annot.get(b"Subtype"), Ok(Object::Name(kind)) if kind == b"Popup")
                && matches!(annot.get(b"Parent"), Ok(Object::Reference(parent)) if ids.contains(parent))
        });
        let goes = indexes.contains(&at) || popup_of_one;
        if goes {
            out.push(at);
        }
        at += 1;
        !goes
    });
    out
}

/// The first `/Highlight` among a page's first `within` items, by its place and `/Rect`: what a fuzzer's removal can name.
pub(super) fn first_highlight(reader: &IndexedReader, page: &Dictionary, within: usize) -> Option<(u32, [f32; 4])> {
    annots_of(reader, page).iter().take(within).enumerate().find_map(|(at, item)| {
        let annot = annotation_at(reader, item)?;
        let highlight = matches!(annot.get(b"Subtype"), Ok(Object::Name(kind)) if kind == b"Highlight");
        let rect = numbers_of(annot.get(b"Rect").ok())?;
        highlight.then_some((u32::try_from(at).ok()?, rect))
    })
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
        // The appearance: the colour set, then each quad as a closed path, filled; drawn within the box the annotation says.
        let content = String::from_utf8(made.appearance.content.clone()).unwrap();
        let words: Vec<&str> = content.split_whitespace().collect();
        let colour = words.iter().position(|word| *word == "rg").expect("a fill colour");
        let rgb: Vec<f32> = words[colour - 3..colour].iter().map(|word| word.parse().unwrap()).collect();
        for (channel, shift) in rgb.iter().zip([16, 8, 0]) {
            assert!((channel * 255.0 - ((HIGHLIGHT_RGB >> shift) & 0xFF) as f32).abs() < 0.5, "{content}");
        }
        let ops: Vec<&str> = words.iter().copied().filter(|word| word.parse::<f32>().is_err()).collect();
        assert_eq!(&ops[ops.len() - 6..], ["m", "l", "l", "l", "h", "f"], "{content}");
        assert_eq!(made.appearance.dict.get(b"BBox").unwrap(), made.annotation.get(b"Rect").unwrap());
    }

    /// Failure: a request naming a page past the document, an empty highlight, a rectangle with a NaN or an enormous edge, too many rectangles, or too
    /// many highlights on one page (at its limit plus one, the limit set small), reaches the writer.
    #[test]
    fn what_a_save_is_asked_to_write_is_checked() {
        let good = [[1.0, 1.0, 5.0, 5.0]];
        assert!(Highlights::within(&[NewHighlight { position: 0, rects: &good }], 1, MAX_PER_PAGE).is_ok());
        let bad: [&[[f32; 4]]; 4] = [&[], &[[f32::NAN, 1.0, 5.0, 5.0]], &[[1.0, 1.0, MAX_EDGE * 2.0, 5.0]], &[[1.0, 1.0, 0.0, 5.0]]];
        for rects in bad {
            assert!(
                Highlights::within(&[NewHighlight { position: 0, rects }], 1, MAX_PER_PAGE).is_err(),
                "{rects:?}"
            );
        }
        assert!(
            Highlights::within(&[NewHighlight { position: 1, rects: &good }], 1, MAX_PER_PAGE).is_err(),
            "a page past the end"
        );
        let many = vec![[1.0, 1.0, 5.0, 5.0]; MAX_RECTS + 1];
        assert!(Highlights::within(&[NewHighlight { position: 0, rects: &many }], 1, MAX_PER_PAGE).is_err());
        let three = [NewHighlight { position: 0, rects: &good }; 3];
        assert!(Highlights::within(&three[..2], 1, 2).is_ok());
        assert!(matches!(Highlights::within(&three, 1, 2), Err(SaveError::TooLarge)));
    }
}
