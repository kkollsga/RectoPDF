//! **File > Print** in the window. The system's print panel chooses the printer, the copies and which pages; Rust then draws those pages through `recto_agent::print`, the same as `recto print --to-pdf`, into a file of the app's own on a thread of its own, saying how far it's got by the page; and the host hands that file to the system's printing, which never asks the app to draw anything on the UI thread. The file goes when the print's done with it.
//!
//! At rest there's nothing: no thread, worker, file or buffer. One print at a time; asking again while one's being prepared says so.

use crate::{Inner, lock};
use recto_agent::print::{self, PrintPage};
use recto_session::TabId;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const STACK: usize = 1024 * 1024;

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

#[derive(Default)]
pub(crate) struct Printing {
    running: AtomicBool,
    cancel: AtomicBool,
    done: AtomicU32,
    of: AtomicU32,
    generation: AtomicU64,
    ready: Mutex<Option<PathBuf>>,
    /// The host has the file and the system's printing is running.
    handing: AtomicBool,
    notice: Mutex<Option<String>>,
}

impl Printing {
    pub(crate) fn busy(&self) -> bool {
        self.running.load(Ordering::SeqCst) || lock(&self.ready).is_some() || self.handing.load(Ordering::SeqCst)
    }

    /// The line by the page while pages are being prepared, or what went wrong.
    pub(crate) fn notice(&self) -> Option<String> {
        if self.running.load(Ordering::SeqCst) {
            let (done, of) = (self.done.load(Ordering::SeqCst), self.of.load(Ordering::SeqCst));
            return Some(if of > 1 { format!("Preparing pages for printing: {} of {of}.", (done + 1).min(of)) } else { "Preparing the page for printing.".to_string() });
        }
        lock(&self.notice).clone()
    }

    pub(crate) fn dismiss_notice(&self) {
        lock(&self.notice).take();
    }

    /// The file to print, once, for the host to hand to the system.
    pub(crate) fn take_ready(&self) -> Option<PathBuf> {
        let ready = lock(&self.ready).take();
        if ready.is_some() {
            self.handing.store(true, Ordering::SeqCst);
        }
        ready
    }

    /// The system's printing is done with a file, however it went: it goes, if it's one of ours.
    pub(crate) fn finished(&self, path: &str) {
        finished(path);
        self.handing.store(false, Ordering::SeqCst);
    }

    pub(crate) fn has_ready(&self) -> bool {
        lock(&self.ready).is_some()
    }

    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// Where a print's file goes: the system's temp folder, a name of this process's own, made new.
fn temp_path(generation: u64) -> PathBuf {
    std::env::temp_dir().join(format!("recto-print-{}-{generation}.pdf", std::process::id()))
}

/// Which pages the panel's answer means. `first` and `last` count from 1, and `all` or a selection's pages override them. None if there's nothing to print.
pub(crate) fn chosen_range(pages: u32, all: bool, first: u32, last: u32, selection: Option<(u32, u32)>) -> Option<(u32, u32)> {
    if pages == 0 {
        return None;
    }
    let (from, to) = match selection {
        Some((start, end)) => (start + 1, end + 1),
        None if all => (1, pages),
        None => (first.max(1), last.max(first.max(1))),
    };
    (from <= pages).then(|| (from, to.min(pages)))
}

/// Starts preparing a print of `range`, pages counted from 1 as the tab has them now, off the UI thread.
pub(crate) fn start(inner: &Arc<Inner>, tab: TabId, range: (u32, u32), dpi: u32) -> bool {
    let printing = &inner.printing;
    if printing.running.swap(true, Ordering::SeqCst) {
        *lock(&printing.notice) = Some("A print is already being prepared.".into());
        return false;
    }
    printing.cancel.store(false, Ordering::SeqCst);
    printing.done.store(0, Ordering::SeqCst);
    printing.of.store(range.1 - range.0 + 1, Ordering::SeqCst);
    lock(&printing.notice).take();
    let generation = printing.generation.fetch_add(1, Ordering::SeqCst) + 1;
    inner.host.running.fetch_add(1, Ordering::SeqCst);
    inner.core.push(crate::CoreCall::Reconcile);
    let runner = Arc::clone(inner);
    let spawned = std::thread::Builder::new().name("recto-print".into()).stack_size(STACK).spawn(move || {
        let printing = &runner.printing;
        let pages = format!("{}-{}", range.0, range.1);
        let listed: Result<Vec<PrintPage>, recto_agent::AgentError> = runner.host.session.with_session(|session| recto_agent::print_pages_in(session, &format!("#{}", tab.0), Some(&pages)));
        let path = temp_path(generation);
        let outcome = listed.and_then(|listed| {
            print::print_to_path(&listed, dpi, &path, |done, of| {
                printing.done.store(done, Ordering::SeqCst);
                printing.of.store(of, Ordering::SeqCst);
                runner.core.push(crate::CoreCall::Reconcile);
                !printing.cancel.load(Ordering::SeqCst)
            })
        });
        match outcome {
            Ok(_) => *lock(&printing.ready) = Some(path),
            Err(error) => *lock(&printing.notice) = Some(error.message),
        }
        printing.running.store(false, Ordering::SeqCst);
        runner.host.running.fetch_sub(1, Ordering::SeqCst);
        runner.core.push(crate::CoreCall::Reconcile);
        runner.listener.model_changed();
    });
    if spawned.is_err() {
        printing.running.store(false, Ordering::SeqCst);
        inner.host.running.fetch_sub(1, Ordering::SeqCst);
        *lock(&printing.notice) = Some("The print couldn't be started.".into());
        return false;
    }
    true
}

/// The system's printing has finished with a file: it goes. Only one of ours.
pub(crate) fn finished(path: &str) {
    let path = std::path::Path::new(path);
    let ours = path.parent() == Some(std::env::temp_dir().as_path()) && path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(&format!("recto-print-{}-", std::process::id())));
    if ours {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the panel's answer prints the wrong pages: a range past the end, a range backwards, a selection's pages counted from 0 as the page view holds them, or All ignored.
    #[test]
    fn the_panels_answer_means_the_pages_it_says() {
        assert_eq!(chosen_range(10, true, 3, 4, None), Some((1, 10)));
        assert_eq!(chosen_range(10, false, 3, 4, None), Some((3, 4)));
        assert_eq!(chosen_range(10, false, 8, 40, None), Some((8, 10)), "held to the end");
        assert_eq!(chosen_range(10, false, 5, 2, None), Some((5, 5)), "backwards is the first");
        assert_eq!(chosen_range(10, false, 11, 12, None), None, "past the end is nothing");
        assert_eq!(chosen_range(10, true, 1, 1, Some((2, 4))), Some((3, 5)), "a selection's pages, from 1");
        assert_eq!(chosen_range(0, true, 1, 1, None), None);
    }

    /// Failure: finishing a print removes a file that isn't one of this app's prints.
    #[test]
    fn only_a_print_file_of_ours_is_removed() {
        let elsewhere = std::env::temp_dir().join(format!("not-a-print-{}.pdf", std::process::id()));
        std::fs::write(&elsewhere, b"keep").unwrap();
        finished(elsewhere.to_str().unwrap());
        assert!(elsewhere.exists());
        std::fs::remove_file(&elsewhere).unwrap();
        let ours = temp_path(9_999_999);
        std::fs::write(&ours, b"print").unwrap();
        finished(ours.to_str().unwrap());
        assert!(!ours.exists());
    }
}
