//! A selection in the page view: what the reader's pointer has done, what's been read to work it out, and a copy on its way. No toolkit, thread or renderer: [`crate::tiles`] holds one of these under its lock, and asks it what to read next.
//!
//! **Nothing per idle page.** The selection is two points. Text is read when the pointer or a selection reaches a page, onto one job at a time on the thread that draws, and kept for at most [`MAX_TEXT_PAGES`] pages and about [`MAX_TEXT_BYTES`]. What a selection needs right now stays past that, and it can't be more than its [`MAX_SELECTED_VISIBLE`] visible pages and two ends. A whole document selected costs nothing until something's on screen, and then only what's on screen. A cold tab has none of this: it's dropped with the tab change.
//!
//! **What the UI thread pays.** A press, a drag or a pointer move looks through a page already here, or notes that it's wanted and returns. It never waits for text.
//!
//! **Hidden text takes a second look.** The first read of a page is quick and has no hidden-text check, since that draws the page. Once a page is in a selection and showing, or being copied, it's read again with the check when everything visible is drawn, and that can move no place: only what's flagged changes. A page known to be heavy isn't drawn for this, since one more draw of a USGS map is hundreds of MiB, and a copy says it wasn't checked.

use recto_index::select::{CopiedText, MAX_COPY_BYTES, MAX_COPY_PAGES, MAX_COPY_TIME, PagePoint, PageText, Range, Rect, Selection, Texts};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

/// Pages of text kept, past what's in use.
pub const MAX_TEXT_PAGES: usize = 16;
/// Text kept, past what's in use. A page of 3,800 characters is about 76 KiB.
pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
/// Visible pages a selection draws on. More than show at the lowest zoom on an ordinary page, and what keeps tiny hostile pages from wanting all their text at once.
pub const MAX_SELECTED_VISIBLE: usize = 12;
/// With this much held, only a selection's ends are read: its visible pages show no highlight, for now.
const MOST_TEXT_BYTES: usize = 2 * MAX_TEXT_BYTES;
/// Past this, a copy says how far it's got.
const SAY_PROGRESS_AFTER: f64 = 0.3;

/// A line for the reader by the page: a copy going on, or how it came out. What it says comes from here, and the host only draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// It offers "Copy anyway".
    pub offers_copy_anyway: bool,
    /// It goes by itself after a moment.
    pub fades: bool,
}

/// One page's text, and how far it's been read.
#[derive(Debug)]
struct Held {
    page: u32,
    text: Arc<PageText>,
    /// The hidden-text check has been tried, whichever way it went.
    check_tried: bool,
    /// It couldn't be read at all.
    failed: bool,
    last_used: u64,
}

/// A copy on its way, a page at a time.
#[derive(Debug)]
struct CopyRun {
    out: CopiedText,
    next: Option<u32>,
    started: Instant,
}

/// What [`Selecting::wanted`] asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WantedText {
    pub page: u32,
    pub check: bool,
}

/// What a pending read is about: a selection the reader is looking at comes before tiles, and the rest after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgency {
    Now,
    WhenIdle,
}

struct HeldTexts<'a>(&'a [Held]);

impl Texts for HeldTexts<'_> {
    fn page_text(&self, page: u32) -> Option<&PageText> {
        self.0.iter().find(|held| held.page == page).map(|held| &*held.text)
    }
}

/// What a tab's page view knows about selecting. See the module's notes.
#[derive(Debug, Default)]
pub struct Selecting {
    selection: Selection,
    /// What `selection` covers, as far as can be said.
    range: Option<Range>,
    texts: Vec<Held>,
    clock: u64,
    /// The page under the pointer, wanted when there's nothing better to do.
    hover: Option<u32>,
    /// Pages whose layer has a highlight.
    on_layers: HashSet<u32>,
    /// Pages whose layer is as it should be, until something changes.
    looked_at: HashSet<u32>,
    copy: Option<CopyRun>,
    /// What came of a copy, for the host to take.
    copied: Option<CopiedText>,
    copied_told: bool,
    notice: Option<Notice>,
    /// The reader has said to copy this file anyway, for as long as it's on screen.
    copy_allowed: bool,
    reads: u64,
}

impl Selecting {
    /// Everything goes, as when another tab or other pages come on screen. Whether "Copy anyway" was said goes too: it's about a file.
    pub fn reset(&mut self) {
        *self = Self { reads: self.reads, clock: self.clock, on_layers: std::mem::take(&mut self.on_layers), ..Self::default() };
    }

    /// The window closed: text goes, and the selection stays, to be worked out again if it comes back.
    pub fn drop_texts(&mut self) {
        self.texts = Vec::new();
        self.hover = None;
        self.copy = None;
        self.range = None;
        self.on_layers.clear();
        self.looked_at.clear();
    }

    /// The host's layers have gone, so nothing's on them.
    pub fn layers_gone(&mut self) {
        self.on_layers.clear();
        self.looked_at.clear();
    }

    pub fn page_hidden(&mut self, page: u32) {
        self.on_layers.remove(&page);
        self.looked_at.remove(&page);
    }

    fn changed(&mut self) {
        self.range = self.selection.range(&HeldTexts(&self.texts));
        self.looked_at.clear();
    }

    pub fn press(&mut self, point: PagePoint, clicks: u32, shift: bool) {
        // A new selection is what a copy on its way was for no longer.
        self.copy = None;
        self.notice = None;
        self.selection.press(point, clicks, shift);
        self.changed();
    }

    pub fn drag(&mut self, point: PagePoint) {
        if self.selection.is_dragging() {
            self.selection.drag(point);
            self.changed();
        }
    }

    pub fn release(&mut self) {
        self.selection.release();
    }

    pub fn set_range(&mut self, range: Range) {
        self.copy = None;
        self.notice = None;
        self.selection.set_range(range);
        self.changed();
    }

    pub fn clear(&mut self) {
        self.copy = None;
        self.notice = None;
        self.selection.clear();
        self.changed();
    }

    /// Whether the page under the pointer has text there, which is where the I-beam goes. A page not read yet is wanted, and says no for now.
    pub fn is_over_text(&mut self, point: PagePoint) -> bool {
        match HeldTexts(&self.texts).page_text(point.page) {
            Some(text) => {
                self.hover = None;
                text.is_over_text(point.x, point.y)
            }
            None => {
                self.hover = Some(point.page);
                false
            }
        }
    }

    pub fn range(&self) -> Option<Range> {
        self.range
    }

    /// Something is selected, as far as can be said. A selection still waiting on its text isn't, yet.
    pub fn has_selection(&self) -> bool {
        self.range.is_some_and(|range| !range.is_empty())
    }

    pub fn is_copying(&self) -> bool {
        self.copy.is_some()
    }

    fn held_bytes(&self) -> usize {
        self.texts.iter().map(|held| held.text.bytes()).sum()
    }

    fn held(&self, page: u32) -> Option<&Held> {
        self.texts.iter().find(|held| held.page == page)
    }

    fn page_wants_check(&self, page: u32, is_heavy: bool) -> bool {
        !is_heavy && self.held(page).is_none_or(|held| !held.check_tried && !held.failed)
    }

    /// What to read next, if anything, at this urgency. Asked when the thread that draws has nothing better to do.
    pub fn wanted(&self, urgency: Urgency, visible: &[u32], is_heavy: impl Fn(u32) -> bool) -> Option<WantedText> {
        match urgency {
            Urgency::Now => {
                // Where the ends are, first, since nothing can be said until that's known.
                for page in self.selection.end_pages().into_iter().flatten() {
                    if self.held(page).is_none() {
                        return Some(WantedText { page, check: false });
                    }
                }
                // What shows of a selection, with no drawing, so a scroll through a whole document selected isn't held up by a draw for each page.
                let range = self.range.filter(|range| !range.is_empty())?;
                if self.held_bytes() >= MOST_TEXT_BYTES {
                    return None;
                }
                visible.iter().filter(|page| range.on_page(**page).is_some()).take(MAX_SELECTED_VISIBLE).find(|page| self.held(**page).is_none()).map(|page| WantedText { page: *page, check: false })
            }
            Urgency::WhenIdle => {
                if let Some(page) = self.copy.as_ref().and_then(|copy| copy.next) {
                    let heavy = is_heavy(page);
                    if self.held(page).is_none() || self.page_wants_check(page, heavy) {
                        return Some(WantedText { page, check: !heavy });
                    }
                }
                // What's selected and showing, checked for hidden text, once it's all drawn.
                if let Some(range) = self.range.filter(|range| !range.is_empty())
                    && let Some(page) = visible.iter().filter(|page| range.on_page(**page).is_some()).take(MAX_SELECTED_VISIBLE).find(|page| self.held(**page).is_some() && self.page_wants_check(**page, is_heavy(**page)))
                {
                    return Some(WantedText { page: *page, check: true });
                }
                self.hover.filter(|page| self.held(*page).is_none()).map(|page| WantedText { page, check: false })
            }
        }
    }

    /// Whether anything's left to do, for QA to wait on.
    pub fn is_settled(&self, visible: &[u32], is_heavy: impl Fn(u32) -> bool) -> bool {
        self.copy.is_none() && self.copied.is_none() && self.wanted(Urgency::Now, visible, &is_heavy).is_none() && self.wanted(Urgency::WhenIdle, visible, &is_heavy).is_none()
    }

    /// A page's text has arrived. `checked` says the hidden-text check was asked for, so it's been tried, however it went.
    pub fn page_read(&mut self, page: u32, text: Arc<PageText>, checked: bool, visible: &[u32]) {
        self.reads += 1;
        self.clock += 1;
        let last_used = self.clock;
        if self.hover == Some(page) {
            self.hover = None;
        }
        self.texts.retain(|held| held.page != page);
        self.texts.push(Held { page, text, check_tried: checked, failed: false, last_used });
        self.evict(visible);
        self.changed();
    }

    /// A page's text couldn't be read. It holds an empty page, so it's not asked for again until something clears it, and a copy says so.
    pub fn page_failed(&mut self, page: u32, visible: &[u32]) {
        self.reads += 1;
        self.clock += 1;
        let last_used = self.clock;
        if self.hover == Some(page) {
            self.hover = None;
        }
        self.texts.retain(|held| held.page != page);
        self.texts.push(Held { page, text: Arc::new(PageText::default()), check_tried: true, failed: true, last_used });
        self.evict(visible);
        self.changed();
    }

    /// Least recently read goes first, leaving what a selection or a copy is using now. Bounded by the caps, and a look along a short list.
    fn evict(&mut self, visible: &[u32]) {
        let ends = self.selection.end_pages();
        let range = self.range;
        let copy_next = self.copy.as_ref().and_then(|copy| copy.next);
        loop {
            if self.texts.len() <= MAX_TEXT_PAGES && self.held_bytes() <= MAX_TEXT_BYTES {
                return;
            }
            let in_use = |page: u32| ends.contains(&Some(page)) || copy_next == Some(page) || (visible.contains(&page) && range.is_some_and(|range| range.on_page(page).is_some()));
            let oldest = self.texts.iter().enumerate().filter(|(_, held)| !in_use(held.page)).min_by_key(|(_, held)| held.last_used).map(|(at, _)| at);
            let Some(at) = oldest else { return };
            self.texts.swap_remove(at);
        }
    }

    /// What a page's layer should hold now, if that's not known to be what it has. `None` says leave it.
    pub fn layer_update(&mut self, page: u32) -> Option<Vec<Rect>> {
        if self.looked_at.contains(&page) {
            return None;
        }
        self.looked_at.insert(page);
        let rects = match (self.range.filter(|range| !range.is_empty()).and_then(|range| range.on_page(page)), HeldTexts(&self.texts).page_text(page)) {
            (Some((start, end)), Some(text)) => text.rects(start, end),
            _ => Vec::new(),
        };
        if rects.is_empty() {
            return self.on_layers.remove(&page).then(Vec::new);
        }
        self.on_layers.insert(page);
        Some(rects)
    }

    /// Whether no page could need its layer touched, which is the cost of every scroll with nothing selected.
    pub fn nothing_to_draw(&self) -> bool {
        self.on_layers.is_empty() && !self.has_selection()
    }

    /// Starts a copy, or says why not. `anyway` is the reader having said so about a file that asks not to be.
    pub fn start_copy(&mut self, anyway: bool, pages: u32, is_heavy: impl Fn(u32) -> bool) {
        if anyway {
            self.copy_allowed = true;
        }
        self.notice = None;
        self.copied = None;
        self.copied_told = false;
        self.copy = Some(CopyRun { out: CopiedText::default(), next: None, started: Instant::now() });
        self.advance_copy(pages, is_heavy);
    }

    /// Takes the copy as far as the text here allows. Bounded by what's held, which is what this gets through without another read.
    pub fn advance_copy(&mut self, pages: u32, is_heavy: impl Fn(u32) -> bool) {
        while let Some(mut run) = self.copy.take() {
            let Some(range) = self.range else {
                // Its ends are still being read.
                self.copy = Some(run);
                return;
            };
            let last = range.end.page.min(pages.saturating_sub(1));
            let page = *run.next.get_or_insert(range.start.page);
            if page > last {
                self.finish_copy(run.out);
                return;
            }
            if page - range.start.page >= MAX_COPY_PAGES || run.started.elapsed() >= MAX_COPY_TIME {
                run.out.stopped_at = Some(page);
                self.finish_copy(run.out);
                return;
            }
            let heavy = is_heavy(page);
            let wants_check = self.page_wants_check(page, heavy);
            let Some(held) = self.held(page).filter(|_| !wants_check) else {
                self.notice = progress(&run, range, last);
                self.copy = Some(run);
                return;
            };
            let room = if held.failed {
                run.out.add_failed_page();
                true
            } else {
                run.out.add_page(page, &held.text, &range, MAX_COPY_BYTES)
            };
            if run.out.restricted && !self.copy_allowed {
                self.notice = Some(Notice { text: "This file's permissions ask that its text isn't copied.".to_string(), offers_copy_anyway: true, fades: false });
                return;
            }
            if !room {
                self.finish_copy(run.out);
                return;
            }
            run.next = Some(page + 1);
            self.copy = Some(run);
        }
    }

    fn finish_copy(&mut self, out: CopiedText) {
        self.notice = how_it_went(&out);
        self.copied_told = false;
        self.copied = (!out.text.is_empty()).then_some(out);
    }

    /// A copy is ready for the host, and it hasn't been told. Says so once.
    pub fn take_copy_signal(&mut self) -> bool {
        let tell = self.copied.is_some() && !self.copied_told;
        self.copied_told |= tell;
        tell
    }

    pub fn take_copied(&mut self) -> Option<CopiedText> {
        self.copied.take()
    }

    /// The selection's text with nothing to wait for, for what can't wait, such as the Services menu. `None` if any of it isn't here yet, or if the file asks not to be copied and hasn't been told otherwise.
    pub fn text_now(&self, pages: u32) -> Option<String> {
        let range = self.range.filter(|range| !range.is_empty())?;
        let last = range.end.page.min(pages.saturating_sub(1));
        if last - range.start.page >= MAX_TEXT_PAGES as u32 {
            return None;
        }
        let mut out = CopiedText::default();
        for page in range.start.page..=last {
            let held = self.held(page).filter(|held| held.check_tried)?;
            out.add_page(page, &held.text, &range, MAX_COPY_BYTES);
        }
        (!out.text.is_empty() && (!out.restricted || self.copy_allowed)).then_some(out.text)
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    pub fn report(&self) -> Value {
        let place = |at: recto_index::select::Place| json!({ "page": at.page + 1, "at": at.at });
        json!({
            "selected": self.has_selection(),
            "dragging": self.selection.is_dragging(),
            "unit": self.selection.unit().name(),
            "start": self.range.map(|range| place(range.start)),
            "end": self.range.map(|range| place(range.end)),
            "pages_with_highlight": self.on_layers.len(),
            "text_pages_held": self.texts.len(),
            "text_bytes_held": self.held_bytes(),
            "text_reads": self.reads,
            "copying": self.copy.is_some(),
            "copy_waiting_for_host": self.copied.is_some(),
        })
    }
}

fn progress(run: &CopyRun, range: Range, last: u32) -> Option<Notice> {
    (run.started.elapsed().as_secs_f64() >= SAY_PROGRESS_AFTER).then(|| Notice { text: format!("Copying… {} of {} pages.", run.next.map_or(0, |page| page - range.start.page), last - range.start.page + 1), offers_copy_anyway: false, fades: false })
}

/// What's worth saying about a copy, or nothing if it went as asked. The clipboard holding what it does is the news, and this is only what the reader wouldn't otherwise know.
fn how_it_went(out: &CopiedText) -> Option<Notice> {
    let mut said: Vec<String> = Vec::new();
    if out.text.is_empty() {
        said.push("There's no text in the selection.".to_string());
    }
    if let Some(page) = out.stopped_at {
        said.push(format!("Copied up to page {}: that's as much as one copy holds.", page + 1));
    }
    if out.hidden_left_out > 0 {
        said.push(format!("{} of text that looks hidden left out.", plural(out.hidden_left_out, "character", "characters")));
    }
    if out.pages_failed > 0 {
        said.push(format!("{} couldn't be read.", plural(u64::from(out.pages_failed), "page", "pages")));
    }
    if out.pages_too_long > 0 {
        said.push(format!("{} too long to copy.", plural(u64::from(out.pages_too_long), "page was", "pages were")));
    }
    if out.pages_unchecked > 0 {
        said.push(format!("{} couldn't be checked for hidden text.", plural(u64::from(out.pages_unchecked), "page", "pages")));
    }
    (!said.is_empty()).then(|| Notice { text: said.join(" "), offers_copy_anyway: false, fades: true })
}

fn plural(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// How far to scroll per step of a drag past the view's edge, by how far past it the pointer is. Faster the further it goes, as a Mac's text views do, and never so fast the page can't be followed.
pub fn autoscroll_step(beyond: f64) -> f64 {
    if !beyond.is_finite() || beyond == 0.0 {
        return 0.0;
    }
    (6.0 + beyond.abs() * 0.5).min(120.0) * beyond.signum()
}

/// How often that step comes, in seconds. A timer only while a drag is past the edge.
pub const AUTOSCROLL_EVERY: f64 = 0.05;

#[cfg(test)]
mod tests {
    use super::*;
    use recto_proto::{CharBox, CharRecord, PageFrame};

    fn text(line: &str) -> Arc<PageText> {
        let frame = PageFrame { turns: 0, left: 0.0, bottom: 0.0, right: 612.0, top: 792.0 };
        let chars: Vec<CharRecord> = line.chars().map(|ch| CharRecord::new(ch as u32, 0)).collect();
        let boxes: Vec<CharBox> = (0..chars.len()).map(|at| [72.0 + at as f32 * 6.0, 700.0, 77.0 + at as f32 * 6.0, 710.0]).collect();
        Arc::new(PageText::build(&chars, &boxes, frame, true, false))
    }

    fn restricted(line: &str) -> Arc<PageText> {
        let frame = PageFrame { turns: 0, left: 0.0, bottom: 0.0, right: 612.0, top: 792.0 };
        let chars: Vec<CharRecord> = line.chars().map(|ch| CharRecord::new(ch as u32, 0)).collect();
        let boxes: Vec<CharBox> = (0..chars.len()).map(|at| [72.0 + at as f32 * 6.0, 700.0, 77.0 + at as f32 * 6.0, 710.0]).collect();
        Arc::new(PageText::build(&chars, &boxes, frame, true, true))
    }

    fn at(page: u32, x: f32) -> PagePoint {
        PagePoint { page, x, y: 87.0 }
    }

    const NOT_HEAVY: fn(u32) -> bool = |_| false;

    /// Failure: a press on a page that hasn't been read waits for it or is lost, or text gets read for pages nobody's looking at.
    #[test]
    fn a_press_asks_for_its_page_and_nothing_else() {
        let mut selecting = Selecting::default();
        assert_eq!(selecting.wanted(Urgency::Now, &[0, 1, 2], NOT_HEAVY), None, "nothing's read for an idle page");
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[0, 1, 2], NOT_HEAVY), None);
        selecting.press(at(1, 80.0), 1, false);
        assert_eq!(selecting.wanted(Urgency::Now, &[0, 1, 2], NOT_HEAVY), Some(WantedText { page: 1, check: false }));
        assert!(!selecting.has_selection());
        selecting.page_read(1, text("abcdefghij"), false, &[0, 1, 2]);
        assert_eq!(selecting.wanted(Urgency::Now, &[0, 1, 2], NOT_HEAVY), None, "a click selects nothing, so no second look");
        selecting.drag(at(1, 110.0));
        assert!(selecting.has_selection());
        assert_eq!(selecting.wanted(Urgency::Now, &[0, 1, 2], NOT_HEAVY), None, "it can highlight with what it has, and tiles don't wait for a draw");
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[0, 1, 2], NOT_HEAVY), Some(WantedText { page: 1, check: true }), "selected and showing, so it's checked for hidden text");
        selecting.page_read(1, text("abcdefghij"), true, &[0, 1, 2]);
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[0, 1, 2], NOT_HEAVY), None);
    }

    /// Failure: the pointer resting on a page draws it for the hidden-text check, or a heavy page is drawn again for one, at hundreds of MiB.
    #[test]
    fn only_a_selection_checks_and_a_heavy_page_never_does() {
        let mut selecting = Selecting::default();
        assert!(!selecting.is_over_text(at(4, 80.0)));
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[4], NOT_HEAVY), Some(WantedText { page: 4, check: false }));
        assert_eq!(selecting.wanted(Urgency::Now, &[4], NOT_HEAVY), None, "after what's being drawn");
        selecting.page_read(4, text("abcdefghij"), false, &[4]);
        assert!(selecting.is_over_text(at(4, 80.0)));
        assert!(!selecting.is_over_text(at(4, 500.0)));
        selecting.press(at(4, 72.0), 1, false);
        selecting.drag(at(4, 110.0));
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[4], |_| true), None, "heavy: it's kept as it is");
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[4], NOT_HEAVY), Some(WantedText { page: 4, check: true }), "any other page is checked");
    }

    /// Failure: selecting a whole long document reads every page, or holds their text.
    #[test]
    fn a_whole_document_reads_only_what_shows() {
        let mut selecting = Selecting::default();
        selecting.set_range(Range::all(100_000));
        assert!(selecting.has_selection());
        let mut read = 0;
        let visible = [500, 501];
        while let Some(wanted) = selecting.wanted(Urgency::Now, &visible, NOT_HEAVY).or_else(|| selecting.wanted(Urgency::WhenIdle, &visible, NOT_HEAVY)) {
            selecting.page_read(wanted.page, text("words"), wanted.check, &visible);
            read += 1;
            assert!(read < 8, "it keeps asking");
        }
        assert_eq!(read, 4, "each page showing, then each checked");
        assert_eq!(selecting.report()["text_pages_held"], 2);
    }

    /// Failure: text stays for every page visited, so a long read costs as much as its pages, or a page in use is thrown out and read again for ever.
    #[test]
    fn text_kept_has_a_cap_and_never_throws_out_what_is_in_use() {
        let mut selecting = Selecting::default();
        selecting.press(at(0, 72.0), 1, false);
        selecting.page_read(0, text("anchor page"), false, &[]);
        for page in 1..200 {
            selecting.drag(at(page, 80.0));
            selecting.page_read(page, text("words on it"), false, &[]);
            let held = selecting.report()["text_pages_held"].as_u64().unwrap();
            assert!(held as usize <= MAX_TEXT_PAGES, "{held} at page {page}");
            assert!(selecting.held(0).is_some(), "the end that's fixed stays");
            assert!(selecting.held(page).is_some(), "the end that's moving stays");
        }
        // Everything visible and selected, at a cap's worth and more: nothing is thrown out to read it again.
        let visible: Vec<u32> = (0..12).collect();
        let mut fresh = Selecting::default();
        fresh.set_range(Range::all(1000));
        for &page in &visible {
            fresh.page_read(page, text("words"), true, &visible);
        }
        assert_eq!(fresh.wanted(Urgency::Now, &visible, NOT_HEAVY), None);
        assert_eq!(fresh.wanted(Urgency::WhenIdle, &visible, NOT_HEAVY), None);
    }

    /// Failure: a copy holds the UI thread until it's done, skips a page, or has no way to say the file asks not to be copied and go on when told.
    #[test]
    fn a_copy_goes_a_page_at_a_time_and_stops_for_a_file_that_asks_not_to_be() {
        let mut selecting = Selecting::default();
        selecting.set_range(Range::pages(0, 2));
        selecting.start_copy(false, 3, NOT_HEAVY);
        assert!(selecting.is_copying());
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[], NOT_HEAVY), Some(WantedText { page: 0, check: true }));
        for page in 0..3 {
            selecting.page_read(page, text(&format!("page {page}")), true, &[]);
            selecting.advance_copy(3, NOT_HEAVY);
        }
        assert!(!selecting.is_copying());
        assert!(selecting.take_copy_signal());
        assert!(!selecting.take_copy_signal(), "told once");
        assert_eq!(selecting.take_copied().unwrap().text, "page 0\npage 1\npage 2");

        let mut asks = Selecting::default();
        asks.set_range(Range::pages(0, 1));
        asks.start_copy(false, 2, NOT_HEAVY);
        asks.page_read(0, restricted("asks not to"), true, &[]);
        asks.advance_copy(2, NOT_HEAVY);
        assert!(!asks.is_copying());
        assert!(asks.notice().unwrap().offers_copy_anyway);
        assert!(asks.take_copied().is_none());
        assert_eq!(asks.text_now(2), None, "and the Services menu can't get round it");
        asks.start_copy(true, 2, NOT_HEAVY);
        asks.page_read(1, restricted("second"), true, &[]);
        asks.advance_copy(2, NOT_HEAVY);
        assert_eq!(asks.take_copied().unwrap().text, "asks not to\nsecond");
        assert_eq!(asks.notice(), None);
    }

    /// Failure: nothing tells the reader a selection had no text in it, or a page that couldn't be read goes unsaid.
    #[test]
    fn a_copy_says_what_the_reader_would_not_know() {
        let mut empty = Selecting::default();
        empty.set_range(Range::pages(0, 0));
        empty.start_copy(false, 1, NOT_HEAVY);
        empty.page_read(0, Arc::new(PageText::default()), true, &[]);
        empty.advance_copy(1, NOT_HEAVY);
        assert!(empty.take_copied().is_none(), "the clipboard keeps what it had");
        assert!(empty.notice().unwrap().text.contains("no text"));
        let mut failing = Selecting::default();
        failing.set_range(Range::pages(0, 1));
        failing.start_copy(false, 2, NOT_HEAVY);
        failing.page_failed(0, &[]);
        failing.advance_copy(2, NOT_HEAVY);
        failing.page_read(1, text("fine"), true, &[]);
        failing.advance_copy(2, NOT_HEAVY);
        assert_eq!(failing.take_copied().unwrap().text, "fine");
        assert!(failing.notice().unwrap().text.contains("1 page couldn't be read"), "{:?}", failing.notice());
    }

    /// Failure: a highlight stays up after it's cleared, or every scroll with nothing selected walks the visible pages.
    #[test]
    fn layers_hear_only_what_has_changed() {
        let mut selecting = Selecting::default();
        assert!(selecting.nothing_to_draw());
        selecting.press(at(0, 72.0), 1, false);
        selecting.page_read(0, text("abcdefghij"), true, &[0]);
        selecting.drag(at(0, 110.0));
        assert!(!selecting.layer_update(0).unwrap().is_empty());
        assert_eq!(selecting.layer_update(0), None, "it hasn't changed");
        selecting.drag(at(0, 120.0));
        assert!(!selecting.layer_update(0).unwrap().is_empty(), "it has now");
        selecting.clear();
        assert_eq!(selecting.layer_update(0), Some(Vec::new()), "the page is told to clear");
        assert_eq!(selecting.layer_update(0), None);
        assert!(selecting.nothing_to_draw());
    }

    /// Failure: dragging past the edge doesn't scroll, or scrolls away too fast to stop, or a NaN from the host scrolls for ever.
    #[test]
    fn a_drag_past_the_edge_speeds_up_and_stays_bounded() {
        assert_eq!(autoscroll_step(0.0), 0.0);
        assert_eq!(autoscroll_step(f64::NAN), 0.0);
        assert!(autoscroll_step(10.0) > 0.0 && autoscroll_step(-10.0) < 0.0);
        assert!(autoscroll_step(100.0) > autoscroll_step(10.0));
        assert!(autoscroll_step(1e12) <= 120.0);
    }
}
