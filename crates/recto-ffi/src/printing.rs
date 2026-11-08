//! **File > Print** in the window. The system's print panel chooses the printer, the copies and which pages; Rust then draws those pages through `recto_agent::print`, the same as `recto print --to-pdf`, into a file of the app's own on a thread of its own, saying how far it's got by the page, with Stop beside it; and the host hands that file to the system's printing, which never asks the app to draw anything on the UI thread. The file goes when the print's done with it.
//!
//! At rest there's nothing: no thread, worker, file or buffer. One print at a time; asking again while one's being prepared says so.
//!
//! - **A PDF's flag against printing is ignored** (owner, 2026-11-08: "Ignore the flag"): no notice and no refusal, and nothing here reads it.
//! - **A print to a file is an export.** When the panel's answer saves or previews a PDF rather than printing on paper, a file that asks not to be copied gets the notice a copy has, with "Print anyway" (owner, 2026-11-08: exports ask "anyway" like Copy). Print to paper never asks.
//! - **Nothing's left behind.** Stop, closing the tab and quitting each stop the preparing at the next page, and its file goes; a launch removes the files of a run that died (`recto-print-PID-*` of a process that's gone).

use crate::{AppCore, CoreCall, Inner, SelectionNotice, lock};
use recto_agent::print;
use recto_session::TabId;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const STACK: usize = 1024 * 1024;

/// The notice's Stop, while pages are being prepared.
const STOP_ID: &str = "print.stop";
/// A notice's "Print anyway", followed by the notice's number.
const ANYWAY_PREFIX: &str = "print.anyway.";
/// Where a print's file starts its name, followed by the process's number.
const FILE_PREFIX: &str = "recto-print-";

/// What the print panel needs to know before it opens, all Rust's to say.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PrintModel {
    /// The tab the print is of, which the answer goes back with. 0 with nothing showing, and then there's nothing to print.
    pub tab: u32,
    pub pages: u32,
    /// Something's selected, so the panel offers to print just its pages.
    pub has_selection: bool,
    /// The job's name in the print queue: the file's name.
    pub job_title: String,
    /// The first page's size as it's shown, in points, for a print to a PDF at the page's own size.
    pub width_pt: f64,
    pub height_pt: f64,
}

/// What a print was asked for, kept so "Print anyway" can ask again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Request {
    tab: TabId,
    range: (u32, u32),
    dpi: u32,
    to_file: bool,
    anyway: bool,
}

/// The line by the page, with its number and button.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Said {
    serial: u64,
    text: String,
    anyway: bool,
}

#[derive(Default)]
pub(crate) struct Printing {
    running: AtomicBool,
    cancel: AtomicBool,
    done: AtomicU32,
    of: AtomicU32,
    generation: AtomicU64,
    serial: AtomicU64,
    ready: Mutex<Option<PathBuf>>,
    /// The host has the file and the system's printing is running.
    handing: AtomicBool,
    notice: Mutex<Option<Said>>,
    /// What the notice with this number refused, for its "Print anyway".
    refused: Mutex<Option<(u64, Request)>>,
    /// A QA print to a file: where the system's printing writes, a new name of ours, and where it's published, never over anything.
    qa_target: Mutex<Option<(PathBuf, PathBuf)>>,
}

impl Printing {
    pub(crate) fn busy(&self) -> bool {
        self.running.load(Ordering::SeqCst) || lock(&self.ready).is_some() || self.handing.load(Ordering::SeqCst)
    }

    /// The line by the page while pages are being prepared, or what went wrong.
    pub(crate) fn notice(&self) -> Option<String> {
        self.selection_notice().map(|notice| notice.text)
    }

    /// The notice as the window shows it: Stop while preparing, "Print anyway" naming the notice that offered it.
    pub(crate) fn selection_notice(&self) -> Option<SelectionNotice> {
        if self.running.load(Ordering::SeqCst) {
            let (done, of) = (self.done.load(Ordering::SeqCst), self.of.load(Ordering::SeqCst));
            let text = if of > 1 { format!("Preparing pages for printing: {} of {of}.", (done + 1).min(of)) } else { "Preparing the page for printing.".to_string() };
            return Some(SelectionNotice { text, action_title: Some("Stop".into()), action_id: Some(STOP_ID.into()), fades: false });
        }
        lock(&self.notice).clone().map(|said| SelectionNotice { action_title: said.anyway.then(|| "Print anyway".to_string()), action_id: said.anyway.then(|| format!("{ANYWAY_PREFIX}{}", said.serial)), fades: !said.anyway, text: said.text })
    }

    pub(crate) fn dismiss_notice(&self) {
        lock(&self.notice).take();
        lock(&self.refused).take();
    }

    fn say(&self, text: impl Into<String>, anyway: bool) -> u64 {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.notice) = Some(Said { serial, text: text.into(), anyway });
        serial
    }

    /// What a notice's "Print anyway" names, once, if that notice is still the one showing.
    fn take_refused(&self, serial: u64) -> Option<Request> {
        let mut refused = lock(&self.refused);
        let request = refused.filter(|(offered, _)| *offered == serial).map(|(_, request)| request)?;
        refused.take();
        lock(&self.notice).take();
        Some(request)
    }

    /// The file to print, once, for the host to hand to the system.
    pub(crate) fn take_ready(&self) -> Option<PathBuf> {
        let ready = lock(&self.ready).take();
        if ready.is_some() {
            self.handing.store(true, Ordering::SeqCst);
        }
        ready
    }

    /// The system's printing is done with a file, however it went: it goes, if it's one of ours, and a QA print to a file is published where it was asked for.
    pub(crate) fn finished(&self, path: &str) {
        remove_if_ours(Path::new(path), &std::env::temp_dir(), std::process::id());
        if let Some((written, out)) = lock(&self.qa_target).take() {
            if written.exists() && recto_agent::publish_picture(&written, &out).is_err() {
                self.say("The print couldn't be put there: a file is already there, and it won't be replaced.", false);
            }
            let _ = std::fs::remove_file(&written);
        }
        self.handing.store(false, Ordering::SeqCst);
    }

    pub(crate) fn has_ready(&self) -> bool {
        lock(&self.ready).is_some()
    }

    /// Stops a print being prepared, at the next page. Cheap, for the UI thread.
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// The app's quitting: preparing stops at the next page, and a file the host hasn't taken goes now. One the system's printing has is the system's until it's done, and a launch removes it if this run never hears.
    pub(crate) fn quitting(&self) {
        self.cancel();
        if let Some(path) = lock(&self.ready).take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Where a print's file goes: the system's temp folder, a name of this process's own, made new.
fn temp_path(generation: u64) -> PathBuf {
    std::env::temp_dir().join(format!("{FILE_PREFIX}{}-{generation}.pdf", std::process::id()))
}

/// The process a print file's name says made it, if it's one of ours.
fn maker_of(name: &str) -> Option<u32> {
    let rest = name.strip_prefix(FILE_PREFIX)?.strip_suffix(".pdf")?;
    let (pid, generation) = rest.split_once('-')?;
    generation.parse::<u64>().ok()?;
    pid.parse().ok()
}

/// A print file goes if it's this process's, in this folder.
fn remove_if_ours(path: &Path, folder: &Path, pid: u32) {
    if path.parent() == Some(folder) && path.file_name().and_then(|name| name.to_str()).and_then(maker_of) == Some(pid) {
        let _ = std::fs::remove_file(path);
    }
}

/// **The files of runs that died.** Each print file in `folder` made by a process that `alive` says is gone is removed, and nothing else: not this run's, not a live one's, not a file of any other name. Says how many went. Bounded by the folder's own listing, read once.
fn sweep_stale(folder: &Path, alive: impl Fn(u32) -> bool) -> usize {
    let Ok(entries) = std::fs::read_dir(folder) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(maker_of) else { continue };
        if pid != std::process::id() && !alive(pid) && entry.file_type().is_ok_and(|kind| kind.is_file()) && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// At launch, off the UI thread on a short-lived thread of its own: the print files of runs that died go. Only on a Mac, where a process's number can be asked about.
pub(crate) fn sweep_at_launch() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let _ = std::thread::Builder::new().name("recto-print-sweep".into()).stack_size(128 * 1024).spawn(|| {
        sweep_stale(&std::env::temp_dir(), |pid| recto_platform::local::process_path(pid).is_some());
    });
}

/// Which pages the panel's answer means. `first` and `last` count from 1, and `all` or a selection's pages override them. A range given backwards is taken forwards. None if there's nothing to print.
pub(crate) fn chosen_range(pages: u32, all: bool, first: u32, last: u32, selection: Option<(u32, u32)>) -> Option<(u32, u32)> {
    if pages == 0 {
        return None;
    }
    let (from, to) = match selection {
        Some((start, end)) => (start.min(end).saturating_add(1), start.max(end).saturating_add(1)),
        None if all => (1, pages),
        None => (first.max(1), last.max(first.max(1))),
    };
    let to = to.min(pages);
    (from <= to).then_some((from, to))
}

/// What a menu item or notice button does, if it's printing's: Stop, or a notice's "Print anyway", which asks again only what that notice refused. Says whether it was printing's.
pub(crate) fn perform(inner: &Arc<Inner>, id: &str) -> bool {
    if id == STOP_ID {
        inner.printing.cancel();
        return true;
    }
    let Some(serial) = id.strip_prefix(ANYWAY_PREFIX) else { return false };
    if let Some(request) = serial.parse().ok().and_then(|serial| inner.printing.take_refused(serial)) {
        start(inner, Request { anyway: true, ..request });
    }
    inner.core.push(CoreCall::Reconcile);
    true
}

/// Starts preparing a print of `range`, pages counted from 1 as the tab has them now, off the UI thread.
fn start(inner: &Arc<Inner>, request: Request) -> bool {
    let printing = &inner.printing;
    if printing.running.swap(true, Ordering::SeqCst) {
        printing.say("A print is already being prepared.", false);
        return false;
    }
    let (first, last) = request.range;
    printing.cancel.store(false, Ordering::SeqCst);
    printing.done.store(0, Ordering::SeqCst);
    printing.of.store(last.saturating_sub(first) + 1, Ordering::SeqCst);
    printing.dismiss_notice();
    let generation = printing.generation.fetch_add(1, Ordering::SeqCst) + 1;
    inner.host.running.fetch_add(1, Ordering::SeqCst);
    inner.core.push(CoreCall::Reconcile);
    let runner = Arc::clone(inner);
    let spawned = std::thread::Builder::new().name("recto-print".into()).stack_size(STACK).spawn(move || {
        let printing = &runner.printing;
        let pages = format!("{first}-{last}");
        let tab = request.tab;
        let listed = runner.host.session.with_session(|session| print::print_pages_in(session, &format!("#{}", tab.0), Some(&pages)));
        let path = temp_path(generation);
        let outcome = listed.and_then(|listed| {
            if request.to_file && !request.anyway && print::any_copy_restricted(&listed) {
                return Err(recto_agent::AgentError::new("copy_restricted", "This file's permissions ask that it isn't copied.", recto_agent::exit::PROTECTED));
            }
            print::print_to_path(&listed, request.dpi, &path, |done, of| {
                printing.done.store(done, Ordering::SeqCst);
                printing.of.store(of, Ordering::SeqCst);
                runner.core.push(CoreCall::Reconcile);
                // Closing the tab stops it, as Stop does.
                let open = runner.host.session.with_session(|session| session.tabs().iter().any(|record| record.id == tab));
                open && !printing.cancel.load(Ordering::SeqCst)
            })
        });
        match outcome {
            Ok(printed) => {
                if let Some(note) = printed.scaled_down_note() {
                    printing.say(note, false);
                }
                *lock(&printing.ready) = Some(path);
            }
            Err(error) if error.code == "copy_restricted" => {
                // A QA print's target stays, for "Print anyway" to write to.
                let serial = printing.say(error.message, true);
                *lock(&printing.refused) = Some((serial, request));
            }
            Err(error) => {
                if error.code != "cancelled" {
                    printing.say(error.message, false);
                }
                lock(&printing.qa_target).take();
            }
        }
        printing.running.store(false, Ordering::SeqCst);
        runner.host.running.fetch_sub(1, Ordering::SeqCst);
        runner.core.push(CoreCall::Reconcile);
        runner.listener.model_changed();
    });
    if spawned.is_err() {
        printing.running.store(false, Ordering::SeqCst);
        inner.host.running.fetch_sub(1, Ordering::SeqCst);
        printing.say("The print couldn't be started.", false);
        return false;
    }
    true
}

#[uniffi::export]
impl AppCore {
    /// What the print panel needs to know before it opens: the tab showing, its pages, whether something's selected, and its first page's size. Never waits: what's published, and short locks on the page view.
    pub fn print_model(&self) -> PrintModel {
        let reader = lock(&self.inner.published).reader.clone();
        let engine = self.inner.panes.reading_engine(&self.inner.engine);
        let (width_pt, height_pt) = engine.page_size_shown(0).map_or((612.0, 792.0), |(width, height)| (f64::from(width), f64::from(height)));
        PrintModel { tab: reader.shown_tab, pages: engine.page_count(), has_selection: engine.has_selection(), job_title: reader.document_name, width_pt, height_pt }
    }

    /// The print panel was answered with Print: these pages, from 1, or all of them, or the selection's, to paper or, with `to_file`, to a PDF the system's printing saves or previews, which is an export. Rust draws them off the UI thread, and `print_ready` says when the file's there. False if nothing was started, with a notice saying why.
    pub fn print_chosen(&self, tab: u32, all: bool, first: u32, last: u32, selection_only: bool, to_file: bool) -> bool {
        if tab == 0 || self.inner.core.frozen.load(Ordering::SeqCst) {
            return false;
        }
        let model = self.print_model();
        let selection = selection_only.then(|| self.inner.panes.reading_engine(&self.inner.engine).hot_selection()).flatten().and_then(|(_, range)| range).map(|range| (range.start.page, range.end.page));
        let Some(range) = chosen_range(model.pages, all, first, last, selection) else { return false };
        start(&self.inner, Request { tab: TabId(tab), range, dpi: print::DEFAULT_DPI, to_file, anyway: false })
    }

    /// For QA's print to a PDF through the system's printing: where it should write, a new name of ours beside `out`, which is published at `out` when it's done, never over anything. None if `out` can't be written to.
    pub fn print_qa_target(&self, out: String) -> Option<String> {
        let out = PathBuf::from(out);
        let folder = out.parent()?;
        if !out.is_absolute() || std::fs::symlink_metadata(&out).is_ok() {
            return None;
        }
        let written = folder.join(format!(".recto-print-{}-{}.pdf", std::process::id(), self.inner.printing.generation.load(Ordering::SeqCst) + 1));
        if std::fs::symlink_metadata(&written).is_ok() {
            return None;
        }
        *lock(&self.inner.printing.qa_target) = Some((written.clone(), out));
        Some(written.to_string_lossy().into_owned())
    }

    /// The file a print's pages are in, once, for the host to hand to the system's printing.
    pub fn take_print(&self) -> Option<String> {
        let path = self.inner.printing.take_ready().map(|path| path.to_string_lossy().into_owned());
        self.inner.core.push(CoreCall::Reconcile);
        path
    }

    /// The system's printing is done with a file, however it went: it goes.
    pub fn print_finished(&self, path: String) {
        self.inner.printing.finished(&path);
        self.inner.core.push(CoreCall::Reconcile);
    }

    /// Stops a print being prepared, at the next page: the notice's Stop.
    pub fn print_cancel(&self) {
        self.inner.printing.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the panel's answer prints the wrong pages: a range past the end, a range backwards, a selection's pages counted from 0 as the page view holds them, or a selection made upwards giving a range whose end is before its start, which underflowed the page count.
    #[test]
    fn the_panels_answer_means_the_pages_it_says() {
        assert_eq!(chosen_range(10, true, 3, 4, None), Some((1, 10)));
        assert_eq!(chosen_range(10, false, 3, 4, None), Some((3, 4)));
        assert_eq!(chosen_range(10, false, 8, 40, None), Some((8, 10)), "held to the end");
        assert_eq!(chosen_range(10, false, 5, 2, None), Some((5, 5)), "backwards is the first");
        assert_eq!(chosen_range(10, false, 11, 12, None), None, "past the end is nothing");
        assert_eq!(chosen_range(10, true, 1, 1, Some((2, 4))), Some((3, 5)), "a selection's pages, from 1");
        assert_eq!(chosen_range(10, true, 1, 1, Some((4, 2))), Some((3, 5)), "a selection made upwards");
        assert_eq!(chosen_range(10, true, 1, 1, Some((12, 14))), None);
        assert_eq!(chosen_range(0, true, 1, 1, None), None);
    }

    /// Failure: finishing a print removes a file that isn't one of this app's prints, or another run's.
    #[test]
    fn only_a_print_file_of_ours_is_removed() {
        let folder = std::env::temp_dir();
        let elsewhere = folder.join(format!("not-a-print-{}.pdf", std::process::id()));
        std::fs::write(&elsewhere, b"keep").unwrap();
        remove_if_ours(&elsewhere, &folder, std::process::id());
        assert!(elsewhere.exists());
        std::fs::remove_file(&elsewhere).unwrap();
        let ours = temp_path(9_999_999);
        std::fs::write(&ours, b"print").unwrap();
        remove_if_ours(&ours, &folder, std::process::id() + 1);
        assert!(ours.exists(), "another run's");
        remove_if_ours(&ours, &folder, std::process::id());
        assert!(!ours.exists());
    }

    /// Failure: a run that died mid-print leaves its print file in the temp folder for ever, or the sweep at launch removes a live run's, this run's, or a file that only looks like one.
    #[test]
    fn a_launch_sweeps_only_the_print_files_of_runs_that_died() {
        let folder = std::env::temp_dir().join(format!("recto-print-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let (dead, live, mine) = (4_000_001u32, 4_000_002u32, std::process::id());
        let file = |name: String| {
            let path = folder.join(name);
            std::fs::write(&path, b"x").unwrap();
            path
        };
        let gone = file(format!("{FILE_PREFIX}{dead}-1.pdf"));
        let kept_live = file(format!("{FILE_PREFIX}{live}-1.pdf"));
        let kept_mine = file(format!("{FILE_PREFIX}{mine}-3.pdf"));
        let kept_other = file(format!("{FILE_PREFIX}{dead}-notanumber.pdf"));
        let kept_name = file(format!("{FILE_PREFIX}{dead}-1.pdf.keep"));
        assert_eq!(sweep_stale(&folder, |pid| pid == live), 1);
        assert!(!gone.exists());
        for kept in [&kept_live, &kept_mine, &kept_other, &kept_name] {
            assert!(kept.exists(), "{}", kept.display());
        }
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// Failure: the progress line has no Stop, so a long print can't be stopped and its file stays; or a "Print anyway" from a notice that's gone prints what was refused before; or a refused print to a file offers nothing.
    #[test]
    fn the_notice_has_stop_while_preparing_and_anyway_names_its_notice() {
        let printing = Printing::default();
        printing.running.store(true, Ordering::SeqCst);
        printing.of.store(10, Ordering::SeqCst);
        printing.done.store(2, Ordering::SeqCst);
        let preparing = printing.selection_notice().unwrap();
        assert_eq!((preparing.action_id.as_deref(), preparing.fades), (Some(STOP_ID), false));
        assert!(preparing.text.contains("3 of 10"), "{}", preparing.text);
        printing.running.store(false, Ordering::SeqCst);
        let request = Request { tab: TabId(1), range: (1, 2), dpi: 300, to_file: true, anyway: false };
        let serial = printing.say("This file's permissions ask that it isn't copied.", true);
        *lock(&printing.refused) = Some((serial, request));
        let refused = printing.selection_notice().unwrap();
        assert_eq!(refused.action_id, Some(format!("{ANYWAY_PREFIX}{serial}")));
        assert_eq!(refused.action_title.as_deref(), Some("Print anyway"));
        assert_eq!(printing.take_refused(serial + 1), None);
        assert_eq!(printing.take_refused(serial), Some(request));
        assert_eq!(printing.take_refused(serial), None, "once");
    }

    /// Failure: an update quits the app while a print's file is ready but not yet handed to the system, or while the system's printing has it: `busy` must hold through both, not only while pages are drawn.
    #[test]
    fn a_print_is_busy_until_the_system_is_done_with_it() {
        let printing = Printing::default();
        assert!(!printing.busy());
        let path = temp_path(9_999_998);
        std::fs::write(&path, b"print").unwrap();
        *lock(&printing.ready) = Some(path.clone());
        assert!(printing.busy(), "ready for the host");
        assert_eq!(printing.take_ready(), Some(path.clone()));
        assert!(printing.busy(), "the system's printing has it");
        printing.finished(path.to_str().unwrap());
        assert!(!printing.busy());
        assert!(!path.exists(), "its file goes");
    }

    /// Failure: quitting leaves a prepared file the host never took in the temp folder, or doesn't stop the preparing.
    #[test]
    fn quitting_stops_the_print_and_takes_its_file() {
        let printing = Printing::default();
        let path = temp_path(9_999_997);
        std::fs::write(&path, b"print").unwrap();
        *lock(&printing.ready) = Some(path.clone());
        printing.quitting();
        assert!(printing.cancel.load(Ordering::SeqCst));
        assert!(!path.exists() && !printing.busy());
    }
}
