//! **File > Print** in the window. The system's print panel chooses the printer, the copies and which pages; its answer is the person's `print --to-pdf`
//! (S2, 2026-11-14: one action, one implementation), run by the one executor as theirs, into a new file of the app's own: the same command an agent sends,
//! with its checks (a quiet run's files, the tab held while it prints, a file that asks not to be copied) and its words. The window watches it
//! ([`recto_agent::print::PrintWatch`]), saying how far it's got by the page, with Stop beside it; and the host hands the file to the system's printing, which
//! never asks the app to draw anything on the UI thread.
//!
//! **The file has no name** (PLAN, "the person can't print a document they opened with its password"): it's unlinked before its first byte is written,
//! and the host is given `/dev/fd/N`, which CoreGraphics opens and maps. This app holds the descriptor from the host's taking it until the system's printing
//! says it's done with it; nothing else can open the file, and the kernel frees it when the last holder lets go, however the app ends.
//!
//! At rest there's nothing: no thread, worker, file or buffer. One print at a time; asking again while one's being prepared says so.
//!
//! - **A PDF's flag against printing is ignored** (owner, 2026-11-08: "Ignore the flag"): no notice and no refusal, and nothing here reads it.
//! - **A print to a file is an export.** When the panel's answer saves or previews a PDF rather than printing on paper, a file that asks not to be copied gets the notice a copy has, with "Print anyway" (owner, 2026-11-08: exports ask "anyway" like Copy). Print to paper never asks.
//! - **Nothing's left behind.** Stop, closing the tab and quitting each stop the preparing at the next page, and its file goes with its descriptor. A
//!   launch still removes the named files of a run of 0.1.0 that died (`recto-print-PID-*` of a process that's gone): for one release.

use super::{CoreCall, HostAction, SelectionNotice, lock, person};
use crate::queue::CoreQueue;
use recto_agent::commands::{Read, export::Print};
use recto_agent::print::{self, PrintWatch};
use recto_agent::{AgentError, Code, Command, TabRef};
use recto_reader::Engine;
use recto_session::TabId;
use serde_json::Value;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// The notice's Stop, while pages are being prepared.
const STOP_ID: &str = "print.stop";
/// A notice's "Print anyway", followed by the notice's number.
const ANYWAY_PREFIX: &str = "print.anyway.";
/// Where a print's file started its name, followed by the process's number, before it had none: for the sweep at launch.
const FILE_PREFIX: &str = "recto-print-";
/// The files the system's printing has at once, past which the oldest one's descriptor goes: one is the usual, and CoreGraphics maps a file as it opens it,
/// so a descriptor let go of early takes nothing from a print under way.
const MAX_HANDING: usize = 4;

/// **What the print panel was answered with:** the tab it was opened for, all the pages or a range from 1, just the selection's pages, and whether the
/// system's printing saves or previews a PDF (a print to a file, which is an export) rather than printing on paper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panel {
    pub tab: u32,
    pub all: bool,
    pub first: u32,
    pub last: u32,
    pub selection_only: bool,
    pub to_file: bool,
    /// The host's own number for this answer, which the file it makes comes back with, so the host prints it with this answer's printer and way.
    pub serial: u64,
}

/// The line by the page, with its number and button.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Said {
    serial: u64,
    text: String,
    anyway: bool,
}

#[derive(Default)]
pub struct Printing {
    running: AtomicBool,
    cancel: AtomicBool,
    /// The tab being printed: closing it stops the print, as Stop does.
    tab: AtomicU32,
    done: AtomicU32,
    of: AtomicU32,
    /// Counts the prints asked for, for a QA print's file name ([`Printing::qa_target_for`]).
    generation: AtomicU64,
    serial: AtomicU64,
    /// The host's number for the print asked for, which its file comes back with ([`Panel::serial`]).
    asked_by: AtomicU64,
    /// A print's file, with no name, for the host, with the host's number for the answer it was asked by.
    ready: Mutex<Option<(File, u64)>>,
    /// The files the host has taken, each until the system's printing is done with it: their descriptors, which keep them, as they have no name.
    handing: Mutex<Vec<File>>,
    notice: Mutex<Option<Said>>,
    /// What the notice with this number refused, for its "Print anyway", and the host's number for the answer it was asked by.
    refused: Mutex<Option<(u64, (Print, u64))>>,
    /// A QA print to a file: where the system's printing writes, a new name of ours, and where it's published, never over anything.
    qa_target: Mutex<Option<(PathBuf, PathBuf)>>,
}

impl Printing {
    /// **A print is asked for,** at once, on the UI thread, before its command runs: from here to what came of it being said, the window shows it being
    /// prepared with Stop, File > Print is off, and another answer isn't taken (review of S2, S4). A Stop or a quit from here on holds. False, and nothing
    /// changed, if one is asked for already.
    fn ask(&self, pages: u32, asked_by: u64) -> bool {
        if self.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.cancel.store(false, Ordering::SeqCst);
        self.done.store(0, Ordering::SeqCst);
        self.of.store(pages, Ordering::SeqCst);
        self.asked_by.store(asked_by, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.dismiss_notice();
        true
    }

    /// The print asked for won't run after all: its command wasn't taken, as during a hand-off.
    pub(crate) fn not_asked(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// A print asked for, from the answer to what came of it being said: Rust's part, which the app's settled waits for ([`crate::settled`]). The host's
    /// part, the file handed to the system, is in [`Self::busy`].
    pub fn preparing(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn busy(&self) -> bool {
        self.running.load(Ordering::SeqCst) || lock(&self.ready).is_some() || !lock(&self.handing).is_empty()
    }

    /// The line by the page while pages are being prepared, or what went wrong.
    pub fn notice(&self) -> Option<String> {
        self.selection_notice().map(|notice| notice.text)
    }

    /// The notice as the window shows it: Stop while preparing, "Print anyway" naming the notice that offered it.
    pub fn selection_notice(&self) -> Option<SelectionNotice> {
        if self.running.load(Ordering::SeqCst) {
            let (done, of) = (self.done.load(Ordering::SeqCst), self.of.load(Ordering::SeqCst));
            let text = if of > 1 {
                format!("Preparing pages for printing: {} of {of}.", (done + 1).min(of))
            } else {
                "Preparing the page for printing.".to_string()
            };
            return Some(SelectionNotice {
                text,
                action_title: Some("Stop".into()),
                action_id: Some(STOP_ID.into()),
                fades: false,
            });
        }
        lock(&self.notice).clone().map(|said| SelectionNotice {
            action_title: said.anyway.then(|| "Print anyway".to_string()),
            action_id: said.anyway.then(|| format!("{ANYWAY_PREFIX}{}", said.serial)),
            fades: !said.anyway,
            text: said.text,
        })
    }

    pub fn dismiss_notice(&self) {
        lock(&self.notice).take();
        lock(&self.refused).take();
    }

    fn say(&self, text: impl Into<String>, anyway: bool) -> u64 {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.notice) = Some(Said {
            serial,
            text: text.into(),
            anyway,
        });
        serial
    }

    /// What a notice's "Print anyway" names, once, if that notice is still the one showing.
    fn take_refused(&self, serial: u64) -> Option<(Print, u64)> {
        let mut refused = lock(&self.refused);
        let request = refused.take_if(|(offered, _)| *offered == serial).map(|(_, request)| request)?;
        lock(&self.notice).take();
        Some(request)
    }

    /// **The command a notice's "Print anyway" sends,** once, if that notice is still the one showing and no other print is asked for: what it refused,
    /// asked again anyway, for the same answer of the host's.
    pub(crate) fn anyway(&self, serial: u64) -> Option<Command> {
        let (mut print, asked_by) = self.take_refused(serial)?;
        if !self.ask(0, asked_by) {
            return None;
        }
        print.anyway = true;
        Some(Command::from(print))
    }

    /// **For QA's print to a PDF through the system's printing:** where it should write, a new name of ours beside `out`, which is published at `out` when
    /// it's done ([`Self::finished`]), never over anything. None if `out` isn't absolute, is there already, or the name of ours is taken.
    pub fn qa_target_for(&self, out: PathBuf) -> Option<PathBuf> {
        let folder = out.parent()?;
        if !out.is_absolute() || std::fs::symlink_metadata(&out).is_ok() {
            return None;
        }
        let written = folder.join(format!(
            ".recto-print-{}-{}.pdf",
            std::process::id(),
            self.generation.load(Ordering::SeqCst) + 1
        ));
        if std::fs::symlink_metadata(&written).is_ok() {
            return None;
        }
        *lock(&self.qa_target) = Some((written.clone(), out));
        Some(written)
    }

    /// **The file to print, once, for the host to hand to the system,** with the host's number for the answer it was asked by: `/dev/fd/N`, this app's
    /// descriptor of a file with no name, which it keeps open until [`Self::finished`] is told that same path. Read from its start, as a descriptor opened
    /// through `/dev/fd` shares this one's place in the file.
    pub fn take_ready(&self) -> Option<(PathBuf, u64)> {
        use std::io::Seek;
        let (mut file, serial) = lock(&self.ready).take()?;
        // A file that can't be read from its start goes now, and nothing's printed: the host is told of no file.
        file.rewind().ok()?;
        let path = descriptor_path(&file)?;
        let mut handing = lock(&self.handing);
        if handing.len() >= MAX_HANDING {
            handing.remove(0);
        }
        handing.push(file);
        Some((path, serial))
    }

    /// **The system's printing is done with the file at `path`,** however it went: its descriptor goes, and with it the file, if `path` is one this app
    /// handed out and still holds. Any other path, a stale one or one the host made up, lets nothing go, and no path the host gives is ever removed: the
    /// print's file has no name to remove. A QA print to a file is published where it was asked for.
    pub fn finished(&self, path: &str) {
        lock(&self.handing).retain(|file| !is_at(file, path));
        if let Some((written, out)) = lock(&self.qa_target).take() {
            if written.exists() && recto_agent::publish_picture(&written, &out).is_err() {
                self.say("The print couldn't be put there: a file is already there, and it won't be replaced.", false);
            }
            let _ = std::fs::remove_file(&written);
        }
    }

    pub fn has_ready(&self) -> bool {
        lock(&self.ready).is_some()
    }

    /// Stops a print being prepared, at the next page. Cheap, for the UI thread.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// The app's quitting: preparing stops at the next page, and a file the host hasn't taken goes now, with its descriptor. One the system's printing has
    /// is the system's until it's done, and goes with the app.
    pub fn quitting(&self) {
        self.cancel();
        lock(&self.ready).take();
    }
}

/// The folder a print's file goes in: the system's own temp folder, from `confstr`, and not `$TMPDIR`, which whoever started the app sets: a quiet run's
/// files are in its own folders (2026-11-13). The same folder for a launch from the Finder, whose `$TMPDIR` is that.
fn print_folder() -> PathBuf {
    recto_platform::local::system_temp_dir().unwrap_or_else(std::env::temp_dir)
}

/// **The path the host opens a print's file by:** `/dev/fd/N`, the descriptor's own, as the file has no name. None where there's no such thing.
fn descriptor_path(file: &File) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        Some(PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd())))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        None
    }
}

/// Whether `path` is the one [`descriptor_path`] gave for `file`.
fn is_at(file: &File, path: &str) -> bool {
    descriptor_path(file).is_some_and(|own| own.as_os_str() == path)
}

/// The process a print file's name says made it, if it's one of ours: `recto-print-PID-N.pdf` as v0.1.0 named them, or `recto-print-PID-N-HEX.pdf` as
/// builds after it did, before the file had no name.
fn maker_of(name: &str) -> Option<u32> {
    let rest = name.strip_prefix(FILE_PREFIX)?.strip_suffix(".pdf")?;
    let (pid, rest) = rest.split_once('-')?;
    let (generation, unguessable) = rest.split_once('-').unwrap_or((rest, "0"));
    generation.parse::<u64>().ok()?;
    u64::from_str_radix(unguessable, 16).ok()?;
    pid.parse().ok()
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

/// At launch, off the UI thread on a short-lived thread of its own: the print files of runs that died go. Only on a Mac, where a process's number can be
/// asked about. Kept for one release after 0.1.0, whose prints had names (PLAN).
pub fn sweep_at_launch() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let _ = std::thread::Builder::new().name("recto-print-sweep".into()).stack_size(128 * 1024).spawn(|| {
        sweep_stale(&print_folder(), |pid| recto_platform::local::process_path(pid).is_some());
    });
}

/// Which pages the panel's answer means. `first` and `last` count from 1, and `all` or a selection's pages override them. A range given backwards is taken forwards. None if there's nothing to print.
pub fn chosen_range(pages: u32, all: bool, first: u32, last: u32, selection: Option<(u32, u32)>) -> Option<(u32, u32)> {
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

/// **Printing's menu ids** ([`super::menu_rows`]), all the window's alone: Print… opens the system's panel, Stop stops the print being prepared for it, and a
/// notice's "Print anyway" asks again only what that notice refused.
pub fn row<'a>(parts: &super::row_parts::RowParts<'a>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    use super::menu_rows::Row;
    let parts = *parts;
    if asked.id == "file.print" {
        return Some(Row::host(
            "opens the system's print panel for the person",
            super::menu_rows::Agents::ItsOwn("`recto print`"),
            asked.host()?,
        ));
    }
    if asked.id == STOP_ID {
        return Some(Row::window(
            "stops the person's print being prepared for the system's print panel",
            super::menu_rows::Agents::Never,
            move || {
                parts.cancel_print();
                HostAction::None
            },
        ));
    }
    let serial = asked.id.strip_prefix(ANYWAY_PREFIX)?.to_string();
    Some(Row::window(
        "a notice's Print anyway, for the person's own print it refused: their decision, where an agent's is `print --anyway`, which is logged",
        super::menu_rows::Agents::ItsOwn("`recto print --anyway`"),
        move || {
            if let Some(command) = serial.parse().ok().and_then(|serial| parts.refused_print_anyway(serial)) {
                parts.person().submit(command);
            }
            parts.reconcile();
            HostAction::None
        },
    ))
}

/// **The print panel's answer, as the person's command:** `print` of the pages it chose, at the resolution a print takes, to the window's printing (no
/// file named: [`recto_agent::print::PrintTo::Window`]), asked for at once ([`Printing::ask`]). Nothing to print, or a print asked for already, and nothing's
/// sent. On the UI thread: what's published and short locks on the page view.
///
/// **On paper it never asks** whether a file that asks not to be copied may be printed (SPEC, Printing: "Print to paper never asks"; owner, 2026-11-08,
/// exports only ask "anyway"): the person's print on paper is sent anyway, and only a print to a file is asked about.
pub fn chosen(engine: &Engine, printing: &Printing, panel: Panel) -> Option<Command> {
    if panel.tab == 0 {
        return None;
    }
    let selection = panel
        .selection_only
        .then(|| engine.selection().hot())
        .flatten()
        .and_then(|(_, range)| range)
        .map(|range| (range.start.page, range.end.page));
    let (first, last) = chosen_range(engine.points().page_count(), panel.all, panel.first, panel.last, selection)?;
    // FOR THE OWNER, NOT DECIDED (PLAN, "the person can't print a document they opened with its password", point 6): Save as PDF or Open in Preview
    // from the system's panel (`panel.to_file`) of a document the person opened with its password makes an unencrypted copy of it, with only the copy
    // flag's "Print anyway" step. Recommended, and built: no step of its own. If the owner says otherwise, the notice goes here, said as the copy flag's
    // is (`Printing::say`), before anything is asked.
    if !printing.ask(last - first + 1, panel.serial) {
        printing.say("A print is already being prepared.", false);
        return None;
    }
    Some(Command::from(Print {
        tab: TabRef::Id(TabId(panel.tab)),
        pages: Some(format!("{first}-{last}")),
        dpi: print::DEFAULT_DPI,
        to_pdf: None,
        anyway: recto_agent::copying::print_anyway(panel.to_file),
    }))
}

/// **The window's watch over the person's print:** the notice says how far it's got, with Stop; Stop, the tab closing and quitting stop it at the next
/// page; one at a time.
struct Watch {
    printing: Arc<Printing>,
    core: Arc<CoreQueue>,
    host: Weak<super::AppHost>,
}

/// The watch, for the host to give the person's print ([`recto_control::host::SessionHost::set_print_watch`]).
pub fn watch(printing: Arc<Printing>, core: Arc<CoreQueue>, host: Weak<super::AppHost>) -> Arc<dyn PrintWatch> {
    Arc::new(Watch { printing, core, host })
}

impl PrintWatch for Watch {
    fn begin(&self, tab: TabId, pages: u32) {
        // Its pages are read: the person's next edit goes on now, and isn't printed.
        crate::queue::lane::pass_here();
        let printing = &self.printing;
        // Asked for already, from the answer, unless it came another way. A Stop or a quit since holds: [`Printing::ask`] alone clears it.
        printing.running.store(true, Ordering::SeqCst);
        printing.done.store(0, Ordering::SeqCst);
        printing.of.store(pages, Ordering::SeqCst);
        printing.tab.store(tab.0, Ordering::SeqCst);
        self.core.push(CoreCall::Reconcile);
    }

    fn page(&self, done: u32, of: u32) -> bool {
        let printing = &self.printing;
        printing.done.store(done, Ordering::SeqCst);
        printing.of.store(of, Ordering::SeqCst);
        self.core.push(CoreCall::Reconcile);
        #[cfg(test)]
        at_page(done);
        let tab = TabId(printing.tab.load(Ordering::SeqCst));
        // Closing the tab stops it, as Stop does.
        let open = self
            .host
            .upgrade()
            .is_some_and(|host| host.session.with_session(|session| session.tabs().iter().any(|record| record.id == tab)));
        open && !printing.cancel.load(Ordering::SeqCst)
    }

    /// The file goes to the host, in place of one the host never took, which goes now with its descriptor: nothing else would ever let it go.
    fn ready(&self, file: File) {
        let asked_by = self.printing.asked_by.load(Ordering::SeqCst);
        drop(lock(&self.printing.ready).replace((file, asked_by)));
    }

    /// What came of it is said by [`answered`], which ends the print once its file is the host's, so nothing sees it neither preparing nor ready (review
    /// of S2, S1). A command that panicked is answered by nobody: it ends here.
    fn end(&self) {
        if std::thread::panicking() {
            self.printing.running.store(false, Ordering::SeqCst);
        }
        #[cfg(test)]
        after_end();
    }
}

/// **What came of the person's print, said:** its file ready for the host (given it by [`Watch::ready`]), with a note if a page printed smaller; a file that asks not to be copied, with
/// "Print anyway"; nothing if it was stopped; or the refusal's own words, whatever refused it, before it ran or as it did.
pub fn answered(parts: &super::answer_parts::AnswerParts<'_>, ran: &person::Ran<'_>) {
    let Command::Read(Read::Print(asked)) = ran.command else { return };
    if ran.result.is_err() && ran.refused().is_none() {
        return;
    }
    parts.print_answered(|printing| said(printing, asked, ran.result));
    #[cfg(test)]
    after_telling();
}

/// What came of the person's print, said to printing's state.
fn said(printing: &Printing, asked: &Print, result: &Result<Value, AgentError>) {
    let asked_by = printing.asked_by.load(Ordering::SeqCst);
    match result {
        Ok(answer) => {
            if let Some(note) = scaled_down(answer).scaled_down_note() {
                printing.say(note, false);
            }
        }
        Err(error) => match said_as(error) {
            Told::Anyway => {
                // A QA print's target stays, for "Print anyway" to write to.
                let serial = printing.say(error.message.clone(), true);
                *lock(&printing.refused) = Some((serial, (asked.clone(), asked_by)));
            }
            Told::Nothing => {
                lock(&printing.qa_target).take();
            }
            Told::Words => {
                let words = if person::not_started(error) {
                    "The print couldn't be started.".to_string()
                } else {
                    error.message.clone()
                };
                printing.say(words, false);
                lock(&printing.qa_target).take();
            }
        },
    }
    // Ended once what came of it is the window's and its file the host's: never neither preparing nor ready.
    printing.running.store(false, Ordering::SeqCst);
}

/// The pages a print's answer says were printed smaller than asked.
fn scaled_down(answer: &Value) -> print::Printed {
    let pages = answer["scaled_down"].as_array().map(Vec::as_slice).unwrap_or_default();
    print::Printed {
        scaled_down: pages
            .iter()
            .filter_map(|page| Some((u32::try_from(page["page"].as_u64()?).ok()?, u32::try_from(page["dpi"].as_u64()?).ok()?)))
            .collect(),
        ..print::Printed::default()
    }
}

/// A test's: the next print's thread says it has told what came of it, then waits to be let go, so what was published then can be looked at.
#[cfg(test)]
static AFTER_TELLING: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>> = Mutex::new(None);

/// Where a print's thread waits, once it has told what came of it, while a test holds it there ([`AFTER_TELLING`]).
#[cfg(test)]
fn after_telling() {
    if let Some((told, go)) = lock(&AFTER_TELLING).take() {
        let _ = told.send(());
        let _ = go.recv_timeout(crate::testkit::headless::WAIT_LIMIT);
    }
}

/// A test's: the next print's thread, once this many pages are done, says so and waits to be let go, so a Stop or a second print can be asked mid-way.
#[cfg(test)]
pub(crate) static AT_PAGE: Mutex<Option<(u32, Hold)>> = Mutex::new(None);

/// A test's hold on a print's thread: it says it's there, then waits to be let go.
#[cfg(test)]
pub(crate) type Hold = (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);

/// A test's: the next print's thread, once its command's drawing has ended and before what came of it is said, says so and waits to be let go.
#[cfg(test)]
pub(crate) static AFTER_END: Mutex<Option<Hold>> = Mutex::new(None);

/// Where a print's thread waits between its drawing's end and its answer, while a test holds it there ([`AFTER_END`]).
#[cfg(test)]
fn after_end() {
    if let Some((told, go)) = lock(&AFTER_END).take() {
        let _ = told.send(());
        let _ = go.recv_timeout(crate::testkit::headless::WAIT_LIMIT);
    }
}

/// Where a print's thread waits mid-way while a test holds it there ([`AT_PAGE`]).
#[cfg(test)]
fn at_page(done: u32) {
    let held = lock(&AT_PAGE).take_if(|(at, _)| *at == done);
    if let Some((_, (told, go))) = held {
        let _ = told.send(());
        let _ = go.recv_timeout(crate::testkit::headless::WAIT_LIMIT);
    }
}

/// How a print that didn't go is said.
#[derive(Debug, PartialEq, Eq)]
enum Told {
    /// The file asks not to be copied: its notice offers "Print anyway".
    Anyway,
    /// It was stopped, as the person asked: nothing.
    Nothing,
    /// Its words.
    Words,
}

/// How a print's refusal is said, by its named code.
fn said_as(error: &AgentError) -> Told {
    match error.code {
        Code::COPY_RESTRICTED => Told::Anyway,
        Code::CANCELLED => Told::Nothing,
        _ => Told::Words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (second review of `build/settled-gaps`, 2026-11-14): a print's thread told the session what came of it before it said it was done, so the
    /// look the window is made from could read it still preparing and keep "Preparing pages for printing… [Stop]" on screen, hiding a refused print's "Print
    /// anyway", with nothing to look again. Held just after it has told: the window made from that word shows no print being prepared.
    #[test]
    fn the_window_made_after_a_print_ends_shows_it_ended() {
        use crate::testkit::headless::{HeadlessApp, WAIT_LIMIT};
        let app = HeadlessApp::start();
        let doc = app.disk().pdf("a.pdf", 2);
        app.open(&doc);
        app.settle();
        let tab = lock(&app.core().published).reader.shown_tab;
        let (told, held) = std::sync::mpsc::channel();
        let (go, waiting) = std::sync::mpsc::channel();
        *lock(&AFTER_TELLING) = Some((told, waiting));
        app.act(crate::core::controls::UiAction::Print {
            tab,
            all: false,
            first: 1,
            last: 1,
            selection_only: false,
            to_file: false,
            serial: 1,
        });
        held.recv_timeout(WAIT_LIMIT).expect("the print told what came of it");
        assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took the word");
        let notice = app.reader().selection_notice.map(|notice| notice.text);
        drop(go);
        app.settle();
        assert!(
            !notice.as_deref().is_some_and(|text| text.starts_with("Preparing")),
            "the window made after the print ended says it's still preparing: {notice:?}"
        );
    }

    /// Failure: QA's print to a PDF writes over a file that's there, writes to a relative place, or takes a name of ours a file already has, as the bridge's
    /// `print_qa_target` refused before it moved here (R3.7, S2): the target is a new name beside `out`, kept for the print's end to publish, and nothing else.
    #[test]
    fn a_qa_prints_target_is_new_and_beside_where_it_goes() {
        let folder = recto_testpdf::TempDir::new("pqa-");
        let printing = Printing::default();
        assert_eq!(printing.qa_target_for(PathBuf::from("relative.pdf")), None, "not absolute");
        assert_eq!(printing.qa_target_for(PathBuf::from("/")), None, "no folder");
        let there = folder.join("there.pdf");
        std::fs::write(&there, b"x").expect("a file");
        assert_eq!(printing.qa_target_for(there), None, "a file's there already");
        assert!(lock(&printing.qa_target).is_none(), "nothing kept for a refusal");
        let out = folder.join("out.pdf");
        let written = printing.qa_target_for(out.clone()).expect("a target");
        assert_eq!(
            written,
            folder.join(format!(".recto-print-{}-1.pdf", std::process::id())),
            "the next print's name, beside it"
        );
        assert_eq!(*lock(&printing.qa_target), Some((written.clone(), out.clone())), "kept for the print's end");
        std::fs::write(&written, b"x").expect("taken");
        assert_eq!(printing.qa_target_for(out), None, "the name of ours taken");
    }

    /// Failure (R2.5): a print to a file that asks not to be copied says its words with no "Print anyway", or a stopped print says it failed, as its refusal
    /// was told apart by a code's string. By named codes: restricted offers Print anyway, stopped says nothing, anything else its words.
    #[test]
    fn a_prints_refusal_is_said_by_its_code() {
        use recto_agent::{AgentError, Code, exit};
        assert_eq!(
            super::said_as(&AgentError::new(Code::COPY_RESTRICTED, "restricted", exit::PROTECTED)),
            super::Told::Anyway
        );
        assert_eq!(super::said_as(&AgentError::new(Code::CANCELLED, "stopped", exit::FAILED)), super::Told::Nothing);
        assert_eq!(
            super::said_as(&AgentError::new(Code::WRITE_FAILED, "not written", exit::FAILED)),
            super::Told::Words
        );
    }

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

    /// Failure (condition 2 of the design's acceptance, 2026-11-14): the host's word that the system's printing is done lets go of another print's file, as a
    /// stale or made-up `/dev/fd/N` would if it were matched loosely, or is taken as a file name and removes whatever is at that path. Two files handed out:
    /// a path of no file of ours, a named file's path, and the other's path each let go only of what they name, and the named file stays.
    #[test]
    fn the_hosts_done_lets_go_only_of_its_own_file_and_removes_nothing() {
        use crate::core::headless::printing::{unnamed_file, unnamed_files_held};
        let printing = Printing::default();
        let before = unnamed_files_held();
        let (one, two) = (unnamed_file(b"one"), unnamed_file(b"two"));
        *lock(&printing.ready) = Some((one, 1));
        let (one_path, _) = printing.take_ready().expect("the first");
        *lock(&printing.ready) = Some((two, 2));
        let (two_path, _) = printing.take_ready().expect("the second");
        let named = recto_testpdf::TempDir::new("print-done-");
        let kept = named.join("kept.pdf");
        std::fs::write(&kept, b"keep").expect("a named file");
        let held = || unnamed_files_held().len() - before.len();
        assert_eq!(held(), 2, "both held while the system's printing has them");
        for stray in ["/dev/fd/999999", kept.to_str().expect("a path"), "", "/dev/fd/"] {
            printing.finished(stray);
            assert_eq!(held(), 2, "{stray:?} let go of a file it doesn't name");
        }
        assert!(kept.exists(), "a path the host gives is never removed");
        printing.finished(two_path.to_str().expect("a path"));
        assert_eq!(held(), 1, "the second let go, by its own path");
        assert!(printing.busy(), "the first is still the system's");
        printing.finished(two_path.to_str().expect("a path"));
        assert_eq!(held(), 1, "its path again lets go of nothing more");
        printing.finished(one_path.to_str().expect("a path"));
        assert_eq!((held(), printing.busy()), (0, false));
    }

    /// Failure: a run of 0.1.0, or of a build after it, that died mid-print leaves its named print file for good, as a name of either form isn't known as a
    /// print's.
    #[test]
    fn the_names_prints_had_are_known_as_a_runs() {
        assert_eq!(maker_of("recto-print-41-3.pdf"), Some(41), "0.1.0's");
        assert_eq!(maker_of("recto-print-41-3-00ff00ff00ff00ff.pdf"), Some(41), "a build's after it");
        assert_eq!(maker_of("recto-print-41-x.pdf"), None);
    }

    /// Failure: a run that died mid-print leaves its print file in the temp folder for ever, or the sweep at launch removes a live run's, this run's, or a file that only looks like one.
    #[test]
    fn a_launch_sweeps_only_the_print_files_of_runs_that_died() {
        let folder = recto_testpdf::TempDir::new("recto-print-sweep-");
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
        let request = Print {
            tab: TabRef::Id(TabId(1)),
            pages: Some("1-2".into()),
            dpi: 300,
            to_pdf: None,
            anyway: false,
        };
        let serial = printing.say("This file's permissions ask that it isn't copied.", true);
        *lock(&printing.refused) = Some((serial, (request.clone(), 7)));
        let refused = printing.selection_notice().unwrap();
        assert_eq!(refused.action_id, Some(format!("{ANYWAY_PREFIX}{serial}")));
        assert_eq!(refused.action_title.as_deref(), Some("Print anyway"));
        assert!(printing.anyway(serial + 1).is_none());
        let Some(Command::Read(Read::Print(again))) = printing.anyway(serial) else {
            panic!("Print anyway sends the print it refused")
        };
        assert_eq!(
            (again.tab, again.pages, again.dpi, again.anyway),
            (request.tab, request.pages, request.dpi, true),
            "the same print, anyway"
        );
        assert_eq!(again.to_pdf, None, "to the window's printing, as the print it refused");
        assert!(printing.anyway(serial).is_none(), "once");
    }

    /// Failure: an update quits the app while a print's file is ready but not yet handed to the system, or while the system's printing has it: `busy` must hold through both, not only while pages are drawn.
    #[test]
    fn a_print_is_busy_until_the_system_is_done_with_it() {
        use crate::core::headless::printing::{unnamed_file, unnamed_files_held};
        let printing = Printing::default();
        let before = unnamed_files_held();
        assert!(!printing.busy());
        *lock(&printing.ready) = Some((unnamed_file(b"print"), 1));
        assert!(printing.busy(), "ready for the host");
        let (path, asked_by) = printing.take_ready().expect("the file");
        assert_eq!(asked_by, 1);
        assert_eq!(std::fs::read(&path).expect("the host reads it by its path"), b"print", "from its start");
        assert!(printing.busy(), "the system's printing has it");
        printing.finished(path.to_str().unwrap());
        assert!(!printing.busy());
        assert_eq!(unnamed_files_held(), before, "its file goes with its descriptor");
    }

    /// Failure: quitting leaves a prepared file the host never took held, or doesn't stop the preparing.
    #[test]
    fn quitting_stops_the_print_and_takes_its_file() {
        use crate::core::headless::printing::{unnamed_file, unnamed_files_held};
        let printing = Printing::default();
        let before = unnamed_files_held();
        *lock(&printing.ready) = Some((unnamed_file(b"print"), 1));
        printing.quitting();
        assert!(printing.cancel.load(Ordering::SeqCst));
        assert!(!printing.busy());
        assert_eq!(unnamed_files_held(), before, "its file goes with its descriptor");
    }
}

/// **What the print panel needs to know before it opens,** from what's published and short locks on the page view: the tab showing, its pages, whether
/// something's selected, and its first page's size. Never waits.
#[must_use]
pub fn model(reader: crate::reader_view::ReaderView, engine: &Engine) -> PrintModel {
    let (width_pt, height_pt) = engine
        .points()
        .page_size_shown(0)
        .map_or((612.0, 792.0), |(width, height)| (f64::from(width), f64::from(height)));
    PrintModel {
        tab: reader.shown_tab,
        pages: engine.points().page_count(),
        has_selection: engine.selection().has_any(),
        job_title: reader.document_name,
        width_pt,
        height_pt,
    }
}

/// **A print's file for the host to hand to the system's printing,** with the host's own number for the panel's answer it was asked by ([`Panel::serial`]):
/// the host prints it with that answer's printer and way, and with nothing else's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyPrint {
    pub path: String,
    pub serial: u64,
}

impl From<(PathBuf, u64)> for ReadyPrint {
    fn from((path, serial): (PathBuf, u64)) -> Self {
        Self {
            path: path.to_string_lossy().into_owned(),
            serial,
        }
    }
}

/// What the print panel needs to know before it opens, all Rust's to say.
#[derive(Clone, Debug, PartialEq)]
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
