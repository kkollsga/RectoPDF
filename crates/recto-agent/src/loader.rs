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

/// Reads through lopdf, then PDFium in its worker. A password-protected file, a missing one or one nobody may read stays that way: only "not a PDF" and "no pages" get the second opinion.
#[derive(Clone, Copy, Debug, Default)]
pub struct DrawableLoader;

impl DocLoader for DrawableLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        match recto_core::inspect(path) {
            Err(OpenError::NotPdf) => renderer_facts(path),
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

    /// Failure (PLAN, Save's robustness): a file the structure reader refuses at one of its limits falls back to PDFium, which follows a chain of
    /// 100,000 `/Prev` sections for 3.4 s where the fork refused it in 3 ms, and the tab then opens as only the renderer's. It's refused, as too
    /// complex, with no worker asked; a file lopdf can't read for other reasons still gets PDFium's opinion.
    #[test]
    fn a_file_refused_at_a_limit_isnt_handed_to_the_renderer() {
        let file = recto_testpdf::TempPdf::new("prev-chain.pdf", &long_prev_chain(2_000));
        assert_eq!(
            recto_core::inspect(&file.0).err(),
            Some(OpenError::TooComplex),
            "the fork refuses it at its cap"
        );
        let started = std::time::Instant::now();
        assert_eq!(DrawableLoader.load(&file.0).err(), Some(OpenError::TooComplex));
        // Bounded by the fork's cap, not by a worker's start or PDFium's walk: generous for a loaded machine, far under a worker's deadline.
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
        let garbage = recto_testpdf::TempPdf::new("garbage.pdf", b"not a pdf at all");
        assert_eq!(
            recto_core::inspect(&garbage.0).err(),
            Some(OpenError::NotPdf),
            "other refusals are still NotPdf"
        );
    }
}
