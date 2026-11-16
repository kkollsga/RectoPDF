//! **An image picked with a click in read mode** (owner, 2026-11-16: "when in normal read mode (with left click to select text) clicking a image will show a
//! selection box. Clicking cmd+c will copy lossless to clipboard."). What the page view holds of it: the click waiting for its image, the image found, and
//! the page whose layer has its box. Finding the image is the app's, a read through a worker of its own (`recto images --at`, found as the snapshot tool's
//! click finds it); deciding whether that read is worth making, keeping what it found, and letting it go, is here.
//!
//! **A click costs a private worker only where an image may be** (the review of `build/image-select`, S1: a still click on body text, on a blank margin,
//! or the first click of a double click each started a worker of 351 MiB on the USGS map). A click goes through its [`Stage`]s, and the app is told to look
//! only at the end of them ([`ImageSelection::take_news`]):
//! 1. **The page's text first** ([`Verdict`]), as it's held: text that shows keeps a click (coordinator, 2026-11-16), as a caption drawn over a figure is
//!    text's to select. A page whose text isn't held yet waits for its read, which the press asked for already.
//! 2. **Then the tab's own worker,** which has the page open, says whether any picture is under the point ([`Under`]): its walk over the page's objects, for
//!    a click (`DocRenderer::image_boxes_for_a_click`): dark paper's boxes where it has them, and past 64 pictures apart the boxes held and one
//!    round the rest, so always a superset. A page it can't say of (its drawing failed or was stopped, it's withheld or its file refused,
//!    its walks killed past their cap) has no picture to a click, which picks nothing there; the snapshot tool still serves it. No picture there:
//!    nothing's looked for.
//! 3. **Then, on text not yet checked for hidden text** (a page's first read has no check, as the check draws the page), the check: hidden text, as a scan's
//!    OCR layer, doesn't keep the click. Asked only where a picture may be, so a click on text with none under it draws nothing. It always comes to an
//!    answer: a check tried and failed, or a page known to be heavy (never drawn for a check, as for a copy), takes its text as showing.
//! 4. **Only then the look,** at most one private worker, and one at a time across the app's clicks and copies (the snapshot tool's one slot).
//!
//! **It's the other kind of selection, held beside the text selection** ([`crate::selection::Selecting`]), which tells it in one place when the text
//! selection ends or changes ([`ImageSelection::ended`]): a press, a drag that selects, Select All, Find's hit, another tab or other pages, the file seen to
//! change, pages withheld. Text and an image are never both selected. A press of one click inside the box of the image picked, off text that shows,
//! keeps it: a click on it again neither looks again nor takes its box off and back.
//!
//! **The last thing the person asked for wins.** Each click has a serial, from the text selection's clock: a look's answer is taken only for the click
//! still waiting with that serial ([`ImageSelection::found`]), so one begun before a press, another click, a page move or anything that resets the
//! selection shows nothing, even at the same point. A click the app was told to look for and that goes is news too, so its look is stopped.
//!
//! **What it costs:** 32 bytes a page view ([`ImageSelection`]'s guard), and once an image is found, one small allocation off the UI thread with its page,
//! number, box and the point clicked. Nothing per page or tab, and nothing at all with nothing picked.
//!
//! **A picture that is the page is never picked** (owner, 2026-11-16: "lets treat true images and scanned pages differently. Click to select only for true
//! images."): a scan, with a text layer over it or without, a full-bleed cover. One rule, [`is_the_page`]: the picture's box, held to the page, leaves
//! under a twentieth of the page ([`crate::paper::LEAST_BACKGROUND`], dark paper's line for a page that "is a picture": with less background than that
//! there is nothing to judge but the picture). Dark paper's rule needs the page's pixels, which a click doesn't have, so it's the box alone, at that
//! one number. It's applied twice: to the boxes the tab's own worker gives ([`under`]), so a click on a plain scan starts no private worker, and to the
//! box the look found ([`ImageSelection::found`]), so a figure placed over a scan is picked and the scan under it isn't. A scan made of several strips or
//! tiles, none covering the page alone, isn't caught: it stays as it was. `recto images --at` is the lookup, not the pick, and still names a scan.
//!
//! **Its box comes from a hostile file** (rule 6): one that isn't finite, has no area or lies wholly off its page is refused, and the rest is held to the page.

use crate::paper::LEAST_BACKGROUND;
use recto_index::select::{PagePoint, PageText, Rect};
use recto_proto::ClickPlaces;
use serde_json::{Value, json};
use std::num::NonZeroU32;

/// **The image a click picked:** its page (as the view counts), its number as `recto images` counts, its box on the page as the file shows it (before an
/// unsaved turn, which the page's layer adds), and the point clicked, which a copy takes it by, as the snapshot tool's click does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImagePick {
    pub page: u32,
    pub number: u32,
    pub rect: Rect,
    pub at: (f32, f32),
}

/// **Whether text that shows is where a click landed,** as the page's text says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The page's text isn't here.
    Unknown,
    /// There's text there whose hidden-text check hasn't been tried.
    Unchecked,
    /// No text that shows there: maybe an image's.
    Image,
    /// Text that shows there: text's to select.
    Text,
}

/// **What a page's text says of a point on it** ([`Verdict`]). `settled`: nothing more will be known of its hidden text, as its check has been tried
/// however it went, or the page is heavy and never drawn for one; text there is then taken as showing, as a copy takes it.
pub fn verdict_at(text: Option<&PageText>, settled: bool, point: PagePoint) -> Verdict {
    match text {
        None => Verdict::Unknown,
        Some(text) if !text.is_over_text(point.x, point.y) => Verdict::Image,
        Some(text) if text.hidden_checked() || settled => Verdict::Text,
        Some(_) => Verdict::Unchecked,
    }
}

/// **Whether any of a page's pictures may be under a click,** as the tab's own worker has their boxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Under {
    Yes,
    No,
}

/// **Whether a picture covering `covered` of a page of `page` (any one unit of area) is the page:** what's left of the page round it is under
/// [`LEAST_BACKGROUND`] of it. A page of no area has no picture, so nothing is the page.
fn covers(covered: f64, page: f64) -> bool {
    covered > 0.0 && page > 0.0 && page - covered <= page * LEAST_BACKGROUND
}

/// **Whether a picture's `rect`, held to its page of `size`, is the page** (the module's notes): the one rule of what a click never picks. A box a hostile
/// file gives is held first; one refused there (not finite, off the page, a page of no size) is no page and no picture.
pub fn is_the_page(rect: Rect, size: (f32, f32)) -> bool {
    held_to_page(rect, size).is_some_and(|held| covers(f64::from(held.width) * f64::from(held.height), f64::from(size.0) * f64::from(size.1)))
}

/// [`is_the_page`] for a box of the worker's, in 65535ths of the page: the same rule, on the same share of the page.
fn is_the_page_in_parts([left, top, right, bottom]: [u16; 4]) -> bool {
    let side = |from: u16, to: u16| f64::from(to.saturating_sub(from));
    covers(side(left, right) * side(top, bottom), f64::from(u16::MAX) * f64::from(u16::MAX))
}

/// **Whether a picture may be under `at`,** as the tab's own worker has the page's pictures (`recto_proto::ClickPlaces`), all in 65535ths of the page
/// from its top left (`[left, top, right, bottom]` and `[x, y]`), edges included. A picture's box that is the page's own ([`is_the_page`]) doesn't count:
/// held alone it may be a scan, or past 64 apart a fold of several pictures into one (`recto-render`'s `places.rs`), which the box can't tell, so the
/// page's figures folded with a scan past 64 pictures are the one thing a click there can't pick. `beyond`, the walk's "a picture may be anywhere in
/// here" (round every picture past the 64 held, or the whole page where a place couldn't be read or the walk was cut short), always counts: it is never
/// a picture's own box, so the look runs and its answer's own box is judged ([`is_the_page`]). `None`, places the worker couldn't give (its page failed,
/// withheld, refused, stopped), is no picture: a click picks nothing there. A box from a hostile file is only compared, so one upside down or of no area
/// holds nothing.
pub fn under(places: Option<&ClickPlaces>, [x, y]: [u16; 2]) -> Under {
    let holds = |&[left, top, right, bottom]: &[u16; 4]| (left..=right).contains(&x) && (top..=bottom).contains(&y);
    let may_be = |places: &ClickPlaces| places.beyond.as_ref().is_some_and(holds) || places.boxes.iter().any(|one| holds(one) && !is_the_page_in_parts(*one));
    if places.is_some_and(may_be) { Under::Yes } else { Under::No }
}

/// **A point on a page of `size` points, in 65535ths of it,** as the worker's boxes are: `None` for a page of no size or a point that isn't finite, and a
/// point off the page held to its edge.
pub fn in_parts(point: PagePoint, (width, height): (f32, f32)) -> Option<[u16; 2]> {
    let finite = [point.x, point.y, width, height].iter().all(|value| value.is_finite());
    if !finite || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let part = |value: f32, of: f32| ((value / of).clamp(0.0, 1.0) * 65_535.0).round() as u16;
    Some([part(point.x, width), part(point.y, height)])
}

/// **Where a click waiting for its image has got** (the module's notes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    /// No click waits.
    #[default]
    Idle,
    /// The page's text is to say whether text that shows is there.
    Text,
    /// No text that shows there: the tab's own worker is to say whether an image is.
    Boxes,
    /// Text not checked for hidden text there: the worker first, and the check only if a picture may be under it.
    BoxesThenCheck,
    /// A picture may be under text not checked: the page's hidden-text check is to say whether the text shows.
    Check,
    /// An image may be there, and the app's to be told to look.
    Look,
    /// The app's been told, and its look is on its way or waits its turn.
    Looking,
}

/// The click waiting, its serial and its stage; and whether a click the app was told to look for has gone, which the app is to hear so its look stops.
#[derive(Clone, Copy, Debug)]
struct Wanted {
    point: PagePoint,
    serial: u32,
    stage: Stage,
    dropped: bool,
}

impl Default for Wanted {
    fn default() -> Self {
        Self {
            point: PagePoint { page: 0, x: 0.0, y: 0.0 },
            serial: 0,
            stage: Stage::Idle,
            dropped: false,
        }
    }
}

/// The image found, and whether the host has been told of it.
#[derive(Debug)]
struct Picked {
    pick: ImagePick,
    told: bool,
}

/// **What the page view holds of an image picked with a click.** See the module's notes.
#[derive(Debug, Default)]
pub struct ImageSelection {
    wanted: Wanted,
    /// The page whose layer has the box, plus one, as far as the host's been told.
    drawn: Option<NonZeroU32>,
    picked: Option<Box<Picked>>,
}

// size: what each page view (a pane) holds for it, nothing picked or something: a click with its serial and stage, the page its box is on, and a pointer
// (32 bytes on 2026-11-16, `build/image-select`; the serial and stage fit as the page became a `NonZeroU32`).
const _: () = assert!(size_of::<ImageSelection>() <= 32, "what a page view holds of an image picked grew");

/// What [`ImageSelection::update`] tells the host: a box taken off a page, and one put on a page.
pub type BoxOps = [Option<(u32, Option<Rect>)>; 2];

impl ImageSelection {
    /// **The text selection ended or changed** ([`crate::selection::Selecting`]'s one call): what's picked and what's waiting go, but for a press of one
    /// click inside the box of what's picked and off text that shows (`pressed_at`, which the text selection gives only for such a press), which may be a
    /// click on it again and keeps it.
    pub fn ended(&mut self, pressed_at: Option<PagePoint>) {
        if pressed_at.is_some_and(|point| self.holds(point)) {
            self.drop_wanted();
            return;
        }
        self.let_go();
    }

    /// **A still single click at `point`,** not on a link, with what the page's text says there, as the `serial`th of the selection's clock: unless it's
    /// on text that shows, which is text's, or inside the box of what's picked, which keeps it, its image is wanted in place of any picked or waiting.
    pub fn clicked(&mut self, point: PagePoint, serial: u32, verdict: Verdict) {
        if self.holds(point) {
            return;
        }
        self.picked = None;
        let stage = match verdict {
            Verdict::Text => Stage::Idle,
            Verdict::Unknown => Stage::Text,
            Verdict::Unchecked => Stage::BoxesThenCheck,
            Verdict::Image => Stage::Boxes,
        };
        self.drop_wanted();
        self.wanted = Wanted {
            point,
            serial,
            stage,
            dropped: self.wanted.dropped,
        };
    }

    /// Whether `point` is inside the box of the image picked, on its page.
    fn holds(&self, point: PagePoint) -> bool {
        self.picked.as_ref().is_some_and(|picked| {
            let rect = picked.pick.rect;
            picked.pick.page == point.page && (rect.x..=rect.x + rect.width).contains(&point.x) && (rect.y..=rect.y + rect.height).contains(&point.y)
        })
    }

    /// No click waits now; one the app was told to look for is news.
    fn drop_wanted(&mut self) {
        if self.wanted.stage == Stage::Looking {
            self.wanted.dropped = true;
        }
        self.wanted.stage = Stage::Idle;
    }

    /// The click whose page's text is to say whose it is.
    pub fn text_wanted(&self) -> Option<PagePoint> {
        (self.wanted.stage == Stage::Text).then_some(self.wanted.point)
    }

    /// **What the page's text says now, of a click on `page` waiting for it:** on text that shows it goes, as text's; off it, or on text not checked, the
    /// worker's asked.
    pub fn text_read(&mut self, page: u32, verdict: impl FnOnce(PagePoint) -> Verdict) {
        if self.wanted.stage != Stage::Text || self.wanted.point.page != page {
            return;
        }
        match verdict(self.wanted.point) {
            Verdict::Text => self.wanted.stage = Stage::Idle,
            Verdict::Image => self.wanted.stage = Stage::Boxes,
            Verdict::Unchecked => self.wanted.stage = Stage::BoxesThenCheck,
            Verdict::Unknown => {}
        }
    }

    /// The click the tab's own worker is to say of, with its serial.
    pub fn boxes_wanted(&self) -> Option<(PagePoint, u32)> {
        matches!(self.wanted.stage, Stage::Boxes | Stage::BoxesThenCheck).then_some((self.wanted.point, self.wanted.serial))
    }

    /// **What the worker said of the `serial`th click,** if it's still the one waiting for that: no picture there and it goes; one may be, and the app's
    /// to look, or on text not checked the check's asked first.
    pub fn boxes_said(&mut self, serial: u32, under: Under) {
        if self.boxes_wanted().is_none() || self.wanted.serial != serial {
            return;
        }
        self.wanted.stage = match (under, self.wanted.stage) {
            (Under::No, _) => Stage::Idle,
            (Under::Yes, Stage::BoxesThenCheck) => Stage::Check,
            (Under::Yes, _) => Stage::Look,
        };
    }

    /// The click on text not checked, with a picture maybe under it, whose page's hidden-text check is to say whose it is.
    pub fn check_wanted(&self) -> Option<PagePoint> {
        (self.wanted.stage == Stage::Check).then_some(self.wanted.point)
    }

    /// **What the page's text says now its check is in,** of a click on `page` waiting for it: text that shows keeps it; hidden text there doesn't, and the
    /// app's to look.
    pub fn check_read(&mut self, page: u32, verdict: impl FnOnce(PagePoint) -> Verdict) {
        if self.wanted.stage != Stage::Check || self.wanted.point.page != page {
            return;
        }
        match verdict(self.wanted.point) {
            Verdict::Text => self.wanted.stage = Stage::Idle,
            Verdict::Image => self.wanted.stage = Stage::Look,
            Verdict::Unchecked | Verdict::Unknown => {}
        }
    }

    /// Whether the app has news to hear ([`Self::take_news`]), not taking it.
    pub fn has_news(&self) -> bool {
        self.wanted.stage == Stage::Look || self.wanted.dropped
    }

    /// **Whether the app is to be told,** once: a click is ready to be looked for, or one it was told to look for has gone. Told with whatever else
    /// changed of the view (`Changed`), so the app's look for it starts or stops.
    pub fn take_news(&mut self) -> bool {
        let ready = self.wanted.stage == Stage::Look;
        if ready {
            self.wanted.stage = Stage::Looking;
        }
        ready | std::mem::take(&mut self.wanted.dropped)
    }

    /// The click the app was told to look for, with its serial.
    pub fn looking(&self) -> Option<(PagePoint, u32)> {
        (self.wanted.stage == Stage::Looking).then_some((self.wanted.point, self.wanted.serial))
    }

    /// **The image found for the `serial`th click,** taken if that click is still the one being looked for: its box held to the page (`page_size`, as
    /// the view has it), or refused if it can't be shown there. Says whether it's picked.
    pub fn found(&mut self, serial: u32, number: u32, rect: Rect, page_size: (f32, f32)) -> bool {
        if self.looking().is_none_or(|(_, looking)| looking != serial) {
            return false;
        }
        let point = self.wanted.point;
        self.wanted.stage = Stage::Idle;
        let Some(rect) = held_to_page(rect, page_size).filter(|_| !is_the_page(rect, page_size)) else {
            return false;
        };
        self.picked = Some(Box::new(Picked {
            pick: ImagePick {
                page: point.page,
                number,
                rect,
                at: (point.x, point.y),
            },
            told: false,
        }));
        true
    }

    /// No image for the `serial`th click, or it couldn't be looked for: it's no longer waiting, if it still was.
    pub fn not_found(&mut self, serial: u32) {
        if self.looking().is_some_and(|(_, looking)| looking == serial) {
            self.wanted.stage = Stage::Idle;
        }
    }

    /// The image picked.
    pub fn picked(&self) -> Option<ImagePick> {
        self.picked.as_ref().map(|picked| picked.pick)
    }

    /// A click is waiting for its image, at whatever stage.
    pub fn finding(&self) -> bool {
        self.wanted.stage != Stage::Idle
    }

    /// **Everything goes:** what's picked and what's waiting. Its box comes off at the host's next update.
    pub fn let_go(&mut self) {
        self.drop_wanted();
        self.picked = None;
    }

    /// The host's layers have gone, and the box with them.
    pub fn layers_gone(&mut self) {
        self.drawn = None;
        if let Some(picked) = &mut self.picked {
            picked.told = false;
        }
    }

    /// A page's layer has gone, and the box with it if it was there.
    pub fn page_hidden(&mut self, page: u32) {
        if self.drawn_page() == Some(page) {
            self.layers_gone();
        }
    }

    fn drawn_page(&self) -> Option<u32> {
        self.drawn.map(|page| page.get() - 1)
    }

    /// Nothing picked and nothing on a layer: no update can have anything to say.
    pub fn at_rest(&self) -> bool {
        self.picked.is_none() && self.drawn.is_none()
    }

    /// **What the host is told now,** for the pages `shown`: the box off the page it's on if it's to go or move, and on its page once that page shows.
    pub fn update(&mut self, shown: impl Fn(u32) -> bool) -> BoxOps {
        let drawn = self.drawn_page();
        let want = self.picked.as_deref_mut().filter(|picked| shown(picked.pick.page));
        let mut ops: BoxOps = [None, None];
        let mut on = drawn;
        if let Some(page) = drawn
            && want.as_ref().is_none_or(|picked| picked.pick.page != page)
        {
            ops[0] = Some((page, None));
            on = None;
        }
        if let Some(picked) = want
            && (!picked.told || on.is_none())
            && let Some(page) = picked.pick.page.checked_add(1).and_then(NonZeroU32::new)
        {
            picked.told = true;
            on = Some(picked.pick.page);
            self.drawn = Some(page);
            ops[1] = Some((picked.pick.page, Some(picked.pick.rect)));
        }
        if on.is_none() {
            self.drawn = None;
        }
        ops
    }

    /// For `qa state`: what's picked (its page from 1, number and box), whether a click's image is being looked for at any stage, and the page (from 1)
    /// the host's been told has its box. Nothing of the file's words.
    pub fn report(&self) -> Value {
        json!({
            "finding": self.finding(),
            "box_on_page": self.drawn.map(NonZeroU32::get),
            "picked": self.picked().map(|pick| json!({
                "page": pick.page + 1,
                "number": pick.number,
                "rect": [pick.rect.x, pick.rect.y, pick.rect.width, pick.rect.height],
            })),
        })
    }
}

/// **A box from the file, held to its page:** refused if any of it isn't finite, the page has no size, or nothing of it is on the page; otherwise cut to
/// the page's edges.
pub fn held_to_page(rect: Rect, (width, height): (f32, f32)) -> Option<Rect> {
    let numbers = [rect.x, rect.y, rect.width, rect.height, width, height];
    if !numbers.iter().all(|number| number.is_finite()) || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let (left, top) = (rect.x.max(0.0), rect.y.max(0.0));
    let (right, bottom) = ((rect.x + rect.width).min(width), (rect.y + rect.height).min(height));
    (right > left && bottom > top).then_some(Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: (f32, f32) = (612.0, 792.0);

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }

    fn at(page: u32, x: f32, y: f32) -> PagePoint {
        PagePoint { page, x, y }
    }

    /// The worker's places for a click of these pictures' boxes, with nothing past them.
    fn only(boxes: &[[u16; 4]]) -> ClickPlaces {
        ClickPlaces {
            boxes: boxes.to_vec(),
            beyond: None,
        }
    }

    /// A click at `point` taken through every stage to its look, as the `serial`th, nothing there being text and the worker saying an image is.
    fn looking_at(image: &mut ImageSelection, point: PagePoint, serial: u32) {
        image.clicked(point, serial, Verdict::Image);
        image.boxes_said(serial, Under::Yes);
        assert!(image.take_news(), "the app's told to look");
    }

    /// A selection with the image for a click at `point` found.
    fn picked_at(point: PagePoint, rect: Rect) -> ImageSelection {
        let mut image = ImageSelection::default();
        looking_at(&mut image, point, 1);
        assert!(image.found(1, 1, rect, LETTER));
        image
    }

    /// Failure (rule 6): a box a hostile file gives (NaN, infinite, of no area, wholly off its page, or larger than it) is drawn as it is, so the host gets
    /// a layer path of nothing or of everything, or a panic on an overflow. Refused or held to the page.
    #[test]
    fn a_hostile_box_is_refused_or_held_to_its_page() {
        assert_eq!(held_to_page(rect(10.0, 20.0, 30.0, 40.0), LETTER), Some(rect(10.0, 20.0, 30.0, 40.0)));
        assert_eq!(
            held_to_page(rect(-50.0, 700.0, 1e30, 1e30), LETTER),
            Some(rect(0.0, 700.0, 612.0, 92.0)),
            "held to the page"
        );
        for bad in [
            rect(f32::NAN, 0.0, 10.0, 10.0),
            rect(0.0, 0.0, f32::INFINITY, 10.0),
            rect(0.0, 0.0, 0.0, 10.0),
            rect(0.0, 0.0, 10.0, -5.0),
            rect(700.0, 10.0, 10.0, 10.0),
            rect(f32::MAX, f32::MAX, f32::MAX, f32::MAX),
        ] {
            assert_eq!(held_to_page(bad, LETTER), None, "{bad:?}");
        }
        assert_eq!(held_to_page(rect(0.0, 0.0, 10.0, 10.0), (f32::NAN, 10.0)), None, "a page of no size");
    }

    /// Failure (owner, 2026-11-16): the line between a figure and a scan is drawn in the wrong place, so a scan with a thin scanner margin is picked, or a
    /// figure that fills most of a page is taken for the page. A picture whose box leaves a twentieth of the page or more uncovered is picked; one that
    /// leaves less is the page, at the line included. Made boxes on a page of 100 by 100, so the sums are exact, and the worker's boxes in 65535ths agree.
    #[test]
    fn a_picture_leaving_under_a_twentieth_of_the_page_is_the_page() {
        let page = (100.0, 100.0);
        assert!(is_the_page(rect(0.0, 0.0, 100.0, 100.0), page), "the whole page");
        assert!(
            is_the_page(rect(0.0, 0.0, 95.0, 100.0), page),
            "at the line: a twentieth left is the page's margin"
        );
        assert!(!is_the_page(rect(0.0, 0.0, 94.0, 100.0), page), "just under it: a figure");
        assert!(is_the_page(rect(-30.0, -30.0, 500.0, 500.0), page), "drawn past the page, held to it");
        assert!(!is_the_page(rect(10.0, 10.0, 50.0, 50.0), page));
        for bad in [rect(f32::NAN, 0.0, 100.0, 100.0), rect(0.0, 0.0, 0.0, 100.0), rect(200.0, 0.0, 100.0, 100.0)] {
            assert!(!is_the_page(bad, page), "a hostile box is no page: {bad:?}");
        }
        assert!(!is_the_page(rect(0.0, 0.0, 10.0, 10.0), (0.0, 0.0)), "a page of no size picks nothing");
        let whole = u16::MAX;
        assert!(is_the_page_in_parts([0, 0, whole, whole]));
        assert!(is_the_page_in_parts([0, 0, 62_259, whole]), "95% across, a hair over the line");
        assert!(!is_the_page_in_parts([0, 0, 61_000, whole]), "93%");
        assert!(!is_the_page_in_parts([40_000, 40_000, 20_000, 20_000]), "upside down");
    }

    /// Failure (owner, 2026-11-16): a click on a plain scan still asks for a private worker, or a figure over a scan can't be picked because the scan's box
    /// hides it. Only the boxes that aren't the page count: with the scan alone under the point, no picture is; with a figure on it too, one is.
    #[test]
    fn a_scans_box_alone_under_a_point_is_no_picture_but_a_figure_over_it_is() {
        let scan = [0, 0, u16::MAX, u16::MAX];
        let figure = [20_000, 20_000, 40_000, 40_000];
        assert_eq!(under(Some(&only(&[scan])), [30_000, 30_000]), Under::No);
        assert_eq!(under(Some(&only(&[scan, figure])), [30_000, 30_000]), Under::Yes);
        assert_eq!(
            under(Some(&only(&[scan, figure])), [50_000, 50_000]),
            Under::No,
            "on the scan beside the figure"
        );
    }

    /// Failure (the review of `build/scan-not-picked`, its second regression, and the case it left): the walk's "a picture may be anywhere" box, which
    /// a walk cut short makes the whole page, is read as a picture that is the page wherever it falls among the boxes, so on a page of over 20,000
    /// objects with an inset picture after them, or a page of fewer than 64 pictures where one's place couldn't be read, a click on the picture picks
    /// nothing. `beyond` always counts, on any page, whatever covers it; a picture's own box that is the page still doesn't.
    #[test]
    fn a_box_where_a_picture_may_be_anywhere_is_never_the_page() {
        let whole = [0, 0, u16::MAX, u16::MAX];
        let cut_short = |boxes: &[[u16; 4]]| ClickPlaces {
            boxes: boxes.to_vec(),
            beyond: Some(whole),
        };
        assert_eq!(under(Some(&cut_short(&[])), [30_000, 30_000]), Under::Yes, "no pictures met before the cap");
        assert_eq!(
            under(Some(&cut_short(&[[1, 1, 9, 9]])), [30_000, 30_000]),
            Under::Yes,
            "one met, of fewer than 64"
        );
        assert_eq!(
            under(Some(&cut_short(&[whole])), [30_000, 30_000]),
            Under::Yes,
            "a scan met, and the walk cut short"
        );
        assert_eq!(under(Some(&only(&[whole])), [30_000, 30_000]), Under::No, "the same scan, the walk whole");
        let round_the_rest = ClickPlaces {
            boxes: vec![[1, 1, 9, 9]],
            beyond: Some([20_000, 20_000, 40_000, 40_000]),
        };
        assert_eq!(under(Some(&round_the_rest), [30_000, 30_000]), Under::Yes);
        assert_eq!(under(Some(&round_the_rest), [50_000, 50_000]), Under::No, "outside every box");
    }

    /// Failure (owner, 2026-11-16): the look finds the picture on top, and it is the page (the figure's box was folded into a scan's, or the scan was all the
    /// worker could say), so a click on a scan is picked. A look's answer that is the page picks nothing; one beside it is picked.
    #[test]
    fn a_look_that_found_the_page_picks_nothing() {
        let mut image = ImageSelection::default();
        looking_at(&mut image, at(0, 300.0, 400.0), 1);
        assert!(!image.found(1, 1, rect(0.0, 0.0, 612.0, 792.0), LETTER));
        assert!(image.picked().is_none() && !image.finding(), "nothing picked and the click is done");
        assert!(picked_at(at(0, 300.0, 400.0), rect(100.0, 100.0, 400.0, 400.0)).picked().is_some());
    }

    /// Failure (S1 of the review): the worker's boxes say an image is under a point it isn't, so a click on a margin starts a private worker anyway, or
    /// say none is under one it is, so an image can't be picked; or a hostile box (upside down, of no area) or a point off the page or not finite panics or
    /// is taken for an image. The point is put in the boxes' 65535ths of the page, held to it, and compared with each box, edges included.
    #[test]
    fn the_workers_boxes_say_whether_an_image_is_under_a_point() {
        let middle = in_parts(at(0, 306.0, 396.0), LETTER).expect("on the page");
        assert_eq!(middle, [32_768, 32_768]);
        assert_eq!(in_parts(at(0, -5.0, 9_000.0), LETTER), Some([0, 65_535]), "held to the page");
        assert_eq!(in_parts(at(0, f32::NAN, 1.0), LETTER), None);
        assert_eq!(in_parts(at(0, 1.0, 1.0), (0.0, 792.0)), None, "a page of no size");
        let figure = [20_000, 20_000, 40_000, 40_000];
        assert_eq!(under(Some(&only(&[figure])), middle), Under::Yes);
        assert_eq!(under(Some(&only(&[figure])), [40_000, 20_000]), Under::Yes, "on its edge");
        assert_eq!(under(Some(&only(&[figure])), [40_001, 30_000]), Under::No);
        assert_eq!(under(Some(&only(&[])), middle), Under::No, "a page with no images");
        assert_eq!(
            under(Some(&only(&[[40_000, 40_000, 20_000, 20_000], [5, 5, 5, 5]])), middle),
            Under::No,
            "upside down, or of no area"
        );
        assert_eq!(under(None, middle), Under::No, "a page the worker can't say of: no picture to a click");
    }

    /// Failure (S1 of the review): a click is looked for before what's known says an image may be there, so a click on text, on a margin with no image,
    /// or on text whose page isn't checked costs a private worker; or the app's told twice, or not at all. Only a click with no text that shows and the
    /// worker saying a picture may be there is news, once.
    #[test]
    fn the_app_is_told_to_look_only_where_an_image_may_be() {
        let point = at(0, 100.0, 100.0);
        let mut image = ImageSelection::default();
        image.clicked(point, 1, Verdict::Text);
        assert!(!image.finding() && !image.take_news(), "on text that shows: nothing's looked for");
        image.clicked(point, 2, Verdict::Image);
        assert_eq!(image.boxes_wanted(), Some((point, 2)), "the worker's asked first");
        assert!(!image.take_news(), "not before it says");
        image.boxes_said(2, Under::No);
        assert!(!image.finding() && !image.take_news(), "no image there: nothing's looked for");
        image.clicked(point, 3, Verdict::Image);
        image.boxes_said(2, Under::Yes);
        assert_eq!(image.looking(), None, "an older click's answer says nothing");
        image.boxes_said(3, Under::Yes);
        assert!(image.take_news(), "looked for");
        assert!(!image.take_news(), "once");
        assert_eq!(image.looking(), Some((point, 3)));
    }

    /// Failure (S1 of the review, "Unknown always resolves"; the second review's fix 1): a click on text whose page isn't checked is decided before the
    /// check (so a scan's hidden words keep the click from the scan), or waits for good: on a page whose check failed, a heavy page that's never checked,
    /// or a page whose text isn't read yet; or the check, a draw of the whole page, is run for a click on text with no picture under it, which `main` never
    /// drew for. The worker's asked first; the check only if a picture may be there, and it decides when it comes; one tried and failed, or a heavy page,
    /// takes the text as showing.
    #[test]
    fn whose_a_click_on_text_is_always_comes_to_an_answer() {
        let line = |checked: bool| {
            let frame = recto_proto::PageFrame {
                turns: 0,
                left: 0.0,
                bottom: 0.0,
                right: 612.0,
                top: 792.0,
            };
            let chars: Vec<_> = "Caption".chars().map(|ch| recto_proto::CharRecord::new(ch as u32, 0)).collect();
            let boxes: Vec<recto_proto::CharBox> = (0..chars.len())
                .map(|at| [72.0 + at as f32 * 6.0, 700.0, 77.0 + at as f32 * 6.0, 710.0])
                .collect();
            PageText::build(&chars, &boxes, frame, checked, false)
        };
        let on_text = at(0, 80.0, 87.0);
        let (unchecked, checked) = (line(false), line(true));
        assert_eq!(verdict_at(None, true, on_text), Verdict::Unknown, "no text held yet: waits for its read");
        assert_eq!(verdict_at(Some(&unchecked), false, on_text), Verdict::Unchecked, "read without the check");
        assert_eq!(
            verdict_at(Some(&unchecked), true, on_text),
            Verdict::Text,
            "a check tried and failed, or a heavy page: showing"
        );
        assert_eq!(verdict_at(Some(&checked), false, on_text), Verdict::Text);
        assert_eq!(verdict_at(Some(&unchecked), false, at(0, 300.0, 300.0)), Verdict::Image, "off the text");

        let mut image = ImageSelection::default();
        image.clicked(on_text, 1, Verdict::Unknown);
        assert_eq!(image.text_wanted(), Some(on_text));
        image.text_read(1, |_| Verdict::Image);
        assert_eq!(image.text_wanted(), Some(on_text), "another page's text says nothing");
        image.text_read(0, |_| Verdict::Unknown);
        assert_eq!(image.text_wanted(), Some(on_text), "still not known");
        image.text_read(0, |_| Verdict::Unchecked);
        assert_eq!(image.boxes_wanted(), Some((on_text, 1)), "text not checked: the worker's asked first");
        assert_eq!(image.check_wanted(), None, "and no check yet");
        image.boxes_said(1, Under::No);
        assert!(
            !image.finding() && image.check_wanted().is_none(),
            "no picture under it: no check, nothing looked for"
        );

        image.clicked(on_text, 2, Verdict::Unchecked);
        image.boxes_said(2, Under::Yes);
        assert_eq!(image.check_wanted(), Some(on_text), "a picture may be under it: now the check");
        assert!(!image.take_news(), "nothing looked for before it");
        image.check_read(0, |_| Verdict::Image);
        assert!(image.take_news(), "hidden there: looked for");
        image.not_found(2);

        image.clicked(on_text, 3, Verdict::Unchecked);
        image.boxes_said(3, Under::Yes);
        image.check_read(0, |_| Verdict::Text);
        assert!(!image.finding() && !image.take_news(), "it shows: text's");
        image.clicked(on_text, 4, Verdict::Unknown);
        image.text_read(0, |_| Verdict::Text);
        assert!(!image.finding(), "read, it shows: text's, and nothing's asked");
    }

    /// Failure ("the last thing a person asked for wins"; should-fix 2 of the review): a look's answer is taken by a click it wasn't for (a newer one, even
    /// at the same point after a page move, or one let go before it landed), so the box comes back after the person has left it or shows another page's
    /// image; or a click the app was told to look for goes without the app hearing, so its private worker runs on. Only the serial looked for takes what's
    /// found, once, and one that goes while it's looked for is news.
    #[test]
    fn only_the_click_still_looked_for_takes_its_image() {
        let point = at(0, 300.0, 300.0);
        let mut image = ImageSelection::default();
        looking_at(&mut image, point, 1);
        image.clicked(point, 2, Verdict::Image);
        assert!(image.take_news(), "the click looked for went: the app hears");
        image.boxes_said(2, Under::Yes);
        assert!(image.take_news());
        assert!(
            !image.found(1, 1, rect(0.0, 0.0, 50.0, 50.0), LETTER),
            "the older click's image, at the same point"
        );
        assert!(image.found(2, 2, rect(250.0, 250.0, 100.0, 100.0), LETTER));
        assert_eq!(image.picked().map(|pick| (pick.number, pick.at)), Some((2, (300.0, 300.0))));
        assert!(!image.found(2, 3, rect(0.0, 0.0, 9.0, 9.0), LETTER), "once");
        looking_at(&mut image, at(0, 5.0, 5.0), 3);
        assert_eq!(image.picked(), None, "a new click lets the old image go");
        image.let_go();
        assert!(image.take_news(), "let go while looked for: the app hears");
        assert!(!image.take_news(), "once");
        assert!(!image.found(3, 1, rect(0.0, 0.0, 9.0, 9.0), LETTER), "let go before it landed");
        image.clicked(at(0, 5.0, 5.0), 4, Verdict::Image);
        image.let_go();
        assert!(!image.take_news(), "one never looked for goes without news");
    }

    /// Failure (S1 (c) of the review): a click inside the box of the image picked looks for it again, a private worker for what's already picked, and its
    /// press takes the box off for the release to put it back, a flicker. The press and the click keep it, and tell the host nothing; a press elsewhere,
    /// or a double click inside it, lets it go.
    #[test]
    fn a_click_inside_the_box_held_keeps_it_and_asks_nothing() {
        let figure = rect(100.0, 100.0, 200.0, 150.0);
        let mut image = picked_at(at(0, 150.0, 150.0), figure);
        assert_eq!(image.update(|_| true), [None, Some((0, Some(figure)))]);
        let inside = at(0, 250.0, 200.0);
        image.ended(Some(inside));
        image.clicked(inside, 9, Verdict::Image);
        assert_eq!(image.picked().map(|pick| pick.rect), Some(figure), "kept");
        assert!(!image.finding() && !image.take_news(), "not looked for again");
        assert_eq!(image.update(|_| true), [None, None], "its box not taken off and put back");
        image.ended(Some(at(1, 250.0, 200.0)));
        assert_eq!(image.picked(), None, "a press on another page");
        let mut image = picked_at(at(0, 150.0, 150.0), figure);
        image.ended(None);
        assert_eq!(image.picked(), None, "a double click, a shift-click or a drag that selects");
    }

    /// Failure: the box isn't drawn when its page shows, stays on a page after it's let go, isn't drawn again on a page shown again after it scrolled away,
    /// or is told twice. What the host is told follows what's picked and what's shown.
    #[test]
    fn the_box_is_told_once_and_taken_off_when_it_goes() {
        let mut image = picked_at(at(2, 10.0, 10.0), rect(0.0, 0.0, 50.0, 50.0));
        let box_on = Some((2, Some(rect(0.0, 0.0, 50.0, 50.0))));
        assert_eq!(image.update(|_| false), [None, None], "not while its page isn't shown");
        assert_eq!(image.update(|_| true), [None, box_on]);
        assert_eq!(image.update(|_| true), [None, None], "once");
        image.page_hidden(2);
        assert_eq!(image.update(|_| true), [None, box_on], "again on the page shown again");
        image.let_go();
        assert!(!image.at_rest(), "the box is still on the layer");
        assert_eq!(image.update(|_| true), [Some((2, None)), None]);
        assert!(image.at_rest());
        let mut kept = picked_at(at(1, 1.0, 1.0), rect(0.0, 0.0, 5.0, 5.0));
        let _ = kept.update(|_| true);
        kept.ended(None);
        assert_eq!(kept.picked(), None, "the text selection ended lets go");
        assert_eq!(kept.update(|_| true), [Some((1, None)), None], "and the box still comes off");
    }
}
