//! **Printing's calls:** the print panel's model, and the system's printing's file. The panel's answer is the person's action ([`crate::controls::UiAction::Print`]),
//! which sends the `print` command as theirs; the rules and the state are `recto_app::core::printing`'s.

use crate::{AppCore, CoreCall, lock};
pub use recto_app::core::printing::{PrintModel, ReadyPrint};
use std::path::PathBuf;

/// What the print panel needs to know before it opens, all Rust's to say.
#[uniffi::remote(Record)]
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

/// A print's file for the host, with the host's number for the answer it was asked by.
#[uniffi::remote(Record)]
pub struct ReadyPrint {
    pub path: String,
    pub serial: u64,
}

#[uniffi::export]
impl AppCore {
    /// What the print panel needs to know before it opens: the tab showing, its pages, whether something's selected, and its first page's size. Never waits: what's published, and short locks on the page view.
    pub fn print_model(&self) -> PrintModel {
        let reader = lock(&self.inner.published).reader.clone();
        recto_app::core::printing::model(reader, &self.inner.panes.reading_engine(&self.inner.engine))
    }

    /// For QA's print to a PDF through the system's printing: where it should write, a new name of ours beside `out`, which is published at `out` when it's done, never over anything. None if `out` can't be written to.
    pub fn print_qa_target(&self, out: String) -> Option<String> {
        self.inner
            .tools
            .printing()
            .qa_target_for(PathBuf::from(out))
            .map(|written| written.to_string_lossy().into_owned())
    }

    /// The file a print's pages are in, once, for the host to hand to the system's printing, with the number of the answer it was asked by.
    pub fn take_print(&self) -> Option<ReadyPrint> {
        let ready = self.inner.tools.printing().take_ready().map(ReadyPrint::from);
        self.inner.core.push(CoreCall::Reconcile);
        ready
    }

    /// Queue the exact host completion without IO on this thread. A failed/cancelled operation only cleans its staging file; success may publish it.
    pub fn print_finished(&self, path: String, serial: u64, success: bool) {
        recto_app::core::printing::completion::request(self.inner.tools.printing(), &self.inner.core, &path, serial, success);
    }
}

#[cfg(test)]
mod tests;
