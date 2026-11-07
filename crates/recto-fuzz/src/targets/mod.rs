//! One check per thing that takes hostile input. Each is a plain function on bytes, so libFuzzer, the seeded smoke run and a regression test all run the very same thing.

pub mod agent;
pub mod control;
pub mod edits;
pub mod generated;
pub mod index;
pub mod pdf;
pub mod proto;
pub mod record;
pub mod select;

use recto_core::{DocFacts, OpenError, Toc};
use recto_session::DocLoader;
use std::path::Path;
use std::sync::Arc;

/// A loader with one document's facts, whatever it's asked for. So no path in a fuzz input is ever opened.
#[derive(Clone)]
pub struct DocOnly(pub Arc<DocFacts>);

impl DocLoader for DocOnly {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        Ok((*self.0).clone())
    }
}

/// A loader that goes by the name it's given, and touches no disk: `/fuzz/p12.pdf` has 12 pages, and anything else isn't there. So what a hostile path can do is limited to what this says, with a real session behind it.
#[derive(Clone, Copy, Debug, Default)]
pub struct NamedLoader;

/// The facts of a document of so many pages.
pub fn facts_of(page_count: u32) -> DocFacts {
    DocFacts { page_count, byte_len: 1024, encrypted: false, version: "1.7".into(), title: Some("A title".into()), toc: Toc::default(), renderer_only: false }
}

impl DocLoader for NamedLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        let pages = path.to_str().and_then(|path| path.strip_prefix("/fuzz/p")).and_then(|rest| rest.strip_suffix(".pdf")).and_then(|count| count.parse::<u32>().ok());
        match pages {
            Some(count) if count > 0 && count <= recto_session::edit::MAX_PAGES as u32 => Ok(facts_of(count)),
            _ => Err(OpenError::NotFound),
        }
    }
}
