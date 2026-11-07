//! A selection in the page view: what the reader's pointer has done, what's been read to work it out, and a copy on its way. No toolkit, thread or renderer: [`crate::tiles`] holds one of these under its lock, and asks it what to read next.
//!
//! **Nothing per idle page.** The selection is two points. Text is read when the pointer or a selection reaches a page, onto one job at a time on the thread that draws, and kept for at most [`MAX_TEXT_PAGES`] pages and about [`MAX_TEXT_BYTES`]. What a selection needs right now stays past that, and it can't be more than its [`MAX_SELECTED_VISIBLE`] visible pages and two ends. A whole document selected costs nothing until something's on screen, and then only what's on screen. A cold tab has none of this: it's dropped with the tab change.
//!
//! **What the UI thread pays.** A press, a drag or a pointer move looks through a page already here, or notes that it's wanted and returns. It never waits for text.
//!
//! **Running heads, footers and page numbers stay out of a copy of body text** (owner, 2026-11-07: "When selecting across pages the page number is copied. Both footer and header should be ignored when coppying body text."). A page's edge lines are compared with those of [`recto_index::chrome::NEIGHBOURS`] pages either side, so a copy, and what Services is offered, waits for those pages' text too: read with no drawing, once, and only their edge lines kept, at most [`MAX_EDGE_PAGES`] pages of them, about 400 B each.
//!
//! **Hidden text takes a second look.** The first read of a page is quick and has no hidden-text check, since that draws the page. Once a page is in a selection and showing, or being copied, it's read again with the check when everything visible is drawn. No character moves. What changes is what's on them: an end that was on a hidden word goes to the nearest thing that shows. A page known to be heavy isn't drawn for this, since one more draw of a USGS map is hundreds of MiB, and a copy says it wasn't checked.

use recto_index::chrome::EdgeLine;
use recto_index::select::{CopiedText, MAX_COPY_BYTES, MAX_COPY_PAGES, MAX_COPY_TIME, PAGE_END, PagePoint, PageText, Range, Rect, Selection, Texts, chrome_for, neighbour_pages};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

/// Pages of text kept, past what's in use. Enough for a drag going back and forth over a few pages, and about half a MiB of ordinary ones.
pub const MAX_TEXT_PAGES: usize = 8;
/// Text kept, past what's in use. A middling real page is 55 KiB, and the largest in bench100 is 242.
pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
/// Visible pages a selection draws on. More than show at the lowest zoom on an ordinary page, and what keeps tiny hostile pages from wanting all their text at once.
pub const MAX_SELECTED_VISIBLE: usize = 12;
/// Pages whose edge lines are kept, for telling running heads and page numbers from body text. A copy needs those of the page it's on and four around it, and Services those around [`MAX_TEXT_PAGES`].
pub const MAX_EDGE_PAGES: usize = 64;
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

/// A copy on its way, a page at a time. It holds what was selected when it was asked for, as a Mac's ⌘C copies that: the selection can go on moving, or go, and this is still what the reader asked for.
#[derive(Debug)]
struct CopyRun {
    out: CopiedText,
    range: Range,
    /// The page it's waiting for, never before the start of `range`.
    next: u32,
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
    /// Each page's edge lines as read, for its neighbours to be compared with. At most [`MAX_EDGE_PAGES`].
    edges: Vec<(u32, Vec<EdgeLine>)>,
}

impl Selecting {
    /// Everything goes, as when another tab or other pages come on screen. Whether "Copy anyway" was said goes too: it's about a file.
    pub fn reset(&mut self) {
        *self = Self { reads: self.reads, clock: self.clock, on_layers: std::mem::take(&mut self.on_layers), ..Self::default() };
    }

    /// The file isn't the one any of this was about. A copy on its way says so, as the reader's waiting for it.
    pub fn file_changed(&mut self) {
        let was_copying = self.copy.is_some();
        self.reset();
        if was_copying {
            self.notice = Some(Notice { text: "This file changed on disk, so the copy stopped.".to_string(), offers_copy_anyway: false, fades: true });
        }
    }

    /// The thread that reads text is starting again, and what it was doing is gone.
    pub fn thread_restarted(&mut self) {
        self.reset();
    }

    /// The window closed: text goes, and the selection stays, to be worked out again if it comes back.
    pub fn drop_texts(&mut self) {
        self.texts = Vec::new();
        self.edges = Vec::new();
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
        // What a copy on its way says about itself stays, and what was said about a finished one has had its moment.
        if self.copy.is_none() {
            self.notice = None;
        }
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
        if self.copy.is_none() {
            self.notice = None;
        }
        self.selection.set_range(range);
        self.changed();
    }

    pub fn clear(&mut self) {
        if self.copy.is_none() {
            self.notice = None;
        }
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

    fn edges_of(&self, page: u32) -> Option<&[EdgeLine]> {
        self.edges.iter().find(|(seen, _)| *seen == page).map(|(_, lines)| lines.as_slice())
    }

    /// The first page around `page` whose edge lines a copy of it needs and doesn't have.
    fn missing_neighbour(&self, page: u32, pages: u32) -> Option<u32> {
        neighbour_pages(page, pages).find(|other| self.edges_of(*other).is_none() && self.held(*other).is_none())
    }

    /// What a copy of `range` leaves out of this page as chrome, with what's known of its neighbours.
    fn leave_out(&self, page: u32, text: &PageText, range: &Range, pages: u32) -> Vec<(u32, u32)> {
        let neighbours: Vec<&[EdgeLine]> = neighbour_pages(page, pages).filter_map(|other| self.edges_of(other).or_else(|| self.held(other).map(|held| held.text.edge_lines()))).collect();
        chrome_for(page, text, range, &neighbours)
    }

    /// Keeps a page's edge lines, letting go of the ones furthest from where a copy or the selection is, past the cap.
    fn keep_edges(&mut self, page: u32, lines: Vec<EdgeLine>) {
        self.edges.retain(|(seen, _)| *seen != page);
        if self.edges.len() >= MAX_EDGE_PAGES {
            let near = self.copy.as_ref().map(|copy| copy.next).or(self.range.map(|range| range.start.page)).unwrap_or(page);
            if let Some(at) = self.edges.iter().enumerate().max_by_key(|(_, (seen, _))| seen.abs_diff(near)).map(|(at, _)| at) {
                self.edges.swap_remove(at);
            }
        }
        self.edges.push((page, lines));
    }

    fn page_wants_check(&self, page: u32, is_heavy: bool) -> bool {
        !is_heavy && self.held(page).is_none_or(|held| !held.check_tried && !held.failed)
    }

    /// What to read next, if anything, at this urgency. Asked when the thread that draws has nothing better to do.
    pub fn wanted(&self, urgency: Urgency, visible: &[u32], pages: u32, is_heavy: impl Fn(u32) -> bool) -> Option<WantedText> {
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
                if let Some(copy) = self.copy.as_ref() {
                    let page = copy.next;
                    let heavy = is_heavy(page);
                    if self.held(page).is_none() || self.page_wants_check(page, heavy) {
                        return Some(WantedText { page, check: !heavy });
                    }
                    // Then the pages around it, for its chrome: one the copy reaches next is read as it will be then.
                    if let Some(other) = self.missing_neighbour(page, pages) {
                        let reaches = other > page && copy.range.on_page(other).is_some();
                        return Some(WantedText { page: other, check: reaches && !is_heavy(other) });
                    }
                }
                // What's selected and showing, checked for hidden text, once it's all drawn.
                if let Some(range) = self.range.filter(|range| !range.is_empty())
                    && let Some(page) = visible.iter().filter(|page| range.on_page(**page).is_some()).take(MAX_SELECTED_VISIBLE).find(|page| self.held(**page).is_some() && self.page_wants_check(**page, is_heavy(**page)))
                {
                    return Some(WantedText { page: *page, check: true });
                }
                // The pages around a short selection, so Services can be offered its text without its chrome.
                if let Some(range) = self.range.filter(|range| !range.is_empty() && range.page_count() <= MAX_TEXT_PAGES as u32)
                    && let Some(other) = (range.start.page..=range.end.page.min(pages.saturating_sub(1))).find_map(|page| self.missing_neighbour(page, pages))
                {
                    return Some(WantedText { page: other, check: false });
                }
                self.hover.filter(|page| self.held(*page).is_none()).map(|page| WantedText { page, check: false })
            }
        }
    }

    /// Whether anything's left to do, for QA to wait on.
    pub fn is_settled(&self, visible: &[u32], pages: u32, is_heavy: impl Fn(u32) -> bool) -> bool {
        self.copy.is_none() && self.copied.is_none() && self.wanted(Urgency::Now, visible, pages, &is_heavy).is_none() && self.wanted(Urgency::WhenIdle, visible, pages, &is_heavy).is_none()
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
        self.keep_edges(page, text.edge_lines().to_vec());
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
        self.keep_edges(page, Vec::new());
        self.texts.push(Held { page, text: Arc::new(PageText::default()), check_tried: true, failed: true, last_used });
        self.evict(visible);
        self.changed();
    }

    /// Least recently read goes first, leaving what a selection or a copy is using now. Bounded by the caps, and a look along a short list.
    fn evict(&mut self, visible: &[u32]) {
        let ends = self.selection.end_pages();
        let range = self.range;
        let copy_next = self.copy.as_ref().map(|copy| copy.next);
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

    /// Starts a copy of what's selected now. With nothing selected there's nothing to start, which is why the menu says it can't be chosen. `anyway` is the reader having said so about a file that asks not to be.
    pub fn start_copy(&mut self, anyway: bool, pages: u32, is_heavy: impl Fn(u32) -> bool) {
        let Some(range) = self.range.filter(|range| !range.is_empty()) else { return };
        if anyway {
            self.copy_allowed = true;
        }
        self.notice = None;
        self.copied = None;
        self.copied_told = false;
        self.copy = Some(CopyRun { out: CopiedText::default(), range, next: range.start.page, started: Instant::now() });
        self.advance_copy(pages, is_heavy);
    }

    /// Takes the copy as far as the text here allows. Bounded by what's held, which is what this gets through without another read.
    pub fn advance_copy(&mut self, pages: u32, is_heavy: impl Fn(u32) -> bool) {
        while let Some(mut run) = self.copy.take() {
            let range = run.range;
            let last = range.end.page.min(pages.saturating_sub(1));
            let page = run.next;
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
            // A file that asks not to be copied says so at its first page, before the pages around it are read.
            let restricted = self.held(page).is_some_and(|held| !wants_check && held.text.copy_restricted()) && !self.copy_allowed;
            if restricted {
                run.out.restricted = true;
                self.notice = Some(Notice { text: "This file's permissions ask that its text isn't copied.".to_string(), offers_copy_anyway: true, fades: false });
                return;
            }
            let Some(held) = self.held(page).filter(|_| !wants_check && self.missing_neighbour(page, pages).is_none()) else {
                self.notice = progress(&run, last).or(self.notice.take());
                self.copy = Some(run);
                return;
            };
            let room = if held.failed {
                run.out.add_failed_page();
                true
            } else {
                let leave_out = self.leave_out(page, &held.text, &range, pages);
                run.out.add_page(page, &held.text, &range, MAX_COPY_BYTES, &leave_out)
            };
            if run.out.restricted && !self.copy_allowed {
                self.notice = Some(Notice { text: "This file's permissions ask that its text isn't copied.".to_string(), offers_copy_anyway: true, fades: false });
                return;
            }
            if !room {
                self.finish_copy(run.out);
                return;
            }
            run.next = page + 1;
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

    /// Whether [`Self::text_now`] has something to give, with none of it built: every page is here and a check has looked at it, and the file hasn't asked not to be copied. Services ask this on each event of a menu's life, on the UI thread. A check that couldn't be done doesn't count, as a copy can say a page wasn't checked and Services can't.
    pub fn can_offer(&self, pages: u32) -> bool {
        let Some(range) = self.range.filter(|range| !range.is_empty()) else { return false };
        let last = range.end.page.min(pages.saturating_sub(1));
        if last < range.start.page || last - range.start.page >= MAX_TEXT_PAGES as u32 {
            return false;
        }
        let mut any = false;
        for page in range.start.page..=last {
            let Some(held) = self.held(page) else { return false };
            if self.missing_neighbour(page, pages).is_some() {
                return false;
            }
            if held.failed || (!held.text.hidden_checked() && !held.text.is_empty()) || (held.text.copy_restricted() && !self.copy_allowed) {
                return false;
            }
            any |= range.on_page(page).is_some_and(|(start, end)| start < end.min(held.text.len()));
        }
        any
    }

    /// The selection's text with nothing to wait for, for what can't wait, such as the Services menu. `None` unless [`Self::can_offer`].
    pub fn text_now(&self, pages: u32) -> Option<String> {
        if !self.can_offer(pages) {
            return None;
        }
        let range = self.range?;
        let mut out = CopiedText::default();
        for page in range.start.page..=range.end.page.min(pages.saturating_sub(1)) {
            let text = &self.held(page)?.text;
            let leave_out = self.leave_out(page, text, &range, pages);
            out.add_page(page, text, &range, MAX_COPY_BYTES, &leave_out);
        }
        (!out.text.is_empty()).then_some(out.text)
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    pub fn report(&self) -> Value {
        // The end of a page whose length isn't known here has no number to say.
        let place = |at: recto_index::select::Place| json!({ "page": at.page + 1, "at": (at.at != PAGE_END).then_some(at.at) });
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
            "edge_pages_held": self.edges.len(),
            "copying": self.copy.is_some(),
            "copy_waiting_for_host": self.copied.is_some(),
        })
    }
}

fn progress(run: &CopyRun, last: u32) -> Option<Notice> {
    let start = run.range.start.page;
    (run.started.elapsed().as_secs_f64() >= SAY_PROGRESS_AFTER).then(|| Notice { text: format!("Copying… {} of {} pages.", run.next - start, last - start + 1), offers_copy_anyway: false, fades: false })
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

    /// A report's page: its running head, a line of body and its number, each a line of its own.
    fn report_page(page: u32) -> Arc<PageText> {
        let frame = PageFrame { turns: 0, left: 0.0, bottom: 0.0, right: 612.0, top: 792.0 };
        let (mut chars, mut boxes) = (Vec::new(), Vec::new());
        for (line, x, y) in [("Annual report 2024".to_string(), 72.0, 750.0), (format!("Body of page {page}"), 72.0, 700.0), (format!("{}", page + 1), 540.0, 30.0)] {
            for (at, ch) in line.chars().enumerate() {
                chars.push(CharRecord::new(ch as u32, 0));
                boxes.push([x + at as f32 * 6.0, y, x + at as f32 * 6.0 + 5.0, y + 10.0]);
            }
            chars.push(CharRecord::new('\n' as u32, CharRecord::GENERATED));
            boxes.push([0.0; 4]);
        }
        Arc::new(PageText::build(&chars, &boxes, frame, true, false))
    }

    /// Failure: ⌘C on a page, or across pages, copies the running head and the page number with the body, as the owner found (2026-11-07: "When selecting across pages the page number is copied."), or Services is offered that text. The pages around are read with no drawing, and both wait for them.
    #[test]
    fn a_copy_and_services_leave_out_running_heads_and_page_numbers() {
        let pages = 4;
        let mut selecting = Selecting::default();
        selecting.set_range(Range::pages(1, 2));
        selecting.start_copy(false, pages, NOT_HEAVY);
        let mut read = Vec::new();
        while selecting.is_copying() {
            let wanted = selecting.wanted(Urgency::WhenIdle, &[], pages, NOT_HEAVY).expect("a copy waits for something it asks for");
            read.push((wanted.page, wanted.check));
            selecting.page_read(wanted.page, report_page(wanted.page), wanted.check, &[]);
            selecting.advance_copy(pages, NOT_HEAVY);
            assert!(read.len() < 10, "it keeps asking: {read:?}");
        }
        assert_eq!(selecting.take_copied().unwrap().text, "Body of page 1\nBody of page 2");
        assert!(read.contains(&(0, false)) && read.contains(&(3, false)), "the pages past the pick are read with no drawing: {read:?}");

        let mut services = Selecting::default();
        services.set_range(Range::pages(1, 1));
        services.page_read(1, report_page(1), true, &[1]);
        assert!(!services.can_offer(pages), "not until the pages around are read");
        while let Some(wanted) = services.wanted(Urgency::WhenIdle, &[1], pages, NOT_HEAVY) {
            assert!(!wanted.check);
            services.page_read(wanted.page, report_page(wanted.page), false, &[1]);
        }
        assert_eq!(services.text_now(pages).as_deref(), Some("Body of page 1"));
        assert!(services.report()["edge_pages_held"].as_u64().unwrap() as usize <= MAX_EDGE_PAGES);
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
    /// A page count for `wanted` that has no pages around any page, so no text is read for chrome. The tests of chrome give real counts.
    const NO_NEIGHBOURS: u32 = 1;

    /// Failure: a press on a page that hasn't been read waits for it or is lost, or text gets read for pages nobody's looking at.
    #[test]
    fn a_press_asks_for_its_page_and_nothing_else() {
        let mut selecting = Selecting::default();
        assert_eq!(selecting.wanted(Urgency::Now, &[0, 1, 2], NO_NEIGHBOURS, NOT_HEAVY), None, "nothing's read for an idle page");
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[0, 1, 2], NO_NEIGHBOURS, NOT_HEAVY), None);
        selecting.press(at(1, 80.0), 1, false);
        assert_eq!(selecting.wanted(Urgency::Now, &[0, 1, 2], NO_NEIGHBOURS, NOT_HEAVY), Some(WantedText { page: 1, check: false }));
        assert!(!selecting.has_selection());
        selecting.page_read(1, text("abcdefghij"), false, &[0, 1, 2]);
        assert_eq!(selecting.wanted(Urgency::Now, &[0, 1, 2], NO_NEIGHBOURS, NOT_HEAVY), None, "a click selects nothing, so no second look");
        selecting.drag(at(1, 110.0));
        assert!(selecting.has_selection());
        assert_eq!(selecting.wanted(Urgency::Now, &[0, 1, 2], NO_NEIGHBOURS, NOT_HEAVY), None, "it can highlight with what it has, and tiles don't wait for a draw");
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[0, 1, 2], NO_NEIGHBOURS, NOT_HEAVY), Some(WantedText { page: 1, check: true }), "selected and showing, so it's checked for hidden text");
        selecting.page_read(1, text("abcdefghij"), true, &[0, 1, 2]);
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[0, 1, 2], NO_NEIGHBOURS, NOT_HEAVY), None);
    }

    /// Failure: the pointer resting on a page draws it for the hidden-text check, or a heavy page is drawn again for one, at hundreds of MiB.
    #[test]
    fn only_a_selection_checks_and_a_heavy_page_never_does() {
        let mut selecting = Selecting::default();
        assert!(!selecting.is_over_text(at(4, 80.0)));
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[4], NO_NEIGHBOURS, NOT_HEAVY), Some(WantedText { page: 4, check: false }));
        assert_eq!(selecting.wanted(Urgency::Now, &[4], NO_NEIGHBOURS, NOT_HEAVY), None, "after what's being drawn");
        selecting.page_read(4, text("abcdefghij"), false, &[4]);
        assert!(selecting.is_over_text(at(4, 80.0)));
        assert!(!selecting.is_over_text(at(4, 500.0)));
        selecting.press(at(4, 72.0), 1, false);
        selecting.drag(at(4, 110.0));
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[4], NO_NEIGHBOURS, |_| true), None, "heavy: it's kept as it is");
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[4], NO_NEIGHBOURS, NOT_HEAVY), Some(WantedText { page: 4, check: true }), "any other page is checked");
    }

    /// Failure: selecting a whole long document reads every page, or holds their text.
    #[test]
    fn a_whole_document_reads_only_what_shows() {
        let mut selecting = Selecting::default();
        selecting.set_range(Range::all(100_000));
        assert!(selecting.has_selection());
        let mut read = 0;
        let visible = [500, 501];
        while let Some(wanted) = selecting.wanted(Urgency::Now, &visible, NO_NEIGHBOURS, NOT_HEAVY).or_else(|| selecting.wanted(Urgency::WhenIdle, &visible, NO_NEIGHBOURS, NOT_HEAVY)) {
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
        assert_eq!(fresh.wanted(Urgency::Now, &visible, NO_NEIGHBOURS, NOT_HEAVY), None);
        assert_eq!(fresh.wanted(Urgency::WhenIdle, &visible, NO_NEIGHBOURS, NOT_HEAVY), None);
    }

    /// Failure: a copy holds the UI thread until it's done, skips a page, or has no way to say the file asks not to be copied and go on when told.
    #[test]
    fn a_copy_goes_a_page_at_a_time_and_stops_for_a_file_that_asks_not_to_be() {
        let mut selecting = Selecting::default();
        selecting.set_range(Range::pages(0, 2));
        selecting.start_copy(false, 3, NOT_HEAVY);
        assert!(selecting.is_copying());
        assert_eq!(selecting.wanted(Urgency::WhenIdle, &[], NO_NEIGHBOURS, NOT_HEAVY), Some(WantedText { page: 0, check: true }));
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
        asks.page_read(1, restricted("second"), true, &[]);
        assert_eq!(asks.text_now(2), None, "and the Services menu can't get round it");
        assert!(!asks.can_offer(2));
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

    /// Failure: ⌘C with the button still down, then the drag goes on: the copy read the live selection, so its next page came before its start, which underflowed and killed the thread that draws, or wrapped into wrong text and an untrue notice. On a Mac ⌘C copies what's selected when it's pressed, whatever happens next.
    #[test]
    fn a_copy_takes_what_was_selected_when_it_was_asked_for() {
        let mut selecting = Selecting::default();
        for page in 0..50 {
            selecting.page_read(page, text(&format!("page {page}")), true, &[]);
        }
        selecting.press(at(30, 72.0), 1, false);
        selecting.drag(at(32, 100.0));
        selecting.page_read(30, text("page 30"), true, &[]);
        selecting.page_read(32, text("page 32"), true, &[]);
        assert!(selecting.has_selection());
        selecting.start_copy(false, 50, NOT_HEAVY);
        // The button's still down, and it goes on: above where the copy started, and then below it.
        selecting.drag(at(5, 72.0));
        selecting.page_read(5, text("page 5"), true, &[]);
        selecting.advance_copy(50, NOT_HEAVY);
        selecting.drag(at(40, 100.0));
        selecting.page_read(40, text("page 40"), true, &[]);
        selecting.release();
        for page in 30..=32 {
            selecting.page_read(page, text(&format!("page {page}")), true, &[]);
            selecting.advance_copy(50, NOT_HEAVY);
        }
        let copied = selecting.take_copied().expect("the copy came out");
        assert!(copied.text.starts_with("page 30") || copied.text.starts_with("ge 30"), "{:?}", copied.text);
        assert!(copied.text.contains("page 31"), "{:?}", copied.text);
        assert!(!copied.text.contains("page 5\n") && !copied.text.contains("page 40"), "{:?}", copied.text);
        assert_eq!(copied.pages_read, 3);
    }

    /// Failure: a click during a long copy throws it away with nothing said, though the reader asked for it and the clipboard never gets it.
    #[test]
    fn a_click_during_a_copy_doesnt_lose_it() {
        let mut selecting = Selecting::default();
        selecting.set_range(Range::pages(0, 1));
        selecting.start_copy(false, 2, NOT_HEAVY);
        selecting.page_read(0, text("first"), true, &[]);
        selecting.advance_copy(2, NOT_HEAVY);
        selecting.press(at(0, 72.0), 1, false);
        selecting.release();
        assert!(selecting.is_copying());
        selecting.page_read(1, text("second"), true, &[]);
        selecting.advance_copy(2, NOT_HEAVY);
        assert_eq!(selecting.take_copied().unwrap().text, "first\nsecond");
    }

    /// Failure: Copy runs with nothing selected, whoever asked, and says it's copying for ever.
    #[test]
    fn a_copy_of_nothing_doesnt_start() {
        let mut selecting = Selecting::default();
        selecting.start_copy(false, 3, NOT_HEAVY);
        assert!(!selecting.is_copying());
        selecting.press(at(0, 72.0), 1, false);
        selecting.start_copy(false, 3, NOT_HEAVY);
        assert!(!selecting.is_copying(), "its text isn't known, so there's nothing selected yet");
        selecting.page_read(0, text("words"), false, &[]);
        selecting.start_copy(false, 3, NOT_HEAVY);
        assert!(!selecting.is_copying(), "a click selects nothing");
    }

    /// A page as it comes back when its check was asked for and couldn't be done: tried, and not checked.
    fn not_checked(line: &str) -> Arc<PageText> {
        let frame = PageFrame { turns: 0, left: 0.0, bottom: 0.0, right: 612.0, top: 792.0 };
        let chars: Vec<CharRecord> = line.chars().map(|ch| CharRecord::new(ch as u32, 0)).collect();
        let boxes: Vec<CharBox> = (0..chars.len()).map(|at| [72.0 + at as f32 * 6.0, 700.0, 77.0 + at as f32 * 6.0, 710.0]).collect();
        Arc::new(PageText::build(&chars, &boxes, frame, false, false))
    }

    /// Failure: Services are sent text no check has looked at, hidden words included, with nothing said, since they go by the check having been tried and not by what it found. A copy says when a page wasn't checked, and Services have no way to, so they aren't offered it.
    #[test]
    fn services_are_only_given_what_a_check_has_looked_at() {
        let mut selecting = Selecting::default();
        selecting.set_range(Range::pages(0, 0));
        selecting.page_read(0, not_checked("can't be vouched for"), true, &[0]);
        assert_eq!(selecting.text_now(1), None);
        assert!(!selecting.can_offer(1));
        selecting.page_read(0, text("looked at"), true, &[0]);
        assert_eq!(selecting.text_now(1).as_deref(), Some("looked at"));
        assert!(selecting.can_offer(1));
    }

    /// Failure: a file claiming millions of pages keeps a copy going for ever.
    #[test]
    fn a_copy_stops_at_its_page_limit_and_says_where() {
        let mut selecting = Selecting::default();
        selecting.set_range(Range::all(MAX_COPY_PAGES + 50));
        selecting.start_copy(false, MAX_COPY_PAGES + 50, NOT_HEAVY);
        selecting.copy.as_mut().unwrap().next = MAX_COPY_PAGES;
        selecting.advance_copy(MAX_COPY_PAGES + 50, NOT_HEAVY);
        assert!(!selecting.is_copying());
        assert!(selecting.notice().unwrap().text.contains(&format!("page {}", MAX_COPY_PAGES + 1)), "{:?}", selecting.notice());
    }

    /// Failure: a file whose pages are each slow to read keeps a copy going past its time, with the reader waiting on nothing they can see.
    #[test]
    fn a_copy_stops_at_its_time_limit_and_says_where() {
        let mut selecting = Selecting::default();
        selecting.set_range(Range::pages(0, 9));
        selecting.start_copy(false, 10, NOT_HEAVY);
        for page in 0..3 {
            selecting.page_read(page, text("words"), true, &[]);
            selecting.advance_copy(10, NOT_HEAVY);
        }
        assert!(selecting.is_copying());
        selecting.copy.as_mut().unwrap().started = Instant::now().checked_sub(MAX_COPY_TIME + std::time::Duration::from_secs(1)).unwrap();
        selecting.page_read(3, text("words"), true, &[]);
        selecting.advance_copy(10, NOT_HEAVY);
        assert!(!selecting.is_copying());
        assert_eq!(selecting.take_copied().unwrap().stopped_at, Some(1), "the page it was on, the first with the pages around it read");
    }

    /// A page as large as can be selected: about 2.6 MiB held.
    fn huge_page() -> Arc<PageText> {
        let frame = PageFrame { turns: 0, left: 0.0, bottom: 0.0, right: 612.0, top: 792.0 };
        let chars = vec![CharRecord::new('a' as u32, 0); recto_index::select::MAX_SELECTABLE_CHARS];
        let boxes: Vec<CharBox> = (0..chars.len()).map(|at| [(at % 500) as f32, (at / 500 % 700) as f32, (at % 500) as f32 + 1.0, (at / 500 % 700) as f32 + 1.0]).collect();
        Arc::new(PageText::build(&chars, &boxes, frame, true, false))
    }

    /// Failure: a copy past 16 MiB goes on, or its text passes the limit, or nothing says it stopped.
    #[test]
    fn a_copy_stops_at_its_byte_limit_and_says_where() {
        let huge = huge_page();
        let pages = 1000;
        let mut selecting = Selecting::default();
        selecting.set_range(Range::all(pages));
        selecting.start_copy(false, pages, NOT_HEAVY);
        let mut page = 0;
        while selecting.is_copying() {
            selecting.page_read(page, Arc::clone(&huge), true, &[]);
            selecting.advance_copy(pages, NOT_HEAVY);
            page += 1;
            assert!(page < pages, "it never stopped");
        }
        let copied = selecting.take_copied().unwrap();
        assert!(copied.text.len() <= MAX_COPY_BYTES, "{}", copied.text.len());
        assert_eq!(copied.stopped_at, Some(page - 1 - recto_index::chrome::NEIGHBOURS), "the page it was on, which is behind what's read by the pages it compares with");
        assert!(selecting.notice().unwrap().text.contains("as much as one copy holds"));
    }

    /// Failure: long pages visited stay held past 8 MiB, or with everything in view selected, more are read past 16 MiB: hundreds of megabytes on a file of tiny pages with long text.
    #[test]
    fn text_held_stays_under_its_byte_limits() {
        let huge = huge_page();
        let mut selecting = Selecting::default();
        for page in 0..7 {
            selecting.page_read(page, Arc::clone(&huge), false, &[]);
            assert!(selecting.held_bytes() <= MAX_TEXT_BYTES, "{} at page {page}", selecting.held_bytes());
        }
        let visible: Vec<u32> = (100..112).collect();
        let mut showing = Selecting::default();
        showing.set_range(Range::all(1000));
        let mut read = 0;
        while let Some(wanted) = showing.wanted(Urgency::Now, &visible, NO_NEIGHBOURS, NOT_HEAVY) {
            showing.page_read(wanted.page, Arc::clone(&huge), false, &visible);
            read += 1;
            assert!(read <= visible.len(), "it keeps asking");
        }
        assert!(read < visible.len(), "it stopped reading: {read}");
        assert!(showing.held_bytes() >= MOST_TEXT_BYTES && showing.held_bytes() < MOST_TEXT_BYTES + huge.bytes(), "{}", showing.held_bytes());
    }

    /// Failure: a whole-page selection reads as ending at character 4,294,967,294, in what QA and the budgets look at.
    #[test]
    fn a_report_doesnt_show_the_end_of_a_page_as_a_number() {
        let mut selecting = Selecting::default();
        selecting.set_range(Range::pages(0, 1));
        assert!(selecting.report()["end"]["at"].is_null(), "{}", selecting.report());
        assert_eq!(selecting.report()["end"]["page"], 2);
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
