//! The library (`Store::Library`): the `.recto` folder a library keeps beside its PDFs.
//!
//! **No build writes one yet** (`docs/PLAN.md` 8.5; owner: it lives at the library root, beside the PDFs), so there is nothing to find and the store is
//! complete and empty. This stub is replaced by 8.5's first writer, in the same change that makes the first writer: a library with a writer and no
//! survey would let a removal pass over material it keeps.

use recto_core::removal::{Store, Survey};

/// Marks `Store::Library` complete. Nothing is listed, opened or added.
pub fn survey_library(survey: &mut Survey) {
    survey.mark_complete(Store::Library);
}
