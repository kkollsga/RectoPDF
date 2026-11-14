//! **The file's highlights a person has taken out and not saved, left out of what's drawn** (review of R4.3, S2). Never a write: each one the set
//! names is checked against the document (a place in the page's `/Annots` past its count, another subtype, or a `/Rect` further than half a point
//! from the set's is drawn as the file has it), and on a match the annotation's flags in PDFium's copy of the document in memory get
//! `FPDF_ANNOT_FLAG_HIDDEN`, which drawing honours. No call here or anywhere in this crate saves a document (`no_path_writes_a_document`).
//!
//! **What it costs.** A document with no set holds an empty one and does nothing. A set is applied to a page as it's loaded to be drawn, touching
//! only its listed places, so a page of thousands of annotations costs its listed ones; and a new set first puts back what the old one changed, a
//! load of each page it hid something on.

use super::{FPDF_ANNOTATION, FPDF_PAGE, FS_RECTF, PdfiumBackend};
use recto_proto::hidden::{HiddenAnnots, HiddenTaken, same_rect};
use std::collections::HashMap;
use std::ffi::c_int;

/// `fpdf_annot.h`: a highlight.
const FPDF_ANNOT_HIGHLIGHT: c_int = 9;
/// `fpdf_annot.h`: an annotation drawing leaves out.
const FPDF_ANNOT_FLAG_HIDDEN: c_int = 1 << 1;

/// What a set has done and has yet to do to the document in memory.
#[derive(Default)]
pub(super) struct Held {
    generation: u64,
    /// Each page's entries not yet looked at: applied when the page is loaded.
    waiting: HashMap<u32, Vec<(u32, [f32; 4])>>,
    /// Each annotation hidden, by page and place, with the flags it had.
    changed: Vec<(u32, u32, c_int)>,
}

impl PdfiumBackend {
    /// **Takes a document's whole set in place of the last:** what the last one hid is shown again, and the new one is applied as its pages are
    /// loaded. Says the generation held and how many of its entries are for pages the document hasn't.
    pub fn hide_annots(&mut self, set: &HiddenAnnots) -> HiddenTaken {
        // The page open now was loaded under the old set.
        self.close_page();
        let changed = std::mem::take(&mut self.hidden.changed);
        let mut by_page: HashMap<u32, Vec<(u32, c_int)>> = HashMap::new();
        // Last first (second review of R4.3, SF2): one annotation a page lists twice is hidden at each place, and what was recorded at the second
        // already had the hidden bit the first set, so only the first record's flags are the ones it had.
        for (page, index, flags) in changed.into_iter().rev() {
            by_page.entry(page).or_default().push((index, flags));
        }
        for (page, back) in by_page {
            self.with_annots_of(page, |api, handle| {
                for (index, flags) in &back {
                    // SAFETY: `handle` is a live page of this document; an annotation got from it is closed before the page.
                    unsafe {
                        let annot = (api.FPDFPage_GetAnnot)(handle, *index as c_int);
                        if !annot.is_null() {
                            (api.FPDFAnnot_SetFlags)(annot, *flags);
                            (api.FPDFPage_CloseAnnot)(annot);
                        }
                    }
                }
            });
        }
        let page_count = self.page_count;
        let unmatched: usize = set.pages.iter().filter(|(page, _)| *page >= page_count).map(|(_, entries)| entries.len()).sum();
        self.hidden = Held {
            generation: set.generation,
            waiting: set.pages.iter().filter(|(page, _)| *page < page_count).cloned().collect(),
            changed: Vec::new(),
        };
        HiddenTaken {
            generation: set.generation,
            unmatched: u32::try_from(unmatched).unwrap_or(u32::MAX),
        }
    }

    /// The set's entries for a page just loaded: each that is what it says is hidden. Called once a page a set, as flags set stay with the document.
    pub(super) fn hide_on(&mut self, index: u32, page: FPDF_PAGE) {
        let Some(entries) = self.hidden.waiting.remove(&index) else { return };
        let api = self.api;
        // SAFETY: `page` is the live page just loaded from this document.
        let count = unsafe { (api.FPDFPage_GetAnnotCount)(page) }.max(0);
        for (place, rect) in entries {
            let Ok(at) = c_int::try_from(place) else { continue };
            if at >= count {
                continue;
            }
            // SAFETY: `at` is within the page's count; the annotation is closed before the page is.
            unsafe {
                let annot = (api.FPDFPage_GetAnnot)(page, at);
                if annot.is_null() {
                    continue;
                }
                if is_the_one(api, annot, rect) {
                    let flags = (api.FPDFAnnot_GetFlags)(annot);
                    if (api.FPDFAnnot_SetFlags)(annot, flags | FPDF_ANNOT_FLAG_HIDDEN) != 0 {
                        self.hidden.changed.push((index, place, flags));
                    }
                }
                (api.FPDFPage_CloseAnnot)(annot);
            }
        }
    }

    /// The generation of the set held, and how many highlights it has hidden so far, for tests and QA.
    pub fn hidden_now(&self) -> (u64, usize) {
        (self.hidden.generation, self.hidden.changed.len())
    }

    /// Loads a page on its own, for putting flags back, and closes it.
    fn with_annots_of(&mut self, page: u32, work: impl FnOnce(&'static super::Api, FPDF_PAGE)) {
        if page >= self.page_count {
            return;
        }
        // SAFETY: the document is live and `page` is in range; the page is closed before this returns, and no other page is open (`close_page`).
        unsafe {
            let handle = (self.api.FPDF_LoadPage)(self.document, page as c_int);
            if handle.is_null() {
                return;
            }
            work(self.api, handle);
            (self.api.FPDF_ClosePage)(handle);
        }
    }
}

/// A highlight whose `/Rect` is the set's.
///
/// # Safety
/// `annot` is a live annotation.
unsafe fn is_the_one(api: &super::Api, annot: FPDF_ANNOTATION, said: [f32; 4]) -> bool {
    // SAFETY: the caller's.
    unsafe {
        if (api.FPDFAnnot_GetSubtype)(annot) != FPDF_ANNOT_HIGHLIGHT {
            return false;
        }
        let mut rect = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        (api.FPDFAnnot_GetRect)(annot, &raw mut rect) != 0 && same_rect([rect.left, rect.bottom, rect.right, rect.top], said)
    }
}

#[cfg(test)]
mod tests {
    /// Failure (review of R4.3, S2, rule 5): something in the crate that draws can write a document, so a person's unsaved removal, done to
    /// PDFium's copy in memory, could reach the file. No call that saves or writes a document is bound, named or reachable here.
    #[test]
    fn no_path_writes_a_document() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![dir];
        let mut read = 0;
        while let Some(at) = stack.pop() {
            for entry in std::fs::read_dir(&at).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    read += 1;
                    for writes in ["FPDF_SaveAsCopy", "FPDF_SaveWithVersion", "FPDF_IncrementalSave", "FPDF_FILEWRITE"] {
                        let named = text.lines().filter(|line| line.contains(writes) && !line.contains("for writes in [")).count();
                        assert_eq!(named, 0, "{writes} in {}", path.display());
                    }
                }
            }
        }
        assert!(read > 3, "the crate's sources were read");
    }
}
