//! **Why a save didn't happen,** in RectoPDF's own words: the errors, their codes and sentences, and what the writers' failures are said to be.

use lopdf::{AppendError, RewriteError};

/// Why a save didn't happen. Every message is RectoPDF's own, never the system's: what's from outside goes only in `detail`, which is for logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    /// The file isn't the version these edits were made on.
    FileChanged,
    /// The file, or a target being replaced, changed under the save while it ran; or, with no edits to speak of, it isn't what was opened (the caller's to say).
    ChangedSinceOpened,
    /// A copy of the document as it is (a Save As with nothing changed in it) whose file changed on disk as it was being copied (the caller's to say).
    ChangedWhileCopying,
    /// The file couldn't be read during the save: a read error, or nothing there to open.
    Unreadable,
    /// The edits claim pages the file doesn't have: a journal that's been tampered with, or one about another version.
    EditsDontMatch { file_pages: u32 },
    /// A PDF whose pages were added has changed, gone, or can't be told apart from another version of itself.
    AddedFileChanged { name: String },
    /// Pages from a password-protected PDF can't go into another file yet.
    AddedFileProtected { name: String },
    /// A page from an added PDF couldn't be copied: something it uses can't be read.
    AddedFileUnreadable { name: String },
    /// The file is password-protected, and writing those isn't supported.
    Protected,
    /// The file's structure is damaged in a way the writer won't build on.
    Damaged,
    /// The document is certified, and these edits change its pages.
    BreaksCertification,
    /// A signature on it locks it against changes, and these edits change its pages.
    LockedBySignature,
    /// These edits are saved by writing the file anew, and it has a person's signature, which that can't keep: Save is off, and Save As makes an unsigned copy (SPEC decision 27).
    SignedNeedsCopy,
    /// Every page of the file would go. The rewrite refuses that, even with pages added from other PDFs.
    NoPagesLeft,
    /// This save writes the file anew, which this caller may not do: an agent below Full.
    RewriteNotAllowed,
    /// An added page's form field has a name the document's fields already have, every way it could be renamed, so their values would mix.
    FieldNamesTaken,
    /// The file or its folder can't be written.
    ReadOnly,
    /// The file has other names, which a save would separate from it.
    HardLinks,
    /// There's already a file at a new file's path.
    TargetExists,
    /// The target isn't an ordinary file.
    NotAFile,
    /// Too much to copy in one save.
    TooLarge,
    /// The file as written didn't read back right, so it wasn't put in place.
    CheckFailed { detail: &'static str },
    /// Reading or writing failed.
    Failed,
    /// The step around the writing failed: noting the save, or carrying the file's attributes.
    PrepareFailed,
}

impl SaveError {
    /// A stable code for agents and tests.
    pub fn code(&self) -> &'static str {
        match self {
            Self::FileChanged => "file_changed",
            Self::ChangedSinceOpened => "changed_since_opened",
            Self::ChangedWhileCopying => "changed_while_copying",
            Self::Unreadable => "file_unreadable",
            Self::EditsDontMatch { .. } => "edits_dont_match",
            Self::AddedFileChanged { .. } => "added_file_changed",
            Self::AddedFileProtected { .. } => "added_file_protected",
            Self::AddedFileUnreadable { .. } => "added_file_unreadable",
            Self::Protected => "protected",
            Self::Damaged => "damaged",
            Self::BreaksCertification => "breaks_certification",
            Self::LockedBySignature => "locked_by_signature",
            Self::SignedNeedsCopy => "signed_needs_copy",
            Self::NoPagesLeft => "no_pages_left",
            Self::FieldNamesTaken => "field_names_taken",
            Self::RewriteNotAllowed => "rewrite_not_allowed",
            Self::ReadOnly => "read_only",
            Self::HardLinks => "hard_links",
            Self::TargetExists => "target_exists",
            Self::NotAFile => "not_a_file",
            Self::TooLarge => "too_large",
            Self::CheckFailed { .. } => "check_failed",
            Self::Failed => "write_failed",
            Self::PrepareFailed => "prepare_failed",
        }
    }

    /// A plain sentence for the person using the app. Each says that nothing changed, as nothing did.
    pub fn message(&self) -> String {
        match self {
            Self::FileChanged => "This file changed on disk after these edits were made, so saving them could put them on the wrong pages. Nothing was written, and your edits are kept.".into(),
            Self::ChangedSinceOpened => "This file has changed on disk since it was opened, so saving over it could lose that. Nothing was written. Save As keeps both.".into(),
            Self::ChangedWhileCopying => "This file changed on disk while it was being saved as a copy, so no copy was made. Nothing was written. Try again to copy it as it is now.".into(),
            Self::Unreadable => "This file couldn't be read during the save, so nothing was written.".into(),
            Self::EditsDontMatch { file_pages } => format!("These edits don't fit the file as it is on disk, which has {file_pages} pages. Nothing was written."),
            Self::AddedFileChanged { name } => format!("“{name}”, whose pages were added, has changed or moved since, so its pages can't be saved from it. Nothing was written."),
            Self::AddedFileProtected { name } => format!("“{name}” is password-protected, and its pages can't be saved into another file yet. Nothing was written."),
            Self::AddedFileUnreadable { name } => format!("Part of the pages added from “{name}” can't be read, so they can't be saved. Nothing was written."),
            Self::Protected => "This PDF is password-protected, and saving protected files isn't supported yet. Nothing was written.".into(),
            Self::Damaged => "This file's structure is damaged, so it can't be saved over safely. Nothing was written.".into(),
            Self::BreaksCertification => "This document is certified, and changing its pages would break that, so it isn't saved over. Nothing was written, and the original keeps its signature.".into(),
            Self::LockedBySignature => "A signature on this document locks it against changes, and changing its pages would break that, so it isn't saved. Nothing was written, and the original keeps its signature.".into(),
            Self::SignedNeedsCopy => "This document is signed, and saving this change means writing the file anew, which would take its signature off, so it isn't saved over. Nothing was written, and the original keeps its signature. Save As makes a new copy without the signature.".into(),
            Self::RewriteNotAllowed => "This save writes the file anew, which takes what was removed out of it for good, and that needs Full access. Nothing was written.".into(),
            Self::FieldNamesTaken => "A form field on the added pages has a name this document's fields already have, however it could be renamed, so their values would mix. Nothing was written.".into(),
            Self::NoPagesLeft => "Every one of the file's own pages would be gone, which a save can't do yet, even with other pages added. Keep one of its pages, or Save As. Nothing was written.".into(),
            Self::ReadOnly => "This file or its folder is locked or read-only, so it can't be saved over. Nothing was written. Save As can put it somewhere else.".into(),
            Self::HardLinks => "This file has other names (hard links), which saving over it would leave with the old version. Nothing was written. Save As makes a separate copy.".into(),
            Self::TargetExists => "There's already a file with that name, and it wasn't replaced. Nothing was written.".into(),
            Self::NotAFile => "That isn't an ordinary file, so it can't be saved over. Nothing was written.".into(),
            Self::TooLarge => "These edits add more than one save can copy at once. Nothing was written.".into(),
            Self::CheckFailed { .. } => "The file as written didn't read back right, so it wasn't put in place. Nothing was changed.".into(),
            Self::Failed => "Writing the file failed, so nothing was changed.".into(),
            Self::PrepareFailed => "The save couldn't get ready to put the file in place, so nothing was changed.".into(),
        }
    }
}

pub(crate) fn from_append(error: AppendError) -> SaveError {
    match error {
        AppendError::RecoveredIndex
        | AppendError::PrevLoop
        | AppendError::AmbiguousHybrid { .. }
        | AppendError::InvalidRoot
        | AppendError::HeaderNotAtStart { .. }
        | AppendError::ChainFull { .. }
        | AppendError::OffsetTooLarge { .. } => SaveError::Damaged,
        AppendError::Encrypted => SaveError::Protected,
        AppendError::TargetChanged { .. } | AppendError::TargetIdentityUnknown => SaveError::ChangedSinceOpened,
        AppendError::TargetExists { .. } => SaveError::TargetExists,
        AppendError::TargetNotAFile { .. } => SaveError::NotAFile,
        AppendError::TargetReadOnly { .. } => SaveError::ReadOnly,
        AppendError::TargetHasHardLinks { .. } => SaveError::HardLinks,
        AppendError::TooManyObjects { .. } | AppendError::ObjectNumberTooLarge { .. } => SaveError::TooLarge,
        AppendError::VerificationFailed { reason } => SaveError::CheckFailed { detail: reason },
        AppendError::PrepareReplacement(error) => match error.kind() {
            // The step's own check of what was written says so in its kind.
            std::io::ErrorKind::InvalidData => SaveError::CheckFailed {
                detail: "pages read back wrong",
            },
            _ => SaveError::PrepareFailed,
        },
        AppendError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => SaveError::ReadOnly,
        AppendError::Source(error) | AppendError::Reader(lopdf::IndexedReaderError::Source(error)) => from_source(&error),
        _ => SaveError::Failed,
    }
}

pub(crate) fn from_rewrite(error: RewriteError) -> SaveError {
    match error {
        RewriteError::RecoveredIndex
        | RewriteError::PrevLoop
        | RewriteError::AmbiguousHybrid { .. }
        | RewriteError::InvalidRoot
        | RewriteError::UnreadableObject { .. }
        | RewriteError::OffsetTooLarge { .. }
        | RewriteError::PageOutOfRange { .. } => SaveError::Damaged,
        RewriteError::Encrypted => SaveError::Protected,
        RewriteError::SignedNeedsConsent { .. } | RewriteError::SignedFileCannotBeReplaced { .. } => SaveError::SignedNeedsCopy,
        RewriteError::NoPagesLeft => SaveError::NoPagesLeft,
        RewriteError::TooManyObjects { .. }
        | RewriteError::ObjectNumberTooLarge { .. }
        | RewriteError::WorkLimitExceeded { .. }
        | RewriteError::StructureTooLarge { .. } => SaveError::TooLarge,
        RewriteError::VerificationFailed { reason } => SaveError::CheckFailed { detail: reason },
        RewriteError::PrepareReplacement(error) => match error.kind() {
            std::io::ErrorKind::InvalidData => SaveError::CheckFailed {
                detail: "pages read back wrong",
            },
            _ => SaveError::PrepareFailed,
        },
        RewriteError::Destination(error) => from_append(error),
        RewriteError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => SaveError::ReadOnly,
        RewriteError::Source(error) | RewriteError::Reader(lopdf::IndexedReaderError::Source(error)) => from_source(&error),
        _ => SaveError::Failed,
    }
}

/// What a failed read of the file under a save was: the file read differently from what was opened, or ran short, so it's been changed under the save;
/// anything else is a read that failed, which isn't said to be a change.
pub(crate) fn from_source(error: &lopdf::SourceError) -> SaveError {
    match error {
        lopdf::SourceError::SourceChanged | lopdf::SourceError::UnexpectedEof { .. } | lopdf::SourceError::OutOfBounds { .. } => SaveError::ChangedSinceOpened,
        _ => SaveError::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::open_for_save;
    use std::path::Path;

    /// Failure: a save that couldn't read the file (a read error, or the file not there to open) was said to be the file changing on disk, and under edits
    /// relabelled as their file having changed, with "put the earlier version back" as the way on. A read error is its own failure; only the file moving under
    /// the save is a change.
    #[test]
    fn a_read_error_during_a_save_is_not_a_change() {
        let io = || lopdf::SourceError::Io(std::io::Error::other("the disk went away"));
        assert_eq!(from_append(AppendError::Source(io())).code(), "file_unreadable");
        assert_eq!(from_rewrite(RewriteError::Source(io())).code(), "file_unreadable");
        assert_eq!(
            open_for_save(Path::new("/nonexistent/recto/never.pdf")).err().map(|error| error.code()),
            Some("file_unreadable")
        );
        assert_eq!(
            from_append(AppendError::Source(lopdf::SourceError::SourceChanged)).code(),
            "changed_since_opened"
        );
        assert_eq!(
            from_rewrite(RewriteError::Source(lopdf::SourceError::SourceChanged)).code(),
            "changed_since_opened"
        );
        assert!(!SaveError::Unreadable.message().contains("changed"));
    }
}
