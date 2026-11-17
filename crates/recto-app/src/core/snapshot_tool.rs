//! **The snapshot tool in the window, and the clipboard** (SPEC section 6, Extraction). Its rule is `recto_reader::snapshot`; this joins it to the mouse, the clipboard and the notice. With the tool on, what a press, drag and release on a page do goes here instead of to text selection: a drag draws a box, and a release takes it, PNG, text, SVG and JPG all on the clipboard at once; a click takes the image under the pointer, as its own JPEG bytes or a lossless PNG.
//!
//! **One action, one implementation** (S2's follow-up (1), 2026-11-14): what a release takes is the person's own command, run by the one executor as any
//! of theirs is ([`super::person::submit`]): `snapshot --to-clipboard` of the box, or `image --at x,y --to-clipboard` of the click, naming the tab. So the
//! rules the executor holds (the tab held while it reads, a file that asks not to be copied and its words, the person's password for their own document,
//! the person's lane) are the command's alone. Its output, the window's clipboard, is given only to the actor at the window
//! ([`recto_agent::clipboard`]): what's made stays in memory, held here until the host takes it, and no file or descriptor is ever made for it.
//!
//! **One job at a time.** A release while another snapshot is being taken stops that one (its worker's cancel word, so a map nobody wants any more isn't finished) and waits in its place, the newest only: ten clicks on a map are one worker at a time, never ten (review, 2026-11-08). The waiting one is sent when the running one's answered. At rest there's no thread, worker, timer or buffer: the tool is a flag and two points, and what's waiting for the clipboard is held only until the host takes it.
//!
//! **The same slot holds the look for an image picked with a click in read mode** ([`super::image_select`]), so the app has one private worker at a time
//! across picks and copies (the review of `build/image-select`, S2): a newer look stops a running look, and so does a copy; a look never stops a copy, and
//! waits behind it; with a copy and a look both waiting, the copy goes first. A look the person has moved on from (let go, another tab, the tool turned
//! on) is stopped by the app's next look at what the page views want ([`Tool::looks`]), and a quit stops whatever runs.
//!
//! **One clipboard.** A text copy and a snapshot reach the host through [`Tool::take_offer`], with every advertised type prepared. The asynchronous snapshot command makes SVG in its existing worker before publishing its result. The synchronous host callback only takes ready bytes: it starts no command and never waits for rendering. Legacy promise/action state remains covered by its cancellation tests, but the host neither receives a promised type from a new snapshot nor calls that action.
//!
//! **A file that asks not to be copied** gets the notice text does, with "Copy anyway" (owner, 2026-11-08). Each notice has a number, and its button names it, so a stale button takes nothing: turning the tool, or starting a new box, takes the notice and what it offered away.

use super::image_select::Look;
use super::{HighlightRect, Inner, ReaderOp, SelectionNotice, lock};
pub use crate::snapshot_menu::{CANCEL_ID, CAPTURE_ID, TOGGLE_ID, is_menu_id};
use recto_agent::clipboard::{ClipboardKind, kind_of};
use recto_agent::commands::Read;
use recto_agent::commands::export::{Image, Snapshot};
use recto_agent::extract::{self, Clip, ClipItem, ClipStop};
use recto_agent::{AgentError, Code, Command, TabRef};
use recto_reader::Engine;
use recto_reader::snapshot::{SnapshotTool, Taken};
use recto_session::TabId;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

/// A notice's "Copy anyway", followed by the notice's number.
const ANYWAY_PREFIX: &str = "tools.copy_anyway.";

/// The legacy explicit-promise action's patience on the app's clock. New snapshots prepare all types first, and the synchronous bridge only takes
/// ready bytes; it never invokes this wait. Keep the existing bounded action for compatibility and its cancellation/recovery guards.
pub const PASTE_PATIENCE_MS: u64 = 3_000;

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
    /// The page at that place as the view showed it, which the command is refused by if another is there when it runs: it waits its turn in the
    /// person's lane, behind any page delete or move asked before it (the branch's review, S1).
    seen: extract::AsSeen,
    wanted: Wanted,
    anyway: bool,
}

impl Job {
    /// **The person's command for it,** naming its tab: the box as `snapshot --as png --to-clipboard`, every type at once; the click as
    /// `image --at x,y --to-clipboard`.
    fn command(self) -> Command {
        let tab = TabRef::Id(TabId(self.tab));
        let page = self.position + 1;
        match self.wanted {
            Wanted::Box { rect } => Command::from(Snapshot {
                tab,
                page,
                rect,
                format: "png".into(),
                out: None,
                scale: extract::DEFAULT_SCALE,
                anyway: self.anyway,
                to_clipboard: true,
                as_seen: Some(self.seen),
            }),
            Wanted::Image { x, y } => Command::from(Image {
                tab,
                page,
                number: None,
                out: None,
                anyway: self.anyway,
                at: Some([x, y]),
                to_clipboard: true,
                as_seen: Some(self.seen),
            }),
        }
    }

    /// The job a command of the tool's is, read back from it: none for any other command, a paste's SVG included.
    fn of(command: &Command) -> Option<Self> {
        let (tab, page, seen, wanted, anyway) = match (kind_of(command)?, command) {
            (ClipboardKind::Box, Command::Read(Read::Snapshot(asked))) => {
                (&asked.tab, asked.page, asked.as_seen?, Wanted::Box { rect: asked.rect }, asked.anyway)
            }
            (ClipboardKind::Click, Command::Read(Read::Image(asked))) => {
                let [x, y] = asked.at?;
                (&asked.tab, asked.page, asked.as_seen?, Wanted::Image { x, y }, asked.anyway)
            }
            _ => return None,
        };
        let TabRef::Id(tab) = tab else { return None };
        Some(Self {
            tab: tab.0,
            position: page.checked_sub(1)?,
            seen,
            wanted,
            anyway,
        })
    }
}

/// Whether a command is the person's paste of a promised SVG.
fn is_paste(command: &Command) -> bool {
    kind_of(command) == Some(ClipboardKind::Paste)
}

/// What to say by the page about the last snapshot, with its number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolNotice {
    pub serial: u64,
    pub text: String,
    pub offers_copy_anyway: bool,
}

/// What a snapshot's promised SVG is made from, if a paste asks: the box on its tab's page, which the person's command reads again then, as the box saw it
/// (its turn, and only the same page of its file), so the clipboard's types are one copy of one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Promise {
    pub tab: u32,
    pub position: u32,
    pub rect: [f32; 4],
    /// As the box's answer said it, read back typed: a box whose answer doesn't say it promises no SVG (fails closed).
    pub as_seen: extract::AsSeen,
}

/// What's on its way to the clipboard, or on it.
#[derive(Default)]
pub struct Offered {
    pub serial: u64,
    pub items: Vec<ClipItem>,
    /// Boxed, so the tool's state stays its size whatever a promise holds (the app's state has a budget: `inner_guards`).
    promise: Option<Box<Promise>>,
}

/// What runs in the one slot, by its stop: a snapshot (a box, a click, or the person's Copy of an image picked), or a click's look for its image.
enum Running {
    Copy(ClipStop),
    Look(Look, ClipStop),
}

impl Running {
    fn stop(&self) {
        match self {
            Self::Copy(stop) | Self::Look(_, stop) => stop.stop(),
        }
    }
}

/// What runs once the running job has answered.
#[derive(Debug)]
enum Next {
    Nothing,
    /// The snapshot that waited, to send now.
    Copy(Job),
    /// A look waits: the app's next look at what the page views want starts it ([`Tool::looks`]), as only they know whether it's still wanted.
    Looks,
}

/// **The one-at-a-time rule**, apart from threads so it's tested alone: the job running, by its stop, the newest snapshot waiting and the newest look
/// waiting, what the running one made before it's answered, and the release's box before `act` takes it.
#[derive(Default)]
struct JobSlot {
    running: Option<Running>,
    waiting: Option<Job>,
    looking: Option<Look>,
    /// The look a snapshot stopped, which runs again after it: its stopped answer says nothing of its click.
    again: Option<Look>,
    made: Option<Clip>,
    taken: Option<Job>,
}

impl JobSlot {
    /// A new snapshot. With nothing running, it's to start now, with this stop. Otherwise what runs is stopped, a snapshot or a look, and this waits in
    /// its place, replacing any snapshot that was waiting. A look it stops waits behind it to run again, unless a newer look waits already.
    fn offer(&mut self, job: Job) -> Option<(Job, ClipStop)> {
        if let Some(running) = &self.running {
            running.stop();
            if let Running::Look(look, _) = running
                && self.looking.is_none()
            {
                self.looking = Some(*look);
                self.again = Some(*look);
            }
            self.waiting = Some(job);
            return None;
        }
        let stop = ClipStop::default();
        self.running = Some(Running::Copy(stop.clone()));
        Some((job, stop))
    }

    /// The running job is done: the snapshot waiting runs next, then a look waiting, or nothing does.
    fn next(&mut self) -> Next {
        self.running = None;
        if let Some(job) = self.waiting.take() {
            self.running = Some(Running::Copy(ClipStop::default()));
            return Next::Copy(job);
        }
        if self.looking.is_some() { Next::Looks } else { Next::Nothing }
    }

    /// **The looks the page views want now,** newest last: a running look no longer wanted is stopped, and one waiting no longer wanted dropped. The newest
    /// wanted that isn't running starts now if nothing runs; otherwise it waits its turn, stopping a running look (never a snapshot). Says the look to send
    /// now, and those wanted that a newer one took the place of, for their page views to be told none was found.
    fn looks(&mut self, wanted: &[Look]) -> (Option<(Look, ClipStop)>, Vec<Look>) {
        let wants = |look: &Look| wanted.iter().any(|other| other.same(look));
        let running = match &self.running {
            Some(Running::Look(look, stop)) => {
                if !wants(look) {
                    stop.stop();
                }
                Some(*look)
            }
            _ => None,
        };
        self.looking = self.looking.filter(|look| wants(look));
        let others: Vec<Look> = wanted
            .iter()
            .filter(|look| running.is_none_or(|running| !running.same(look)))
            .copied()
            .collect();
        let Some((newest, older)) = others.split_last() else {
            return (None, Vec::new());
        };
        let replaced = older.to_vec();
        match &self.running {
            None => {
                self.looking = None;
                let stop = ClipStop::default();
                self.running = Some(Running::Look(*newest, stop.clone()));
                (Some((*newest, stop)), replaced)
            }
            Some(running) => {
                if matches!(running, Running::Look(..)) {
                    running.stop();
                }
                self.looking = Some(*newest);
                (None, replaced)
            }
        }
    }

    /// The running job is the look for this click (its tab and point): taken off, as its answer is in, and whether it runs again (a snapshot stopped it).
    fn take_look(&mut self, asked: (u32, recto_index::select::PagePoint)) -> Option<(Look, bool)> {
        let look = match &self.running {
            Some(Running::Look(look, _)) if (look.tab, look.point) == asked => *look,
            _ => return None,
        };
        let again = self.again.take().is_some_and(|again| again.same(&look));
        Some((look, again))
    }
}

/// **The system's paste of a promised SVG, waiting:** one at a time, as the system asks one at a time on its UI thread.
#[derive(Default)]
struct PasteState {
    /// The person's command for it is under way.
    running: bool,
    /// A paste waits for it, and gives up at `deadline`, on the app's clock.
    waiting: bool,
    deadline: u64,
    made: Option<Vec<u8>>,
    stop: ClipStop,
}

#[derive(Default)]
struct Paste {
    state: Mutex<PasteState>,
    changed: Condvar,
    /// The app's clock, set once at the start ([`sink`]): a test's own.
    clock: std::sync::OnceLock<Arc<dyn crate::clock::Clock>>,
}

impl Paste {
    /// Now, by the app's clock; past any deadline without one.
    fn now_ms(&self) -> u64 {
        self.clock.get().map_or(u64::MAX, |clock| clock.now_ms())
    }
}

#[derive(Default)]
pub struct Tool {
    state: Mutex<SnapshotTool>,
    /// The tool's on, readable with no lock on every press.
    on: AtomicBool,
    /// A one-shot capture's release put the tool off, and the host isn't told yet ([`taken`]).
    once_ended: AtomicBool,
    /// The page the box was last drawn on, by pane, to take it off when it moves or goes.
    drawn: Mutex<Option<(usize, u32)>>,
    jobs: Mutex<Box<JobSlot>>,
    pub ready: Mutex<Option<Offered>>,
    /// What the host has been offered, held until a paste takes it or another copy replaces it.
    pub offered: Mutex<Option<Offered>>,
    pub serial: AtomicU64,
    notice: Mutex<Option<ToolNotice>>,
    /// What the notice with this number refused, for its "Copy anyway".
    refused: Mutex<Option<(u64, Job)>>,
    /// The app's quitting: a promised SVG isn't made on the way out.
    quitting: AtomicBool,
    paste: Box<Paste>,
}

impl Tool {
    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::SeqCst)
    }

    /// The menu's and the panel's toggle. Turning it either way takes any box and any notice away, and what a notice offered with it.
    pub fn toggle(&self) -> bool {
        let mut state = lock(&self.state);
        let on = !state.is_on();
        state.set_on(on);
        self.on.store(on, Ordering::SeqCst);
        drop(state);
        self.dismiss_notice();
        on
    }

    /// **Capture** (the page's right-click menu; owner, 2026-11-16): the tool on for one capture, the next release it handles, which turns it off. Chosen
    /// with the tool on from the menu bar, it leaves it on and makes it one-shot. Whether anything changed, which is what the host is told.
    pub fn arm_once(&self) -> bool {
        let mut state = lock(&self.state);
        let was_on = state.is_on();
        let changed = state.arm_once();
        self.on.store(true, Ordering::SeqCst);
        drop(state);
        if !was_on {
            self.dismiss_notice();
        }
        changed
    }

    /// Escape before a capture is taken: the one-shot ends, the tool off. Nothing for a tool that's on from the menu bar. Whether it ended one.
    pub fn cancel_once(&self) -> bool {
        let mut state = lock(&self.state);
        let ended = state.end_once();
        self.on.store(state.is_on(), Ordering::SeqCst);
        drop(state);
        if ended {
            self.dismiss_notice();
        }
        ended
    }

    /// Whether the tool is on for one capture, which Escape ends.
    pub fn is_once(&self) -> bool {
        lock(&self.state).is_once()
    }

    /// A snapshot or a click's look is being taken now, or waits its turn, which a hand-off and `qa wait-settled` wait for.
    pub fn busy(&self) -> bool {
        let jobs = lock(&self.jobs);
        jobs.running.is_some() || jobs.waiting.is_some() || jobs.looking.is_some()
    }

    /// **The looks the page views want now** ([`super::image_select::looks_ready`]), by the slot's rule ([`JobSlot::looks`]): the look to send now, and
    /// those a newer one took the place of.
    pub(super) fn looks(&self, wanted: &[Look]) -> (Option<Look>, Vec<Look>) {
        // On the way out no new private worker starts: what runs is stopped, and the clicks are told none was found.
        if self.quitting.load(Ordering::SeqCst) {
            let mut jobs = lock(&self.jobs);
            jobs.looks(&[]);
            jobs.looking = None;
            return (None, wanted.to_vec());
        }
        let (start, replaced) = lock(&self.jobs).looks(wanted);
        (start.map(|(look, _)| look), replaced)
    }

    /// A look that couldn't be sent, as the app hands off: it isn't running.
    pub(super) fn look_not_sent(&self) {
        let mut jobs = lock(&self.jobs);
        if matches!(jobs.running, Some(Running::Look(..))) {
            jobs.running = None;
        }
    }

    /// A test's: whether the look running in the slot has been stopped, or none runs.
    #[cfg(test)]
    pub(crate) fn look_stopped(&self) -> Option<bool> {
        match &lock(&self.jobs).running {
            Some(Running::Look(_, stop)) => Some(stop.is_stopped()),
            _ => None,
        }
    }

    /// Nothing runs and a look waits its turn: the app's to look at what the page views want, which starts it.
    pub(super) fn look_to_start(&self) -> bool {
        let jobs = lock(&self.jobs);
        jobs.running.is_none() && jobs.looking.is_some()
    }

    pub fn is_drawing(&self) -> bool {
        lock(&self.state).is_drawing()
    }

    pub fn notice(&self) -> Option<ToolNotice> {
        lock(&self.notice).clone()
    }

    /// The notice as the window shows it, its button naming this notice.
    pub fn selection_notice(&self) -> Option<SelectionNotice> {
        self.notice().map(|notice| SelectionNotice {
            action_title: notice.offers_copy_anyway.then(|| "Copy anyway".to_string()),
            action_id: notice.offers_copy_anyway.then(|| format!("{ANYWAY_PREFIX}{}", notice.serial)),
            fades: !notice.offers_copy_anyway,
            text: notice.text,
        })
    }

    pub fn dismiss_notice(&self) {
        lock(&self.notice).take();
        lock(&self.refused).take();
    }

    /// Says something by the page, under a new number.
    fn say(&self, text: impl Into<String>, offers_copy_anyway: bool) -> u64 {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.notice) = Some(ToolNotice {
            serial,
            text: text.into(),
            offers_copy_anyway,
        });
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
            ops.push(ReaderOp::SnapshotBox {
                page,
                rect: Some(HighlightRect {
                    x: f64::from(rect.x),
                    y: f64::from(rect.y),
                    width: f64::from(rect.width),
                    height: f64::from(rect.height),
                }),
            });
            *drawn = Some((pane, page));
        }
        ops
    }

    /// A press on a page starts a new box or click, so an old notice, and what it offered, goes.
    pub fn press(&self, engine: &Engine, pane: usize, x: f64, y: f64) -> Vec<ReaderOp> {
        self.dismiss_notice();
        let mut state = lock(&self.state);
        if let Some((point, size)) = engine.points().on_page(x, y) {
            state.press(point, size);
        }
        let shown = state.shown_box();
        drop(state);
        self.box_ops(pane, shown)
    }

    /// A drag goes on the page the box was pressed on, wherever the pointer is, held to that page's edges.
    pub fn drag(&self, engine: &Engine, pane: usize, x: f64, y: f64) -> Vec<ReaderOp> {
        let mut state = lock(&self.state);
        if let Some(point) = state.pressed_page().and_then(|page| engine.points().on_given_page(page, x, y)) {
            state.drag(point);
        }
        let shown = state.shown_box();
        drop(state);
        self.box_ops(pane, shown)
    }

    /// **The button came up:** the box goes, and what it took is kept for the person's action that takes it ([`super::controls::UiAction::SnapshotTaken`],
    /// [`taken`]), which the bridge sends next, with the page it was taken from as the view shows it now.
    pub fn release(&self, engine: &Engine, pane: usize) -> Vec<ReaderOp> {
        let (taken, ended) = {
            let mut state = lock(&self.state);
            let was_on = state.is_on();
            let taken = state.release();
            self.on.store(state.is_on(), Ordering::SeqCst);
            (taken, was_on && !state.is_on())
        };
        // The notice isn't dismissed here, as a toggle does: the capture's own answer is coming, and "Copy anyway" must still work after.
        if ended {
            self.once_ended.store(true, Ordering::SeqCst);
        }
        let ops = self.box_ops(pane, None);
        let Some(tab) = engine.hot_tab() else { return ops };
        let wanted = match taken {
            // An image goes as itself, its own bytes, so it isn't turned.
            Taken::Click(point) => Some((point.page, Wanted::Image { x: point.x, y: point.y })),
            Taken::Box { page, rect } => Some((
                page,
                Wanted::Box {
                    rect: [rect.x, rect.y, rect.width, rect.height],
                },
            )),
            Taken::Nothing => None,
        };
        lock(&self.jobs).taken = wanted.and_then(|(position, wanted)| {
            let seen = engine.points().page_seen(position)?;
            Some(Job {
                tab,
                position,
                seen: extract::AsSeen {
                    quarter_turns: seen.quarter_turns,
                    file_page: seen.file_page,
                    own: seen.own,
                },
                wanted,
                anyway: false,
            })
        });
        ops
    }

    /// **The person's Copy of an image picked with a click in read mode** ([`super::image_select::copy`]): kept as a release keeps this tool's click on it,
    /// for [`taken`], so it's the same command by the same job, with this tool's words, Copy anyway and one job at a time.
    pub(super) fn take_image(&self, tab: u32, position: u32, seen: extract::AsSeen, [x, y]: [f32; 2]) {
        let wanted = Wanted::Image { x, y };
        lock(&self.jobs).taken = Some(Job {
            tab,
            position,
            seen,
            wanted,
            anyway: false,
        });
    }

    /// The app's on its way out: a paste that asks now for a promised SVG gets nothing, rather than holding the quit up for a worker.
    pub fn quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        if let Some(running) = &lock(&self.jobs).running {
            running.stop();
        }
        lock(&self.jobs).looking = None;
        lock(&self.paste.state).stop.stop();
    }

    /// The person stayed after a quit's question: a paste's promised SVG is made again (found on the way, 2026-11-15: it was given nothing till the app
    /// was started again).
    pub fn quit_not_taken(&self) {
        self.quitting.store(false, Ordering::SeqCst);
    }

    /// One type of the offer with this number: its bytes, once, dropped from here as they go, or what a promised one is made from. Nothing for an offer replaced since, a type not in it, or a promise on the way out.
    pub fn data(&self, serial: u64, uti: &str) -> Option<Data> {
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
                Some(Data::Make(*current.promise.clone()?, cap))
            }
        }
    }

    /// **What a copy has for the clipboard,** this tool's snapshot or else the selection's text in one of `engines`, as one offer, under a new number: the
    /// types in order, and which are made only when a paste asks. What was offered before is gone from the clipboard now, its promise with it. The
    /// bridge's `take_clipboard`.
    pub fn take_offer(&self, engines: &[Engine]) -> Option<ClipboardOffer> {
        let mut offered = lock(&self.ready).take().or_else(|| {
            engines
                .iter()
                .find_map(|engine| engine.selection().take_copied())
                .map(|copied| self.offer_text(copied.text))
        })?;
        offered.serial = self.serial.fetch_add(1, Ordering::SeqCst) + 1;
        let offer = ClipboardOffer {
            serial: offered.serial,
            types: offered.items.iter().map(|item| item.uti().to_string()).collect(),
            promised: offered
                .items
                .iter()
                .filter(|item| matches!(item, ClipItem::Promised { .. }))
                .map(|item| item.uti().to_string())
                .collect(),
        };
        *lock(&self.offered) = Some(offered);
        Some(offer)
    }

    /// **One type's bytes made with the copy, for the offer with this number,** once, handed over and dropped from here. Nothing for a promised one, which
    /// the legacy paste path can resolve. New snapshot offers contain prepared types only. The bridge's `clipboard_data`.
    pub fn ready_bytes(&self, serial: u64, uti: &str) -> Option<Vec<u8>> {
        crate::watchdog::note_doing_here("clipboard_data");
        match self.data(serial, uti)? {
            Data::Ready(bytes) => Some(bytes),
            Data::Make(..) => None,
        }
    }

    /// The clipboard has something else on it now: what the offer with this number promised is let go. The bridge's `clipboard_released`.
    pub fn offer_released(&self, serial: u64) {
        let mut offered = lock(&self.offered);
        if offered.as_ref().is_some_and(|offered| offered.serial == serial) {
            offered.take();
        }
    }

    /// A text copy, as one offer like any other.
    pub fn offer_text(&self, text: String) -> Offered {
        Offered {
            serial: 0,
            items: vec![ClipItem::Ready {
                uti: extract::UTI_TEXT,
                bytes: text.into_bytes(),
            }],
            promise: None,
        }
    }

    /// **A paste of a promised SVG is asked:** waited for from now, on the app's clock, if no other is being made. False if one is, or the app's quitting.
    fn arm_paste(&self) -> bool {
        let mut paste = lock(&self.paste.state);
        if paste.running || self.quitting.load(Ordering::SeqCst) {
            return false;
        }
        *paste = PasteState {
            running: true,
            waiting: true,
            deadline: self.paste.now_ms().saturating_add(PASTE_PATIENCE_MS),
            made: None,
            stop: ClipStop::default(),
        };
        true
    }

    /// A paste's SVG is being made, for a test.
    #[cfg(test)]
    pub(crate) fn paste_running(&self) -> bool {
        lock(&self.paste.state).running
    }

    /// A test's: an offer on the clipboard whose SVG is promised from `promise`, as a box's answer leaves one, under its number.
    #[cfg(test)]
    pub(crate) fn offer_a_promise_for_a_test(&self, promise: Promise) -> u64 {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.offered) = Some(Offered {
            serial,
            items: vec![ClipItem::Promised {
                uti: extract::UTI_SVG,
                cap: extract::MAX_CLIP_ITEM,
            }],
            promise: Some(Box::new(promise)),
        });
        serial
    }

    /// **The bytes the system's paste is waiting for,** on the UI thread, where the system asks: the person's command's SVG once it's made, or nothing
    /// once it's refused, or at [`PASTE_PATIENCE_MS`] on the app's clock, when the SVG being made is stopped. Takes no lock but this paste's own, never
    /// the session's.
    pub fn wait_for_paste(&self) -> Option<Vec<u8>> {
        let mut paste = lock(&self.paste.state);
        loop {
            if let Some(bytes) = paste.made.take() {
                paste.waiting = false;
                return Some(bytes);
            }
            let now = self.paste.now_ms();
            if !paste.waiting || !paste.running || now >= paste.deadline {
                paste.waiting = false;
                paste.stop.stop();
                return None;
            }
            // The clock may be a test's, which moves without waking anything: looked at again every 20 ms. ratchet: poll
            paste = match self.paste.changed.wait_timeout(paste, Duration::from_millis(20)) {
                Ok((paste, _)) => paste,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }
}

/// **A release's snapshot, taken:** the box or click [`Tool::release`] kept, as the person's command ([`super::controls::UiAction::SnapshotTaken`]).
pub(super) fn taken(inner: &Arc<Inner>) {
    // A one-shot capture's release put the tool back to text selection: the host is told once, to untick it and draw the pointer.
    if inner.tools.tool().once_ended.swap(false, Ordering::SeqCst) {
        inner.listener.model_changed();
    }
    let job = lock(&inner.tools.tool().jobs).taken.take();
    if let Some(job) = job {
        start(inner, job);
    }
}

/// Another tab came to the front, a new one included: text selection is the default there, so the tool goes off (owner, 2026-11-08: "Text selection should be the default tool (not camera tool), enable camera tool via hotkey or sidebar menu"). It's off at every launch too, as it's never kept. Its menu item, its key, the panel's button and the page's Capture turn it on, and that is all that does.
pub fn tab_came_forward(inner: &super::Inner) {
    if inner.tools.tool().is_on() {
        inner.tools.tool().toggle();
    }
}

/// **The tool turned on, by its toggle or by Capture:** an image picked with a click in read mode goes, its box with it, so ⌘C with the tool on copies
/// nothing of read mode's (the review of `build/image-select`, S3).
fn turned_on(parts: super::row_parts::RowParts<'_>) {
    parts.let_go_image();
}

/// **The snapshot tool's menu ids** ([`super::menu_rows`]), all the window's alone: toggling the tool, Capture (on for one capture) and Escape's end of it, and a notice's "Copy anyway", which takes again only
/// what that notice refused.
pub fn row<'a>(parts: &super::row_parts::RowParts<'a>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    use super::menu_rows::Row;
    let parts = *parts;
    if asked.id == TOGGLE_ID {
        return Some(Row::window(
            "the snapshot tool is what the pointer does in the window",
            super::menu_rows::Agents::AT_FULL,
            move || {
                if parts.toggle_snapshot_tool() {
                    turned_on(parts);
                }
                parts.reconcile();
                parts.tell_host();
                super::HostAction::None
            },
        ));
    }
    if asked.id == CAPTURE_ID {
        return Some(Row::window(
            "Capture is the snapshot tool for one capture, what the pointer does in the window",
            super::menu_rows::Agents::AT_FULL,
            move || {
                turned_on(parts);
                if parts.arm_snapshot_once() {
                    parts.reconcile();
                    parts.tell_host();
                }
                super::HostAction::None
            },
        ));
    }
    if asked.id == CANCEL_ID {
        return Some(Row::window(
            "Escape before a Capture is taken, the window's own key",
            super::menu_rows::Agents::AT_FULL,
            move || {
                if parts.cancel_snapshot_once() {
                    parts.reconcile();
                    parts.tell_host();
                }
                super::HostAction::None
            },
        ));
    }
    let serial = asked.id.strip_prefix(ANYWAY_PREFIX)?.to_string();
    Some(Row::window(
        "a notice's Copy anyway, for the person's own snapshot it refused, to the system's pasteboard: their decision, where an agent's is `--anyway`, which is logged",
        super::menu_rows::Agents::ItsOwn("`recto snapshot --anyway`"),
        move || {
            if let Ok(serial) = serial.parse() {
                parts.person().snapshot_anyway(serial);
            }
            parts.reconcile();
            super::HostAction::None
        },
    ))
}

/// **A notice's Copy anyway,** for the person's snapshot it refused, if it's still the one refused: taken again, as theirs ([`super::person::Person`]).
pub(super) fn anyway(inner: &Arc<Inner>, serial: u64) {
    if let Some(job) = inner.tools.tool().take_refused(serial) {
        start(inner, Job { anyway: true, ..job });
    }
}

/// **What the notice says of the person's snapshot,** in Rust's words, and whether it offers "Copy anyway": nothing for one a newer one stopped.
fn words_for(result: &Result<Value, AgentError>) -> Option<(String, bool)> {
    let error = match result {
        Ok(answer) => return Some((answer["says"].as_str().unwrap_or_default().to_string(), false)),
        Err(error) => error,
    };
    Some(match error.code {
        Code::COPY_RESTRICTED => ("This file's permissions ask that it isn't copied.".into(), true),
        Code::NO_SUCH_IMAGE => ("There's no image there. Drag a box to take part of the page.".into(), false),
        Code::CANCELLED => return None,
        Code::SOURCE_CHANGED => ("That page can't be read now: its file has changed or gone.".into(), false),
        Code::NOT_STARTED => ("The snapshot couldn't be started.".into(), false),
        // Its thread ended in a panic ([`super::person::PANICKED`]).
        Code::INTERNAL => ("The snapshot couldn't be taken.".into(), false),
        _ => (error.message.clone(), false),
    })
}

/// **A job sent as the person's command,** now or after the one running, which it stops. Never waits.
fn start(inner: &Arc<Inner>, job: Job) {
    let tool = inner.tools.tool();
    // A box on a heavy page can take seconds, so the reader is told it's on its way, until the answer replaces this.
    tool.say("Copying…", false);
    lock(&tool.refused).take();
    inner.core.push(super::CoreCall::Reconcile);
    let first = lock(&tool.jobs).offer(job);
    if let Some((job, _)) = first
        && !super::person::submit(inner, job.command())
    {
        // Not taken, as the app hands off: nothing runs, and nothing's said to be on its way.
        **lock(&tool.jobs) = JobSlot::default();
        lock(&tool.notice).take();
    }
}

/// **What came of the person's snapshot, said** ([`super::person`]'s list of answers): the notice, what's for the clipboard, and the job waiting next sent;
/// or, for a paste's SVG, the paste told it's done.
pub fn answered(parts: &super::answer_parts::AnswerParts<'_>, ran: &super::person::Ran<'_>) {
    let person = parts.person();
    if kind_of(ran.command).is_some() {
        parts.snapshot_answered(|tool| answer(tool, person, ran));
    }
}

/// What came of it, to the tool's state: whether something's ready for the host.
fn answer(tool: &Tool, person: super::person::Person<'_>, ran: &super::person::Ran<'_>) -> bool {
    if is_paste(ran.command) {
        lock(&tool.paste.state).running = false;
        tool.paste.changed.notify_all();
        return false;
    }
    let Some(job) = Job::of(ran.command) else { return false };
    // Under the slot's lock, so a release now either waits for this or finds the slot free after the answer is in. Nothing here takes the session's lock.
    let mut jobs = lock(&tool.jobs);
    let made = jobs.made.take();
    // A look waiting is started by the app's next look at what's wanted ([`Tool::look_to_start`]).
    if let Next::Copy(next) = jobs.next() {
        drop(jobs);
        send_next(tool, person, next);
        return false;
    }
    publish(tool, job, ran.result, made)
}

/// The snapshot that waited, sent as the person's now; not taken, as the app hands off, nothing runs and nothing's said to be on its way.
fn send_next(tool: &Tool, person: super::person::Person<'_>, next: Job) {
    if !person.submit(next.command()) {
        **lock(&tool.jobs) = JobSlot::default();
        lock(&tool.notice).take();
    }
}

/// **A click's look has answered** ([`super::image_select::answered`]): the look that ran, if it's the one for this click (its tab and point), taken off
/// the slot, with whether it runs again (a snapshot stopped it, and it waits behind that), and a snapshot that waited behind it sent. A look that waited is
/// started by the app's next look at what's wanted.
pub(super) fn look_answered(tool: &Tool, person: super::person::Person<'_>, asked: (u32, recto_index::select::PagePoint)) -> Option<(Look, bool)> {
    let mut jobs = lock(&tool.jobs);
    let answered = jobs.take_look(asked)?;
    if let Next::Copy(next) = jobs.next() {
        drop(jobs);
        send_next(tool, person, next);
    }
    Some(answered)
}

/// The newest job's answer: the notice, and what's for the clipboard. Whether there's something ready for the host.
fn publish(tool: &Tool, job: Job, result: &Result<Value, AgentError>, made: Option<Clip>) -> bool {
    let Some((text, offers)) = words_for(result) else {
        lock(&tool.notice).take();
        return false;
    };
    let serial = tool.say(text, offers);
    *lock(&tool.refused) = offers.then_some((serial, job));
    let (Ok(_), Some(clip)) = (result, made) else { return false };
    // A box's SVG is promised only with the page it saw, which the paste is held to: none that reads, no promise.
    let promise = match (job.wanted, result) {
        (Wanted::Box { rect }, Ok(answer)) if clip.items.iter().any(|item| matches!(item, ClipItem::Promised { .. })) => {
            serde_json::from_value(answer["as_seen"].clone()).ok().map(|as_seen| Promise {
                tab: job.tab,
                position: job.position,
                rect,
                as_seen,
            })
        }
        _ => None,
    };
    let items = clip
        .items
        .into_iter()
        .filter(|item| matches!(item, ClipItem::Ready { .. }) || promise.is_some())
        .collect();
    *lock(&tool.ready) = Some(Offered {
        serial: 0,
        items,
        promise: promise.map(Box::new),
    });
    true
}

/// **A paste asks for a promised type of the offer with this number** ([`super::controls::UiAction::Pasted`]): the person's `snapshot --as svg
/// --to-clipboard` of the box it promised, read again from the tab as it is now, whose bytes [`Tool::wait_for_paste`] waits for. False if there's
/// nothing to make (an offer replaced since, a type not promised, the app quitting) or a paste's SVG is being made already.
pub(super) fn pasted(inner: &Arc<Inner>, serial: u64, uti: &str) -> bool {
    let tool = inner.tools.tool();
    let Some(Data::Make(promise, _)) = tool.data(serial, uti) else { return false };
    if !tool.arm_paste() {
        return false;
    }
    let command = Command::from(Snapshot {
        tab: TabRef::Id(TabId(promise.tab)),
        page: promise.position + 1,
        rect: promise.rect,
        format: "svg".into(),
        out: None,
        scale: extract::DEFAULT_SCALE,
        anyway: false,
        to_clipboard: true,
        as_seen: Some(promise.as_seen),
    });
    if !super::person::submit(inner, command) {
        lock(&tool.paste.state).running = false;
        return false;
    }
    true
}

/// **The window's clipboard, for the person's own snapshot** ([`recto_agent::clipboard::ClipboardSink`]): the app's side of it, given to the host once
/// ([`recto_control::host::SessionHost::set_clipboard`]), which gives it only to the person's own commands.
struct Sink {
    app: Weak<Inner>,
}

/// The sink, for the host to give the person's snapshot; and `tool` set to go by the app's clock, which a paste's patience goes by.
pub fn sink(app: &Arc<Inner>, clock: Arc<dyn crate::clock::Clock>) -> Arc<dyn recto_agent::clipboard::ClipboardSink> {
    let _ = app.tools.tool().paste.clock.set(clock);
    Arc::new(Sink { app: Arc::downgrade(app) })
}

impl recto_agent::clipboard::ClipboardSink for Sink {
    fn begin(&self) -> ClipStop {
        // Its page is found: the person's next edit goes on now.
        crate::queue::lane::pass_here();
        let Some(app) = self.app.upgrade() else { return stopped() };
        #[cfg(test)]
        app.core.holds.at("snapshot_drawing");
        match &lock(&app.tools.tool().jobs).running {
            Some(Running::Copy(stop)) => stop.clone(),
            _ => stopped(),
        }
    }

    /// Only this feature's look ([`super::image_select`]): the running look's stop if it's for this click, its tab, page and point; any other look of the
    /// window's is one nobody waits for any more, and stopped.
    fn begin_look(&self, tab: u32, page: u32, at: [f32; 2]) -> ClipStop {
        let Some(app) = self.app.upgrade() else { return stopped() };
        #[cfg(test)]
        app.core.holds.at("look_drawing");
        match &lock(&app.tools.tool().jobs).running {
            // The command's page counts from 1, the click's from 0: the click's page, not where its tab is.
            Some(Running::Look(look, stop)) if (look.tab, Some(look.point.page), [look.point.x, look.point.y]) == (tab, page.checked_sub(1), at) => {
                stop.clone()
            }
            _ => stopped(),
        }
    }

    fn begin_paste(&self) -> ClipStop {
        let Some(app) = self.app.upgrade() else { return stopped() };
        #[cfg(test)]
        app.core.holds.at("paste_drawing");
        lock(&app.tools.tool().paste.state).stop.clone()
    }

    fn offer(&self, clip: Clip) {
        if let Some(app) = self.app.upgrade() {
            lock(&app.tools.tool().jobs).made = Some(clip);
        }
    }

    fn made(&self, bytes: Vec<u8>) {
        let Some(app) = self.app.upgrade() else { return };
        let tool = app.tools.tool();
        let mut paste = lock(&tool.paste.state);
        if paste.waiting {
            paste.made = Some(bytes);
        }
        drop(paste);
        tool.paste.changed.notify_all();
    }
}

/// A stop already set: for a snapshot nothing is waiting for.
fn stopped() -> ClipStop {
    let stop = ClipStop::default();
    stop.stop();
    stop
}

/// What [`Tool::data`] hands over: bytes made with the copy, or what to make a promised type from.
#[derive(Debug, PartialEq)]
pub enum Data {
    Ready(Vec<u8>),
    Make(Promise, usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE_1: extract::AsSeen = extract::AsSeen {
        quarter_turns: 0,
        file_page: 0,
        own: true,
    };

    fn job(x: f32) -> Job {
        Job {
            tab: 1,
            position: 0,
            seen: PAGE_1,
            wanted: Wanted::Image { x, y: 0.0 },
            anyway: false,
        }
    }

    /// Failure: a refusal for permissions has no "Copy anyway", or a plain answer offers one, or the reader isn't told why nothing was copied by a click off any image.
    #[test]
    fn the_notice_says_what_happened_and_offers_copy_anyway_only_when_refused() {
        let refused = |code: Code| Err(AgentError::new(code, "an agent's words", recto_agent::exit::FAILED));
        assert!(words_for(&refused(Code::COPY_RESTRICTED)).expect("said").1);
        let (text, offers) = words_for(&refused(Code::NO_SUCH_IMAGE)).expect("said");
        assert!(!offers && text.contains("no image"));
        let ready = words_for(&Ok(serde_json::json!({ "says": "Copied." })));
        assert_eq!(ready, Some(("Copied.".to_string(), false)));
        assert_eq!(words_for(&refused(Code::CANCELLED)), None, "one a newer one stopped says nothing");
    }

    /// Failure: the box stays on a page it's left, or isn't drawn where the tool says, so the reader sees two boxes, or none, while dragging.
    #[test]
    fn the_box_moves_with_the_drag_and_leaves_no_trace() {
        let tool = Tool::default();
        let rect = recto_index::select::Rect {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
        };
        assert_eq!(
            tool.box_ops(0, Some((5, rect))),
            vec![ReaderOp::SnapshotBox {
                page: 5,
                rect: Some(HighlightRect {
                    x: 1.0,
                    y: 2.0,
                    width: 3.0,
                    height: 4.0
                })
            }]
        );
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
        let Next::Copy(next) = slot.next() else { panic!("the newest waits") };
        assert_eq!(next, job(10.0), "only the newest");
        assert!(!running_stop(&slot).is_stopped());
        assert!(matches!(slot.next(), Next::Nothing), "and nothing after it");
        assert!(slot.offer(job(11.0)).is_some(), "the slot's free again");
    }

    /// The stop of what runs in the slot.
    fn running_stop(slot: &JobSlot) -> ClipStop {
        match slot.running.as_ref().expect("something runs") {
            Running::Copy(stop) | Running::Look(_, stop) => stop.clone(),
        }
    }

    /// A click's look, the `serial`th, on tab 1 in pane 0.
    fn look(serial: u32) -> Look {
        Look::for_test(0, 1, serial)
    }

    /// Failure (the review of `build/image-select`, S2): a click's look runs beside a copy, or beside another look, so the app has two private workers of
    /// 351 MiB at once on the USGS map; a look stops a copy the person asked for; a copy waits behind a look the person no longer needs; a look nobody wants
    /// any more runs on to its 60 s limit; or the click a newer one took the place of is never told, and waits for good. One slot: a newer look stops a
    /// running look and waits; a copy stops a look and goes first; a look never stops a copy and waits behind it; a look no longer wanted is stopped.
    #[test]
    fn looks_and_copies_share_one_slot() {
        let mut slot = JobSlot::default();
        let (started, replaced) = slot.looks(&[look(1)]);
        let (first, first_stop) = started.expect("nothing running: the look starts");
        assert_eq!((first, replaced), (look(1), vec![]));
        assert!(
            slot.looks(&[look(1)]).0.is_none() && !first_stop.is_stopped(),
            "the same click again starts nothing"
        );
        assert!(slot.looks(&[look(2)]).0.is_none(), "a newer click waits");
        assert!(first_stop.is_stopped(), "and stops the look running");
        assert_eq!(slot.take_look((1, look(1).point)), Some((look(1), false)), "the running look answers");
        assert!(
            matches!(slot.next(), Next::Looks),
            "the look waiting is started by the next look at what's wanted"
        );
        let (started, _) = slot.looks(&[look(2)]);
        let (_, second_stop) = started.expect("the newer look starts");

        assert!(slot.offer(job(1.0)).is_none(), "a copy waits for the look");
        assert!(second_stop.is_stopped(), "and stops it");
        assert!(slot.looks(&[look(3)]).0.is_none(), "a look waits too");
        let Next::Copy(copy) = slot.next() else { panic!("the copy first") };
        assert_eq!(copy, job(1.0));
        let copy_stop = running_stop(&slot);
        assert!(slot.looks(&[look(3)]).0.is_none(), "a look waits behind the copy");
        assert!(!copy_stop.is_stopped(), "and never stops it");
        assert!(matches!(slot.next(), Next::Looks), "then the look");

        let (started, _) = slot.looks(&[look(3)]);
        let (_, third_stop) = started.expect("its turn");
        slot.looks(&[]);
        assert!(third_stop.is_stopped(), "a look no longer wanted is stopped");
        let _ = slot.take_look((1, look(3).point));
        let _ = slot.next();
        slot.offer(job(2.0));
        slot.looks(&[look(4)]);
        slot.looks(&[]);
        assert!(slot.looking.is_none(), "and one waiting, dropped");
        let _ = slot.next();
        let (_, replaced) = slot.looks(&[Look::for_test(0, 2, 5), look(6)]);
        assert_eq!(replaced, vec![Look::for_test(0, 2, 5)], "an older click another took the place of is told");
    }

    /// Failure (the second review of `build/image-select`, fixes 2 and 6): a look a copy stops is lost, its stopped answer taken as "no image there", so
    /// the click picks nothing though the copy said it would wait; or two panes' clicks with the same serial (each pane has its own clock) are taken for one,
    /// so one pane's look is never sent, or its answer lands on the other. A look a copy stops waits behind the copy and runs again; the same serial in two
    /// panes is two clicks.
    #[test]
    fn a_look_a_copy_stops_runs_again_and_two_panes_never_collide() {
        let mut slot = JobSlot::default();
        let (started, _) = slot.looks(&[look(1)]);
        let (_, stop) = started.expect("the look starts");
        assert!(slot.offer(job(1.0)).is_none() && stop.is_stopped(), "a copy stops it");
        assert_eq!(slot.take_look((1, look(1).point)), Some((look(1), true)), "its stopped answer is to run again");
        assert!(matches!(slot.next(), Next::Copy(_)), "the copy first");
        assert!(slot.looks(&[look(1)]).0.is_none(), "the look waits behind the copy");
        assert!(matches!(slot.next(), Next::Looks), "then its turn");
        assert_eq!(slot.looks(&[look(1)]).0.map(|(look, _)| look), Some(look(1)), "and it runs again");

        let mut slot = JobSlot::default();
        let (left, right) = (Look::for_test(0, 1, 7), Look::for_test(1, 1, 7));
        assert!(slot.looks(&[left]).0.is_some(), "the left pane's look runs");
        let (started, _) = slot.looks(&[left, right]);
        assert!(started.is_none(), "one at a time");
        assert_eq!(slot.looking, Some(right), "the right pane's click, the same serial, waits its turn: two clicks");
    }

    /// Failure: a "Copy anyway" from a notice that's gone, or from an older one, copies whatever was refused last, perhaps another box or file; or turning the tool or starting a new box leaves the old offer standing (review, 2026-11-08). Each button names its notice, and only that notice's refusal is taken, once.
    #[test]
    fn copy_anyway_takes_only_what_its_own_notice_refused() {
        let tool = Tool::default();
        let serial = tool.say("This file's permissions ask that it isn't copied.", true);
        *lock(&tool.refused) = Some((serial, job(1.0)));
        assert_eq!(
            tool.selection_notice().and_then(|notice| notice.action_id),
            Some(format!("{ANYWAY_PREFIX}{serial}"))
        );
        assert_eq!(tool.take_refused(serial + 1), None, "another notice's button");
        assert_eq!(tool.take_refused(serial), Some(job(1.0)));
        assert_eq!(tool.take_refused(serial), None, "once");
        let serial = tool.say("This file's permissions ask that it isn't copied.", true);
        *lock(&tool.refused) = Some((serial, job(2.0)));
        tool.toggle();
        assert_eq!(
            (tool.take_refused(serial), tool.notice()),
            (None, None),
            "turning the tool takes the offer away"
        );
        let serial = tool.say("This file's permissions ask that it isn't copied.", true);
        *lock(&tool.refused) = Some((serial, job(3.0)));
        tool.dismiss_notice();
        assert_eq!(tool.take_refused(serial), None);
    }

    /// Failure: the snapshot tool starts on, so a drag at launch draws a box instead of selecting text. Owner, 2026-11-08: "Text selection should be the default tool (not camera tool)". It starts off, and a toggle turns it on and off again.
    #[test]
    fn the_tool_starts_off() {
        let tool = Tool::default();
        assert!(!tool.is_on());
        assert!(tool.toggle() && tool.is_on());
        assert!(!tool.toggle() && !tool.is_on());
    }

    /// **Failure (the branch's review): a box's answer whose page as seen can't be read still promises its SVG,** with nothing to hold the paste to the
    /// box's page and turn, so the guard against another page's SVG was dropped without a word. It fails closed: the SVG isn't offered; the rest is.
    #[test]
    fn a_box_whose_page_as_seen_cant_be_read_promises_no_svg() {
        let tool = Tool::default();
        let job = Job {
            wanted: Wanted::Box { rect: [0.0, 0.0, 9.0, 9.0] },
            ..job(0.0)
        };
        let clip = || Clip {
            items: vec![
                ClipItem::Ready {
                    uti: extract::UTI_PNG,
                    bytes: vec![1],
                },
                ClipItem::Promised { uti: extract::UTI_SVG, cap: 9 },
            ],
            note: String::new(),
        };
        let types = |answer: Value| {
            assert!(publish(&tool, job, &Ok(answer), Some(clip())), "something's ready");
            let offered = lock(&tool.ready).take().expect("offered");
            offered.items.iter().map(ClipItem::uti).collect::<Vec<_>>()
        };
        let seen = serde_json::to_value(PAGE_1).expect("as JSON");
        assert_eq!(
            types(serde_json::json!({ "says": "Copied.", "as_seen": seen })),
            [extract::UTI_PNG, extract::UTI_SVG]
        );
        assert_eq!(types(serde_json::json!({ "says": "Copied." })), [extract::UTI_PNG], "no page as seen, no SVG");
        assert_eq!(
            types(serde_json::json!({ "says": "Copied.", "as_seen": { "file_page": "one" } })),
            [extract::UTI_PNG],
            "one that doesn't read, no SVG"
        );
    }

    /// Failure: the clipboard hands a type's bytes to an offer that's been replaced, or twice, so they're held after the host has them; or a promised SVG is made on the way out, holding up a quit for a worker; or a text copy isn't one offer like a snapshot's.
    #[test]
    fn an_offers_bytes_go_once_to_that_offer() {
        let tool = Tool::default();
        let promise = Promise {
            tab: 1,
            position: 0,
            rect: [0.0, 0.0, 1.0, 1.0],
            as_seen: PAGE_1,
        };
        *lock(&tool.offered) = Some(Offered {
            serial: 7,
            items: vec![
                ClipItem::Ready {
                    uti: extract::UTI_PNG,
                    bytes: vec![1],
                },
                ClipItem::Promised { uti: extract::UTI_SVG, cap: 9 },
            ],
            promise: Some(Box::new(promise)),
        });
        assert_eq!(tool.data(6, extract::UTI_PNG), None, "an older offer");
        assert_eq!(tool.data(7, extract::UTI_PNG), Some(Data::Ready(vec![1])));
        assert_eq!(tool.data(7, extract::UTI_PNG), None, "once");
        assert_eq!(
            tool.data(7, extract::UTI_SVG),
            Some(Data::Make(promise, 9)),
            "a promise, as often as a paste asks"
        );
        assert_eq!(tool.data(7, extract::UTI_SVG), Some(Data::Make(promise, 9)));
        tool.quitting();
        assert_eq!(tool.data(7, extract::UTI_SVG), None, "nothing made on the way out");
        assert_eq!(
            tool.offer_text("words".into()).items,
            vec![ClipItem::Ready {
                uti: extract::UTI_TEXT,
                bytes: b"words".to_vec()
            }]
        );
    }

    /// Pixels of a snapshot's PNG inside `[left, top, right, bottom]` (in its pixels) that a highlight's yellow is on: the blue taken down, the red
    /// and green left.
    fn yellow_in(png: &[u8], [left, top, right, bottom]: [usize; 4]) -> usize {
        let recto_svg::verify::Embedded::Png { width, channels, pixels, .. } = recto_svg::verify::read_png(png) else {
            panic!("a PNG")
        };
        pixels
            .chunks(channels)
            .enumerate()
            .filter(|(at, pixel)| {
                let (x, y) = (at % width as usize, at / width as usize);
                (left..right).contains(&x) && (top..bottom).contains(&y) && pixel[2] < 160 && pixel[0] > 200 && pixel[1] > 150
            })
            .count()
    }

    /// **Failure (second review of R4.3, S3): the person's Snapshot tool copies a highlight they've taken out**, as it draws the page with a renderer of
    /// its own that's never given the tab's set. With the file's highlight, then taken out, then undone: yellow inside its rectangle, none, and
    /// yellow again. Through the person's command, as a release sends it; the SVG a paste makes is the same command's, given the same set by it.
    #[test]
    fn the_snapshot_tool_leaves_out_a_highlight_taken_out() {
        use crate::testkit::headless::Disk;
        use recto_agent::Level;
        let disk = Disk::new();
        let doc = disk.pdf("file.pdf", 1);
        let app = disk.start();
        app.open(&doc);
        app.settle();
        app.agent(Level::Control, r##"{"cmd":"highlight","tab":"active","page":1,"chars":"0-5"}"##)
            .expect("highlight");
        app.agent(Level::Control, r##"{"cmd":"save","tab":"active"}"##).expect("save");
        app.settle();
        let listed = app
            .agent(Level::ReadOnly, r##"{"cmd":"highlights","tab":"active","page":1}"##)
            .expect("highlights");
        let [x, y, width, height] = [0, 1, 2, 3].map(|at| listed["file"]["highlights"][0]["rects"][0][at].as_f64().unwrap() as f32);
        let tab = app.core().host.session.with_session(|session| session.tabs().active()).expect("a tab").0;
        // A box 10 points around the highlight; the highlight is the middle of it, inset a point, at the snapshot's two pixels a point.
        let rect = [x - 10.0, y - 10.0, width + 20.0, height + 20.0];
        let scale = extract::DEFAULT_SCALE;
        let inside = [11.0 * scale, 11.0 * scale, (width + 9.0) * scale, (height + 9.0) * scale].map(|edge| edge as usize);
        let job = Job {
            tab,
            position: 0,
            seen: PAGE_1,
            wanted: Wanted::Box { rect },
            anyway: false,
        };
        let snapshot = || {
            start(app.core(), job);
            app.wait_until("the snapshot's answered", || !app.core().tools.tool().busy());
            app.settle();
            let offered = lock(&app.core().tools.tool().ready).take().expect("a snapshot");
            let png = offered
                .items
                .iter()
                .find_map(|item| match item {
                    ClipItem::Ready { uti, bytes } if *uti == extract::UTI_PNG => Some(bytes.clone()),
                    _ => None,
                })
                .expect("a PNG");
            yellow_in(&png, inside)
        };
        let with = snapshot();
        assert!(with > 100, "the file's highlight is in the picture: {with}");
        let at = format!("{},{}", x + width / 2.0, y + height / 2.0);
        app.recto(Level::Control, &["highlight", "remove", "--page", "1", "--at", &at])
            .expect("taken out");
        app.settle();
        assert_eq!(snapshot(), 0, "taken out, it isn't in the picture");
        app.agent(Level::Control, r#"{"cmd":"undo"}"#).expect("undo");
        app.settle();
        assert_eq!(snapshot(), with, "undone, it's back");
    }
}

/// What the clipboard is offered: its types in the order to offer them, which of those are made only when a paste asks, and the number that names this offer for the bridge's `clipboard_data`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardOffer {
    pub serial: u64,
    /// Uniform type identifiers, such as `public.png`, in the order a paste should prefer them.
    pub types: Vec<String>,
    /// The ones made only when a paste asks for them: the host declares them and gives them when asked.
    pub promised: Vec<String>,
}
