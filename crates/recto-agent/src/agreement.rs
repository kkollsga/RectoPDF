//! **The agreement gate's look,** for every way of saving (SPEC section 3: "when the two parsers disagree about a file, it stays readable but saving
//! and signing turn off"). `recto_core::agreement` has the rule, the structure reader's half and the words; this asks the renderer's half of a worker
//! of its own, through the very file the structure is read from, so both read the same bytes, and that file's own stamp says which version it was.
//!
//! A look costs a worker's start and a size per page, up to `MAX_COMPARED_PAGES`, and at most [`LOOK_DEADLINE`], so it's made **once per version
//! of a file**, off the UI thread, and kept for the version asked about, whatever it found, so nothing asks again in a loop. [`check`] is what a save
//! asks, on its own thread: it joins a look already under way for the same version, and looks again at an answer that was only about the moment
//! (`Disagreement::Unchecked`). [`known_or_start`] is what the window asks, which starts the look on a thread of its own and calls back when it's known.
//! At most [`LOOKS_AT_ONCE`] run at once, a save's counting, and [`KEPT`] answers are kept, so nothing grows with the files opened.

use crate::{AgentError, SessionAccess, select};
use recto_core::OpenError;
use recto_core::agreement::{Disagreement, View, compare, compared_pages, structure_view};
use recto_platform::FileStamp;
use recto_render_service::{DocRenderer, Policy, RenderFailure, ServiceConfig};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// How many window looks run at once. Each holds a worker while it runs. A save's look counts toward it, and isn't held back by it.
pub const LOOKS_AT_ONCE: usize = 2;
/// How many answers are kept, newest last.
pub const KEPT: usize = 16;
/// The longest a look may take, past which it fails closed, as `Unchecked`: a worker's start and 10,000 sizes take about a second.
pub const LOOK_DEADLINE: Duration = Duration::from_secs(30);
/// Pages the renderer may fail to size before the look stops and counts the file as one it couldn't read: each failure costs a worker.
const PAGE_FAILURES: u32 = 8;

/// What does the looking, so a test can make it slow, stuck or panic.
type Looker = dyn FnOnce(&Path, FileStamp, Instant) -> Option<Disagreement> + Send;

/// **What the two readers make of the file at `path`, as it is now,** compared: `None` when they agree. Blocks: never on the UI thread.
pub fn look(path: &Path) -> Option<Disagreement> {
    match recto_platform::file_stamp(path) {
        Ok(stamp) => look_at(path, stamp, Instant::now() + LOOK_DEADLINE),
        Err(_) => Some(Disagreement::Unchecked),
    }
}

/// [`look`] at the version `stamp` names, by `deadline`. The file opened isn't that version, a worker that can't start, a read that fails or a
/// deadline passed is `Unchecked`, never agreement.
fn look_at(path: &Path, stamp: FileStamp, deadline: Instant) -> Option<Disagreement> {
    let Ok(config) = ServiceConfig::discover() else {
        return Some(Disagreement::Unchecked);
    };
    let Ok(mut doc) = DocRenderer::new(path, None, config, Policy::default()) else {
        return Some(Disagreement::Unchecked);
    };
    let answer = look_with(&mut doc, stamp, deadline);
    doc.kill_worker();
    answer
}

fn look_with(doc: &mut DocRenderer, stamp: FileStamp, deadline: Instant) -> Option<Disagreement> {
    let read = doc.read_structure(|file, _| {
        // The version the renderer opened, by its descriptor, not by whatever's at the path now.
        if recto_platform::open_file_stamp(&file).ok() != Some(stamp) {
            return None;
        }
        Some(structure_view(file, None))
    });
    let structure = match read.flatten() {
        Some(Ok(view)) => view,
        // lopdf needs a password: a save refuses an encrypted file itself. They differ only if PDFium opened it without one.
        Some(Err(OpenError::PasswordNeeded)) => return doc.page_count().is_ok().then_some(Disagreement::Encryption),
        // lopdf can't read the very descriptor the renderer opened: that's the file's answer, kept (review round 2).
        Some(Err(OpenError::NotPdf | OpenError::TooComplex)) => return Some(Disagreement::StructureUnreadable),
        // Any other failed read may be the disk's, not the file's: never taken as agreement, and looked at again by the next save (review round 1).
        Some(Err(_)) | None => return Some(Disagreement::Unchecked),
    };
    let page_count = match doc.page_count() {
        Ok(count) => count,
        Err(failure) => return Some(renderer_refused(failure)),
    };
    // The counts first: a document whose counts differ needs no page sized (review round 1).
    if page_count != structure.page_count {
        return compare(&structure, &View { page_count, ..View::default() });
    }
    match renderer_view(doc, page_count, deadline) {
        Ok(renderer) => compare(&structure, &renderer),
        Err(failure) => Some(renderer_refused(failure)),
    }
}

/// What the renderer failing says: it needs a password lopdf didn't (`Encryption`); it can't read the file at all, which is the file's
/// (`RendererFailed`); or something about the moment, a worker that couldn't start or a deadline passed (`Unchecked`).
fn renderer_refused(failure: RenderFailure) -> Disagreement {
    match failure {
        RenderFailure::NeedsPassword => Disagreement::Encryption,
        RenderFailure::NotPdf | RenderFailure::Security | RenderFailure::OpenFailed { .. } | RenderFailure::PageReadFailed { .. } | RenderFailure::BadPage => {
            Disagreement::RendererFailed
        }
        _ => Disagreement::Unchecked,
    }
}

/// The renderer's half, once its count is known to be the structure's: whether it rebuilt the file's index, and the sizes of the pages compared,
/// by `deadline`. A page it can't size is `None`, up to [`PAGE_FAILURES`] of them; a failure of the whole file is the answer.
fn renderer_view(doc: &mut DocRenderer, page_count: u32, deadline: Instant) -> Result<View, RenderFailure> {
    let repaired = doc.repaired()?;
    let mut failures = 0;
    let mut sizes = Vec::new();
    for page in compared_pages(page_count) {
        if Instant::now() >= deadline {
            return Err(RenderFailure::Unavailable);
        }
        sizes.push(match doc.page_size(page) {
            Ok(size) => Some(size),
            Err(RenderFailure::PageReadFailed { .. } | RenderFailure::BadPage) if failures < PAGE_FAILURES => {
                failures += 1;
                None
            }
            Err(failure) => return Err(failure),
        });
    }
    Ok(View { page_count, repaired, sizes })
}

/// The files a save of `tab` would write from, each with the version the session has: the tab's own and every PDF its pages were added from. None
/// for a tab only the renderer could read, which `begin_save` refuses as such: no worker is started for an answer that's already known (review round 2).
fn files_of<A: SessionAccess>(access: &A, tab: &str) -> Result<Vec<(PathBuf, Option<FileStamp>, bool)>, AgentError> {
    access.with(|s| {
        let id = select(s, tab)?;
        if s.tabs().tab(id).is_some_and(|tab| tab.renderer_only) {
            return Ok(Vec::new());
        }
        let mut files: Vec<(PathBuf, Option<FileStamp>, bool)> = s.tabs().path_of(id).map(|path| (path, s.tabs().own_stamp(id), false)).into_iter().collect();
        if let Some(edits) = s.page_edits(id) {
            files.extend(edits.added_sources().iter().map(|added| (added.path.to_path_buf(), added.stamp, true)));
        }
        Ok(files)
    })
}

/// **The gate, before a save holds its tab** (review round 1): a look can take seconds, and edits shouldn't be frozen for it. The save asks again
/// once it holds the tab ([`save_refusal`]), which is then answered from what this kept.
pub fn refusal_for<A: SessionAccess>(access: &A, tab: &str) -> Result<Option<AgentError>, AgentError> {
    let files = files_of(access, tab)?;
    Ok(refusal_among(files.iter().map(|(path, stamp, added)| (path.as_path(), *stamp, *added))))
}

/// **The gate for a save,** of the file and of every PDF its pages were added from, as a copied page is written from lopdf's reading of its file too.
/// A refusal names which file and what differs; nothing's been written.
pub fn save_refusal(snapshot: &recto_session::SaveSnapshot) -> Option<AgentError> {
    let own = std::iter::once((&*snapshot.path, snapshot.stamp, false));
    let added = snapshot.added.iter().map(|added| (&*added.path, added.stamp, true));
    refusal_among(own.chain(added))
}

fn refusal_among<'a>(files: impl Iterator<Item = (&'a Path, Option<FileStamp>, bool)>) -> Option<AgentError> {
    files.into_iter().find_map(|(path, stamp, added)| {
        // A file with no stamp, or that's another version than the session has, is refused by the save itself: no look is spent on it. One that
        // can't be looked at just now isn't skipped: it can't be checked (review round 2).
        let stamp = stamp?;
        let disagreement = match recto_platform::file_stamp(path) {
            Ok(now) if now != stamp => return None,
            Ok(_) => check(path, stamp)?,
            Err(_) => Disagreement::Unchecked,
        };
        let message = if added {
            format!(
                "Pages were added from “{}”, which RectoPDF couldn't check or whose two readers see it differently: {}. So nothing was saved, as what's written might not be what you see. Nothing was written, and the edits are still here.",
                crate::file_label(path),
                disagreement.detail()
            )
        } else {
            format!("{} Nothing was written, and the edits are still here.", disagreement.message())
        };
        let next = if disagreement.is_transient() {
            "Try the save again in a moment. The edits stay unsaved."
        } else {
            "Tell the user. The file can still be read; to save changes to it, it first needs repairing in another tool. The edits stay unsaved."
        };
        Some(AgentError::new("parsers_disagree", message, crate::exit::FAILED).then(next))
    })
}

/// Looks under way and answers kept, for every save in this process.
struct Looks {
    known: Vec<(FileStamp, Option<Disagreement>)>,
    looking: Vec<FileStamp>,
}

static LOOKS: Mutex<Looks> = Mutex::new(Looks {
    known: Vec::new(),
    looking: Vec::new(),
});
/// Told whenever a look ends, for a save waiting on one.
static LOOKED: Condvar = Condvar::new();

fn looks() -> std::sync::MutexGuard<'static, Looks> {
    LOOKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keeps the answer for a version, in place of any older one for it, dropping the oldest past [`KEPT`].
fn remember(held: &mut Looks, stamp: FileStamp, answer: Option<Disagreement>) {
    held.known.retain(|(known, _)| *known != stamp);
    held.known.push((stamp, answer));
    if held.known.len() > KEPT {
        held.known.remove(0);
    }
}

fn known_in(held: &Looks, stamp: FileStamp) -> Option<Option<Disagreement>> {
    held.known.iter().find(|(known, _)| *known == stamp).map(|(_, answer)| answer.clone())
}

/// Ends a look however its thread does: its answer is kept for the version asked, as `Unchecked` if the look panicked, its place is given back,
/// a save waiting on it is told, and so is the window.
struct LookEnds {
    stamp: FileStamp,
    answer: Option<Option<Disagreement>>,
    done: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for LookEnds {
    fn drop(&mut self) {
        {
            let mut held = looks();
            let answer = self.answer.take().unwrap_or(Some(Disagreement::Unchecked));
            remember(&mut held, self.stamp, answer);
            held.looking.retain(|stamp| *stamp != self.stamp);
        }
        LOOKED.notify_all();
        if let Some(done) = self.done.take() {
            done();
        }
    }
}

/// Runs a look for `stamp` and keeps its answer, whatever it is, for that stamp: so the window, told it's done, finds it and doesn't ask again.
fn run_look(path: &Path, stamp: FileStamp, looker: Box<Looker>, done: Option<Box<dyn FnOnce() + Send>>) -> Option<Disagreement> {
    #[cfg(test)]
    LOOKS_RUN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut ends = LookEnds { stamp, answer: None, done };
    let answer = looker(path, stamp, Instant::now() + LOOK_DEADLINE);
    ends.answer = Some(answer.clone());
    drop(ends);
    answer
}

/// **What a save asks,** on its own thread: the answer for this version of the file. One kept is the answer, unless it was only about the moment;
/// a look under way for it is waited for, up to [`LOOK_DEADLINE`]; otherwise it's looked at now, which counts toward the window's cap.
pub fn check(path: &Path, stamp: FileStamp) -> Option<Disagreement> {
    check_by(path, stamp, Box::new(look_at))
}

fn check_by(path: &Path, stamp: FileStamp, looker: Box<Looker>) -> Option<Disagreement> {
    let deadline = Instant::now() + LOOK_DEADLINE;
    let mut held = looks();
    loop {
        if !held.looking.contains(&stamp) {
            match known_in(&held, stamp) {
                Some(answer) if !answer.as_ref().is_some_and(Disagreement::is_transient) => return answer,
                _ => break,
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Some(Disagreement::Unchecked);
        }
        held = LOOKED.wait_timeout(held, left).unwrap_or_else(std::sync::PoisonError::into_inner).0;
    }
    held.looking.push(stamp);
    drop(held);
    run_look(path, stamp, looker, None)
}

/// **What the window asks:** the answer for this version if it's known, whatever it is; if not, `None`, and the look starts on a thread of its own,
/// calling `done` once it's known. One look per version, and at most [`LOOKS_AT_ONCE`] at once: past that, `None` until one ends.
pub fn known_or_start(path: &Path, stamp: FileStamp, done: impl FnOnce() + Send + 'static) -> Option<Option<Disagreement>> {
    match known_or_start_by(path, stamp, Box::new(look_at), Box::new(done)) {
        Ask::Known(answer) => Some(answer),
        Ask::Started | Ask::Busy => None,
    }
}

/// What asking the window's way did.
#[derive(Debug, PartialEq)]
enum Ask {
    Known(Option<Disagreement>),
    /// A look started, and will call back.
    Started,
    /// None started: one's under way for this version, or the cap's reached.
    Busy,
}

fn known_or_start_by(path: &Path, stamp: FileStamp, looker: Box<Looker>, done: Box<dyn FnOnce() + Send>) -> Ask {
    let mut held = looks();
    if let Some(answer) = known_in(&held, stamp) {
        return Ask::Known(answer);
    }
    if held.looking.contains(&stamp) || held.looking.len() >= LOOKS_AT_ONCE {
        return Ask::Busy;
    }
    held.looking.push(stamp);
    drop(held);
    let path = path.to_path_buf();
    // As deep as a file read's thread (`recto-read`): the structure walk is bounded, but it's a hostile file's.
    let spawned = std::thread::Builder::new()
        .name("recto-agreement".into())
        .stack_size(1024 * 1024)
        .spawn(move || {
            run_look(&path, stamp, looker, Some(done));
        });
    if spawned.is_err() {
        looks().looking.retain(|looking| *looking != stamp);
        // A save waiting on this look would otherwise wait out its deadline.
        LOOKED.notify_all();
        return Ask::Busy;
    }
    Ask::Started
}

/// Held by each test that uses the process-wide [`LOOKS`], so `cargo test`, which runs a binary's tests on threads of one process, doesn't race them.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How many looks have run in this process, for a test that none did.
#[cfg(test)]
pub(crate) static LOOKS_RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn stamp_of(path: &Path) -> FileStamp {
        recto_platform::file_stamp(path).unwrap()
    }

    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < Duration::from_secs(10), "waited 10 s for {what}");
            std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
        }
    }

    fn known(stamp: FileStamp) -> Option<Option<Disagreement>> {
        known_in(&looks(), stamp)
    }

    /// Failure: the gate looks at a file again at every update of the window or every save, starting a worker each time; or a look that panics holds
    /// its place for ever and is never answered. Once per version: a second ask is answered from what's kept, a new version is looked at again, and a
    /// look that panics ends, counting as one that couldn't check.
    #[test]
    fn a_version_is_looked_at_once_and_a_look_ends_however_it_ends() {
        let _lock = test_lock();
        let file = recto_testpdf::TempPdf::new("once.pdf", &recto_testpdf::pages(&[recto_testpdf::Page::new("")]));
        let stamp = stamp_of(&file.0);
        assert_eq!(check_by(&file.0, stamp, Box::new(|_, _, _| None)), None);
        assert_eq!(check_by(&file.0, stamp, Box::new(|_, _, _| unreachable!("known already"))), None);
        let other = FileStamp { size: stamp.size + 1, ..stamp };
        assert_eq!(
            known_or_start_by(&file.0, other, Box::new(|_, _, _| panic!("a hostile file")), Box::new(|| {})),
            Ask::Started
        );
        wait_for("the look to end", || looks().looking.is_empty());
        assert_eq!(
            known(other),
            Some(Some(Disagreement::Unchecked)),
            "a look that panicked can't vouch for the file"
        );
        assert_eq!(
            known_or_start_by(&file.0, stamp, Box::new(|_, _, _| unreachable!("known already")), Box::new(|| {})),
            Ask::Known(None),
            "and the first version is still known"
        );
    }

    /// Failure (review round 1, blocking): a look whose file isn't the version asked about, or is gone, keeps no answer; the window, told it's done,
    /// asks again, and every ask spawns a thread: a hot loop. The answer is kept for the version asked, so the next ask starts nothing.
    #[test]
    fn a_look_at_a_file_thats_changed_or_gone_is_kept_and_not_started_again() {
        let _lock = test_lock();
        let gone = std::path::Path::new("/nowhere/at/all.pdf");
        let stamp = FileStamp {
            size: 1,
            mtime_ns: 1,
            device: 1,
            inode: 424_242,
        };
        let (told, heard) = mpsc::channel();
        assert_eq!(
            known_or_start(gone, stamp, move || told.send(()).unwrap()),
            None,
            "the real look starts, at a file that isn't there"
        );
        heard.recv_timeout(Duration::from_secs(10)).expect("told when it's known");
        assert_eq!(
            known_or_start_by(
                gone,
                stamp,
                Box::new(|_, _, _| unreachable!("asked again")),
                Box::new(|| unreachable!("told again"))
            ),
            Ask::Known(Some(Disagreement::Unchecked)),
            "kept, and nothing starts"
        );
    }

    /// Failure (review round 1): a look that couldn't check, as on a disk that failed a read, is kept as agreement, and the next save skips the
    /// comparison. It's `Unchecked`, which refuses the save, and a save looks again rather than take it as the file's answer; the window keeps it.
    #[test]
    fn a_look_that_couldnt_check_refuses_and_a_save_looks_again() {
        let _lock = test_lock();
        let file = recto_testpdf::TempPdf::new("transient.pdf", &recto_testpdf::pages(&[recto_testpdf::Page::new("")]));
        let stamp = stamp_of(&file.0);
        assert_eq!(
            check_by(&file.0, stamp, Box::new(|_, _, _| Some(Disagreement::Unchecked))),
            Some(Disagreement::Unchecked)
        );
        assert_eq!(
            known_or_start_by(&file.0, stamp, Box::new(|_, _, _| unreachable!("the window keeps it")), Box::new(|| {})),
            Ask::Known(Some(Disagreement::Unchecked))
        );
        assert_eq!(
            check_by(&file.0, stamp, Box::new(|_, _, _| None)),
            None,
            "a save looks again, and that answer stands"
        );
        assert_eq!(check_by(&file.0, stamp, Box::new(|_, _, _| unreachable!("known now"))), None);
    }

    /// Failure (review round 1): a save starts a second look while the window's is under way for the same version, a second worker, and outside the
    /// cap. It waits for that look and takes its answer; and a save's own look counts toward the cap while it runs.
    #[test]
    fn a_save_joins_a_look_under_way_and_its_own_counts_toward_the_cap() {
        let _lock = test_lock();
        let file = recto_testpdf::TempPdf::new("joined.pdf", &recto_testpdf::pages(&[recto_testpdf::Page::new("")]));
        let stamp = stamp_of(&file.0);
        let (go, wait) = mpsc::channel::<()>();
        let slow = Box::new(move |_: &Path, _, _| {
            let _ = wait.recv();
            Some(Disagreement::PageSize { page: 0 })
        });
        assert_eq!(known_or_start_by(&file.0, stamp, slow, Box::new(|| {})), Ask::Started);
        let saving = std::thread::spawn({
            let path = file.0.clone();
            move || check_by(&path, stamp, Box::new(|_, _, _| unreachable!("joined the window's look")))
        });
        std::thread::sleep(Duration::from_millis(50));
        go.send(()).unwrap();
        assert_eq!(saving.join().unwrap(), Some(Disagreement::PageSize { page: 0 }), "the window's answer");
        // A save's own look, under way, is a place taken.
        let other = FileStamp { size: stamp.size + 7, ..stamp };
        let (go, wait) = mpsc::channel::<()>();
        let saving = std::thread::spawn({
            let path = file.0.clone();
            move || {
                check_by(
                    &path,
                    other,
                    Box::new(move |_, _, _| {
                        let _ = wait.recv();
                        None
                    }),
                )
            }
        });
        wait_for("the save's look to start", || looks().looking.contains(&other));
        go.send(()).unwrap();
        assert_eq!(saving.join().unwrap(), None);
    }

    /// Failure: one slow disk holds every look, or looks pile up a worker each past any cap. At most [`LOOKS_AT_ONCE`] at once, one per version, and
    /// the window is told when each is known. Each ask says whether it started a look or found one busy (review round 2), and the tests on the
    /// process-wide looks hold one lock, so `cargo test`'s threads don't race them.
    #[test]
    fn looks_are_capped_and_say_when_they_are_done() {
        let _lock = test_lock();
        let file = recto_testpdf::TempPdf::new("capped.pdf", &recto_testpdf::pages(&[recto_testpdf::Page::new("")]));
        let base = stamp_of(&file.0);
        let stamp = |n: u64| FileStamp {
            inode: base.inode.wrapping_add(1_000 + n),
            ..base
        };
        let (told, heard) = mpsc::channel();
        let mut release = Vec::new();
        for n in 0..LOOKS_AT_ONCE as u64 {
            let (go, wait) = mpsc::channel::<()>();
            release.push(go);
            let told = told.clone();
            let look = Box::new(move |_: &Path, _, _| {
                let _ = wait.recv();
                Some(Disagreement::PageSize { page: 0 })
            });
            assert_eq!(
                known_or_start_by(&file.0, stamp(n), look, Box::new(move || told.send(n).unwrap())),
                Ask::Started
            );
        }
        assert_eq!(
            known_or_start_by(&file.0, stamp(99), Box::new(|_, _, _| unreachable!("past the cap")), Box::new(|| {})),
            Ask::Busy,
            "past the cap, busy"
        );
        assert_eq!(
            known_or_start_by(&file.0, stamp(0), Box::new(|_, _, _| unreachable!("under way")), Box::new(|| {})),
            Ask::Busy,
            "under way for that version, busy"
        );
        drop(release);
        for _ in 0..LOOKS_AT_ONCE {
            heard.recv_timeout(Duration::from_secs(10)).expect("told when it's known");
        }
        wait_for("the places to be free", || looks().looking.is_empty());
        assert_eq!(known(stamp(0)), Some(Some(Disagreement::PageSize { page: 0 })), "kept for the version asked");
    }

    /// Failure (review round 2, blocking): Save As of a tab only the renderer could read runs the gate before `begin_save`'s own refusal, starting
    /// a worker and PDFium's whole read, and answers "couldn't check, try again", which an agent retries for ever. It's refused as renderer-only,
    /// and no look runs.
    #[test]
    fn save_as_of_a_renderer_only_tab_says_so_and_starts_no_look() {
        let _lock = test_lock();
        let mut bytes = recto_testpdf::pages(&[recto_testpdf::Page::new("")]);
        // A trailer naming an `/Encrypt` that isn't there: lopdf refuses the file, PDFium reads it.
        let at = bytes.windows(7).rposition(|window| window == b"trailer").unwrap() + "trailer\n<< ".len();
        bytes.splice(at..at, b"/Encrypt 99 0 R ".iter().copied());
        let file = recto_testpdf::TempPdf::new("renderer-only.pdf", &bytes);
        let access = Mutex::new(recto_session::Session::new(crate::loader::DrawableLoader));
        let tab = access.lock().unwrap().open(&file.0).expect("PDFium reads it");
        assert!(access.lock().unwrap().tabs().tab(tab).unwrap().renderer_only);
        let out = file.0.with_file_name("copy.pdf");
        let before = LOOKS_RUN.load(std::sync::atomic::Ordering::SeqCst);
        let refused = crate::saving::save(
            &access,
            crate::Level::Full,
            &format!("#{}", tab.0),
            &crate::saving::SaveTo::As {
                out: out.clone(),
                replace: false,
            },
            &mut Vec::new(),
        )
        .expect_err("refused");
        assert_eq!(refused.code, "renderer_only", "{}", refused.message);
        assert_eq!(LOOKS_RUN.load(std::sync::atomic::Ordering::SeqCst), before, "no look, so no worker");
        assert!(!out.exists());
    }

    /// Failure (review round 2): a file that can't be looked at just now (a failed stat) skips the gate, and is saved unchecked; and so does an
    /// added PDF. It can't be checked, so the save is refused, saying to try again; a file that's simply another version is left to the save's own
    /// refusal, with no look spent on it.
    #[test]
    fn a_file_that_cant_be_looked_at_isnt_saved_unchecked() {
        let _lock = test_lock();
        let file = recto_testpdf::TempPdf::new("there.pdf", &recto_testpdf::pages(&[recto_testpdf::Page::new("")]));
        let stamp = stamp_of(&file.0);
        let gone = std::path::Path::new("/nowhere/at/all.pdf");
        let before = LOOKS_RUN.load(std::sync::atomic::Ordering::SeqCst);
        for added in [false, true] {
            let refused = refusal_among([(gone, Some(stamp), added)].into_iter()).expect("refused");
            assert_eq!(refused.code, "parsers_disagree", "{}", refused.message);
            assert!(refused.next.contains("again"), "{}", refused.next);
        }
        let other = FileStamp { size: stamp.size + 1, ..stamp };
        assert!(
            refusal_among([(file.0.as_path(), Some(other), false)].into_iter()).is_none(),
            "another version: the save's to refuse"
        );
        assert_eq!(LOOKS_RUN.load(std::sync::atomic::Ordering::SeqCst), before, "and no look was spent on either");
    }

    /// Failure (review round 1): a look has no overall deadline, so a slow disk or a worker that answers slowly holds a save for as long as it
    /// takes. Past the deadline it fails closed, as `Unchecked`, on a real file and the real renderer.
    #[test]
    fn a_look_past_its_deadline_fails_closed() {
        let _lock = test_lock();
        let pages: Vec<recto_testpdf::Page> = (0..50).map(|_| recto_testpdf::Page::new("")).collect();
        let file = recto_testpdf::TempPdf::new("deadline.pdf", &recto_testpdf::pages(&pages));
        let stamp = stamp_of(&file.0);
        assert_eq!(look_at(&file.0, stamp, Instant::now()), Some(Disagreement::Unchecked));
        assert_eq!(look_at(&file.0, stamp, Instant::now() + LOOK_DEADLINE), None, "and with time, the file agrees");
        // The version asked about isn't the one opened: unchecked, whatever's at the path.
        let other = FileStamp { size: stamp.size + 1, ..stamp };
        assert_eq!(look_at(&file.0, other, Instant::now() + LOOK_DEADLINE), Some(Disagreement::Unchecked));
    }
}
