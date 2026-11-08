//! **The agreement gate's look,** for every way of saving (SPEC section 3: "when the two parsers disagree about a file, it stays readable but saving
//! and signing turn off"). `recto_core::agreement` has the rule, the structure reader's half and the words; this asks the renderer's half of a worker
//! of its own, through the very file the structure is read from, so both read the same bytes.
//!
//! A look costs a worker's start and a size per page, up to `MAX_COMPARED_PAGES`, so it's made **once per version of a file**, off the UI thread, and
//! kept: [`check`] is what a save asks, on its own thread, and [`known_or_start`] is what the window asks, which starts the look on a thread of its own
//! and calls back when it's known. At most [`LOOKS_AT_ONCE`] run at once, and [`KEPT`] answers are kept, so nothing grows with the files opened.

use recto_core::agreement::{Disagreement, View, compare, compared_pages, structure_view};
use recto_platform::FileStamp;
use recto_render_service::{DocRenderer, Policy, RenderFailure, ServiceConfig};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How many looks run at once. Each holds a worker while it runs.
pub const LOOKS_AT_ONCE: usize = 2;
/// How many answers are kept, newest last.
pub const KEPT: usize = 16;
/// Pages the renderer may fail to size before the look stops and counts the file as one it couldn't read: each failure costs a worker.
const PAGE_FAILURES: u32 = 8;

/// **What the two readers make of the file at `path`,** compared: `None` when they agree, or when there's nothing for the gate to keep from a save
/// (a file that needs a password, or one the structure reader can't read at all, both of which the save refuses on its own). Blocks: never on the UI thread.
pub fn look(path: &Path) -> Option<Disagreement> {
    let Ok(config) = ServiceConfig::discover() else {
        return Some(Disagreement::RendererFailed);
    };
    let Ok(mut doc) = DocRenderer::new(path, None, config, Policy::default()) else {
        return Some(Disagreement::RendererFailed);
    };
    let answer = look_with(&mut doc);
    doc.kill_worker();
    answer
}

fn look_with(doc: &mut DocRenderer) -> Option<Disagreement> {
    let structure = match doc.read_structure(|file, _| structure_view(file, None)) {
        Some(Ok(view)) => view,
        // Protected, or not readable by the structure reader: Save refuses both itself, and a tab only the renderer can read can't be edited.
        Some(Err(_)) => return None,
        None => return Some(Disagreement::RendererFailed),
    };
    let renderer = match renderer_view(doc) {
        Ok(view) => view,
        Err(RenderFailure::NeedsPassword) => return None,
        Err(_) => return Some(Disagreement::RendererFailed),
    };
    compare(&structure, &renderer)
}

/// The renderer's half: its page count, whether it rebuilt the file's index, and the sizes of the pages compared. A page it can't size is `None`, up
/// to [`PAGE_FAILURES`] of them; a failure of the whole file is the answer.
fn renderer_view(doc: &mut DocRenderer) -> Result<View, RenderFailure> {
    let page_count = doc.page_count()?;
    let repaired = doc.repaired()?;
    let mut failures = 0;
    let mut sizes = Vec::new();
    for page in compared_pages(page_count) {
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

/// **The gate for a save,** of the file and of every PDF its pages were added from, as a copied page is written from lopdf's reading of its file too.
/// A refusal names which file and what differs; nothing's been written. A file with no stamp is refused by the save itself.
pub fn save_refusal(snapshot: &recto_session::SaveSnapshot) -> Option<crate::AgentError> {
    let own = snapshot.stamp.map(|stamp| (&*snapshot.path, stamp, false));
    let added = snapshot.added.iter().filter_map(|added| Some((&*added.path, added.stamp?, true)));
    own.into_iter().chain(added).find_map(|(path, stamp, added)| {
        let disagreement = check(path, stamp)?;
        let message = if added {
            format!(
                "Pages were added from “{}”, which is damaged in a way RectoPDF's two readers see differently: {}. So nothing was saved, as what's written might not be what you see. Nothing was written, and the edits are still here.",
                crate::file_label(path),
                disagreement.detail()
            )
        } else {
            format!("{} Nothing was written, and the edits are still here.", disagreement.message())
        };
        Some(crate::AgentError::new("parsers_disagree", message, crate::exit::FAILED).then(
            "Tell the user. The file can still be read; to save changes to it, it first needs repairing in another tool. The edits stay unsaved.",
        ))
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

fn looks() -> std::sync::MutexGuard<'static, Looks> {
    LOOKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keeps the answer for a version, in place of any older one for it, dropping the oldest past [`KEPT`].
fn remember(stamp: FileStamp, answer: Option<Disagreement>) {
    let mut looks = looks();
    looks.known.retain(|(known, _)| *known != stamp);
    looks.known.push((stamp, answer));
    if looks.known.len() > KEPT {
        looks.known.remove(0);
    }
}

fn known(stamp: FileStamp) -> Option<Option<Disagreement>> {
    looks().known.iter().find(|(known, _)| *known == stamp).map(|(_, answer)| answer.clone())
}

/// Ends a look however its thread does: its place is given back, a look that panicked counts as one that couldn't read the file, as the save
/// refuses what it can't check, so it's not started again at every update of the window, and the window is told either way.
struct LookEnds {
    stamp: FileStamp,
    done: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for LookEnds {
    fn drop(&mut self) {
        if std::thread::panicking() {
            remember(self.stamp, Some(Disagreement::RendererFailed));
        }
        looks().looking.retain(|stamp| *stamp != self.stamp);
        if let Some(done) = self.done.take() {
            done();
        }
    }
}

/// **What a save asks,** on its own thread: the answer for this version of the file, looked at now if it isn't known. The answer is kept only if the
/// file was still that version when the look ended; either way it's the answer for this save, which checks the version again before it writes.
pub fn check(path: &Path, stamp: FileStamp) -> Option<Disagreement> {
    check_by(path, stamp, look)
}

fn check_by(path: &Path, stamp: FileStamp, look: impl FnOnce(&Path) -> Option<Disagreement>) -> Option<Disagreement> {
    if let Some(answer) = known(stamp) {
        return answer;
    }
    let answer = look(path);
    if recto_platform::file_stamp(path).ok() == Some(stamp) {
        remember(stamp, answer.clone());
    }
    answer
}

/// **What the window asks:** the answer for this version if it's known; if not, `None`, and the look starts on a thread of its own, calling `done`
/// once it's known. One look per version, and at most [`LOOKS_AT_ONCE`] at once: past that, `None` until one ends.
pub fn known_or_start(path: &Path, stamp: FileStamp, done: impl FnOnce() + Send + 'static) -> Option<Option<Disagreement>> {
    known_or_start_by(path, stamp, look, done)
}

fn known_or_start_by(
    path: &Path,
    stamp: FileStamp,
    look: impl FnOnce(&Path) -> Option<Disagreement> + Send + 'static,
    done: impl FnOnce() + Send + 'static,
) -> Option<Option<Disagreement>> {
    let mut held = looks();
    if let Some((_, answer)) = held.known.iter().find(|(known, _)| *known == stamp) {
        return Some(answer.clone());
    }
    if held.looking.contains(&stamp) || held.looking.len() >= LOOKS_AT_ONCE {
        return None;
    }
    held.looking.push(stamp);
    drop(held);
    let path: PathBuf = path.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("recto-agreement".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            let _ends = LookEnds {
                stamp,
                done: Some(Box::new(done)),
            };
            check_by(&path, stamp, look);
        });
    if spawned.is_err() {
        looks().looking.retain(|looking| *looking != stamp);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn stamp_of(path: &Path) -> FileStamp {
        recto_platform::file_stamp(path).unwrap()
    }

    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < Duration::from_secs(10), "waited 10 s for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Failure: the gate looks at a file again at every update of the window or every save, starting a worker each time; or a look that panics holds
    /// its place for ever and is never answered. Once per version: a second ask is answered from what's kept, a new version is looked at again, and a
    /// look that panics ends, counting as a file it couldn't read.
    #[test]
    fn a_version_is_looked_at_once_and_a_look_ends_however_it_ends() {
        let file = recto_testpdf::TempPdf::new("once.pdf", &recto_testpdf::pages(&[recto_testpdf::Page::new("")]));
        let stamp = stamp_of(&file.0);
        assert_eq!(check_by(&file.0, stamp, |_| None), None);
        assert_eq!(check_by(&file.0, stamp, |_| unreachable!("known already")), None);
        // Another version of the file: its own look.
        let other = FileStamp { size: stamp.size + 1, ..stamp };
        assert_eq!(known_or_start_by(&file.0, other, |_| panic!("a hostile file"), || {}), None);
        wait_for("the look to end", || looks().looking.is_empty());
        assert_eq!(
            known(other),
            Some(Some(Disagreement::RendererFailed)),
            "a look that panicked can't vouch for the file"
        );
        assert_eq!(
            known_or_start_by(&file.0, stamp, |_| unreachable!("known already"), || {}),
            Some(None),
            "and the first version is still known"
        );
    }

    /// Failure: one slow disk holds every look, or looks pile up a worker each past any cap. At most [`LOOKS_AT_ONCE`] at once, one per version, and
    /// the window is told when each is known.
    #[test]
    fn looks_are_capped_and_say_when_they_are_done() {
        use std::sync::mpsc;
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
            let look = move |_: &Path| {
                let _ = wait.recv();
                Some(Disagreement::PageSize { page: 0 })
            };
            assert_eq!(known_or_start_by(&file.0, stamp(n), look, move || told.send(n).unwrap()), None);
        }
        assert_eq!(known_or_start_by(&file.0, stamp(99), |_| unreachable!("past the cap"), || {}), None);
        assert_eq!(known_or_start_by(&file.0, stamp(0), |_| unreachable!("under way"), || {}), None);
        drop(release);
        for _ in 0..LOOKS_AT_ONCE {
            heard.recv_timeout(Duration::from_secs(10)).expect("told when it's known");
        }
        wait_for("the places to be free", || looks().looking.is_empty());
        // A stamp that isn't the file's now isn't kept: the file isn't that version, so the answer may be about another.
        assert_eq!(known(stamp(0)), None);
    }
}
