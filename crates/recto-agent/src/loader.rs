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
