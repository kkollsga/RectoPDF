//! **The snapshot tool in the window, and the clipboard** (SPEC section 6, Extraction). Its rule is `recto_reader::snapshot`; this joins it to the mouse, the clipboard and the notice. With the tool on, what a press, drag and release on a page do goes here instead of to text selection: a drag draws a box, and a release takes it, PNG, text, SVG and JPG all on the clipboard at once; a click takes the image under the pointer, as its own JPEG bytes or a lossless PNG. What goes on the clipboard, in what order, is `recto_agent::extract`'s to say, the same functions `recto image` and `recto snapshot` use.
//!
//! **One job at a time.** A release while another snapshot is being taken stops that one (its worker's cancel word, so a map nobody wants any more isn't finished) and waits in its place, the newest only: ten clicks on a map are one worker at a time, never ten (review, 2026-11-08). The job runs on one short-lived thread, `recto-snapshot`, which takes the waiting job when it's done with its own. At rest there's no thread, worker, timer or buffer: the tool is a flag and two points, and what's waiting for the clipboard is held only until the host takes it.
//!
//! **One clipboard.** A text copy and a snapshot both reach the host as one offer from [`AppCore::take_clipboard`]: the types in order, and which are made now and which only when a paste asks for them ([`AppCore::clipboard_data`]). A snapshot's SVG is the one promised: making it can take the larger worker, 742 MiB on the USGS map, and most pastes never want it.
//!
//! **A file that asks not to be copied** gets the notice text does, with "Copy anyway" (owner, 2026-11-08). Each notice has a number, and its button names it, so a stale button takes nothing: turning the tool, or starting a new box, takes the notice and what it offered away.

use crate::menus::{MenuEntry, MenuModel, PanelItem, PanelModel, PanelSection};
use crate::{AppCore, HighlightRect, Inner, ReaderOp, SelectionNotice, lock};
use recto_agent::extract::{self, ClipItem, ClipOutcome, ClipStop};
use recto_reader::Engine;
use recto_reader::snapshot::{SnapshotTool, Taken};
use recto_session::TabId;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A snapshot thread's stack: a worker's replies and a PNG's compressor, with the rows on the heap.
const STACK: usize = 1024 * 1024;

/// The menu's and the panel's id for the tool.
const TOGGLE_ID: &str = "tools.snapshot";
/// A notice's "Copy anyway", followed by the notice's number.
const ANYWAY_PREFIX: &str = "tools.copy_anyway.";
/// The tool's name in the menu and the panel.
const SNAPSHOT_TITLE: &str = "Snapshot";

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
    anyway: bool,
}

/// What to say by the page about the last snapshot, with its number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolNotice {
    pub serial: u64,
    pub text: String,
    pub offers_copy_anyway: bool,
}

/// What the clipboard is offered: its types in the order to offer them, which of those are made only when a paste asks, and the number that names this offer for [`AppCore::clipboard_data`].
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ClipboardOffer {
    pub serial: u64,
    /// Uniform type identifiers, such as `public.png`, in the order a paste should prefer them.
    pub types: Vec<String>,
    /// The ones made only when a paste asks for them: the host declares them and gives them when asked.
    pub promised: Vec<String>,
}

/// What a snapshot's promised SVG is made from, if a paste asks.
#[derive(Clone, Debug, PartialEq)]
struct Promise {
    path: PathBuf,
    page: u32,
    rect: [f32; 4],
}

/// What's on its way to the clipboard, or on it.
#[derive(Default)]
struct Offered {
    serial: u64,
    items: Vec<ClipItem>,
    promise: Option<Promise>,
}

/// **The one-at-a-time rule**, apart from threads so it's tested alone: the job running, by its stop, and the newest one waiting.
#[derive(Default)]
struct JobSlot {
    running: Option<ClipStop>,
    waiting: Option<Job>,
}

impl JobSlot {
    /// A new job. With nothing running, it's to start now, with this stop. Otherwise the running one is stopped and this waits in its place, replacing any that was waiting.
    fn offer(&mut self, job: Job) -> Option<(Job, ClipStop)> {
        if let Some(running) = &self.running {
            running.stop();
            self.waiting = Some(job);
            return None;
        }
        let stop = ClipStop::default();
        self.running = Some(stop.clone());
        Some((job, stop))
    }

    /// The running job is done: the waiting one runs next, or nothing does.
    fn next(&mut self) -> Option<(Job, ClipStop)> {
        self.running = None;
        let job = self.waiting.take()?;
        self.offer(job)
    }
}

#[derive(Default)]
pub(crate) struct Tool {
    state: Mutex<SnapshotTool>,
    /// The tool's on, readable with no lock on every press.
    on: AtomicBool,
    /// The page the box was last drawn on, by pane, to take it off when it moves or goes.
    drawn: Mutex<Option<(usize, u32)>>,
    jobs: Mutex<JobSlot>,
    running: AtomicU32,
    ready: Mutex<Option<Offered>>,
    /// What the host has been offered, held until a paste takes it or another copy replaces it.
    offered: Mutex<Option<Offered>>,
    serial: AtomicU64,
    notice: Mutex<Option<ToolNotice>>,
    /// What the notice with this number refused, for its "Copy anyway".
    refused: Mutex<Option<(u64, Job)>>,
    /// The app's quitting: a promised SVG isn't made on the way out.
    quitting: AtomicBool,
}

impl Tool {
    pub(crate) fn is_on(&self) -> bool {
        self.on.load(Ordering::SeqCst)
    }

    /// The menu's and the panel's toggle. Turning it either way takes any box and any notice away, and what a notice offered with it.
    pub(crate) fn toggle(&self) -> bool {
        let mut state = lock(&self.state);
        let on = !state.is_on();
        state.set_on(on);
        self.on.store(on, Ordering::SeqCst);
        drop(state);
        self.dismiss_notice();
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

    /// The notice as the window shows it, its button naming this notice.
    pub(crate) fn selection_notice(&self) -> Option<SelectionNotice> {
        self.notice().map(|notice| SelectionNotice { action_title: notice.offers_copy_anyway.then(|| "Copy anyway".to_string()), action_id: notice.offers_copy_anyway.then(|| format!("{ANYWAY_PREFIX}{}", notice.serial)), fades: !notice.offers_copy_anyway, text: notice.text })
    }

    pub(crate) fn dismiss_notice(&self) {
        lock(&self.notice).take();
        lock(&self.refused).take();
    }

    /// Says something by the page, under a new number.
    fn say(&self, text: impl Into<String>, offers_copy_anyway: bool) -> u64 {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.notice) = Some(ToolNotice { serial, text: text.into(), offers_copy_anyway });
        serial
    }

    /// The job a notice's "Copy anyway" names, once, if that notice is still the one showing.
    fn take_refused(&self, serial: u64) -> Option<Job> {
        let mut refused = lock(&self.refused);
        let job = refused.filter(|(offered, _)| *offered == serial).map(|(_, job)| job)?;
        refused.take();
        lock(&self.notice).take();
        Some(job)
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

    /// A press on a page starts a new box or click, so an old notice, and what it offered, goes.
    pub(crate) fn press(&self, engine: &Engine, pane: usize, x: f64, y: f64) -> Vec<ReaderOp> {
        self.dismiss_notice();
        let mut state = lock(&self.state);
        if let Some((point, size)) = engine.point_on_page(x, y) {
            state.press(point, size);
        }
        let shown = state.shown_box();
        drop(state);
        self.box_ops(pane, shown)
    }

    /// A drag goes on the page the box was pressed on, wherever the pointer is, held to that page's edges.
    pub(crate) fn drag(&self, engine: &Engine, pane: usize, x: f64, y: f64) -> Vec<ReaderOp> {
        let mut state = lock(&self.state);
        if let Some(point) = state.pressed_page().and_then(|page| engine.point_on_given_page(page, x, y)) {
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
            Taken::Click(point) => Job { tab, position: point.page, wanted: Wanted::Image { x: point.x, y: point.y }, anyway: false },
            Taken::Box { page, rect } => Job { tab, position: page, wanted: Wanted::Box { rect: [rect.x, rect.y, rect.width, rect.height] }, anyway: false },
            Taken::Nothing => return ops,
        };
        start(inner, job);
        ops
    }

    /// The app's on its way out: a paste that asks now for a promised SVG gets nothing, rather than holding the quit up for a worker.
    pub(crate) fn quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        if let Some(running) = &lock(&self.jobs).running {
            running.stop();
        }
    }

    /// One type of the offer with this number: its bytes, once, dropped from here as they go, or what a promised one is made from. Nothing for an offer replaced since, a type not in it, or a promise on the way out.
    fn data(&self, serial: u64, uti: &str) -> Option<Data> {
        let mut offered = lock(&self.offered);
        let current = offered.as_mut().filter(|offered| offered.serial == serial)?;
        let at = current.items.iter().position(|item| item.uti() == uti)?;
        match current.items[at] {
            ClipItem::Ready { .. } => match current.items.remove(at) {
                ClipItem::Ready { bytes, .. } => Some(Data::Ready(bytes)),
                ClipItem::Promised { .. } => None,
            },
            ClipItem::Promised { cap, .. } => {
                if self.quitting.load(Ordering::SeqCst) {
                    return None;
                }
                Some(Data::Make(current.promise.clone()?, cap))
            }
        }
    }

    /// A text copy, as one offer like any other.
    fn offer_text(&self, text: String) -> Offered {
        Offered { serial: 0, items: vec![ClipItem::Ready { uti: extract::UTI_TEXT, bytes: text.into_bytes() }], promise: None }
    }
}

/// The tool's place in the menu bar: Tools, with Snapshot ticked while it's on. ⇧⌘A, as a key nothing else here takes, and the screenshot keys are the system's.
pub(crate) fn tools_menu(snapshot_on: bool) -> MenuModel {
    MenuModel { title: "Tools".into(), entries: vec![MenuEntry { id: TOGGLE_ID.into(), title: SNAPSHOT_TITLE.into(), key: "A".into(), with_control: false, checked: snapshot_on, hidden: false, system_action: String::new(), parent: String::new() }] }
}

/// The panel's tools, above its view options. The panel holds tools while reading (owner, 2026-11-07: "The sidebar menu is more for tools, like edit mode etc."), and commands are in the menu bar too (decision 17).
pub(crate) fn tools_section(snapshot_on: bool) -> PanelSection {
    PanelSection { title: "Tools".into(), items: vec![PanelItem { id: TOGGLE_ID.into(), title: SNAPSHOT_TITLE.into(), selected: snapshot_on, enabled: true }] }
}

/// The tools go first in an open panel, except in edit mode, whose Done leads the panel and the tools come after it.
pub(crate) fn add_to_panel(panel: &mut PanelModel, snapshot_on: bool, edit_mode: bool) {
    if panel.open {
        let at = usize::from(edit_mode).min(panel.sections.len());
        panel.sections.insert(at, tools_section(snapshot_on));
    }
}

/// Whether a menu item is the tool's, for QA's `menu`, which takes only what a menu has.
pub(crate) fn is_menu_id(id: &str) -> bool {
    id == TOGGLE_ID
}

/// What a menu item, panel button or notice button does, if it's the tool's: toggling it, or a notice's "Copy anyway", which takes again only what that notice refused. Says whether it was the tool's.
pub(crate) fn perform(inner: &Arc<Inner>, id: &str) -> bool {
    if id == TOGGLE_ID {
        inner.tool.toggle();
        inner.core.push(crate::CoreCall::Reconcile);
        inner.listener.model_changed();
        return true;
    }
    let Some(serial) = id.strip_prefix(ANYWAY_PREFIX) else { return false };
    if let Some(job) = serial.parse().ok().and_then(|serial| inner.tool.take_refused(serial)) {
        start(inner, Job { anyway: true, ..job });
    }
    inner.core.push(crate::CoreCall::Reconcile);
    true
}

/// What the notice says when a snapshot couldn't be taken, in Rust's words, and whether it offers "Copy anyway".
fn words_for(outcome: &ClipOutcome) -> (String, bool) {
    match outcome {
        ClipOutcome::Ready(clip) => (clip.note.clone(), false),
        ClipOutcome::Restricted => ("This file's permissions ask that it isn't copied.".into(), true),
        ClipOutcome::NoImage => ("There's no image there. Drag a box to take part of the page.".into(), false),
        ClipOutcome::Stopped => (String::new(), false),
        ClipOutcome::Failed(error) => (error.message.clone(), false),
    }
}

/// Takes a job off the UI thread: now, on a thread of its own, or after the one running, which it stops.
fn start(inner: &Arc<Inner>, job: Job) {
    let tool = &inner.tool;
    // A box on a heavy page can take seconds, so the reader is told it's on its way, until the answer replaces this.
    tool.say("Copying…", false);
    lock(&tool.refused).take();
    inner.core.push(crate::CoreCall::Reconcile);
    let Some(first) = lock(&tool.jobs).offer(job) else { return };
    tool.running.fetch_add(1, Ordering::SeqCst);
    // An update waits for it, as it does for a command.
    inner.host.running.fetch_add(1, Ordering::SeqCst);
    let runner = Arc::clone(inner);
    let spawned = std::thread::Builder::new().name("recto-snapshot".into()).stack_size(STACK).spawn(move || {
        let mut next = Some(first);
        while let Some((job, stop)) = next {
            let (outcome, source) = take(&runner, job, &stop);
            // Under the slot's lock, so a release now either waits for this thread or finds the slot free after the answer is in. Nothing here takes the session's lock.
            let mut jobs = lock(&runner.tool.jobs);
            next = jobs.next();
            if next.is_none() {
                publish(&runner, job, outcome, source);
            }
        }
        runner.tool.running.fetch_sub(1, Ordering::SeqCst);
        runner.host.running.fetch_sub(1, Ordering::SeqCst);
        runner.core.push(crate::CoreCall::Reconcile);
    });
    if spawned.is_err() {
        *lock(&tool.jobs) = JobSlot::default();
        tool.running.fetch_sub(1, Ordering::SeqCst);
        inner.host.running.fetch_sub(1, Ordering::SeqCst);
        tool.say("The snapshot couldn't be started.", false);
        inner.core.push(crate::CoreCall::Reconcile);
    }
}

/// The newest job's answer: the notice, and what's for the clipboard.
fn publish(inner: &Inner, job: Job, outcome: ClipOutcome, source: Option<(PathBuf, u32)>) {
    let tool = &inner.tool;
    if matches!(outcome, ClipOutcome::Stopped) {
        lock(&tool.notice).take();
        return;
    }
    let (text, offers) = words_for(&outcome);
    let serial = tool.say(text, offers);
    *lock(&tool.refused) = offers.then_some((serial, job));
    if let ClipOutcome::Ready(clip) = outcome {
        let promise = match job.wanted {
            Wanted::Box { rect } => source.map(|(path, page)| Promise { path, page, rect }),
            Wanted::Image { .. } => None,
        };
        let items = clip.items.into_iter().filter(|item| matches!(item, ClipItem::Ready { .. }) || promise.is_some()).collect();
        *lock(&tool.ready) = Some(Offered { serial: 0, items, promise });
        inner.listener.copy_ready();
    }
}

/// The page as it is in its file, for the tab and place the release was on.
fn source_of(inner: &Inner, job: Job) -> Option<(PathBuf, u32)> {
    let source = inner.host.session.with_session(|session| session.page_source(TabId(job.tab), job.position));
    source.filter(recto_session::PageSource::is_current).map(|source| (source.path, source.page))
}

/// What goes on the clipboard for a job, from the page as it is in its file.
fn take(inner: &Inner, job: Job, stop: &ClipStop) -> (ClipOutcome, Option<(PathBuf, u32)>) {
    let Some((path, page)) = source_of(inner, job) else {
        return (ClipOutcome::Failed(recto_agent::AgentError::new("source_changed", "That page can't be read now: its file has changed or gone.", recto_agent::exit::FAILED)), None);
    };
    let outcome = match job.wanted {
        Wanted::Image { x, y } => extract::image_clip(&path, page, x, y, job.anyway, stop),
        Wanted::Box { rect } => extract::snapshot_clip(&path, page, rect, job.anyway, stop),
    };
    (outcome, Some((path, page)))
}

/// What [`Tool::data`] hands over: bytes made with the copy, or what to make a promised type from.
#[derive(Debug, PartialEq)]
enum Data {
    Ready(Vec<u8>),
    Make(Promise, usize),
}

#[uniffi::export]
impl AppCore {
    /// What a copy has for the clipboard, a snapshot's or the selection's text, as one offer: the types in order, and which are made only when a paste asks. `copy_ready` says when. The bytes come one type at a time from `clipboard_data`, so no more than one is copied across at once.
    pub fn take_clipboard(&self) -> Option<ClipboardOffer> {
        let tool = &self.inner.tool;
        let mut offered = lock(&tool.ready).take().or_else(|| self.inner.panes.engines(&self.inner.engine).iter().find_map(Engine::take_copied).map(|copied| tool.offer_text(copied.text)))?;
        offered.serial = tool.serial.fetch_add(1, Ordering::SeqCst) + 1;
        let types = offered.items.iter().map(|item| item.uti().to_string()).collect();
        let promised = offered.items.iter().filter(|item| matches!(item, ClipItem::Promised { .. })).map(|item| item.uti().to_string()).collect();
        let offer = ClipboardOffer { serial: offered.serial, types, promised };
        // Whatever was offered before is gone from the clipboard now, its promise with it.
        *lock(&tool.offered) = Some(offered);
        Some(offer)
    }

    /// One type's bytes for the offer with this number, once. A type made with the copy is handed over and dropped from here. A promised one is made now, as a paste asks for it, which takes a worker and can take seconds: the system asks on the UI thread and waits for the answer, which is the one time the UI thread waits here, and only for a paste of that type from another app. Nothing for an offer replaced since, or on the way out.
    pub fn clipboard_data(&self, serial: u64, uti: String) -> Option<Vec<u8>> {
        match self.inner.tool.data(serial, &uti)? {
            Data::Ready(bytes) => Some(bytes),
            Data::Make(promise, cap) => extract::snapshot_svg(&promise.path, promise.page, promise.rect, cap).ok(),
        }
    }

    /// The clipboard has something else on it now: what this offer promised is let go.
    pub fn clipboard_released(&self, serial: u64) {
        let mut offered = lock(&self.inner.tool.offered);
        if offered.as_ref().is_some_and(|offered| offered.serial == serial) {
            offered.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(x: f32) -> Job {
        Job { tab: 1, position: 0, wanted: Wanted::Image { x, y: 0.0 }, anyway: false }
    }

    /// Failure: a refusal for permissions has no "Copy anyway", or a plain answer offers one, or the reader isn't told why nothing was copied by a click off any image.
    #[test]
    fn the_notice_says_what_happened_and_offers_copy_anyway_only_when_refused() {
        assert!(words_for(&ClipOutcome::Restricted).1);
        let (text, offers) = words_for(&ClipOutcome::NoImage);
        assert!(!offers && text.contains("no image"));
        let ready = words_for(&ClipOutcome::Ready(extract::Clip { items: Vec::new(), note: "Copied.".into() }));
        assert_eq!(ready, ("Copied.".to_string(), false));
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

    /// Failure: each release starts a worker of its own, so ten clicks on the USGS map are ten workers of 350 to 750 MiB at once (review, 2026-11-08); or the job that runs after a burst isn't the newest; or the running one isn't stopped.
    #[test]
    fn jobs_run_one_at_a_time_and_only_the_newest_waits() {
        let mut slot = JobSlot::default();
        let (first, running) = slot.offer(job(1.0)).expect("nothing running, so it starts");
        assert_eq!(first, job(1.0));
        for x in 2..=10 {
            assert!(slot.offer(job(x as f32)).is_none(), "one running, so the rest wait");
        }
        assert!(running.is_stopped(), "the running one is stopped for the newer");
        let (next, stop) = slot.next().expect("the newest waits");
        assert_eq!(next, job(10.0), "only the newest");
        assert!(!stop.is_stopped());
        assert!(slot.next().is_none(), "and nothing after it");
        assert!(slot.offer(job(11.0)).is_some(), "the slot's free again");
    }

    /// Failure: a "Copy anyway" from a notice that's gone, or from an older one, copies whatever was refused last, perhaps another box or file; or turning the tool or starting a new box leaves the old offer standing (review, 2026-11-08). Each button names its notice, and only that notice's refusal is taken, once.
    #[test]
    fn copy_anyway_takes_only_what_its_own_notice_refused() {
        let tool = Tool::default();
        let serial = tool.say("This file's permissions ask that it isn't copied.", true);
        *lock(&tool.refused) = Some((serial, job(1.0)));
        assert_eq!(tool.selection_notice().and_then(|notice| notice.action_id), Some(format!("{ANYWAY_PREFIX}{serial}")));
        assert_eq!(tool.take_refused(serial + 1), None, "another notice's button");
        assert_eq!(tool.take_refused(serial), Some(job(1.0)));
        assert_eq!(tool.take_refused(serial), None, "once");
        let serial = tool.say("This file's permissions ask that it isn't copied.", true);
        *lock(&tool.refused) = Some((serial, job(2.0)));
        tool.toggle();
        assert_eq!((tool.take_refused(serial), tool.notice()), (None, None), "turning the tool takes the offer away");
        let serial = tool.say("This file's permissions ask that it isn't copied.", true);
        *lock(&tool.refused) = Some((serial, job(3.0)));
        tool.dismiss_notice();
        assert_eq!(tool.take_refused(serial), None);
    }

    /// Failure: the tool can't be found or doesn't say whether it's on, in the menu bar or the panel, or its key is one something else takes.
    #[test]
    fn the_snapshot_tool_is_in_the_tools_menu_and_the_panel() {
        let menu = tools_menu(true);
        assert_eq!((menu.entries[0].id.as_str(), menu.entries[0].checked, menu.entries[0].key.as_str()), (TOGGLE_ID, true, "A"));
        assert!(!tools_menu(false).entries[0].checked);
        assert!(tools_section(true).items[0].selected && !tools_section(false).items[0].selected);
        let taken: Vec<(String, bool)> = crate::menus::menu_bar(recto_session::ViewSettings::default(), None, "").iter().flat_map(|menu| &menu.entries).map(|item| (item.key.clone(), item.with_control)).collect();
        assert!(!taken.contains(&("A".to_string(), false)), "⇧⌘A is free");
    }

    /// Failure: the snapshot tool pushes edit mode's Done off the top of the panel, where the reader leaves edit mode from, or shows in a closed panel.
    #[test]
    fn the_tools_go_first_except_after_edit_modes_done() {
        let shown = recto_session::Paper::White;
        let mut reading = crate::menus::panel(true, shown, false, true);
        add_to_panel(&mut reading, false, false);
        assert_eq!(reading.sections[0].items[0].id, TOGGLE_ID);
        let mut editing = crate::menus::panel(true, shown, true, true);
        add_to_panel(&mut editing, false, true);
        assert_eq!((editing.sections[0].items[0].id.as_str(), editing.sections[1].items[0].id.as_str()), ("edit_mode.done", TOGGLE_ID));
        let mut closed = crate::menus::panel(false, shown, false, true);
        add_to_panel(&mut closed, true, false);
        assert!(closed.sections.iter().all(|section| section.items.iter().all(|item| item.id != TOGGLE_ID)));
    }

    /// Failure: the clipboard hands a type's bytes to an offer that's been replaced, or twice, so they're held after the host has them; or a promised SVG is made on the way out, holding up a quit for a worker; or a text copy isn't one offer like a snapshot's.
    #[test]
    fn an_offers_bytes_go_once_to_that_offer() {
        let tool = Tool::default();
        let promise = Promise { path: "/a.pdf".into(), page: 0, rect: [0.0, 0.0, 1.0, 1.0] };
        *lock(&tool.offered) = Some(Offered { serial: 7, items: vec![ClipItem::Ready { uti: extract::UTI_PNG, bytes: vec![1] }, ClipItem::Promised { uti: extract::UTI_SVG, cap: 9 }], promise: Some(promise.clone()) });
        assert_eq!(tool.data(6, extract::UTI_PNG), None, "an older offer");
        assert_eq!(tool.data(7, extract::UTI_PNG), Some(Data::Ready(vec![1])));
        assert_eq!(tool.data(7, extract::UTI_PNG), None, "once");
        assert_eq!(tool.data(7, extract::UTI_SVG), Some(Data::Make(promise.clone(), 9)), "a promise, as often as a paste asks");
        assert_eq!(tool.data(7, extract::UTI_SVG), Some(Data::Make(promise, 9)));
        tool.quitting();
        assert_eq!(tool.data(7, extract::UTI_SVG), None, "nothing made on the way out");
        assert_eq!(tool.offer_text("words".into()).items, vec![ClipItem::Ready { uti: extract::UTI_TEXT, bytes: b"words".to_vec() }]);
    }
}
