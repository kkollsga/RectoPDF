//! **Saving: the one place that changes a PDF's bytes,** through the lopdf fork's two entry points and nothing else (AGENTS.md rule 5,
//! SPEC section 6). Adding things appends to the file, with `append_atomically`, so every byte that was there stays, a signature
//! included. Removing things rewrites it, with `rewrite_atomically`, so what's removed is really gone. Never `Writer::write_object`,
//! an eager `Document` save, `IncrementalDocument` or `copy_revision_to` over the original: `clippy.toml` refuses them by name, and
//! `RewriteChanges::remove_object` too, as nothing reports a kept page that still uses a removed object.
//!
//! **Which edit needs which writer, and why.**
//! - A turn is a new version of the page with its `/Rotate`: an addition.
//! - A move, or pages added from another PDF, is a new page tree and, for added pages, their objects copied in under new numbers:
//!   additions too. The old tree stays in the file's earlier revision, which holds nothing that isn't in the new one.
//! - A delete takes a page out. Appending a tree without it would leave the page's objects in the file for anyone to read back,
//!   so it's a rewrite, which copies only what's still reachable.
//!
//! **Everything about a page is checked against the real file, never the journal.** A journal can be forged, so its page counts
//! and stamps are only claims: the file's own page count, each added PDF's stamp and length, and every page asked for are read
//! from the files themselves before a byte is written, and the result is read back, page by page, before it's moved into place.
//!
//! **The reader it writes from** has a limit of 1 MiB an object: at lopdf's default of 4 MiB a 4 KB hostile file reached about
//! 1.5 GB during a save, and the largest object in the corpus is 50 KB.
//!
//! **Signatures.** A signed file's existing bytes never change. An approval signature stays as it was when pages are only added to,
//! as the bytes it signs are all still there. A certification (`/Perms /DocMDP`) allows no change to pages at any level, so a page
//! edit is refused on such a file, and Save As makes an unsigned copy (SPEC decision 27). Usage rights (`/Perms /UR3`), as on IRS
//! forms, aren't a person's signature: a save drops them from the new revision and says so quietly (owner, 2026-11-08). Nothing
//! here ever says a signature is valid.

use crate::OpenError;
use lopdf::{AppendDestination, AppendError, AppendOptions, AppendWriter, Dictionary, EncodedStreamReader, FileSource, IndexedReader, IndexedReaderOptions, IndexedSignatureKind, Object, ObjectId, PageMapEntry, RewriteChanges, RewriteError, RewriteOptions, RewritePolicy};
use recto_platform::FileStamp;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Most bytes one object may take while a save reads it. The review's measurement: at lopdf's 4 MiB a 4 KB hostile file reaches about 1.5 GB during a save. The corpus's largest is 50 KB.
pub const SAVE_OBJECT_BYTES: u64 = 1024 * 1024;
/// Most objects one save copies in from other PDFs. Each costs a few dozen bytes while the save runs, and a real document's pages hold far fewer.
pub const MAX_COPIED_OBJECTS: usize = 1_000_000;
/// The one backup of an original is kept this long (owner, 2026-11-08: "The first Save keeps one backup of the original").
pub const BACKUP_DAYS: u64 = 30;
/// And within this much in all, oldest out first.
pub const MAX_BACKUP_BYTES: u64 = 1024 * 1024 * 1024;

/// One page of the document being saved: from which file, which page there, and the quarter turns added to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRef {
    /// 0 is the file being saved. `n` is [`SaveRequest::added`]'s entry `n - 1`.
    pub source: u16,
    /// Zero-based, in that file.
    pub page: u32,
    pub quarter_turns: u8,
}

/// A PDF whose pages were added: where it is, and what was known of it when they were.
#[derive(Clone, Copy, Debug)]
pub struct AddedFile<'a> {
    pub path: &'a Path,
    pub page_count: u32,
    /// `None` if it couldn't be looked at then, and then nothing can say it's the same file, so it isn't saved from.
    pub stamp: Option<FileStamp>,
}

/// What's to be saved: the document as edited, about the file it was opened from.
#[derive(Clone, Copy, Debug)]
pub struct SaveRequest<'a> {
    pub path: &'a Path,
    /// What the tab knows of its file. The file at `path` has to still be that.
    pub stamp: Option<FileStamp>,
    /// How long the file was when the edits began, as the journal says. Only a claim, checked against the file.
    pub file_pages: u32,
    pub pages: &'a [PageRef],
    pub added: &'a [AddedFile<'a>],
}

/// Where a save goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    /// Save: over the file itself.
    Same,
    /// Save As to a path with nothing there.
    NewFile(&'a Path),
    /// Save As over another file the person chose to replace.
    Replace(&'a Path),
}

/// Which of the fork's writers a save takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writer {
    /// Adds a revision. Every byte that was there stays.
    Append,
    /// A whole new file holding only what's reachable, so what was removed is gone.
    Rewrite,
}

impl Writer {
    pub fn name(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Rewrite => "rewrite",
        }
    }
}

/// What a file says about signatures, from its catalog. Facts only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SignatureFacts {
    /// `/Perms /DocMDP`: a certification, which allows no change to pages.
    pub certified: bool,
    /// `/Perms /UR3` or `/UR`: usage rights, which aren't a person's signature.
    pub usage_rights: bool,
    /// A signed signature field with a `/Lock` that has a `/P`: like a certification, it allows no change to pages once it's signed.
    pub field_locked: bool,
    /// A signature field with a value: a person's signature, which additions keep and a rewrite of the file can't. Says nothing of whether it's valid.
    pub signed: bool,
}

impl SignatureFacts {
    /// Whether a person's signature is on it, of any kind: usage rights aren't one.
    pub fn has_persons_signature(&self) -> bool {
        self.signed || self.certified || self.field_locked
    }
}

/// Why a save didn't happen. Every message is RectoPDF's own, never the system's: what's from outside goes only in `detail`, which is for logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    /// The file isn't the one the tab has open any more.
    FileChanged,
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
            Self::FileChanged => "This file has changed on disk since it was opened, so saving over it could lose that. Nothing was written. Save As keeps both.".into(),
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

fn file_label(path: &Path) -> String {
    crate::clean_line(&path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default())
}

fn from_append(error: AppendError) -> SaveError {
    match error {
        AppendError::RecoveredIndex | AppendError::PrevLoop | AppendError::AmbiguousHybrid { .. } | AppendError::InvalidRoot | AppendError::HeaderNotAtStart { .. } | AppendError::ChainFull { .. } | AppendError::OffsetTooLarge { .. } => SaveError::Damaged,
        AppendError::Encrypted => SaveError::Protected,
        AppendError::TargetChanged { .. } | AppendError::TargetIdentityUnknown => SaveError::FileChanged,
        AppendError::TargetExists { .. } => SaveError::TargetExists,
        AppendError::TargetNotAFile { .. } => SaveError::NotAFile,
        AppendError::TargetReadOnly { .. } => SaveError::ReadOnly,
        AppendError::TargetHasHardLinks { .. } => SaveError::HardLinks,
        AppendError::TooManyObjects { .. } | AppendError::ObjectNumberTooLarge { .. } => SaveError::TooLarge,
        AppendError::VerificationFailed { reason } => SaveError::CheckFailed { detail: reason },
        AppendError::PrepareReplacement(error) => match error.kind() {
            // The step's own check of what was written says so in its kind.
            std::io::ErrorKind::InvalidData => SaveError::CheckFailed { detail: "pages read back wrong" },
            _ => SaveError::PrepareFailed,
        },
        AppendError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => SaveError::ReadOnly,
        // The original read differently from what was opened, or ran short: it's been changed under the save.
        AppendError::Source(_) | AppendError::Reader(lopdf::IndexedReaderError::Source(_)) => SaveError::FileChanged,
        _ => SaveError::Failed,
    }
}

fn from_rewrite(error: RewriteError) -> SaveError {
    match error {
        RewriteError::RecoveredIndex | RewriteError::PrevLoop | RewriteError::AmbiguousHybrid { .. } | RewriteError::InvalidRoot | RewriteError::UnreadableObject { .. } | RewriteError::OffsetTooLarge { .. } | RewriteError::PageOutOfRange { .. } => SaveError::Damaged,
        RewriteError::Encrypted => SaveError::Protected,
        RewriteError::SignedNeedsConsent { .. } | RewriteError::SignedFileCannotBeReplaced { .. } => SaveError::SignedNeedsCopy,
        RewriteError::NoPagesLeft => SaveError::NoPagesLeft,
        RewriteError::TooManyObjects { .. } | RewriteError::ObjectNumberTooLarge { .. } | RewriteError::WorkLimitExceeded { .. } | RewriteError::StructureTooLarge { .. } => SaveError::TooLarge,
        RewriteError::VerificationFailed { reason } => SaveError::CheckFailed { detail: reason },
        RewriteError::PrepareReplacement(error) => match error.kind() {
            std::io::ErrorKind::InvalidData => SaveError::CheckFailed { detail: "pages read back wrong" },
            _ => SaveError::PrepareFailed,
        },
        RewriteError::Destination(error) => from_append(error),
        RewriteError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => SaveError::ReadOnly,
        RewriteError::Source(_) | RewriteError::Reader(lopdf::IndexedReaderError::Source(_)) => SaveError::FileChanged,
        _ => SaveError::Failed,
    }
}

/// Opens a file the way a save reads it: with the 1 MiB limit on an object.
pub fn open_for_save(path: &Path) -> Result<IndexedReader, SaveError> {
    let source = FileSource::open(path).map_err(|_| SaveError::FileChanged)?;
    let options = IndexedReaderOptions { object_bytes: SAVE_OBJECT_BYTES, ..IndexedReaderOptions::default() };
    let reader = IndexedReader::open_with_options(source, options).map_err(|_| SaveError::Damaged)?;
    if reader.is_encrypted() {
        return Err(SaveError::Protected);
    }
    Ok(reader)
}

/// What a file's catalog says about signatures. Cheap: the catalog and its `/Perms`.
pub fn signature_facts(reader: &IndexedReader) -> SignatureFacts {
    let Some(root) = catalog(reader) else { return SignatureFacts::default() };
    let perms = root.get(b"Perms").ok().cloned().and_then(|perms| dict_value(reader, &perms)).unwrap_or_default();
    let (signed, field_locked) = signed_fields(reader, &root);
    SignatureFacts { certified: perms.has(b"DocMDP"), usage_rights: perms.has(b"UR3") || perms.has(b"UR"), field_locked, signed }
}

/// Fields looked at for a lock. A form has far fewer; a hostile one can't make the look long.
const MAX_FIELDS_FOR_LOCKS: usize = 16_384;

/// Whether a signature field is signed, and whether a signed one has a `/Lock` with a `/P`. That permission, 1 to 3, allows no change to pages in any of them. A field that can't be read
/// is passed over, and a form past the limit counts as signed and locked, on the side of refusing.
fn signed_fields(reader: &IndexedReader, root: &Dictionary) -> (bool, bool) {
    let Some(form) = root.get(b"AcroForm").ok().cloned().and_then(|form| dict_value(reader, &form)) else { return (false, false) };
    // Each field with whether a parent of it says it's a signature field, as `/FT` is inherited.
    let mut stack: Vec<(Object, bool)> = match form.get(b"Fields").ok().cloned().map(|fields| match fields {
        Object::Reference(id) => reader.resolve_object(id).unwrap_or(Object::Null),
        other => other,
    }) {
        Some(Object::Array(fields)) => fields.into_iter().map(|field| (field, false)).collect(),
        _ => return (false, false),
    };
    let mut seen = HashSet::new();
    let mut visits = 0;
    let mut queued = stack.len();
    let mut any_signed = false;
    while let Some((field, parent_is_sig)) = stack.pop() {
        visits += 1;
        if visits > MAX_FIELDS_FOR_LOCKS {
            return (true, true);
        }
        if let Object::Reference(id) = &field
            && !seen.insert(*id)
        {
            continue;
        }
        let Some(field) = dict_value(reader, &field) else { continue };
        let is_sig = match field.get(b"FT") {
            Ok(Object::Name(kind)) => kind == b"Sig",
            _ => parent_is_sig,
        };
        // A value with a byte range is a signature whatever the field says it is (review round 2).
        let signed = (is_sig && field.has(b"V")) || value_is_signature(reader, &field);
        any_signed |= signed;
        if signed && field.get(b"Lock").ok().cloned().and_then(|lock| dict_value(reader, &lock)).is_some_and(|lock| lock.has(b"P")) {
            return (true, true);
        }
        if let Ok(Object::Array(kids)) = field.get(b"Kids") {
            // Everything ever queued counts, so a tree of wide nodes can't hold millions (review).
            let room = MAX_FIELDS_FOR_LOCKS.saturating_sub(queued);
            if kids.len() > room {
                return (true, true);
            }
            queued += kids.len();
            stack.extend(kids.iter().cloned().map(|kid| (kid, is_sig)));
        }
    }
    (any_signed, false)
}

/// **Whether a field's own `/V` is a signature's value:** a dictionary with a `/ByteRange`, written in place or named. One object resolved at most.
fn value_is_signature(reader: &IndexedReader, field: &Dictionary) -> bool {
    match field.get(b"V") {
        Ok(Object::Dictionary(value)) => value.has(b"ByteRange"),
        Ok(Object::Reference(id)) => matches!(reader.resolve_object(*id), Ok(Object::Dictionary(value)) if value.has(b"ByteRange")),
        _ => false,
    }
}

/// [`signature_facts`] of a file on disk, for deciding what's on offer before anyone saves, with the fork's own look through the file's bytes for a signature,
/// which is what the rewrite refuses by: so Save is off exactly when a save over the file would be refused. Reads the whole file, so never on the UI thread or the
/// session's. A file that won't open says nothing.
pub fn signature_facts_of(path: &Path) -> Result<SignatureFacts, OpenError> {
    let reader = crate::open_reader(path)?;
    let mut facts = signature_facts(&reader);
    facts.signed |= persons_signature(&reader);
    Ok(facts)
}

/// **Whether the fork finds a person's signature in the file:** by form field, by `/Perms`, and by every `/ByteRange` in its bytes, old revisions included, as the
/// rewrite looks before it refuses. Usage rights aren't one. A file it can't look through counts as signed, on the side of refusing.
pub fn persons_signature(reader: &IndexedReader) -> bool {
    reader.signatures().map_or(true, |found| found.iter().any(|signature| signature.kind != IndexedSignatureKind::UsageRights))
}

fn catalog(reader: &IndexedReader) -> Option<Dictionary> {
    match reader.trailer_entry_owned(b"Root").ok()?? {
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

/// The catalog's own number, which a new version of it has to have.
fn catalog_id(reader: &IndexedReader) -> Option<ObjectId> {
    reader.trailer_entry_raw_owned(b"Root")?.as_reference().ok()
}

fn dict_value(reader: &IndexedReader, value: &Object) -> Option<Dictionary> {
    match value {
        Object::Dictionary(dict) => Some(dict.clone()),
        Object::Reference(id) => match reader.resolve_object(*id).ok()? {
            Object::Dictionary(dict) => Some(dict),
            _ => None,
        },
        _ => None,
    }
}

/// Which writer these edits need. Every page of the file has to be there exactly once for an append: a page that's gone was deleted, and a deleted page's objects would stay readable in an appended file.
pub fn writer_for(file_pages: u32, pages: &[PageRef]) -> Writer {
    let mut seen = vec![false; file_pages as usize];
    for page in pages.iter().filter(|page| page.source == 0) {
        match seen.get_mut(page.page as usize) {
            Some(slot) if !*slot => *slot = true,
            _ => return Writer::Rewrite,
        }
    }
    if seen.iter().all(|seen| *seen) { Writer::Append } else { Writer::Rewrite }
}

/// Steps a save tells its caller about, for progress on a big file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStep {
    /// Reading the files and checking every page against them.
    Checking,
    /// Writing the new file beside the old.
    Writing,
    /// Written and synced, attributes carried, pages read back. The caller notes the save here, before it's put in place.
    Prepared,
    /// The writer's own check of every byte, then the move into place.
    Committing,
}

/// What a save that happened did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveOutcome {
    pub writer: Writer,
    pub previous_len: u64,
    pub new_len: u64,
    pub pages: u32,
    /// Usage rights were dropped from the new revision.
    pub usage_rights_removed: bool,
    /// Some of the file's extended attributes or its ACL couldn't be carried to the new file.
    pub attributes_missed: bool,
    /// Objects something kept names that the file doesn't have, so they went as nothing, as viewers read them: an added page's, or, in a rewrite, anything's. MuPDF and poppler can repair what a copy drops, so this is a warning.
    pub objects_missing: u64,
    /// Pages the file had that it doesn't now.
    pub pages_removed: u32,
    /// A person's signatures a copy no longer has (SPEC decision 27). Never checked, and never said to have been valid.
    pub signatures_removed: u32,
    /// What a rewrite kept that's the document's rather than a page's, which can repeat what was on a removed page. `None` for an append.
    pub document_data_kept: Option<DocumentData>,
    /// Shared resource dictionaries a removed page used that couldn't be searched, so what only it drew may still be in the file.
    pub resources_left_whole: u64,
    /// The document's own form couldn't be read, so added pages' fields weren't joined to it: they draw, and can't be filled.
    pub form_fields_left_out: bool,
    /// The file was replaced, but its folder couldn't be synced, so a power cut in the next moments could bring the old one back.
    pub durability_warning: bool,
    /// The file as it is now.
    pub stamp: Option<FileStamp>,
    /// Where the one backup of the original went, if this save made it.
    pub backup: Option<PathBuf>,
}

/// What a save asks of whoever's saving.
pub trait SaveHooks {
    /// How it's going.
    fn step(&mut self, _step: SaveStep) {}
    /// Whether this caller may save by writing the file anew, which takes what was removed out of it for good. Asked once the writer's decided, before anything's written: an agent below Full may not.
    fn may_rewrite(&self) -> bool {
        true
    }
    /// **The file is written, beside its target, and will be `stamp` once it's in place:** the rename keeps its number and time. Anything to note first, such as the session's `save_prepared`, goes here, and an error stops the save with nothing changed.
    fn prepared(&mut self, _written: &Path, _stamp: FileStamp) -> std::io::Result<()> {
        Ok(())
    }
}

/// No hooks.
pub struct NoHooks;
impl SaveHooks for NoHooks {}

/// **Saves the document as edited.** Checks every page against the real files, decides the writer, writes beside the target, reads the result back, lets the caller note it, and moves it into place. `backups` is where the one backup of an original goes on its first save over itself.
pub fn save(request: &SaveRequest<'_>, target: Target<'_>, backups: Option<&Path>, hooks: &mut dyn SaveHooks) -> Result<SaveOutcome, SaveError> {
    hooks.step(SaveStep::Checking);
    // With no stamp nothing can say the file is still the one the edits are about, so it isn't saved from.
    let known = request.stamp.ok_or(SaveError::FileChanged)?;
    let unchanged = || recto_platform::file_stamp(request.path).is_ok_and(|now| now.same_contents(known) && now.inode == known.inode && now.device == known.device);
    if !unchanged() {
        return Err(SaveError::FileChanged);
    }
    let reader = open_for_save(request.path)?;
    // Looked at again once it's open: a file swapped in between the look and the open would be read as this one. The writer then checks that what it opened is
    // what's at the path, so what it writes from is the file the tab knows.
    if !unchanged() {
        return Err(SaveError::FileChanged);
    }
    let map = reader.page_map().map_err(|_| SaveError::Damaged)?;
    let real_pages = u32::try_from(map.len()).map_err(|_| SaveError::Damaged)?;
    if real_pages != request.file_pages {
        return Err(SaveError::EditsDontMatch { file_pages: real_pages });
    }
    if request.pages.is_empty() {
        return Err(SaveError::EditsDontMatch { file_pages: real_pages });
    }
    let sources = open_added(request)?;
    for page in request.pages {
        let pages_there = if page.source == 0 { real_pages } else { sources.get(usize::from(page.source) - 1).and_then(Option::as_ref).map_or(0, |(_, map)| u32::try_from(map.len()).unwrap_or(0)) };
        if page.page >= pages_there || page.quarter_turns > 3 {
            return Err(SaveError::EditsDontMatch { file_pages: real_pages });
        }
    }
    let removed: Vec<usize> = removed_pages(real_pages, request.pages);
    let facts = signature_facts(&reader);
    let changes_pages = !removed.is_empty() || request.pages.iter().enumerate().any(|(at, page)| page.source != 0 || page.page as usize != at || page.quarter_turns != 0);
    let copy = target != Target::Same;
    if !copy && facts.field_locked && changes_pages {
        return Err(SaveError::LockedBySignature);
    }
    if !copy && facts.certified && changes_pages {
        return Err(SaveError::BreaksCertification);
    }
    let mut writer = writer_for(real_pages, request.pages);
    // The same look at the file's bytes the rewrite takes, so what Save offers and what the rewrite refuses agree.
    if !copy && writer == Writer::Rewrite && (facts.has_persons_signature() || persons_signature(&reader)) {
        return Err(SaveError::SignedNeedsCopy);
    }
    // A copy of a certified or locked document is a new, unsigned file (SPEC decision 27): its pages can't change under that signature, and even an unchanged copy
    // would append a revision the certification can show as a change (review round 2).
    if copy && (facts.certified || facts.field_locked) {
        writer = Writer::Rewrite;
    }
    if writer == Writer::Rewrite && !hooks.may_rewrite() {
        return Err(SaveError::RewriteNotAllowed);
    }
    let plan = Plan::make(&reader, &map, request, &sources, facts, writer, MAX_COPY_WORK)?;
    match writer {
        Writer::Append => reader.check_appendable().map_err(from_append)?,
        Writer::Rewrite => reader.check_rewritable().map_err(from_rewrite)?,
    }

    let backup = match (target, backups) {
        (Target::Same, Some(folder)) => keep_backup(request.path, folder).ok().flatten(),
        _ => None,
    };
    hooks.step(SaveStep::Writing);
    pause_for_a_kill("writing");
    let destination = match target {
        Target::Same => AppendDestination::SameFile(request.path),
        Target::NewFile(path) => AppendDestination::NewFile(path),
        Target::Replace(path) => AppendDestination::OverwriteOther(path),
    };
    let original = request.path.to_path_buf();
    let expected = std::cell::RefCell::new(Vec::new());
    let mut prepared_stamp = None;
    let attributes_missed = std::cell::Cell::new(false);
    let mut copy_failed = None;
    let hooks_cell = std::cell::RefCell::new(&mut *hooks);
    let prepare = |written: &Path, _target: &Path| -> std::io::Result<()> {
        // A save over the file is the same file to the person using it, so it keeps its tags, quarantine and ACL. A copy keeps where it came from.
        // One the system won't let go, as `com.apple.macl` is, costs a note and not the save.
        let carried = match target {
            Target::Same => recto_platform::local::carry_attributes(&original, written),
            Target::NewFile(_) | Target::Replace(_) => recto_platform::local::carry_origin(&original, written),
        };
        attributes_missed.set(!matches!(carried, Ok(0)));
        check_pages(written, &expected.borrow())?;
        let stamp = recto_platform::file_stamp(written)?;
        prepared_stamp = Some(stamp);
        let mut hooks = hooks_cell.borrow_mut();
        hooks.step(SaveStep::Prepared);
        hooks.prepared(written, stamp)?;
        pause_for_a_kill("prepared");
        hooks.step(SaveStep::Committing);
        Ok(())
    };
    let pages = u32::try_from(request.pages.len()).unwrap_or(u32::MAX);
    let pages_removed = u32::try_from(removed.len()).unwrap_or(u32::MAX);
    let outcome = match writer {
        Writer::Append => {
            let options = AppendOptions::new().prepare_replacement(prepare);
            let appended = lopdf::append_atomically(&reader, destination, options, |writer| {
                // The temporary file exists here, with the original copied into it and the update not yet written.
                pause_for_a_kill("copying");
                let began = writer.position();
                match plan.write(&reader, &sources, &mut AppendSink { writer, began }) {
                    Ok(ids) => {
                        *expected.borrow_mut() = ids;
                        Ok(())
                    }
                    Err(error) => {
                        copy_failed = Some(error);
                        Err(AppendError::Abandoned)
                    }
                }
            });
            let appended = match appended {
                Ok(appended) => appended,
                Err(error) => return Err(copy_failed.unwrap_or_else(|| from_append(error))),
            };
            SaveOutcome {
                writer: Writer::Append,
                previous_len: appended.summary.previous_len,
                new_len: appended.summary.new_len,
                pages,
                usage_rights_removed: plan.drop_usage_rights,
                attributes_missed: attributes_missed.get(),
                objects_missing: plan.missing,
                pages_removed,
                signatures_removed: 0,
                document_data_kept: None,
                resources_left_whole: 0,
                form_fields_left_out: plan.fields_left_out,
                durability_warning: appended.durability_warning.is_some(),
                stamp: None,
                backup,
            }
        }
        Writer::Rewrite => {
            // Over the file itself only usage rights may go, which aren't a person's signature (owner, 2026-11-08): the fork refuses anything else there. A copy goes
            // without every signature, as SPEC decision 27 has it.
            let policy = if copy { RewritePolicy::new().remove_signatures().remove_usage_rights() } else { RewritePolicy::new().remove_usage_rights() };
            let options = RewriteOptions::new().policy(policy).prepare_replacement(prepare);
            let cap = plan.append_cap.min(MAX_REWRITE_HELD_BYTES);
            let rewritten = lopdf::rewrite_atomically(&reader, destination, options, |changes| {
                for index in &removed {
                    changes.remove_page(*index);
                }
                match plan.write(&reader, &sources, &mut RewriteSink { changes, held: 0, cap }) {
                    Ok(ids) => {
                        *expected.borrow_mut() = ids;
                        Ok(())
                    }
                    Err(error) => {
                        copy_failed = Some(error);
                        Err(RewriteError::Io(std::io::Error::other("the save's plan stopped")))
                    }
                }
            });
            let rewritten = match rewritten {
                Ok(rewritten) => rewritten,
                Err(error) => return Err(copy_failed.unwrap_or_else(|| from_rewrite(error))),
            };
            let summary = &rewritten.summary;
            let persons = summary.removed_signatures.iter().filter(|signature| signature.kind != IndexedSignatureKind::UsageRights).count();
            let rights = summary.removed_signatures.iter().any(|signature| signature.kind == IndexedSignatureKind::UsageRights);
            SaveOutcome {
                writer: Writer::Rewrite,
                previous_len: summary.previous_len,
                new_len: summary.new_len,
                pages,
                usage_rights_removed: rights,
                attributes_missed: attributes_missed.get(),
                objects_missing: plan.missing.saturating_add(summary.objects_missing.len() as u64),
                pages_removed,
                signatures_removed: u32::try_from(persons).unwrap_or(u32::MAX),
                document_data_kept: Some(DocumentData::from(&summary.document_data_kept)),
                resources_left_whole: (summary.removal.resources_left_whole.len() as u64).saturating_add(summary.removal.resource_dictionaries_unlisted),
                form_fields_left_out: plan.fields_left_out,
                durability_warning: rewritten.durability_warning.is_some(),
                stamp: None,
                backup,
            }
        }
    };
    pause_for_a_kill("committed");
    // What it was as it was written, which the rename keeps: a look now could see another app's write in the moment since, and take it as this save's.
    Ok(SaveOutcome { stamp: prepared_stamp, ..outcome })
}

/// The file's pages that the document doesn't have: each is deleted by the save. By position in the file, in order.
pub fn removed_pages(file_pages: u32, pages: &[PageRef]) -> Vec<usize> {
    let mut there = vec![false; file_pages as usize];
    for page in pages.iter().filter(|page| page.source == 0) {
        if let Some(slot) = there.get_mut(page.page as usize) {
            *slot = true;
        }
    }
    there.iter().enumerate().filter(|(_, there)| !**there).map(|(at, _)| at).collect()
}

/// Most a rewrite holds in new and replaced objects until it writes them: page dictionaries, a new tree, catalogs. Streams aren't held, they're read as they're written.
pub const MAX_REWRITE_HELD_BYTES: u64 = 256 * 1024 * 1024;

/// What a rewrite kept that's the document's rather than a page's (the fork's `DocumentDataKept`), as counts. It can repeat what was on a page that's gone, so a save that removed pages says so (the streaming rewrite's review).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocumentData {
    pub embedded_files: u64,
    pub metadata_streams: u64,
    pub info_entries: u64,
    pub javascript: u64,
    pub piece_info: u64,
    pub document_parts: bool,
    pub xfa: bool,
    pub hidden_layers: u64,
}

impl From<&lopdf::DocumentDataKept> for DocumentData {
    fn from(kept: &lopdf::DocumentDataKept) -> Self {
        Self { embedded_files: kept.embedded_files, metadata_streams: kept.metadata_streams, info_entries: kept.info_entries, javascript: kept.javascript, piece_info: kept.piece_info, document_parts: kept.document_parts, xfa: kept.xfa, hidden_layers: kept.hidden_layers }
    }
}

impl DocumentData {
    /// What's there, in words, for a note: "2 attached files", "the document's information (title, author and the like)". Empty if nothing is.
    pub fn kinds(&self) -> Vec<String> {
        let mut out = Vec::new();
        let counted = |n: u64, one: &str, many: &str| if n == 1 { one.to_string() } else { format!("{n} {many}") };
        if self.embedded_files > 0 {
            out.push(counted(self.embedded_files, "an attached file", "attached files"));
        }
        if self.metadata_streams > 0 {
            out.push("XMP metadata".to_string());
        }
        if self.info_entries > 0 {
            out.push("the document's information (title, author and the like)".to_string());
        }
        if self.javascript > 0 {
            out.push(counted(self.javascript, "a script", "scripts"));
        }
        if self.piece_info > 0 {
            out.push("another app's private data".to_string());
        }
        if self.document_parts {
            out.push("document-part metadata".to_string());
        }
        if self.xfa {
            out.push("an XFA form".to_string());
        }
        if self.hidden_layers > 0 {
            out.push(counted(self.hidden_layers, "a hidden layer", "hidden layers"));
        }
        out
    }
}

/// **A copy of only some of a file's pages,** for undoing past a save that deletes them (owner, 2026-11-07: "keeps only the removed pages"). Written by the
/// rewrite to a new file, so the copy holds what those pages use and nothing of the rest, without a signature, as it's a working copy for the history and
/// never the document. Synced, checked and moved into place by the fork, so a power cut leaves either nothing or the whole copy. Its pages are `pages` in
/// that order, which has to be ascending. Gives its stamp.
pub fn keep_pages(original: &Path, pages: &[u32], dest: &Path, max_bytes: u64) -> Result<FileStamp, SaveError> {
    if pages.is_empty() || pages.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(SaveError::EditsDontMatch { file_pages: 0 });
    }
    let reader = open_for_save(original)?;
    let map = reader.page_map().map_err(|_| SaveError::Damaged)?;
    let count = u32::try_from(map.len()).map_err(|_| SaveError::Damaged)?;
    if pages.last().is_some_and(|last| *last >= count) {
        return Err(SaveError::EditsDontMatch { file_pages: count });
    }
    // Estimated before anything's written, from the file's size and its share of pages, so a copy past the limit doesn't wear the drive to be thrown away.
    let estimate = reader.source_len().saturating_mul(pages.len() as u64) / u64::from(count.max(1));
    if estimate > max_bytes {
        return Err(SaveError::TooLarge);
    }
    let keep: HashSet<u32> = pages.iter().copied().collect();
    let options = RewriteOptions::new().policy(RewritePolicy::new().remove_signatures().remove_usage_rights());
    let written = lopdf::rewrite_atomically(&reader, AppendDestination::NewFile(dest), options, |changes| {
        for page in (0..count).filter(|page| !keep.contains(page)) {
            changes.remove_page(page as usize);
        }
        Ok(())
    })
    .map_err(from_rewrite)?;
    // A page object the tree lists twice is one page to the rewrite, which removes it from every place, so the copy can hold other pages than asked for: its
    // numbers would then be wrong, so it isn't used. Object numbers don't change in a rewrite, so the pages held are compared one for one.
    let expected: Vec<ObjectId> = pages.iter().filter_map(|page| map.get(*page as usize).map(PageMapEntry::id)).collect();
    let held: Vec<ObjectId> = written.reader.page_map().map(|map| map.iter().map(PageMapEntry::id).collect()).unwrap_or_default();
    if written.durability_warning.is_some() || held != expected {
        // Not trusted to be there after a power cut, or not what was asked for: not relied on, and the history rolls those steps off instead.
        let _ = std::fs::remove_file(dest);
        return Err(SaveError::Failed);
    }
    recto_platform::file_stamp(dest).map_err(|_| SaveError::Failed)
}

/// **A test's seam, in debug builds only:** with `RECTO_TEST_SAVE_PAUSE` naming this step, the save says so in a file beside the state and waits, so a test can kill the process right there. A release build has none of it.
fn pause_for_a_kill(step: &str) {
    #[cfg(debug_assertions)]
    if std::env::var("RECTO_TEST_SAVE_PAUSE").is_ok_and(|wanted| wanted == step)
        && let Ok(marker) = std::env::var("RECTO_TEST_SAVE_MARKER")
    {
        let _ = std::fs::write(marker, step);
        std::thread::sleep(Duration::from_secs(60));
    }
    #[cfg(not(debug_assertions))]
    let _ = step;
}

type Opened = Option<(IndexedReader, lopdf::PageMap)>;

/// Opens bytes the way a save reads a file, for [`plan_in_memory`].
fn open_bytes_for_save(bytes: &[u8]) -> Result<(IndexedReader, lopdf::PageMap), SaveError> {
    let options = IndexedReaderOptions { object_bytes: SAVE_OBJECT_BYTES, ..IndexedReaderOptions::default() };
    let reader = IndexedReader::open_with_options(lopdf::BytesSource::new(std::sync::Arc::from(bytes)), options).map_err(|_| SaveError::Damaged)?;
    if reader.is_encrypted() {
        return Err(SaveError::Protected);
    }
    let map = reader.page_map().map_err(|_| SaveError::Damaged)?;
    Ok((reader, map))
}

/// **The planner, the copier and the remapper on bytes in memory, for the fuzzer:** `target` with its last page moved to the front, page 1 turned, and every page
/// of `added` copied in at the end, written as an update into memory and read back. Gives back the pages the result has. The same code a save runs, with no file.
pub fn plan_in_memory(target: &[u8], added: &[u8]) -> Result<usize, SaveError> {
    plan_in_memory_within(target, added, MAX_COPY_WORK)
}

/// [`plan_in_memory`] with a smaller bound on the copy's work, so a test can show the bound on a small file.
pub fn plan_in_memory_within(target: &[u8], added: &[u8], work_limit: u64) -> Result<usize, SaveError> {
    let (reader, map) = open_bytes_for_save(target)?;
    reader.check_appendable().map_err(from_append)?;
    let pages = u32::try_from(map.len()).map_err(|_| SaveError::Damaged)?;
    let (source, source_map) = open_bytes_for_save(added)?;
    let added_pages = u32::try_from(source_map.len()).map_err(|_| SaveError::Damaged)?.min(64);
    let mut order: Vec<PageRef> = Vec::new();
    order.push(PageRef { source: 0, page: pages.saturating_sub(1), quarter_turns: 0 });
    order.extend((0..pages.saturating_sub(1)).map(|page| PageRef { source: 0, page, quarter_turns: u8::from(page == 0) }));
    order.extend((0..added_pages).map(|page| PageRef { source: 1, page, quarter_turns: 2 }));
    let added_file = [AddedFile { path: Path::new("added.pdf"), page_count: added_pages, stamp: None }];
    let request = SaveRequest { path: Path::new("target.pdf"), stamp: None, file_pages: pages, pages: &order, added: &added_file };
    let sources = vec![Some((source, source_map))];
    let plan = Plan::make(&reader, &map, &request, &sources, signature_facts(&reader), Writer::Append, work_limit.min(MAX_COPY_WORK))?;
    let mut out = Vec::new();
    let mut writer = AppendWriter::new(&reader, &mut out).map_err(from_append)?;
    let began = writer.position();
    let expected = plan.write(&reader, &sources, &mut AppendSink { writer: &mut writer, began })?;
    writer.finish().map_err(from_append)?;
    // Read back as a save reads back what it wrote: the pages are the objects written, in order.
    let (_, written) = open_bytes_for_save(&out).map_err(|_| SaveError::CheckFailed { detail: "doesn't open" })?;
    if written.len() != expected.len() || written.iter().zip(&expected).any(|(entry, id)| entry.id() != *id) {
        return Err(SaveError::CheckFailed { detail: "pages out of place" });
    }
    Ok(expected.len())
}

/// **The rewrite's planner on bytes in memory, for the fuzzer:** `target` with its first page deleted, and with `added`'s pages after the rest when `with_added`,
/// written by the rewrite into memory, checked by the fork's own verify and read back. Gives back the pages the result has. The same plan and sink a save
/// uses, with no file. A target of one page has nothing left to delete, so it's refused as a save would refuse it.
pub fn rewrite_in_memory(target: &[u8], added: &[u8], with_added: bool) -> Result<usize, SaveError> {
    let (reader, map) = open_bytes_for_save(target)?;
    reader.check_rewritable().map_err(from_rewrite)?;
    let pages = u32::try_from(map.len()).map_err(|_| SaveError::Damaged)?;
    let (source, source_map) = open_bytes_for_save(added)?;
    let added_pages = if with_added { u32::try_from(source_map.len()).map_err(|_| SaveError::Damaged)?.min(64) } else { 0 };
    let mut order: Vec<PageRef> = (1..pages).map(|page| PageRef { source: 0, page, quarter_turns: u8::from(page == 1) }).collect();
    order.extend((0..added_pages).map(|page| PageRef { source: 1, page, quarter_turns: 3 }));
    if order.is_empty() {
        return Err(SaveError::NoPagesLeft);
    }
    let added_file = [AddedFile { path: Path::new("added.pdf"), page_count: added_pages, stamp: None }];
    let request = SaveRequest { path: Path::new("target.pdf"), stamp: None, file_pages: pages, pages: &order, added: &added_file };
    let sources = vec![Some((source, source_map))];
    let plan = Plan::make(&reader, &map, &request, &sources, signature_facts(&reader), Writer::Rewrite, MAX_COPY_WORK)?;
    let mut changes = RewriteChanges::new(&reader).map_err(from_rewrite)?;
    changes.remove_page(0);
    let expected = plan.write(&reader, &sources, &mut RewriteSink { changes: &mut changes, held: 0, cap: plan.append_cap.min(MAX_REWRITE_HELD_BYTES) })?;
    let mut out = Vec::new();
    let unverified = changes.write_unverified_to(RewritePolicy::new().remove_signatures().remove_usage_rights(), &mut out).map_err(from_rewrite)?;
    let bytes: std::sync::Arc<[u8]> = out.into();
    unverified.verify(&reader, std::sync::Arc::new(lopdf::BytesSource::new(std::sync::Arc::clone(&bytes)))).map_err(from_rewrite)?;
    let (_, written) = open_bytes_for_save(&bytes).map_err(|_| SaveError::CheckFailed { detail: "doesn't open" })?;
    if written.len() != expected.len() || written.iter().zip(&expected).any(|(entry, id)| entry.id() != *id) {
        return Err(SaveError::CheckFailed { detail: "pages out of place" });
    }
    Ok(expected.len())
}

/// **The signature rules and the kept copy on bytes in memory, for the fuzzer:** what `save` decides about a file's signatures, and the rewrite `keep_pages`
/// runs for these pages, through the fork's own verify, with the pages held compared one for one. Gives back how many pages the copy holds.
pub fn signatures_and_keep_in_memory(bytes: &[u8], pages: &[u32]) -> Result<usize, SaveError> {
    let (reader, map) = open_bytes_for_save(bytes)?;
    let facts = signature_facts(&reader);
    let _ = (facts.has_persons_signature(), persons_signature(&reader));
    reader.check_rewritable().map_err(from_rewrite)?;
    let count = u32::try_from(map.len()).map_err(|_| SaveError::Damaged)?;
    let keep: HashSet<u32> = pages.iter().copied().filter(|page| *page < count).collect();
    if keep.is_empty() {
        return Err(SaveError::NoPagesLeft);
    }
    let mut changes = RewriteChanges::new(&reader).map_err(from_rewrite)?;
    for page in (0..count).filter(|page| !keep.contains(page)) {
        changes.remove_page(page as usize);
    }
    let mut out = Vec::new();
    let unverified = changes.write_unverified_to(RewritePolicy::new().remove_signatures().remove_usage_rights(), &mut out).map_err(from_rewrite)?;
    let written: std::sync::Arc<[u8]> = out.into();
    let (_, copy) = unverified.verify(&reader, std::sync::Arc::new(lopdf::BytesSource::new(written))).map_err(from_rewrite)?;
    Ok(copy.page_map().map(|map| map.len()).unwrap_or(0))
}

/// Each added PDF that a page of the document comes from, opened and checked against what was known of it. One no page uses isn't opened.
fn open_added(request: &SaveRequest<'_>) -> Result<Vec<Opened>, SaveError> {
    let mut used = vec![false; request.added.len()];
    for page in request.pages.iter().filter(|page| page.source != 0) {
        match used.get_mut(usize::from(page.source) - 1) {
            Some(slot) => *slot = true,
            None => return Err(SaveError::EditsDontMatch { file_pages: request.file_pages }),
        }
    }
    let mut opened = Vec::with_capacity(request.added.len());
    for (added, used) in request.added.iter().zip(used) {
        if !used {
            opened.push(None);
            continue;
        }
        let name = file_label(added.path);
        let changed = || SaveError::AddedFileChanged { name: name.clone() };
        let known = added.stamp.ok_or_else(changed)?;
        let now = recto_platform::file_stamp(added.path).map_err(|_| changed())?;
        if !now.same_contents(known) || now.inode != known.inode || now.device != known.device {
            return Err(changed());
        }
        let reader = match open_for_save(added.path) {
            Ok(reader) => reader,
            Err(SaveError::Protected) => return Err(SaveError::AddedFileProtected { name }),
            Err(_) => return Err(SaveError::AddedFileUnreadable { name }),
        };
        let map = reader.page_map().map_err(|_| SaveError::AddedFileUnreadable { name: name.clone() })?;
        if map.len() != added.page_count as usize {
            return Err(changed());
        }
        opened.push(Some((reader, map)));
    }
    Ok(opened)
}

/// A page's own turn, as the file has it: a multiple of 90, from 0 to 270. Anything else is read as 0, as PDFium reads it.
fn own_rotation(reader: &IndexedReader, entry: &PageMapEntry) -> i64 {
    let Some(owner) = entry.inherited().rotate() else { return 0 };
    let Ok(Object::Dictionary(dict)) = reader.resolve_object(owner) else { return 0 };
    let value = match dict.get(b"Rotate") {
        Ok(Object::Reference(id)) => reader.resolve_object(*id).ok(),
        Ok(other) => Some(other.clone()),
        Err(_) => None,
    };
    match value.and_then(|value| value.as_i64().ok()) {
        Some(degrees) if degrees % 90 == 0 => degrees.rem_euclid(360),
        _ => 0,
    }
}

/// An attribute a page takes from above it, as its own value, so a page that's moved to a new tree draws as it did.
fn inherited_value(reader: &IndexedReader, owner: Option<ObjectId>, page: ObjectId, key: &[u8]) -> Option<Object> {
    let owner = owner.filter(|owner| *owner != page)?;
    match reader.resolve_object(owner).ok()? {
        Object::Dictionary(dict) => dict.get(key).ok().cloned(),
        _ => None,
    }
}

/// What a save will write: worked out from the files before anything's written, so that the writing itself has nothing left to refuse.
struct Plan {
    /// A new page tree, as pages were moved or added. Otherwise only turned pages are written again.
    new_tree: Option<ObjectId>,
    /// For each page of the document, in order: where it comes from and its turn in degrees as written.
    pages: Vec<(PageRef, i64)>,
    /// For each added PDF, what's copied from it: its object numbers, in order, and which are dropped.
    copies: Vec<Option<Copy>>,
    drop_usage_rights: bool,
    missing: u64,
    /// The most an update may append: twice what's saved and added, and 64 MiB more.
    append_cap: u64,
    /// Added pages' fields couldn't join the document's form, which can't be read: they draw, and can't be filled.
    fields_left_out: bool,
}

/// What's copied from one added PDF.
#[derive(Default)]
struct Copy {
    /// Source object number to what it becomes: `None` for one that goes, as it's another page, the source's page tree or its catalog, or can't be read.
    map: HashMap<u32, Option<()>>,
    /// The ones kept, in the order they were found.
    order: Vec<ObjectId>,
    /// The form fields its pages' widgets belong to, which come along so they can be filled.
    fields: FormFields,
}

/// **The form fields of added pages** (save review round 1: IRS forms added to IRS forms drew their fields, which then couldn't be filled, as qpdf says "widget
/// annotation is not reachable from /AcroForm"). Each widget on an added page keeps its field and the field's parents, up to a top field, which joins the
/// target's `/AcroForm /Fields`. A field's `/Kids` keep only what's on added pages, so a field's other pages don't come along. A top field whose name another
/// already has is renamed, so their values stay apart. Signature fields aren't brought: a signature from another document would only be a broken one here.
#[derive(Default)]
struct FormFields {
    /// For each field node brought, its kids that come too.
    kids: HashMap<u32, Vec<Object>>,
    /// Widgets and fields whose `/Parent` stays.
    keep_parent: HashSet<u32>,
    /// Top fields, in the order found.
    tops: Vec<ObjectId>,
    /// New names for top fields whose names were taken.
    renamed: HashMap<u32, Vec<u8>>,
    /// The source form's default resources' fonts, which fields' `/DA` names.
    fonts: Option<Dictionary>,
    /// Signed signature widgets on added pages, which don't come along. A blank one, a place to sign, comes as any field does.
    signature_widgets: HashSet<u32>,
}

impl FormFields {
    /// What a source object becomes, as far as fields go: a field node's kids filtered and its name changed if it was taken.
    fn adjusted(&self, id: u32, object: Object) -> Object {
        let Object::Dictionary(mut dict) = object else { return object };
        if let Some(kids) = self.kids.get(&id) {
            dict.set("Kids", Object::Array(kids.clone()));
        }
        if let Some(name) = self.renamed.get(&id) {
            dict.set("T", Object::String(name.clone(), lopdf::StringFormat::Literal));
        }
        Object::Dictionary(dict)
    }
}

/// What a save adds to the target's form.
#[derive(Default)]
struct FormAdditions {
    /// Top fields of added pages, at their new numbers.
    fields: Vec<ObjectId>,
    /// Fonts for the form's default resources, at their new numbers.
    fonts: Dictionary,
}

/// One field node, as [`form_fields`] needs it.
#[derive(Clone, Copy)]
struct Node {
    parent: Option<ObjectId>,
    /// `/FT /Sig`.
    sig: bool,
    /// It has `/T` or `/FT`.
    field: bool,
    /// It has a `/V`.
    valued: bool,
    /// Its `/V` is a dictionary with a `/ByteRange`.
    signature_value: bool,
}

/// Fields walked up from one widget, and nodes in all. Real forms have a few levels and hundreds of fields.
const MAX_FIELD_DEPTH: usize = 64;
const MAX_FIELD_NODES: usize = 100_000;

/// The fields the widgets on these pages belong to. A widget that's a field of its own with no parent is a top field itself. **A signed signature widget goes:** a
/// signature from another document would only be a broken one here, and outside the form, where nothing that decides about signatures looks, it would still make
/// the file read as signed by its bytes. A field whose value has a `/ByteRange` counts as signed, whatever its `/FT`. A blank signature field, with no value, is
/// a place to sign and comes as any other field does, as the fork keeps blank ones through a rewrite (review round 2). The page's own drawn content stays. Bounded by the bytes looked at, every node resolved once, and what can't be read is
/// passed over, leaving that widget as it was: drawn, and not fillable.
fn form_fields(source: &IndexedReader, map: &lopdf::PageMap, wanted: &[u32], work: &mut u64) -> Result<FormFields, SaveError> {
    let mut fields = FormFields::default();
    let mut tops: HashSet<u32> = HashSet::new();
    let mut seen_kids: HashSet<(u32, u32)> = HashSet::new();
    // Each node looked at once: its parent, whether it says it's a signature field, whether it says it's a field at all, whether it has a value, and whether
    // that value is a signature's.
    let mut nodes: HashMap<u32, Option<Node>> = HashMap::new();
    let mut node = |id: ObjectId, work: &mut u64| -> Result<Option<Node>, SaveError> {
        if let Some(known) = nodes.get(&id.0) {
            return Ok(*known);
        }
        if nodes.len() >= MAX_FIELD_NODES {
            return Err(SaveError::TooLarge);
        }
        let info = match source.resolve_object(id) {
            Ok(object @ Object::Dictionary(_)) => {
                charge(work, held_size(&object, 0) / 32)?;
                let Object::Dictionary(dict) = object else { unreachable!() };
                let parent = dict.get(b"Parent").ok().and_then(|parent| parent.as_reference().ok());
                let sig = matches!(dict.get(b"FT"), Ok(Object::Name(kind)) if kind == b"Sig");
                if matches!(dict.get(b"V"), Ok(Object::Reference(_))) {
                    charge(work, 1)?;
                }
                Some(Node { parent, sig, field: dict.has(b"T") || dict.has(b"FT"), valued: dict.has(b"V"), signature_value: value_is_signature(source, &dict) })
            }
            _ => None,
        };
        nodes.insert(id.0, info);
        Ok(info)
    };
    let mut seen_pages = HashSet::new();
    for page in wanted {
        let Some(entry) = map.get(*page as usize) else { continue };
        if !seen_pages.insert(entry.id().0) {
            continue;
        }
        let Ok(Object::Dictionary(page_dict)) = source.resolve_object(entry.id()) else { continue };
        let annots = match page_dict.get(b"Annots") {
            Ok(Object::Reference(id)) => source.resolve_object(*id).ok(),
            Ok(other) => Some(other.clone()),
            Err(_) => None,
        };
        let Some(Object::Array(annots)) = annots else { continue };
        for annot in annots {
            charge(work, 1)?;
            let Object::Reference(widget) = annot else { continue };
            let Ok(Object::Dictionary(dict)) = source.resolve_object(widget) else { continue };
            if !matches!(dict.get(b"Subtype"), Ok(Object::Name(kind)) if kind == b"Widget") {
                continue;
            }
            // Up the chain, each node once.
            let mut chain: Vec<ObjectId> = vec![widget];
            let mut signature = false;
            let mut valued = false;
            let mut signed = false;
            let mut a_field = false;
            let mut readable_to_top = true;
            let mut at = widget;
            loop {
                let Some(Node { parent, sig, field, valued: has_value, signature_value }) = node(at, work)? else {
                    readable_to_top = false;
                    break;
                };
                signature |= sig;
                valued |= has_value;
                signed |= signature_value;
                a_field |= field;
                match parent {
                    Some(parent) if chain.len() <= MAX_FIELD_DEPTH && !chain.contains(&parent) => {
                        chain.push(parent);
                        at = parent;
                    }
                    Some(_) => {
                        readable_to_top = false;
                        break;
                    }
                    None => break,
                }
            }
            // `/FT` and `/V` are both inherited, so a signature field with a value anywhere up its chain is signed.
            if signed || (signature && valued) {
                fields.signature_widgets.insert(widget.0);
                continue;
            }
            if !(a_field || chain.len() > 1) || !readable_to_top {
                // A widget of no field, or a chain that couldn't be read to its top: left as it was.
                continue;
            }
            for pair in chain.windows(2) {
                let (child, parent) = (pair[0], pair[1]);
                if seen_kids.insert((parent.0, child.0)) {
                    fields.kids.entry(parent.0).or_default().push(Object::Reference(child));
                }
                fields.keep_parent.insert(child.0);
            }
            let top = *chain.last().expect("a chain has its widget");
            if tops.insert(top.0) {
                fields.tops.push(top);
            }
        }
    }
    if !fields.tops.is_empty() {
        let form = catalog(source).and_then(|root| root.get(b"AcroForm").ok().cloned()).and_then(|form| dict_value(source, &form));
        fields.fonts = form.and_then(|form| form.get(b"DR").ok().cloned()).and_then(|dr| dict_value(source, &dr)).and_then(|dr| dr.get(b"Font").ok().cloned()).and_then(|fonts| dict_value(source, &fonts));
    }
    Ok(fields)
}

/// Most renames tried for one field's name.
const MAX_RENAMES: u32 = 1_000;

/// **A field name as a reader compares it:** decoded, so `(Name)` in PDFDocEncoding and the same name in UTF-16 are one name (review round 2). A name that won't
/// decode is kept as its bytes, behind a byte UTF-8 never has, so it can't meet a decoded one.
fn name_key(name: &[u8]) -> Vec<u8> {
    match lopdf::decode_text_string(&Object::String(name.to_vec(), lopdf::StringFormat::Literal)) {
        Ok(text) => text.into_bytes(),
        Err(_) => [&[0xFF], name].concat(),
    }
}

/// `name`, or `name_2`, `name_3`… if that's taken, in the name's own encoding: a UTF-16 name stays UTF-16. `taken` holds names as [`name_key`] gives them.
/// `None` past [`MAX_RENAMES`], as settling on a taken name would merge the added field into the document's own and change its value.
fn free_name(name: &[u8], taken: &HashSet<Vec<u8>>) -> Option<Vec<u8>> {
    if !taken.contains(&name_key(name)) {
        return Some(name.to_vec());
    }
    let utf16 = name.starts_with(&[0xFE, 0xFF]);
    (2..=MAX_RENAMES).map(|n| {
        let suffix = format!("_{n}");
        let tail: Vec<u8> = if utf16 { suffix.encode_utf16().flat_map(u16::to_be_bytes).collect() } else { suffix.into_bytes() };
        [name, tail.as_slice()].concat()
    }).find(|candidate| !taken.contains(&name_key(candidate)))
}

/// The names of a form's top fields, as [`name_key`] gives them, each field looked at once, within the bound on fields. `None` if the form or its list can't be read, so nothing's
/// said to be free that isn't.
fn top_field_names(reader: &IndexedReader, work: &mut u64) -> Result<Option<HashSet<Vec<u8>>>, SaveError> {
    let Some(form_value) = catalog(reader).and_then(|root| root.get(b"AcroForm").ok().cloned()) else { return Ok(Some(HashSet::new())) };
    let Some(form) = dict_value(reader, &form_value) else { return Ok(None) };
    let fields = match form.get(b"Fields") {
        Ok(Object::Reference(id)) => reader.resolve_object(*id).ok(),
        Ok(other) => Some(other.clone()),
        Err(_) => return Ok(Some(HashSet::new())),
    };
    let Some(Object::Array(fields)) = fields else { return Ok(None) };
    if fields.len() > MAX_FIELD_NODES {
        return Err(SaveError::TooLarge);
    }
    let mut seen = HashSet::new();
    let mut names = HashSet::new();
    for field in &fields {
        if let Object::Reference(id) = field
            && !seen.insert(id.0)
        {
            continue;
        }
        let Some(dict) = dict_value(reader, field) else { continue };
        charge(work, held_size(&Object::Dictionary(dict.clone()), 0) / 32)?;
        if let Some(name) = dict.get(b"T").ok().and_then(|name| name.as_str().ok()) {
            names.insert(name_key(name));
        }
    }
    Ok(Some(names))
}

impl Plan {
    fn make(reader: &IndexedReader, map: &lopdf::PageMap, request: &SaveRequest<'_>, sources: &[Opened], facts: SignatureFacts, writer: Writer, work_limit: u64) -> Result<Self, SaveError> {
        // The file's own pages in their own order, some perhaps deleted: the tree stays, and a rewrite takes the deleted ones out of it. Anything else is a new tree.
        let in_order = request.pages.iter().all(|page| page.source == 0) && request.pages.windows(2).all(|pair| pair[0].page < pair[1].page) && (writer == Writer::Rewrite || request.pages.len() == map.len());
        let tree = if in_order {
            None
        } else {
            let root = catalog(reader).ok_or(SaveError::Damaged)?;
            Some(root.get(b"Pages").and_then(Object::as_reference).map_err(|_| SaveError::Damaged)?)
        };
        let mut pages = Vec::with_capacity(request.pages.len());
        for page in request.pages {
            let base = if page.source == 0 {
                map.get(page.page as usize).map_or(0, |entry| own_rotation(reader, entry))
            } else {
                match sources.get(usize::from(page.source) - 1).and_then(Option::as_ref) {
                    Some((source, source_map)) => source_map.get(page.page as usize).map_or(0, |entry| own_rotation(source, entry)),
                    None => 0,
                }
            };
            pages.push((*page, (base + 90 * i64::from(page.quarter_turns)).rem_euclid(360)));
        }
        let mut copies: Vec<Option<Copy>> = (0..sources.len()).map(|_| None).collect();
        let mut missing = 0;
        let mut total = 0;
        // Counted down from the bound, so a smaller one can be asked for.
        let mut work = MAX_COPY_WORK.saturating_sub(work_limit);
        for (at, opened) in sources.iter().enumerate() {
            let Some((source, source_map)) = opened else { continue };
            let wanted: Vec<u32> = request.pages.iter().filter(|page| usize::from(page.source) == at + 1).map(|page| page.page).collect();
            let copy = discover(source, source_map, &wanted, &mut missing, MAX_COPIED_OBJECTS.saturating_sub(total), &mut work).map_err(|error| match error {
                SaveError::TooLarge => SaveError::TooLarge,
                _ => SaveError::AddedFileUnreadable { name: file_label(request.added[at].path) },
            })?;
            total += copy.order.len();
            copies[at] = Some(copy);
        }
        // A top field named as one the document has, or one added before it, is renamed `name_2`, `name_3`…, so their values stay apart.
        // Looked at only when there's a field to add. A form of the document's own that can't be read isn't replaced: the added widgets stay unfillable, and that's said.
        let mut fields_left_out = false;
        if copies.iter().flatten().any(|copy| !copy.fields.tops.is_empty()) {
            match top_field_names(reader, &mut work)? {
                None => {
                    fields_left_out = true;
                    for copy in copies.iter_mut().flatten() {
                        copy.fields.tops.clear();
                    }
                }
                Some(mut taken) => {
                    for (at, copy) in copies.iter_mut().enumerate() {
                        let (Some(copy), Some((source, _))) = (copy.as_mut(), sources.get(at).and_then(Option::as_ref)) else { continue };
                        for top in copy.fields.tops.clone() {
                            let Some(name) = dict_value(source, &Object::Reference(top)).and_then(|field| field.get(b"T").ok().and_then(|name| name.as_str().ok()).map(<[u8]>::to_vec)) else { continue };
                            let chosen = free_name(&name, &taken).ok_or(SaveError::FieldNamesTaken)?;
                            if chosen != name {
                                copy.fields.renamed.insert(top.0, chosen.clone());
                            }
                            taken.insert(name_key(&chosen));
                        }
                    }
                }
            }
        }
        let sizes: u64 = reader.source_len() + sources.iter().flatten().map(|(source, _)| source.source_len()).sum::<u64>();
        // A rewrite's policy takes usage rights off, with whatever shows of them; an append takes them out of the new revision's catalog.
        let drop_usage_rights = writer == Writer::Append && facts.usage_rights && !facts.certified;
        Ok(Self { new_tree: tree, pages, copies, drop_usage_rights, missing, append_cap: sizes.saturating_mul(2).saturating_add(64 * 1024 * 1024), fields_left_out })
    }

    /// Writes the plan into an update or a rewrite's changes. Gives back the document's page objects in order, for reading the result back.
    fn write<'c>(&self, reader: &IndexedReader, sources: &'c [Opened], writer: &mut impl Sink<'c>) -> Result<Vec<ObjectId>, SaveError> {
        let map = reader.page_map().map_err(|_| SaveError::Damaged)?;
        // Bounded by what's being saved: a small file's pages rewritten under a new tree can't append gigabytes (review).
        let cap = self.append_cap;
        let within = |writer: &dyn Sink<'c>| if writer.taken() > cap { Err(SaveError::TooLarge) } else { Ok(()) };
        // An inherited value written in place, as a `/Resources` dictionary on the root of the tree can be, goes in once as an object of its own that every
        // page names: otherwise each page would carry its own copy, and 200,000 pages under a 1 MiB dictionary would append 200 GB (review).
        let mut shared: HashMap<(usize, ObjectId, &'static [u8]), Object> = HashMap::new();
        // New numbers for everything copied, so references between copies can be written in any order.
        let mut renumbered: Vec<HashMap<u32, ObjectId>> = Vec::with_capacity(self.copies.len());
        for copy in &self.copies {
            let mut numbers = HashMap::new();
            if let Some(copy) = copy {
                for id in &copy.order {
                    numbers.insert(id.0, writer.allocate()?);
                }
            }
            renumbered.push(numbers);
        }
        let mut ids = Vec::with_capacity(self.pages.len());
        let mut used_own: HashSet<ObjectId> = HashSet::new();
        let mut used_copy: Vec<HashSet<u32>> = vec![HashSet::new(); self.copies.len()];
        for (page, degrees) in &self.pages {
            within(&*writer)?;
            if page.source == 0 {
                let entry = map.get(page.page as usize).ok_or(SaveError::Damaged)?;
                let id = entry.id();
                let turned = *degrees != own_rotation(reader, entry);
                let Some(tree) = self.new_tree else {
                    if turned {
                        let mut dict = page_dict(reader, id)?;
                        dict.set("Rotate", Object::Integer(*degrees));
                        writer.put(id, Object::Dictionary(dict))?;
                    }
                    ids.push(id);
                    continue;
                };
                let mut dict = page_dict(reader, id)?;
                let inherited = entry.inherited();
                for (key, owner) in [(&b"Resources"[..], inherited.resources()), (b"MediaBox", inherited.media_box()), (b"CropBox", inherited.crop_box())] {
                    if !dict.has(key)
                        && let Some(owner) = owner.filter(|owner| *owner != id)
                        && let Some(value) = inherited_value(reader, Some(owner), id, key)
                    {
                        let value = share(writer, &mut shared, (0, owner, key), value)?;
                        dict.set(key, value);
                    }
                }
                dict.set("Parent", Object::Reference(tree));
                dict.set("Rotate", Object::Integer(*degrees));
                // A page the tree lists twice is one object in the file, and can't take one new version for each place: the second is a copy of it.
                if used_own.insert(id) {
                    writer.put(id, Object::Dictionary(dict))?;
                    ids.push(id);
                } else {
                    ids.push(writer.add(Object::Dictionary(dict))?);
                }
            } else {
                let at = usize::from(page.source) - 1;
                let (Some((source, source_map)), Some(copy)) = (sources.get(at).and_then(Option::as_ref), self.copies.get(at).and_then(Option::as_ref)) else { return Err(SaveError::Damaged) };
                let entry = source_map.get(page.page as usize).ok_or(SaveError::Damaged)?;
                let numbers = &renumbered[at];
                let new_id = *numbers.get(&entry.id().0).ok_or(SaveError::Damaged)?;
                let mut dict = copied_page(source, entry)?;
                dict = match remap(&Object::Dictionary(dict), copy, numbers) {
                    Some(Object::Dictionary(dict)) => dict,
                    _ => return Err(SaveError::Damaged),
                };
                // What didn't come along, a signature widget among them, is out of its list.
                if let Ok(Object::Array(items)) = dict.get_mut(b"Annots") {
                    items.retain(|item| !matches!(item, Object::Null));
                }
                let inherited = entry.inherited();
                for (key, owner) in [(&b"Resources"[..], inherited.resources()), (b"MediaBox", inherited.media_box()), (b"CropBox", inherited.crop_box())] {
                    if let Some(owner) = owner.filter(|owner| *owner != entry.id())
                        && let Some(value) = dict.get(key).ok().cloned()
                    {
                        let value = share(writer, &mut shared, (at + 1, owner, key), value)?;
                        dict.set(key, value);
                    }
                }
                if let Some(tree) = self.new_tree {
                    dict.set("Parent", Object::Reference(tree));
                }
                dict.set("Rotate", Object::Integer(*degrees));
                if used_copy[at].insert(entry.id().0) {
                    writer.put(new_id, Object::Dictionary(dict))?;
                    ids.push(new_id);
                } else {
                    ids.push(writer.add(Object::Dictionary(dict))?);
                }
            }
        }
        // What's copied besides the pages themselves.
        for (at, copy) in self.copies.iter().enumerate() {
            let (Some(copy), Some((source, source_map))) = (copy, sources.get(at).and_then(Option::as_ref)) else { continue };
            let pages: HashSet<u32> = source_map.iter().map(|entry| entry.id().0).collect();
            let numbers = &renumbered[at];
            for id in &copy.order {
                within(&*writer)?;
                if pages.contains(&id.0) {
                    if !used_copy[at].contains(&id.0) {
                        // A page found only as something another page names, such as a link's target, isn't in this document: it goes as nothing, as remap has it. Its number was taken, so it's written as null.
                        writer.put(numbers[&id.0], Object::Null)?;
                    }
                    continue;
                }
                let new_id = numbers[&id.0];
                if let Ok(descriptor) = source.resolve_stream_descriptor(*id) {
                    let mut dict = match remap(&Object::Dictionary(descriptor.dictionary().clone()), copy, numbers) {
                        Some(Object::Dictionary(dict)) => dict,
                        _ => return Err(SaveError::Damaged),
                    };
                    dict.remove(b"Length");
                    let length = descriptor.encoded_len().ok_or(SaveError::Damaged)?;
                    drop(descriptor);
                    writer.put_stream(new_id, dict, length, source, *id)?;
                } else {
                    let object = source.resolve_object(*id).map_err(|_| SaveError::Damaged)?;
                    let object = remap_copied(id.0, object, copy, numbers).unwrap_or(Object::Null);
                    writer.put(new_id, object)?;
                }
            }
        }
        if let Some(tree) = self.new_tree {
            let mut node = Dictionary::new();
            node.set("Type", Object::Name(b"Pages".to_vec()));
            node.set("Kids", Object::Array(ids.iter().map(|id| Object::Reference(*id)).collect()));
            node.set("Count", Object::Integer(i64::try_from(ids.len()).map_err(|_| SaveError::TooLarge)?));
            writer.put(tree, Object::Dictionary(node))?;
        }
        // Layers the added pages draw with, and which of them their own files hide, so the saved file hides them too.
        let mut layers: Vec<(ObjectId, bool)> = Vec::new();
        for (at, copy) in self.copies.iter().enumerate() {
            let (Some(_), Some((source, _))) = (copy, sources.get(at).and_then(Option::as_ref)) else { continue };
            let (all, off) = layer_states(source);
            let numbers = &renumbered[at];
            for id in all {
                if let Some(new_id) = numbers.get(&id) {
                    layers.push((*new_id, off.contains(&id)));
                }
            }
        }
        // The added pages' top fields join the form, with the fonts their appearances name.
        let mut form = FormAdditions::default();
        for (at, copy) in self.copies.iter().enumerate() {
            let Some(copy) = copy else { continue };
            let numbers = &renumbered[at];
            form.fields.extend(copy.fields.tops.iter().filter_map(|top| numbers.get(&top.0).copied()));
            if let Some(Object::Dictionary(fonts)) = copy.fields.fonts.as_ref().and_then(|fonts| remap(&Object::Dictionary(fonts.clone()), copy, numbers)) {
                for (name, font) in fonts.iter() {
                    if !form.fonts.has(name) {
                        form.fonts.set(name.clone(), font.clone());
                    }
                }
            }
        }
        if self.drop_usage_rights || !layers.is_empty() || !form.fields.is_empty() {
            update_catalog(reader, writer, self.drop_usage_rights, &layers, &form)?;
        }
        // A copy of the document as it is, for a Save As with nothing unsaved: an update has to hold something, and the catalog as it was changes nothing.
        within(&*writer)?;
        if writer.taken() == 0 && writer.needs_something() {
            let root = catalog_id(reader).ok_or(SaveError::Damaged)?;
            writer.put(root, Object::Dictionary(page_dict(reader, root)?))?;
        }
        Ok(ids)
    }
}

/// An inherited value as a page names it: a reference stays one, and a value written in place goes in once, as an object of its own, for every page that inherits it.
fn share<'c>(writer: &mut impl Sink<'c>, shared: &mut HashMap<(usize, ObjectId, &'static [u8]), Object>, key: (usize, ObjectId, &'static [u8]), value: Object) -> Result<Object, SaveError> {
    if matches!(value, Object::Reference(_)) {
        return Ok(value);
    }
    if let Some(known) = shared.get(&key) {
        return Ok(known.clone());
    }
    let written = Object::Reference(writer.add(value)?);
    shared.insert(key, written.clone());
    Ok(written)
}

fn page_dict(reader: &IndexedReader, id: ObjectId) -> Result<Dictionary, SaveError> {
    match reader.resolve_object(id) {
        Ok(Object::Dictionary(dict)) => Ok(dict),
        _ => Err(SaveError::Damaged),
    }
}

/// An added page as it's copied: what it takes from above it made its own, and what ties it to its old document taken off.
fn copied_page(source: &IndexedReader, entry: &PageMapEntry) -> Result<Dictionary, SaveError> {
    let id = entry.id();
    let mut dict = page_dict(source, id)?;
    let inherited = entry.inherited();
    for (key, owner) in [(&b"Resources"[..], inherited.resources()), (b"MediaBox", inherited.media_box()), (b"CropBox", inherited.crop_box())] {
        if !dict.has(key)
            && let Some(value) = inherited_value(source, owner, id, key)
        {
            dict.set(key, value);
        }
    }
    // Its tree, its place in the old document's structure and its article beads are all the old document's. `/PieceInfo` is another app's private data,
    // which can hold a whole second copy of what was drawn, and nothing draws from it.
    for key in [&b"Parent"[..], b"StructParents", b"B", b"Rotate", b"PieceInfo"] {
        dict.remove(key);
    }
    // Its annotations as a list of its own, so one that doesn't come along can be taken out of it rather than left as a null.
    if let Ok(Object::Reference(list)) = dict.get(b"Annots").cloned()
        && let Ok(Object::Array(items)) = source.resolve_object(list)
    {
        dict.set("Annots", Object::Array(items));
    }
    Ok(dict)
}

/// What an annotation names that belongs to its old document: its page, its form field and its place in the structure. Without them it still draws, from its own appearance.
fn is_annotation(dict: &Dictionary) -> bool {
    dict.has(b"Rect") && dict.has(b"Subtype")
}

/// Whether this entry of a dictionary is left out of a copy.
fn left_behind(annotation: bool, key: &[u8]) -> bool {
    annotation && matches!(key, b"P" | b"Parent" | b"StructParent")
}

/// Everything the wanted pages of an added PDF reach, found before anything's written. Another page of that PDF, its page tree and its catalog aren't followed: a link or a bookmark would otherwise bring its whole document along. What can't be read refuses the save. **Bounded for hostile files:** an id is queued once, when it's first seen, and every value looked at counts against [`MAX_COPY_WORK`], so a page that names millions of references, or objects that each name all the others, is refused with [`SaveError::TooLarge`] rather than held. Nothing is cloned to be looked at.
fn discover(source: &IndexedReader, map: &lopdf::PageMap, wanted: &[u32], missing: &mut u64, room: usize, work: &mut u64) -> Result<Copy, SaveError> {
    let wanted_ids: HashSet<u32> = wanted.iter().filter_map(|page| map.get(*page as usize)).map(|entry| entry.id().0).collect();
    let all_pages: HashSet<u32> = map.iter().map(|entry| entry.id().0).collect();
    let mut copy = Copy { fields: form_fields(source, map, wanted, work)?, ..Copy::default() };
    let mut queued: HashSet<u32> = HashSet::new();
    let mut stack: Vec<ObjectId> = Vec::new();
    // A signature widget doesn't come along, nor does what only it names, its value with its byte range among them.
    for widget in &copy.fields.signature_widgets {
        copy.map.insert(*widget, None);
        queued.insert(*widget);
    }
    // The fonts the fields' default appearances name come along, for the form's own resources.
    if let Some(fonts) = &copy.fields.fonts {
        references_in(&Object::Dictionary(fonts.clone()), &mut stack, &mut queued, work)?;
    }
    for page in wanted {
        let entry = map.get(*page as usize).ok_or(SaveError::Damaged)?;
        if copy.order.len() >= room {
            return Err(SaveError::TooLarge);
        }
        if copy.map.insert(entry.id().0, Some(())).is_none() {
            queued.insert(entry.id().0);
            copy.order.push(entry.id());
            let dict = copied_page(source, entry)?;
            references_in(&Object::Dictionary(dict), &mut stack, &mut queued, work)?;
        }
    }
    while let Some(id) = stack.pop() {
        if copy.order.len() >= room {
            return Err(SaveError::TooLarge);
        }
        if all_pages.contains(&id.0) && !wanted_ids.contains(&id.0) {
            copy.map.insert(id.0, None);
            continue;
        }
        let object = match source.resolve_stream_descriptor(id) {
            Ok(descriptor) => {
                if descriptor.encoded_len().is_none() {
                    return Err(SaveError::Damaged);
                }
                Object::Dictionary(descriptor.dictionary().clone())
            }
            // A number the file's cross-reference data doesn't have is null, by ISO 32000, as every viewer reads it: it goes as nothing, and is counted. One that's
            // there and can't be read, as one past the 1 MiB limit, refuses the save: leaving it out would change the page unasked.
            Err(_) if source.object_location(id).is_none() => {
                *missing += 1;
                copy.map.insert(id.0, None);
                continue;
            }
            Err(_) => source.resolve_object(id).map_err(|_| SaveError::Damaged)?,
        };
        if let Object::Dictionary(dict) = &object
            && (dict.has_type(b"Pages") || dict.has_type(b"Catalog") || dict.has_type(b"Page"))
        {
            copy.map.insert(id.0, None);
            continue;
        }
        copy.map.insert(id.0, Some(()));
        copy.order.push(id);
        let object = copy.fields.adjusted(id.0, object);
        if copy.fields.keep_parent.contains(&id.0) {
            references_kept_parent(&object, &mut stack, &mut queued, work)?;
        } else {
            references_in(&object, &mut stack, &mut queued, work)?;
        }
    }
    Ok(copy)
}

/// Most values one save looks at and writes in copying pages from other PDFs: every number, name, reference and entry counts. Far past any real page, which has thousands; a hostile one that names millions is refused.
pub const MAX_COPY_WORK: u64 = 16_000_000;

fn charge(work: &mut u64, by: u64) -> Result<(), SaveError> {
    *work = work.saturating_add(by);
    if *work > MAX_COPY_WORK { Err(SaveError::TooLarge) } else { Ok(()) }
}

/// The references an object holds, as it will be written, each queued the first time it's seen: an annotation's ties to its old document aren't among them.
fn references_in(object: &Object, out: &mut Vec<ObjectId>, queued: &mut HashSet<u32>, work: &mut u64) -> Result<(), SaveError> {
    charge(work, 1)?;
    match object {
        Object::Reference(id) => {
            if queued.insert(id.0) {
                out.push(*id);
            }
        }
        Object::Array(items) => {
            for item in items {
                references_in(item, out, queued, work)?;
            }
        }
        Object::Dictionary(dict) => references_in_dict(dict, out, queued, work)?,
        Object::Stream(stream) => references_in_dict(&stream.dict, out, queued, work)?,
        _ => {}
    }
    Ok(())
}

/// As [`references_in`], for a widget or field whose field comes along: its `/Parent` is followed.
fn references_kept_parent(object: &Object, out: &mut Vec<ObjectId>, queued: &mut HashSet<u32>, work: &mut u64) -> Result<(), SaveError> {
    let Object::Dictionary(dict) = object else { return references_in(object, out, queued, work) };
    let annotation = is_annotation(dict);
    for (key, value) in dict.iter() {
        if key.as_slice() == b"Parent" || !left_behind(annotation, key) {
            references_in(value, out, queued, work)?;
        }
    }
    Ok(())
}

fn references_in_dict(dict: &Dictionary, out: &mut Vec<ObjectId>, queued: &mut HashSet<u32>, work: &mut u64) -> Result<(), SaveError> {
    let annotation = is_annotation(dict);
    for (key, value) in dict.iter() {
        if !left_behind(annotation, key) {
            references_in(value, out, queued, work)?;
        }
    }
    Ok(())
}

/// An object as it's written into the new file: references renumbered, and those to what doesn't come along dropped, from a dictionary as an entry and from an array as `null`, keeping the other places. `None` if the object itself is one that goes. Builds the copy once, cloning nothing on the way down.
fn remap(object: &Object, copy: &Copy, numbers: &HashMap<u32, ObjectId>) -> Option<Object> {
    match object {
        Object::Reference(id) => match copy.map.get(&id.0) {
            Some(Some(())) => numbers.get(&id.0).map(|id| Object::Reference(*id)),
            _ => None,
        },
        Object::Array(items) => Some(Object::Array(items.iter().map(|item| remap(item, copy, numbers).unwrap_or(Object::Null)).collect())),
        Object::Dictionary(dict) => Some(Object::Dictionary(remap_dict(dict, copy, numbers))),
        other => Some(other.clone()),
    }
}

/// An object copied as itself, `id` in its source: as [`remap`], with its field's changes, and its `/Parent` kept if its field comes along.
fn remap_copied(id: u32, object: Object, copy: &Copy, numbers: &HashMap<u32, ObjectId>) -> Option<Object> {
    let object = copy.fields.adjusted(id, object);
    match (&object, copy.fields.keep_parent.contains(&id)) {
        (Object::Dictionary(dict), true) => Some(Object::Dictionary(remap_dict_keeping(dict, copy, numbers, true))),
        _ => remap(&object, copy, numbers),
    }
}

fn remap_dict(dict: &Dictionary, copy: &Copy, numbers: &HashMap<u32, ObjectId>) -> Dictionary {
    remap_dict_keeping(dict, copy, numbers, false)
}

fn remap_dict_keeping(dict: &Dictionary, copy: &Copy, numbers: &HashMap<u32, ObjectId>, keep_parent: bool) -> Dictionary {
    let annotation = is_annotation(dict);
    let mut out = Dictionary::new();
    for (key, value) in dict.iter() {
        if (!left_behind(annotation, key) || (keep_parent && key.as_slice() == b"Parent"))
            && let Some(value) = remap(value, copy, numbers)
        {
            out.set(key.clone(), value);
        }
    }
    out
}

/// **Where a plan's objects go:** an appended update, or a rewrite's changes. The plan is the same for both, so what a save copies, renumbers and drops is
/// one piece of code whichever writer takes it.
trait Sink<'c> {
    fn allocate(&mut self) -> Result<ObjectId, SaveError>;
    fn put(&mut self, id: ObjectId, object: Object) -> Result<(), SaveError>;
    fn add(&mut self, object: Object) -> Result<ObjectId, SaveError>;
    /// A copied stream: `dict` as it's written, and `length` stored bytes of object `from` in `source`, copied as they are.
    fn put_stream(&mut self, id: ObjectId, dict: Dictionary, length: u64, source: &'c IndexedReader, from: ObjectId) -> Result<(), SaveError>;
    /// What this has taken so far: bytes appended, or about what a rewrite holds until it writes.
    fn taken(&self) -> u64;
    /// An update has to hold something; a rewrite doesn't.
    fn needs_something(&self) -> bool;
}

struct AppendSink<'w, 'r, W: lopdf::AppendOutput> {
    writer: &'w mut AppendWriter<'r, W>,
    began: u64,
}

impl<'c, W: lopdf::AppendOutput> Sink<'c> for AppendSink<'_, '_, W> {
    fn allocate(&mut self) -> Result<ObjectId, SaveError> {
        self.writer.allocate().map_err(from_append)
    }
    fn put(&mut self, id: ObjectId, object: Object) -> Result<(), SaveError> {
        self.writer.write_object(id, &object).map(|_| ()).map_err(from_append)
    }
    fn add(&mut self, object: Object) -> Result<ObjectId, SaveError> {
        self.writer.add_object(&object).map(|written| written.id).map_err(from_append)
    }
    fn put_stream(&mut self, id: ObjectId, dict: Dictionary, length: u64, source: &'c IndexedReader, from: ObjectId) -> Result<(), SaveError> {
        let descriptor = source.resolve_stream_descriptor(from).map_err(|_| SaveError::Damaged)?;
        let mut bytes = EncodedRead(descriptor.open_plain_encoded().map_err(|_| SaveError::Damaged)?);
        self.writer.write_stream_from(id, &dict, length, &mut bytes).map(|_| ()).map_err(from_append)
    }
    fn taken(&self) -> u64 {
        self.writer.position().saturating_sub(self.began)
    }
    fn needs_something(&self) -> bool {
        true
    }
}

/// A rewrite's changes, which it holds until it writes the file: what's held is counted, and a plan that would hold more than `cap` is refused.
struct RewriteSink<'w, 'r, 'c> {
    changes: &'w mut RewriteChanges<'r, 'c>,
    held: u64,
    cap: u64,
}

impl RewriteSink<'_, '_, '_> {
    fn hold(&mut self, object: &Object) -> Result<(), SaveError> {
        self.held = self.held.saturating_add(held_size(object, 0));
        if self.held > self.cap { Err(SaveError::TooLarge) } else { Ok(()) }
    }
}

impl<'c> Sink<'c> for RewriteSink<'_, '_, 'c> {
    fn allocate(&mut self) -> Result<ObjectId, SaveError> {
        self.changes.allocate().map_err(from_rewrite)
    }
    fn put(&mut self, id: ObjectId, object: Object) -> Result<(), SaveError> {
        self.hold(&object)?;
        self.changes.write_object(id, object).map_err(from_rewrite)
    }
    fn add(&mut self, object: Object) -> Result<ObjectId, SaveError> {
        self.hold(&object)?;
        self.changes.add_object(object).map_err(from_rewrite)
    }
    fn put_stream(&mut self, id: ObjectId, dict: Dictionary, length: u64, source: &'c IndexedReader, from: ObjectId) -> Result<(), SaveError> {
        self.hold(&Object::Dictionary(dict.clone()))?;
        // Opened only when the rewrite reaches it, and let go at its end, so a million copied streams hold no reader each until then.
        self.changes.write_stream_from(id, dict, length, LazyStream { source, id: from, open: None }).map_err(from_rewrite)
    }
    fn taken(&self) -> u64 {
        self.held
    }
    fn needs_something(&self) -> bool {
        false
    }
}

/// About what an object costs held in memory: a few dozen bytes for each value and its bytes. Only for a bound, so close is enough.
fn held_size(object: &Object, depth: usize) -> u64 {
    const EACH: u64 = 32;
    if depth > 128 {
        return EACH;
    }
    match object {
        Object::String(bytes, _) | Object::Name(bytes) => EACH + bytes.len() as u64,
        Object::Array(items) => EACH + items.iter().map(|item| held_size(item, depth + 1)).sum::<u64>(),
        Object::Dictionary(dict) => EACH + dict.iter().map(|(key, value)| EACH + key.len() as u64 + held_size(value, depth + 1)).sum::<u64>(),
        Object::Stream(stream) => EACH + held_size(&Object::Dictionary(stream.dict.clone()), depth + 1) + stream.content.len() as u64,
        _ => EACH,
    }
}

/// A copied stream's stored bytes, opened at the first read.
struct LazyStream<'c> {
    source: &'c IndexedReader,
    id: ObjectId,
    open: Option<EncodedRead>,
}

impl Read for LazyStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.open.is_none() {
            let descriptor = self.source.resolve_stream_descriptor(self.id).map_err(|_| std::io::Error::other("a stream's bytes couldn't be read"))?;
            self.open = Some(EncodedRead(descriptor.open_plain_encoded().map_err(|_| std::io::Error::other("a stream's bytes couldn't be read"))?));
        }
        let read = self.open.as_mut().map_or(Ok(0), |open| open.read(buf))?;
        if read == 0 {
            self.open = None;
        }
        Ok(read)
    }
}

/// A stream's stored bytes, read as they are, a piece at a time.
struct EncodedRead(EncodedStreamReader);

impl Read for EncodedRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read_chunk(buf).map_err(|_| std::io::Error::other("a stream's bytes couldn't be read"))
    }
}

/// A file's layers, by object number, and which of them it hides by default: `/OFF`, or all but `/ON` when its base state is off. Nothing for a file with none or one that can't be read, which then hides nothing.
fn layer_states(reader: &IndexedReader) -> (Vec<u32>, HashSet<u32>) {
    let Some(properties) = catalog(reader).and_then(|root| root.get(b"OCProperties").ok().cloned()).and_then(|value| dict_value(reader, &value)) else { return (Vec::new(), HashSet::new()) };
    let ids = |value: Option<&Object>| -> Vec<u32> {
        match value.cloned().map(|value| match value {
            Object::Reference(id) => reader.resolve_object(id).unwrap_or(Object::Null),
            other => other,
        }) {
            Some(Object::Array(items)) => items.iter().filter_map(|item| item.as_reference().ok().map(|id| id.0)).take(MAX_COPIED_OBJECTS).collect(),
            _ => Vec::new(),
        }
    };
    let all = ids(properties.get(b"OCGs").ok());
    let defaults = properties.get(b"D").ok().cloned().and_then(|value| dict_value(reader, &value)).unwrap_or_default();
    let base_off = matches!(defaults.get(b"BaseState"), Ok(Object::Name(name)) if name == b"OFF");
    let off: HashSet<u32> = if base_off {
        let on: HashSet<u32> = ids(defaults.get(b"ON").ok()).into_iter().collect();
        all.iter().copied().filter(|id| !on.contains(id)).collect()
    } else {
        ids(defaults.get(b"OFF").ok()).into_iter().collect()
    };
    (all, off)
}

/// **A new version of the catalog,** for what a save changes there. Usage rights go: any page edit ends them, a reader that finds them broken says so, and they aren't a person's signature (owner, 2026-11-08). Layers of added pages join the file's own, with the ones their files hide hidden here too: otherwise a layer a file hides, which can be what it keeps out of sight, would show on its page in this one.
fn update_catalog<'c>(reader: &IndexedReader, writer: &mut impl Sink<'c>, drop_rights: bool, layers: &[(ObjectId, bool)], form: &FormAdditions) -> Result<(), SaveError> {
    let root = catalog_id(reader).ok_or(SaveError::Damaged)?;
    let mut catalog = page_dict(reader, root)?;
    if !form.fields.is_empty() {
        // The form is a dictionary of its own or written in the catalog: a new version of whichever it is.
        let own = catalog.get(b"AcroForm").ok().and_then(|form| form.as_reference().ok());
        let mut acro = catalog.get(b"AcroForm").ok().cloned().and_then(|form| dict_value(reader, &form)).unwrap_or_default();
        // A list that's there and can't be read isn't replaced: the plan has left these fields out already, and this holds to it whatever's happened since.
        let mut fields = match acro.get(b"Fields") {
            Ok(Object::Reference(id)) => match reader.resolve_object(*id) {
                Ok(Object::Array(items)) => items,
                _ => return Err(SaveError::Damaged),
            },
            Ok(Object::Array(items)) => items.clone(),
            Ok(_) => return Err(SaveError::Damaged),
            Err(_) => Vec::new(),
        };
        fields.extend(form.fields.iter().map(|id| Object::Reference(*id)));
        acro.set("Fields", Object::Array(fields));
        if form.fonts.iter().next().is_some() {
            let mut resources = acro.get(b"DR").ok().cloned().and_then(|dr| dict_value(reader, &dr)).unwrap_or_default();
            let mut fonts = resources.get(b"Font").ok().cloned().and_then(|fonts| dict_value(reader, &fonts)).unwrap_or_default();
            for (name, font) in form.fonts.iter() {
                if !fonts.has(name) {
                    fonts.set(name.clone(), font.clone());
                }
            }
            resources.set("Font", Object::Dictionary(fonts));
            acro.set("DR", Object::Dictionary(resources));
        }
        match own {
            Some(id) => writer.put(id, Object::Dictionary(acro))?,
            None => catalog.set("AcroForm", Object::Dictionary(acro)),
        }
    }
    if drop_rights && let Some(mut perms) = catalog.get(b"Perms").ok().cloned().and_then(|perms| dict_value(reader, &perms)) {
        perms.remove(b"UR3");
        perms.remove(b"UR");
        if perms.iter().next().is_none() {
            catalog.remove(b"Perms");
        } else {
            catalog.set("Perms", Object::Dictionary(perms));
        }
    }
    if !layers.is_empty() {
        let mut properties = catalog.get(b"OCProperties").ok().cloned().and_then(|value| dict_value(reader, &value)).unwrap_or_default();
        let resolved = |value: Option<&Object>| -> Vec<Object> {
            match value.cloned().map(|value| match value {
                Object::Reference(id) => reader.resolve_object(id).unwrap_or(Object::Null),
                other => other,
            }) {
                Some(Object::Array(items)) => items,
                _ => Vec::new(),
            }
        };
        let mut all = resolved(properties.get(b"OCGs").ok());
        all.extend(layers.iter().map(|(id, _)| Object::Reference(*id)));
        properties.set("OCGs", Object::Array(all));
        let mut defaults = properties.get(b"D").ok().cloned().and_then(|value| dict_value(reader, &value)).unwrap_or_default();
        let base_off = matches!(defaults.get(b"BaseState"), Ok(Object::Name(name)) if name == b"OFF");
        // With everything off unless it's listed on, it's the shown ones that need listing.
        let (key, wanted) = if base_off { (&b"ON"[..], false) } else { (&b"OFF"[..], true) };
        let mut listed = resolved(defaults.get(key).ok());
        listed.extend(layers.iter().filter(|(_, off)| *off == wanted).map(|(id, _)| Object::Reference(*id)));
        defaults.set(key, Object::Array(listed));
        properties.set("D", Object::Dictionary(defaults));
        catalog.set("OCProperties", Object::Dictionary(properties));
    }
    writer.put(root, Object::Dictionary(catalog))?;
    Ok(())
}

/// **Reads the written file back, page by page, before anything's moved:** the pages are the objects written, in that order. A failure here is `InvalidData`, which the save reports as a failed check.
fn check_pages(written: &Path, expected: &[ObjectId]) -> std::io::Result<()> {
    let bad = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string());
    let reader = open_for_save(written).map_err(|_| bad("doesn't open"))?;
    let map = reader.page_map().map_err(|_| bad("no page tree"))?;
    if map.len() != expected.len() || map.iter().zip(expected).any(|(entry, id)| entry.id() != *id) {
        return Err(bad("pages out of place"));
    }
    Ok(())
}

/// **The one backup of an original, made before its first save over itself** (owner, 2026-11-08). An APFS clone where it can be, which costs no space until the original's blocks are gone, or else a copy, synced. Named for the file's real path, so a later save of the same file finds it and makes none: it's the original, not the last version. Says where it went, or `None` if there already was one.
pub fn keep_backup(original: &Path, folder: &Path) -> std::io::Result<Option<PathBuf>> {
    recto_platform::local::ensure_private_dir(folder)?;
    let _ = recto_platform::local::exclude_from_backups(folder);
    let real = original.canonicalize()?;
    let target = folder.join(backup_name(&real));
    if target.exists() {
        return Ok(None);
    }
    let partial = folder.join(format!(".{}.partial", backup_name(&real)));
    let _ = std::fs::remove_file(&partial);
    if recto_platform::local::clone_file(&real, &partial).is_err() {
        std::fs::copy(&real, &partial)?;
    }
    std::fs::File::open(&partial)?.sync_all()?;
    let _ = recto_platform::local::make_private_file(&partial);
    std::fs::rename(&partial, &target)?;
    let _ = recto_platform::local::sync_folder_of(&target);
    Ok(Some(target))
}

/// A backup's name: a hash of the path it's of, so the name says nothing of the file, with `.pdf` so Quick Look shows it.
pub fn backup_name(real: &Path) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(real.as_os_str().as_encoded_bytes());
    let mut name = String::from("original-");
    for byte in &digest[..12] {
        name.push_str(&format!("{byte:02x}"));
    }
    name.push_str(".pdf");
    name
}

/// The backup of this file, if there is one.
pub fn backup_of(original: &Path, folder: &Path) -> Option<PathBuf> {
    let found = folder.join(backup_name(&original.canonicalize().ok()?));
    found.is_file().then_some(found)
}

/// Backups past [`BACKUP_DAYS`] go, then the oldest until what's left is within [`MAX_BACKUP_BYTES`]. Says how many went. Run when a session starts, never on a timer.
pub fn prune_backups(folder: &Path, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(folder) else { return 0 };
    let mut found: Vec<(SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .take(65_536)
        .filter(|entry| entry.file_name().to_str().is_some_and(|name| name.starts_with("original-") && name.ends_with(".pdf")))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), entry.path()))
        })
        .collect();
    found.sort();
    let mut gone = 0;
    let mut total: u64 = found.iter().map(|(_, len, _)| *len).sum();
    let limit = Duration::from_secs(BACKUP_DAYS * 24 * 60 * 60);
    for (made, len, path) in found {
        let old = now.duration_since(made).is_ok_and(|age| age > limit);
        if (old || total > MAX_BACKUP_BYTES) && std::fs::remove_file(&path).is_ok() {
            total -= len;
            gone += 1;
        }
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (review round 2 of stage 2): a form listing more top fields than a save looks at is walked anyway, holding them all. Through a save, the signature
    /// look's own bound of 16,384 field visits refuses it first (such a form counts as signed and locked), so this asks the name look itself: past 100,000 it's
    /// refused as too much, before any field is read.
    #[test]
    fn a_form_past_the_field_bound_is_refused_by_the_name_look() {
        let many: String = (0..=MAX_FIELD_NODES).map(|_| "4 0 R ").collect();
        let bytes = recto_testpdf::build(&[
            recto_testpdf::Object::Dict(format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{many}] >> >>")),
            recto_testpdf::Object::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into()),
            recto_testpdf::Object::Dict("<< /T (X) >>".into()),
        ]);
        let path = std::env::temp_dir().join(format!("recto-fields-{}.pdf", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let reader = crate::open_reader(&path).unwrap();
        let mut work = 0;
        assert!(matches!(top_field_names(&reader, &mut work), Err(SaveError::TooLarge)));
        assert_eq!(work, 0, "refused before a field was read");
        let _ = std::fs::remove_file(path);
    }

    /// Failure (review round 2 of stage 2): `(Name)` in PDFDocEncoding and `Name` in UTF-16 are one name to a reader, but two to the collision check. As text they're one, and one that won't decode can't meet a decoded one.
    #[test]
    fn field_names_meet_as_text() {
        assert_eq!(name_key(b"Name"), name_key(&[0xFE, 0xFF, 0, b'N', 0, b'a', 0, b'm', 0, b'e']));
        let taken: HashSet<Vec<u8>> = [name_key(b"Name")].into();
        assert_eq!(free_name(&[0xFE, 0xFF, 0, b'N', 0, b'a', 0, b'm', 0, b'e'], &taken), Some(vec![0xFE, 0xFF, 0, b'N', 0, b'a', 0, b'm', 0, b'e', 0, b'_', 0, b'2']));
        assert_ne!(name_key(&[0xFE, 0xFF, 0xD8, 0x00]), name_key(b"x"), "an unpaired surrogate is kept as bytes");
    }
    /// Failure: someone calls one of lopdf's writers that take any `Write`, such as an eager save, and an original is truncated before it's written. `clippy.toml` refuses them by name: this checks the list is there and names each, so it can't be emptied quietly.
    #[test]
    fn the_writers_that_take_any_write_are_refused_by_name() {
        let config = include_str!("../../../clippy.toml");
        for path in ["lopdf::Writer::write_object", "lopdf::Document::save", "lopdf::Document::save_to", "lopdf::IncrementalDocument::save", "lopdf::IncrementalDocument::save_to", "lopdf::IndexedReader::copy_revision_to", "lopdf::RewriteChanges::remove_object", "lopdf::xref::XrefEntry::write_xref_entry", "lopdf::xref::XrefSection::write_xref_section"] {
            assert!(config.contains(&format!("path = \"{path}\"")), "{path} isn't refused");
        }
    }
}
