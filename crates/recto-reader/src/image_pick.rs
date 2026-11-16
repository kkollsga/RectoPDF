//! **An image picked with a click in read mode** (owner, 2026-11-16: "when in normal read mode (with left click to select text) clicking a image will show a
//! selection box. Clicking cmd+c will copy lossless to clipboard."). What the page view holds of it: the click waiting for its image to be found, the image
//! found, and the page whose layer has its box. Finding the image is the app's, a read through a worker off this thread (`recto images --at`, found as the
//! snapshot tool's click finds it); keeping it, and letting it go, is here.
//!
//! **It's the other kind of selection, held beside the text selection** ([`crate::selection::Selecting`]), so whatever ends or changes that ends this: a
//! press, Select All, Find's hit, another tab or other pages, the file seen to change, pages withheld. Text and an image are never both selected.
//!
//! **Text that shows keeps a click** (coordinator, 2026-11-16): a caption drawn over a figure is text's to select, and hidden text, as a scan's OCR layer,
//! isn't. Which it is, is the page's text with its hidden-text check ([`Verdict`]): a page's first read has no check, as the check draws the page, so a
//! click on text not yet checked is looked at again once the check is in, which a click asks for. A page known to be heavy isn't drawn for the check, as
//! for a copy, and its text is taken as showing. Nothing's drawn as picked until it's known to be an image's.
//!
//! **The last thing the person asked for wins.** A click's image is taken only if that click is still the one waiting ([`ImageSelection::found`]): a press
//! since, another click, or anything that resets the selection drops what's waiting, so a read landing late shows nothing.
//!
//! **What it costs:** a point while a click waits, and once an image is found, one small allocation off the UI thread with its page, number, box and the
//! point clicked. Nothing per page or tab, and nothing at all with nothing picked.
//!
//! **Its box comes from a hostile file** (rule 6): one that isn't finite, has no area or lies wholly off its page is refused, and the rest is held to the page.

use recto_index::select::{PagePoint, PageText, Rect};
use serde_json::{Value, json};

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
    /// The page's text isn't here, or there's text there whose hidden-text check hasn't run.
    Unknown,
    /// No text that shows there: the image's.
    Image,
    /// Text that shows there: text's to select.
    Text,
}

/// What a page's text says of a point on it ([`Verdict`]).
pub fn verdict_at(text: Option<&PageText>, point: PagePoint) -> Verdict {
    match text {
        None => Verdict::Unknown,
        Some(text) if !text.is_over_text(point.x, point.y) => Verdict::Image,
        Some(text) if text.hidden_checked() => Verdict::Text,
        Some(_) => Verdict::Unknown,
    }
}

/// A click waiting for its image, and what's known of the text there.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Wanted {
    point: PagePoint,
    verdict: Verdict,
}

/// The image found, what's known of the text where it was clicked, and whether the host has been told of it.
#[derive(Debug)]
struct Picked {
    pick: ImagePick,
    verdict: Verdict,
    told: bool,
}

/// **What the page view holds of an image picked with a click.** See the module's notes.
#[derive(Debug, Default)]
pub struct ImageSelection {
    wanted: Option<Wanted>,
    picked: Option<Box<Picked>>,
    /// The page whose layer has the box, as far as the host's been told.
    drawn: Option<u32>,
}

// size: what each page view (a pane) holds for it, nothing picked or something: a point, a pointer and a page (2026-11-16, `build/image-select`).
const _: () = assert!(size_of::<ImageSelection>() <= 32, "what a page view holds of an image picked grew");

/// What [`ImageSelection::update`] tells the host: a box taken off a page, and one put on a page.
pub type BoxOps = [Option<(u32, Option<Rect>)>; 2];

impl ImageSelection {
    /// **A still single click at `point`,** not on a link, with what the page's text says there: its image is wanted, in place of any picked or waiting,
    /// unless it's on text that shows, which is text's.
    pub fn clicked(&mut self, point: PagePoint, verdict: Verdict) {
        self.picked = None;
        self.wanted = (verdict != Verdict::Text).then_some(Wanted { point, verdict });
    }

    /// The click waiting for its image.
    pub fn wanted(&self) -> Option<PagePoint> {
        self.wanted.map(|wanted| wanted.point)
    }

    /// **A page's text came in:** what it says at a click on that page that's waiting, for its image or its verdict, now. One that's on text that shows
    /// goes, as text's.
    pub fn text_read(&mut self, page: u32, verdict_at: impl Fn(PagePoint) -> Verdict) {
        if let Some(wanted) = &mut self.wanted
            && wanted.point.page == page
            && wanted.verdict == Verdict::Unknown
        {
            wanted.verdict = verdict_at(wanted.point);
            if wanted.verdict == Verdict::Text {
                self.wanted = None;
            }
        }
        if let Some(picked) = &mut self.picked
            && picked.pick.page == page
            && picked.verdict == Verdict::Unknown
        {
            let (x, y) = picked.pick.at;
            picked.verdict = verdict_at(PagePoint { page, x, y });
            if picked.verdict == Verdict::Text {
                self.picked = None;
            }
        }
    }

    /// The page whose text a waiting click needs checked for hidden text before it's known whose the click is.
    pub fn awaiting_check(&self) -> Option<u32> {
        let wanted = self.wanted.filter(|wanted| wanted.verdict == Verdict::Unknown).map(|wanted| wanted.point.page);
        let picked = self
            .picked
            .as_ref()
            .filter(|picked| picked.verdict == Verdict::Unknown)
            .map(|picked| picked.pick.page);
        wanted.or(picked)
    }

    /// **The image found for the click at `point`,** taken if that click is still the one waiting: its box held to the page (`page_size`, as the view
    /// has it), or refused if it can't be shown there. Text not checked on a page known to be `heavy` is taken as showing, as a copy takes it. Says whether
    /// it's held now, picked or waiting for its page's check.
    pub fn found(&mut self, point: PagePoint, number: u32, rect: Rect, page_size: (f32, f32), heavy: bool) -> bool {
        let Some(wanted) = self.wanted.filter(|wanted| wanted.point == point) else {
            return false;
        };
        self.wanted = None;
        let verdict = match wanted.verdict {
            Verdict::Unknown if heavy => Verdict::Text,
            verdict => verdict,
        };
        let Some(rect) = held_to_page(rect, page_size).filter(|_| verdict != Verdict::Text) else {
            return false;
        };
        self.picked = Some(Box::new(Picked {
            pick: ImagePick {
                page: point.page,
                number,
                rect,
                at: (point.x, point.y),
            },
            verdict,
            told: false,
        }));
        true
    }

    /// No image for the click at `point`, or it couldn't be looked for: it's no longer waiting, if it still was.
    pub fn not_found(&mut self, point: PagePoint) {
        if self.wanted.is_some_and(|wanted| wanted.point == point) {
            self.wanted = None;
        }
    }

    /// The image picked, once it's known to be the image's.
    pub fn picked(&self) -> Option<ImagePick> {
        self.picked.as_ref().filter(|picked| picked.verdict == Verdict::Image).map(|picked| picked.pick)
    }

    /// **Everything goes:** what's picked and what's waiting. Its box comes off at the host's next update.
    pub fn let_go(&mut self) {
        self.wanted = None;
        self.picked = None;
    }

    /// The same, keeping what the host has been told, for a selection that starts again ([`crate::selection::Selecting::reset`]).
    pub fn reset(&mut self) -> Self {
        Self {
            drawn: self.drawn,
            ..Self::default()
        }
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
        if self.drawn == Some(page) {
            self.layers_gone();
        }
    }

    /// Nothing found and nothing on a layer: no update can have anything to say.
    pub fn at_rest(&self) -> bool {
        self.picked.is_none() && self.drawn.is_none()
    }

    /// **What the host is told now,** for the pages `shown`: the box off the page it's on if it's to go or move, and on its page once that page shows.
    pub fn update(&mut self, shown: impl Fn(u32) -> bool) -> BoxOps {
        let want = self
            .picked
            .as_deref_mut()
            .filter(|picked| picked.verdict == Verdict::Image && shown(picked.pick.page));
        let mut ops: BoxOps = [None, None];
        if let Some(drawn) = self.drawn
            && want.as_ref().is_none_or(|picked| picked.pick.page != drawn)
        {
            ops[0] = Some((drawn, None));
            self.drawn = None;
        }
        if let Some(picked) = want
            && (!picked.told || self.drawn.is_none())
        {
            picked.told = true;
            self.drawn = Some(picked.pick.page);
            ops[1] = Some((picked.pick.page, Some(picked.pick.rect)));
        }
        ops
    }

    /// For `qa state`: what's picked (its page from 1, number and box), whether a click's image is being looked for or its page checked, and the page
    /// (from 1) the host's been told has its box. Nothing of the file's words.
    pub fn report(&self) -> Value {
        json!({
            "finding": self.wanted.is_some() || self.awaiting_check().is_some(),
            "box_on_page": self.drawn.map(|page| page + 1),
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

    /// A selection with the image for a click at `point` found, nothing there being text.
    fn picked_at(point: PagePoint, rect: Rect) -> ImageSelection {
        let mut image = ImageSelection::default();
        image.clicked(point, Verdict::Image);
        assert!(image.found(point, 1, rect, LETTER, false));
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

    /// Failure: a read landing for a click the person has moved on from (another click, or a press since, which lets go) shows its image anyway, so the
    /// box comes back after they've left it. Only the click still waiting takes what's found, once.
    #[test]
    fn only_the_click_still_waiting_takes_its_image() {
        let mut image = ImageSelection::default();
        image.clicked(at(0, 10.0, 10.0), Verdict::Image);
        image.clicked(at(0, 300.0, 300.0), Verdict::Image);
        assert!(
            !image.found(at(0, 10.0, 10.0), 1, rect(0.0, 0.0, 50.0, 50.0), LETTER, false),
            "an older click's image"
        );
        assert!(image.found(at(0, 300.0, 300.0), 2, rect(250.0, 250.0, 100.0, 100.0), LETTER, false));
        assert_eq!(image.picked().map(|pick| (pick.number, pick.at)), Some((2, (300.0, 300.0))));
        assert!(!image.found(at(0, 300.0, 300.0), 3, rect(0.0, 0.0, 9.0, 9.0), LETTER, false), "once");
        image.clicked(at(0, 5.0, 5.0), Verdict::Image);
        assert_eq!(image.picked(), None, "a new click lets the old image go");
        image.let_go();
        assert!(
            !image.found(at(0, 5.0, 5.0), 1, rect(0.0, 0.0, 9.0, 9.0), LETTER, false),
            "let go before it landed"
        );
        image.clicked(at(0, 5.0, 5.0), Verdict::Image);
        image.not_found(at(0, 5.0, 5.0));
        assert_eq!(image.wanted(), None);
    }

    /// Failure (coordinator, 2026-11-16: a caption over a figure keeps text's click, and a scan's OCR layer doesn't): a click on text that shows picks the
    /// image under it, or a click on text the first read hasn't checked is decided before the check (so a scan's hidden words keep the click from the
    /// scan), or the check that would decide it is never asked for. On text that shows: nothing's wanted. On text not checked: the image isn't drawn as
    /// picked until the check says, and then it is if the text is hidden, or goes if it shows, whether the check comes before the look lands or after.
    /// On a heavy page, never drawn for a check, unchecked text is taken as showing.
    #[test]
    fn whose_a_click_on_text_is_waits_for_the_pages_hidden_text_check() {
        let point = at(1, 100.0, 100.0);
        let image_rect = rect(50.0, 50.0, 200.0, 200.0);
        let mut image = ImageSelection::default();
        image.clicked(point, Verdict::Text);
        assert_eq!(image.wanted(), None, "on text that shows, the click is text's");

        image.clicked(point, Verdict::Unknown);
        assert_eq!(image.awaiting_check(), Some(1), "the check is asked for");
        assert!(image.found(point, 1, image_rect, LETTER, false));
        assert_eq!(image.picked(), None, "not picked before the check");
        assert_eq!(image.update(|_| true), [None, None], "nor drawn");
        image.text_read(1, |_| Verdict::Image);
        assert_eq!(image.picked().map(|pick| pick.number), Some(1), "hidden text: the image's");
        assert_eq!(image.awaiting_check(), None);

        image.clicked(point, Verdict::Unknown);
        image.text_read(1, |_| Verdict::Text);
        assert_eq!(image.wanted(), None, "checked before the look landed, and it shows: text's");
        assert!(!image.found(point, 1, image_rect, LETTER, false));

        image.clicked(point, Verdict::Unknown);
        assert!(image.found(point, 1, image_rect, LETTER, false));
        image.text_read(0, |_| Verdict::Image);
        assert_eq!(image.picked(), None, "another page's text says nothing");
        image.text_read(1, |_| Verdict::Text);
        assert_eq!((image.picked(), image.awaiting_check()), (None, None), "checked after, and it shows: gone");

        image.clicked(point, Verdict::Unknown);
        assert!(!image.found(point, 1, image_rect, LETTER, true), "a heavy page's unchecked text shows");
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
        let mut reset = kept.reset();
        assert_eq!(reset.picked(), None, "a reset lets go");
        assert_eq!(reset.update(|_| true), [Some((1, None)), None], "and the box still comes off");
    }
}
