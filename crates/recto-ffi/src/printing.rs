//! **Printing's calls:** the print panel's model, its answer, and the system's printing's file. The rules and the state are `recto_app::core::printing`'s.

use crate::{AppCore, CoreCall, lock};
use recto_agent::print;
pub use recto_app::core::printing::PrintModel;
use recto_app::core::printing::{Request, chosen_range, start};
use recto_session::TabId;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

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

#[uniffi::export]
impl AppCore {
    /// What the print panel needs to know before it opens: the tab showing, its pages, whether something's selected, and its first page's size. Never waits: what's published, and short locks on the page view.
    pub fn print_model(&self) -> PrintModel {
        let reader = lock(&self.inner.published).reader.clone();
        let engine = self.inner.panes.reading_engine(&self.inner.engine);
        let (width_pt, height_pt) = engine
            .page_size_shown(0)
            .map_or((612.0, 792.0), |(width, height)| (f64::from(width), f64::from(height)));
        PrintModel {
            tab: reader.shown_tab,
            pages: engine.page_count(),
            has_selection: engine.has_selection(),
            job_title: reader.document_name,
            width_pt,
            height_pt,
        }
    }

    /// The print panel was answered with Print: these pages, from 1, or all of them, or the selection's, to paper or, with `to_file`, to a PDF the system's printing saves or previews, which is an export. Rust draws them off the UI thread, and `print_ready` says when the file's there. False if nothing was started, with a notice saying why.
    pub fn print_chosen(&self, tab: u32, all: bool, first: u32, last: u32, selection_only: bool, to_file: bool) -> bool {
        if tab == 0 || self.inner.core.frozen.load(Ordering::SeqCst) {
            return false;
        }
        let model = self.print_model();
        let selection = selection_only
            .then(|| self.inner.panes.reading_engine(&self.inner.engine).hot_selection())
            .flatten()
            .and_then(|(_, range)| range)
            .map(|range| (range.start.page, range.end.page));
        let Some(range) = chosen_range(model.pages, all, first, last, selection) else {
            return false;
        };
        start(
            &self.inner,
            Request {
                tab: TabId(tab),
                range,
                dpi: print::DEFAULT_DPI,
                to_file,
                anyway: false,
            },
        )
    }

    /// For QA's print to a PDF through the system's printing: where it should write, a new name of ours beside `out`, which is published at `out` when it's done, never over anything. None if `out` can't be written to.
    pub fn print_qa_target(&self, out: String) -> Option<String> {
        let out = PathBuf::from(out);
        let folder = out.parent()?;
        if !out.is_absolute() || std::fs::symlink_metadata(&out).is_ok() {
            return None;
        }
        let written = folder.join(format!(
            ".recto-print-{}-{}.pdf",
            std::process::id(),
            self.inner.tools.printing.generation.load(Ordering::SeqCst) + 1
        ));
        if std::fs::symlink_metadata(&written).is_ok() {
            return None;
        }
        *lock(&self.inner.tools.printing.qa_target) = Some((written.clone(), out));
        Some(written.to_string_lossy().into_owned())
    }

    /// The file a print's pages are in, once, for the host to hand to the system's printing.
    pub fn take_print(&self) -> Option<String> {
        let path = self.inner.tools.printing.take_ready().map(|path| path.to_string_lossy().into_owned());
        self.inner.core.push(CoreCall::Reconcile);
        path
    }

    /// The system's printing is done with a file, however it went: it goes.
    pub fn print_finished(&self, path: String) {
        self.inner.tools.printing.finished(&path);
        self.inner.core.push(CoreCall::Reconcile);
    }

    /// Stops a print being prepared, at the next page: the notice's Stop.
    pub fn print_cancel(&self) {
        self.inner.tools.printing.cancel();
    }
}
