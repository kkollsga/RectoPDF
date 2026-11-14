//! **Where a path a command names becomes a tab or a page's source:** `open` and `page insert`, for every session and actor. The file wall itself is
//! [`recto_platform::document_file`], which every open of a document by its path goes through; this is its first check made before the command looks at
//! anything, so the answer for a path into `/dev` is the one refusal whatever is behind it (S1's twins: whether `/dev/fd/N` is open, or the person's
//! print, or a file a tab has open, isn't told), then the quiet run's rule.

use crate::{AgentError, SessionAccess};
use std::borrow::Cow;
use std::path::Path;

/// **The path to read a document by, once it's been put to the walls:** whole, not into `/dev` (refused as any file that isn't a PDF is, "This file
/// couldn't be read as a PDF."), and where a confined session's rule looked.
///
/// # Errors
///
/// A relative path's refusal, the wall's `not_pdf`, or the quiet run's `outside_quiet_run`.
pub(crate) fn to_read<'a, A: SessionAccess>(access: &A, path: &'a Path) -> Result<Cow<'a, Path>, AgentError> {
    crate::require_absolute(path)?;
    if recto_platform::document_file::leads_into_devices(path) {
        return Err(AgentError::from(recto_core::OpenError::NotPdf));
    }
    // The file is read where the session's rule looked, and not wherever its name leads by the time the read starts; the tab keeps the name (F3).
    access
        .with(|s| s.confinement().to_read(path))
        .ok_or_else(|| crate::quiet_files::outside_the_run(path))
}
