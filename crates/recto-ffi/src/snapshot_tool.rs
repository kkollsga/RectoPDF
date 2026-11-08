//! **The snapshot tool in the window** (SPEC section 6, Extraction). Its rule is `recto_reader::snapshot`; this joins it to the mouse, the clipboard and the notice. With the tool on, what a press, drag and release on a page do goes here instead of to text selection: a drag draws a box, and a release takes it, PNG, text, SVG and JPG all on the clipboard at once; a click takes the image under the pointer, as its own JPEG bytes or a lossless PNG. What goes on the clipboard, in what order, is `recto_agent::extract`'s to say, the same functions `recto image` and `recto snapshot` use.
//!
//! **Nothing waits on the UI thread.** A release starts one short-lived thread, `recto-snapshot`, with a worker of its own for as long as it takes, and only the newest one's answer is kept. At rest there's no thread, worker, timer or buffer: the tool is a flag and two points, and what's waiting for the clipboard is held only until the host takes it.
//!
//! **A file that asks not to be copied** gets the notice text does, with "Copy anyway" (owner, 2026-11-08), which takes the same box or image again.

use crate::{HighlightRect, Inner, ReaderOp, lock};
use recto_agent::extract::{self, ClipOutcome};
use recto_reader::Engine;
use recto_reader::snapshot::{SnapshotTool, Taken};
use recto_session::TabId;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A snapshot thread's stack: a worker's replies and a PNG's compressor, with the rows on the heap.
const STACK: usize = 1024 * 1024;

/// What a release asked for, kept so "Copy anyway" can ask again.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Wanted {
    Image { x: f32, y: f32 },
    Box { rect: [f32; 4] },
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Job {
    tab: u32,
    /// The page's place in the tab, edits and all.
    position: u32,
    wanted: Wanted,
}

/// What to say by the page about the last snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolNotice {
    pub text: String,
    pub offers_copy_anyway: bool,
}

/// One thing for the clipboard, as the host puts it there.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ClipboardItem {
    /// Its uniform type identifier, such as `public.png`.
    pub uti: String,
    pub data: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct Tool {
    state: Mutex<SnapshotTool>,
    /// The tool's on, readable with no lock on every press.
    on: AtomicBool,
    /// The page the box was last drawn on, by pane, to take it off when it moves or goes.
    drawn: Mutex<Option<(usize, u32)>>,
    /// Bumped by each release, so only the newest answer is kept.
    generation: AtomicU64,
    running: AtomicU32,
    ready: Mutex<Option<Vec<ClipboardItem>>>,
    notice: Mutex<Option<ToolNotice>>,
    refused: Mutex<Option<Job>>,
}

impl Tool {
    pub(crate) fn is_on(&self) -> bool {
        self.on.load(Ordering::SeqCst)
    }

    /// The menu's and the panel's toggle. Turning it off takes any box away, which the next ops from a press would do anyway.
    pub(crate) fn toggle(&self) -> bool {
        let mut state = lock(&self.state);
        let on = !state.is_on();
        state.set_on(on);
        self.on.store(on, Ordering::SeqCst);
        on
    }

    /// A snapshot is being taken now, which a hand-off and `qa wait-settled` wait for.
    pub(crate) fn busy(&self) -> bool {
        self.running.load(Ordering::SeqCst) > 0
    }

    pub(crate) fn is_drawing(&self) -> bool {
        lock(&self.state).is_drawing()
    }

    pub(crate) fn notice(&self) -> Option<ToolNotice> {
        lock(&self.notice).clone()
    }

    pub(crate) fn dismiss_notice(&self) {
        lock(&self.notice).take();
        lock(&self.refused).take();
    }

    /// What's waiting for the clipboard, once. The host puts it there and it's gone from here.
    pub(crate) fn take_ready(&self) -> Option<Vec<ClipboardItem>> {
        lock(&self.ready).take()
    }

    /// The ops that draw the box as it stands, and take it off a page it's left.
    fn box_ops(&self, pane: usize, shown: Option<(u32, recto_index::select::Rect)>) -> Vec<ReaderOp> {
        let mut drawn = lock(&self.drawn);
        let mut ops = Vec::new();
        if let Some((was_pane, was_page)) = *drawn
            && (shown.is_none_or(|(page, _)| page != was_page) || was_pane != pane)
        {
            ops.push(ReaderOp::SnapshotBox { page: was_page, rect: None });
            *drawn = None;
        }
        if let Some((page, rect)) = shown {
            ops.push(ReaderOp::SnapshotBox { page, rect: Some(HighlightRect { x: f64::from(rect.x), y: f64::from(rect.y), width: f64::from(rect.width), height: f64::from(rect.height) }) });
            *drawn = Some((pane, page));
        }
        ops
    }

    pub(crate) fn press(&self, engine: &Engine, pane: usize, x: f64, y: f64) -> Vec<ReaderOp> {
        let mut state = lock(&self.state);
        if let Some((point, size)) = engine.point_on_page(x, y) {
            state.press(point, size);
        }
        let shown = state.shown_box();
        drop(state);
        self.box_ops(pane, shown)
    }

    pub(crate) fn drag(&self, engine: &Engine, pane: usize, x: f64, y: f64) -> Vec<ReaderOp> {
        let mut state = lock(&self.state);
        if let Some((point, _)) = engine.point_on_page(x, y) {
            state.drag(point);
        }
        let shown = state.shown_box();
        drop(state);
        self.box_ops(pane, shown)
    }

    /// The button came up: the box goes, and what it took is on its way to the clipboard.
    pub(crate) fn release(&self, inner: &Arc<Inner>, engine: &Engine, pane: usize) -> Vec<ReaderOp> {
        let taken = lock(&self.state).release();
        let ops = self.box_ops(pane, None);
        let Some(tab) = engine.hot_tab() else { return ops };
        let job = match taken {
            Taken::Click(point) => Job { tab, position: point.page, wanted: Wanted::Image { x: point.x, y: point.y } },
            Taken::Box { page, rect } => Job { tab, position: page, wanted: Wanted::Box { rect: [rect.x, rect.y, rect.width, rect.height] } },
            Taken::Nothing => return ops,
        };
        start(inner, job, false);
        ops
    }

    /// "Copy anyway" on the notice: the box or image the file refused, taken again. Says whether there was one.
    pub(crate) fn copy_anyway(&self, inner: &Arc<Inner>) -> bool {
        let Some(job) = lock(&self.refused).take() else { return false };
        lock(&self.notice).take();
        start(inner, job, true);
        true
    }
}

/// What the notice says when a snapshot couldn't be taken, in Rust's words.
fn notice_for(outcome: &ClipOutcome) -> ToolNotice {
    let plain = |text: &str| ToolNotice { text: text.into(), offers_copy_anyway: false };
    match outcome {
        ClipOutcome::Ready(clip) => plain(&clip.note),
        ClipOutcome::Restricted => ToolNotice { text: "This file's permissions ask that it isn't copied.".into(), offers_copy_anyway: true },
        ClipOutcome::NoImage => plain("There's no image there. Drag a box to take part of the page."),
        ClipOutcome::Failed(error) => plain(&error.message),
    }
}

/// Takes a job off the UI thread, a thread of its own for as long as it runs.
fn start(inner: &Arc<Inner>, job: Job, anyway: bool) {
    let tool = &inner.tool;
    let generation = tool.generation.fetch_add(1, Ordering::SeqCst) + 1;
    tool.running.fetch_add(1, Ordering::SeqCst);
    // A box on a heavy page can take seconds, so the reader is told it's on its way, until the answer replaces this.
    *lock(&tool.notice) = Some(ToolNotice { text: "Copying…".into(), offers_copy_anyway: false });
    lock(&tool.refused).take();
    inner.core.push(crate::CoreCall::Reconcile);
    // An update waits for it, as it does for a command.
    inner.host.running.fetch_add(1, Ordering::SeqCst);
    let runner = Arc::clone(inner);
    let spawned = std::thread::Builder::new().name("recto-snapshot".into()).stack_size(STACK).spawn(move || {
        let outcome = take(&runner, job, anyway);
        let tool = &runner.tool;
        // A newer release has the last word.
        if tool.generation.load(Ordering::SeqCst) == generation {
            *lock(&tool.notice) = Some(notice_for(&outcome));
            *lock(&tool.refused) = matches!(outcome, ClipOutcome::Restricted).then_some(job);
            if let ClipOutcome::Ready(clip) = outcome {
                *lock(&tool.ready) = Some(clip.items.into_iter().map(|item| ClipboardItem { uti: item.uti.to_string(), data: item.bytes }).collect());
                runner.listener.copy_ready();
            }
        }
        tool.running.fetch_sub(1, Ordering::SeqCst);
        runner.host.running.fetch_sub(1, Ordering::SeqCst);
        runner.core.push(crate::CoreCall::Reconcile);
    });
    if spawned.is_err() {
        tool.running.fetch_sub(1, Ordering::SeqCst);
        inner.host.running.fetch_sub(1, Ordering::SeqCst);
        *lock(&tool.notice) = Some(ToolNotice { text: "The snapshot couldn't be started.".into(), offers_copy_anyway: false });
        inner.core.push(crate::CoreCall::Reconcile);
    }
}

/// The page as it is in its file, for the tab and place the release was on, and what goes on the clipboard from it.
fn take(inner: &Inner, job: Job, anyway: bool) -> ClipOutcome {
    let source = inner.host.session.with_session(|session| session.page_source(TabId(job.tab), job.position));
    let Some(source) = source.filter(recto_session::PageSource::is_current) else {
        return ClipOutcome::Failed(recto_agent::AgentError::new("source_changed", "That page can't be read now: its file has changed or gone.", recto_agent::exit::FAILED));
    };
    match job.wanted {
        Wanted::Image { x, y } => extract::image_clip(&source.path, source.page, x, y, anyway),
        Wanted::Box { rect } => extract::snapshot_clip(&source.path, source.page, rect, anyway),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a refusal for permissions has no "Copy anyway", or a plain answer offers one, or the reader isn't told why nothing was copied by a click off any image.
    #[test]
    fn the_notice_says_what_happened_and_offers_copy_anyway_only_when_refused() {
        assert!(notice_for(&ClipOutcome::Restricted).offers_copy_anyway);
        let none = notice_for(&ClipOutcome::NoImage);
        assert!(!none.offers_copy_anyway && none.text.contains("no image"));
        let ready = notice_for(&ClipOutcome::Ready(extract::Clip { items: Vec::new(), note: "Copied.".into() }));
        assert_eq!((ready.text.as_str(), ready.offers_copy_anyway), ("Copied.", false));
    }

    /// Failure: the box stays on a page it's left, or isn't drawn where the tool says, so the reader sees two boxes, or none, while dragging.
    #[test]
    fn the_box_moves_with_the_drag_and_leaves_no_trace() {
        let tool = Tool::default();
        let rect = recto_index::select::Rect { x: 1.0, y: 2.0, width: 3.0, height: 4.0 };
        assert_eq!(tool.box_ops(0, Some((5, rect))), vec![ReaderOp::SnapshotBox { page: 5, rect: Some(HighlightRect { x: 1.0, y: 2.0, width: 3.0, height: 4.0 }) }]);
        let moved = tool.box_ops(0, Some((6, rect)));
        assert_eq!(moved[0], ReaderOp::SnapshotBox { page: 5, rect: None }, "off the page it left");
        assert_eq!(tool.box_ops(0, None), vec![ReaderOp::SnapshotBox { page: 6, rect: None }]);
        assert!(tool.box_ops(0, None).is_empty(), "nothing to take off twice");
    }
}
