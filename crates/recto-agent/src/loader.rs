//! How the session reads a document's facts: the structure reader first, then the renderer for a file only it can read.
//!
//! lopdf refuses some damaged files that PDFium repairs and draws, and a reader that can't show them is worse than every other reader. So the tab opens, with the page count PDFium reports and
//! `renderer_only` set. The rule that a tab's facts stay trustworthy holds, because nothing is made up: no title, no contents and no signature report, each saying it couldn't be read rather than
//! that there's none. Editing is refused, since only the structure reader's writer may ever change a PDF.

use recto_core::{DocFacts, OpenError, Toc};
use recto_render_service::{DocRenderer, Policy, RenderFailure, ServiceConfig};
use recto_session::DocLoader;
use std::path::Path;

/// More pages than this from a worker is a lie or a bomb. The worker isn't trusted.
const MAX_RENDERER_PAGES: u32 = 2_000_000;

/// Reads through lopdf, then PDFium in its worker. A password-protected file, a missing one or one nobody may read stays that way: only "not a PDF",
/// "past the structure reader's limits" and "no pages" get the second opinion. A file past the limits opens to be read, with Save off, as the two
/// readers disagree about it (the coordinator's decision, review round 1). Each worker that opens such a file pays PDFium's own cost of reading
/// it: a chain of 100,000 `/Prev` sections took 3.4 s to load, which is accepted.
#[derive(Clone, Copy, Debug, Default)]
pub struct DrawableLoader;

impl DocLoader for DrawableLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        match recto_core::inspect(path) {
            Err(OpenError::NotPdf) => renderer_facts(path),
            // What the structure reader refused at a limit says so if the renderer can't read it either.
            Err(OpenError::TooComplex) => renderer_facts(path).map_err(|failure| match failure {
                OpenError::NotPdf => OpenError::TooComplex,
                other => other,
            }),
            // lopdf can get through a broken page tree and find nothing, where PDFium finds the pages.
            Ok(facts) if facts.page_count == 0 => renderer_facts(path).or(Ok(facts)),
            other => other,
        }
    }
}

fn renderer_facts(path: &Path) -> Result<DocFacts, OpenError> {
    let meta = std::fs::metadata(path).map_err(|_| OpenError::NotPdf)?;
    if !meta.is_file() {
        return Err(OpenError::NotPdf);
    }
    let config = ServiceConfig::discover().map_err(|_| OpenError::NotPdf)?;
    let mut doc = DocRenderer::new(path, None, config, Policy::default()).map_err(|_| OpenError::NotPdf)?;
    let page_count = match doc.page_count() {
        Ok(count) if count > 0 && count <= MAX_RENDERER_PAGES => count,
        Err(RenderFailure::NeedsPassword) => return Err(OpenError::PasswordNeeded),
        _ => return Err(OpenError::NotPdf),
    };
    Ok(DocFacts {
        page_count,
        byte_len: meta.len(),
        encrypted: false,
        version: "".into(),
        title: None,
        toc: Toc::default(),
        renderer_only: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file of one page whose cross-reference data is `sections` sections long, each naming the one before: a chain past the structure reader's
    /// cap of 1,024, which PDFium follows however long it is.
    fn long_prev_chain(sections: usize) -> Vec<u8> {
        let mut out = recto_testpdf::pages(&[recto_testpdf::Page::new("")]);
        let tail = out.windows(9).rposition(|window| window == b"startxref").unwrap();
        let mut prev: usize = std::str::from_utf8(&out[tail + 10..]).unwrap().lines().next().unwrap().trim().parse().unwrap();
        out.truncate(tail);
        for _ in 0..sections {
            let here = out.len();
            out.extend(format!("xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 6 /Root 1 0 R /Prev {prev} >>\n").as_bytes());
            prev = here;
        }
        out.extend(format!("startxref\n{prev}\n%%EOF\n").as_bytes());
        out
    }

    /// Failure (PLAN, Save's robustness, and the coordinator's decision in review round 1): a file the structure reader refuses at one of its
    /// limits, a chain of `/Prev` sections past its cap, either can't be read at all or can be edited and saved from a reading that isn't what's
    /// shown. It opens to be read, by PDFium, in bounded time, as only the renderer's: edits are refused, so there's nothing to save. A file
    /// neither can read is still "not a PDF", and one past the limits that PDFium can't read either says it's too complex.
    #[test]
    fn a_file_refused_at_a_limit_opens_to_be_read_with_edits_refused() {
        let file = recto_testpdf::TempPdf::new("prev-chain.pdf", &long_prev_chain(2_000));
        assert_eq!(
            recto_core::inspect(&file.0).err(),
            Some(OpenError::TooComplex),
            "the fork refuses it at its cap"
        );
        let started = std::time::Instant::now();
        let facts = DrawableLoader.load(&file.0).expect("PDFium reads it");
        let took = started.elapsed();
        assert!((facts.renderer_only, facts.page_count) == (true, 1), "{facts:?}");
        // A worker's start and PDFium's walk of 2,000 sections: generous for a loaded machine.
        assert!(took < std::time::Duration::from_secs(20), "{took:?}");
        let mut session = recto_session::Session::new(DrawableLoader);
        let tab = session.open(&file.0).unwrap();
        assert!(!session.can_edit_pages(tab), "only the renderer's: no edits");
        assert!(
            session.edit_pages_as(tab, recto_session::Author::Owner, |edits| edits.delete(0, 1)).is_err(),
            "an edit is refused, so Save has nothing to write"
        );
        let garbage = recto_testpdf::TempPdf::new("garbage.pdf", b"not a pdf at all");
        assert_eq!(
            DrawableLoader.load(&garbage.0).err(),
            Some(OpenError::NotPdf),
            "other refusals are still NotPdf"
        );
    }
}
