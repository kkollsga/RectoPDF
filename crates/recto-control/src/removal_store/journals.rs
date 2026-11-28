//! The journals folders (`<root>/session/journals` and `<root>/session-windowless/journals`): which of their files are the target's, for
//! `Store::{Journal, JournalVariant, PageIds, SleepFile, KeptPages}` (`docs/design/removal.md` §3, §4 and §10 "D2c-1 as built").
//!
//! **Read-only.** Each folder is opened once by its descriptor (`recto_platform::folder`: no link followed at any step, this user's) and listed, looked
//! at and opened from through that descriptor, so a path swapped for a link after the check changes nothing. A journal or a sidecar is opened only
//! with no link followed, no pipe waited on, only this user's, nobody else able to write it, and read for its first bytes alone: 8 KiB of a journal, 110 bytes of a sidecar. A sleep file and a kept copy are
//! placed by their names and never opened (they hold headings, page pictures and PDFs). A document's path in a header is compared as bytes with the
//! ledger's and never looked at. Every entry becomes an artifact, so one nobody can place (a name this build doesn't write, a folder, a link, a
//! pipe) is `Unknown` and refuses the plan instead of being skipped as "none found".
//!
//! **Placing.** A journal is the target's only if both the version of its file and its path are found on the same side of the lineage (a path
//! alone never joins, and nor does a same-version copy elsewhere), with a version in `saving` added. A sidecar takes its journal's owner, joined with the
//! version its own header names; an orphan is `Unknown`. A sleep or kept copy takes the owner of the version in its name. A version seen on both sides
//! (a disk with no UUID, or a block copy) is `Unknown`.

mod names;

use super::backups::{self, Finder};
use super::placing::Maps;
use super::purge::HistoryPins;
use names::Kind;
use recto_core::removal::{ArtifactKey, Format, Found, Lineage, Owner, Refusal, Sides, Store, Survey, TargetMode};
use recto_platform::folder::{Entry, Folder, Opened};
use recto_session::edit::identity::sidecar;
use recto_session::record::{MARKED_JOURNAL_VERSION, journal_header};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read as _;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Arc;

/// The most of a journal's start read for its header line, in bytes. Also what the ledger's adoption reads.
pub(super) const HEADER_BYTES: usize = 8 * 1024;
/// The most entries looked at in one folder. Design limit, not a measurement.
pub(super) const MAX_ENTRIES: usize = 8_192;
/// The two sessions' folders.
pub(super) const FLAVORS: [&str; 2] = ["session", "session-windowless"];

#[cfg(test)]
use super::testing::Hook;

/// The caps, so a test can make them small.
#[derive(Clone)]
pub(crate) struct Caps {
    pub entries: usize,
    pub header_bytes: usize,
    /// Called with a path just before it is opened: a test's way to change the folder under the listing.
    #[cfg(test)]
    pub before_open: Option<Hook>,
    /// Called with a folder's path once it is checked and before it is listed: a test's way to swap the folder for a link.
    #[cfg(test)]
    pub before_list: Option<Hook>,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            entries: MAX_ENTRIES,
            header_bytes: HEADER_BYTES,
            #[cfg(test)]
            before_open: None,
            #[cfg(test)]
            before_list: None,
        }
    }
}

/// Adds every file of both journals folders to `survey`, and marks the five stores complete only after both were listed to the end.
///
/// Folders that aren't there are complete and empty. A `session` or `journals` that isn't a real folder (a link, a file), a folder that can't be
/// listed, and an entry that vanishes between the listing and its opening leave the stores unmarked, so the plan says `Incomplete` rather than
/// "none found". The adapter does not check the lineage's gap: the plan's `HistoryIncomplete` gate (`Survey::history`) refuses a lineage with a gap or
/// a fork. A folder of more than 8,192 entries is `TooLarge`. `add`'s refusal passes through.
pub fn survey_journals(root: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey) -> Result<(), Refusal> {
    survey_journals_with(root, lineage, mode, survey, &Caps::default())
}

/// `survey_journals`, and the identity (device, inode, kind) of every artifact added as the target's noted in `pins`, for the purge by pin (`purge`).
pub fn survey_journals_pinned(root: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey, pins: &mut HistoryPins) -> Result<(), Refusal> {
    survey_journals_run(root, lineage, mode, survey, &Caps::default(), Some(pins))
}

/// A flavor's `journals` folder under the state root, opened once by descriptor with no link followed at any step and only if it is this user's. The
/// survey and the purge both open it by this, so they can't disagree on which folder it is.
pub(super) fn open_journals(root: &Path, flavor: &str) -> Opened {
    match Folder::open(&root.join(flavor)) {
        Opened::Folder(session) => session.open_in(OsStr::new("journals")),
        other => other,
    }
}

/// What every key of a flavor's `journals` folder starts with: the flavor and `/journals/`.
pub(super) fn flavor_prefix(flavor: &str) -> Vec<u8> {
    let mut prefix = flavor.as_bytes().to_vec();
    prefix.extend_from_slice(b"/journals/");
    prefix
}

/// Why a folder wasn't listed.
pub(super) enum ListFailed {
    /// It, or an entry of it, couldn't be read.
    Unreadable,
    /// It holds more entries than the cap.
    TooLarge,
}

/// A folder's entries, at most `cap`; one more is `TooLarge`. The survey's listing and the purge's, so they stop at the same place.
pub(super) fn list_capped(folder: &Folder, cap: usize) -> Result<Vec<Entry>, ListFailed> {
    let entries = folder.entries().map_err(|_| ListFailed::Unreadable)?;
    let mut listed = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| ListFailed::Unreadable)?;
        if listed.len() >= cap {
            return Err(ListFailed::TooLarge);
        }
        listed.push(entry);
    }
    Ok(listed)
}

/// The store a name in a `journals` folder is in, as the survey says.
pub(super) fn entry_store(name: &OsStr) -> Store {
    store_of(names::kind_of(name))
}

/// The key of a name in a `journals` folder: the folder's place and the name, in `store`. The survey's key, and the purge's.
pub(super) fn journal_key(store: Store, prefix: &[u8], name: &OsStr) -> ArtifactKey {
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(name.as_encoded_bytes());
    ArtifactKey::of(store, &bytes)
}

pub(crate) fn survey_journals_with(root: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey, caps: &Caps) -> Result<(), Refusal> {
    survey_journals_run(root, lineage, mode, survey, caps, None)
}

fn survey_journals_run(
    root: &Path,
    lineage: &Lineage<'_>,
    mode: TargetMode,
    survey: &mut Survey,
    caps: &Caps,
    mut pins: Option<&mut HistoryPins>,
) -> Result<(), Refusal> {
    let maps = Maps::of(lineage);
    for flavor in FLAVORS {
        let folder = match open_journals(root, flavor) {
            Opened::Missing => continue,
            Opened::Other => return Ok(()),
            Opened::Folder(folder) => folder,
        };
        let at = root.join(flavor).join("journals");
        if !survey_journals_pinned_folder(&folder, &flavor_prefix(flavor), None, &at, &maps, mode, caps, survey, pins.as_deref_mut())? {
            return Ok(());
        }
    }
    for store in [Store::Journal, Store::JournalVariant, Store::PageIds, Store::SleepFile, Store::KeptPages] {
        survey.mark_complete(store);
    }
    Ok(())
}

/// One `journals` folder's files added to `survey`. Whether it was listed to the end and every entry added (`false` leaves the stores unmarked).
///
/// `prefix` is the folder's place under the state root with its closing `/`, such as `session/journals/` (it starts every key, so the same name in two
/// folders is two artifacts). `store` is the store every entry is in, or `None` to give each entry the store its name says (the live folders). `at`
/// is where the folder was when opened, for a test's seams; it is never opened.
#[allow(clippy::too_many_arguments)]
pub(super) fn survey_journals_folder(
    folder: &Folder,
    prefix: &[u8],
    store: Option<Store>,
    at: &Path,
    maps: &Maps,
    mode: TargetMode,
    caps: &Caps,
    survey: &mut Survey,
) -> Result<bool, Refusal> {
    survey_journals_pinned_folder(folder, prefix, store, at, maps, mode, caps, survey, None)
}

/// `survey_journals_folder`, noting the target's artifacts in `pins`.
#[allow(clippy::too_many_arguments)]
fn survey_journals_pinned_folder(
    folder: &Folder,
    prefix: &[u8],
    store: Option<Store>,
    at: &Path,
    maps: &Maps,
    mode: TargetMode,
    caps: &Caps,
    survey: &mut Survey,
    pins: Option<&mut HistoryPins>,
) -> Result<bool, Refusal> {
    #[cfg(not(test))]
    let _ = at;
    let run = Run {
        prefix,
        store,
        #[cfg(test)]
        path: at.to_path_buf(),
        folder,
        maps,
        mode,
        caps,
    };
    run.survey(survey, &mut Vec::new(), pins)
}

/// One folder's survey.
struct Run<'a> {
    /// Every key starts with this: the folder's place under the state root.
    prefix: &'a [u8],
    /// The store of every entry, or `None` for the store each name says.
    store: Option<Store>,
    /// Where the folder was when opened, for a test's seams; never opened again.
    #[cfg(test)]
    path: PathBuf,
    folder: &'a Folder,
    maps: &'a Maps,
    mode: TargetMode,
    caps: &'a Caps,
}

/// What placing decided: the sides found, or `None` when it can't be told.
struct Placed {
    sides: Option<Sides>,
    format: Format,
}

impl Placed {
    fn unknown() -> Self {
        Self {
            sides: None,
            format: Format::Current,
        }
    }
}

/// The file went away between the listing and the opening, so the folder changed under the survey.
struct Gone;

impl Run<'_> {
    /// Whether the folder was listed and every entry added; `false` leaves the stores unmarked.
    fn survey(&self, survey: &mut Survey, buffer: &mut Vec<u8>, mut pins: Option<&mut HistoryPins>) -> Result<bool, Refusal> {
        let Some(listed) = self.list()? else { return Ok(false) };
        // Journals first: a sidecar takes its journal's owner.
        let mut journals: HashMap<String, Option<Sides>> = HashMap::new();
        for pass in [Pass::Journals, Pass::Rest] {
            for entry in &listed {
                let name = entry.name.to_str();
                let kind = names::kind_of(&entry.name);
                if (matches!(kind, Kind::Journal | Kind::Variant)) != (pass == Pass::Journals) {
                    continue;
                }
                let finder = backups::finder_file(entry.name.as_encoded_bytes(), entry.is_file, entry.links);
                if finder == Some(Finder::Skip) {
                    continue;
                }
                let placed = if entry.is_file {
                    match self.place(entry, name, kind, &journals, buffer) {
                        Ok(placed) => placed,
                        Err(Gone) => return Ok(false),
                    }
                } else {
                    Placed::unknown()
                };
                if kind == Kind::Journal
                    && let Some(name) = name.filter(|name| name.starts_with("journal-"))
                {
                    journals.insert(name.to_string(), placed.sides);
                }
                let owner = match placed.sides {
                    _ if let Some(Finder::Owner(owner)) = finder => owner,
                    None => Owner::Unknown,
                    Some(_) if entry.links != 1 => Owner::Shared,
                    Some(sides) => sides.owner(self.mode),
                };
                let format = if owner == Owner::Unknown { placed.format } else { Format::Current };
                let key = self.key(entry);
                survey.add(Found {
                    store: self.store_of(kind),
                    key,
                    owner,
                    format,
                })?;
                // Only the target's: a file placed by its bytes was proved the listed one (its descriptor's device, inode and one link, `read`), and one placed by
                // its name alone is pinned from the listing.
                if owner == Owner::Target
                    && let Some(pins) = pins.as_deref_mut()
                {
                    // Placed by its name alone (sleep, kept): pinned only if this user's, which the open of a journal or a sidecar has proved of its own.
                    match kind {
                        Kind::SleepFile | Kind::KeptPages => pins.note_named(key, entry.identity(), entry)?,
                        _ => pins.note(key, entry.identity())?,
                    }
                }
            }
        }
        Ok(true)
    }

    /// The folder's entries, at most the cap; one more is `TooLarge`. `None` if the folder can't be read.
    fn list(&self) -> Result<Option<Vec<Entry>>, Refusal> {
        #[cfg(test)]
        if let Some(hook) = &self.caps.before_list {
            hook(&self.path);
        }
        match list_capped(self.folder, self.caps.entries) {
            Ok(listed) => Ok(Some(listed)),
            Err(ListFailed::Unreadable) => Ok(None),
            Err(ListFailed::TooLarge) => Err(Refusal::TooLarge),
        }
    }

    /// Root-relative, with the folder's place: the same name in two folders is two artifacts.
    fn key(&self, entry: &Entry) -> ArtifactKey {
        journal_key(self.store_of(names::kind_of(&entry.name)), self.prefix, &entry.name)
    }

    fn store_of(&self, kind: Kind) -> Store {
        self.store.unwrap_or_else(|| store_of(kind))
    }

    fn place(&self, entry: &Entry, name: Option<&str>, kind: Kind, journals: &HashMap<String, Option<Sides>>, buffer: &mut Vec<u8>) -> Result<Placed, Gone> {
        let Some(name) = name else { return Ok(Placed::unknown()) };
        match kind {
            Kind::Journal | Kind::Variant => {
                let Some(bytes) = self.read(entry, name, self.caps.header_bytes, buffer)? else {
                    return Ok(Placed::unknown());
                };
                Ok(self.journal(bytes))
            }
            Kind::PageIds => {
                let Some(bytes) = self.read(entry, name, sidecar::HEADER_BYTES, buffer)? else {
                    return Ok(Placed::unknown());
                };
                Ok(Placed {
                    sides: self.sidecar(name, bytes, journals),
                    format: Format::Current,
                })
            }
            Kind::SleepFile | Kind::KeptPages => {
                let sides = names::version_key(name).and_then(|key| self.maps.versions.get(key).copied());
                Ok(Placed {
                    sides,
                    format: Format::Current,
                })
            }
            Kind::KeptTemp | Kind::Other => Ok(Placed::unknown()),
        }
    }

    /// The first `cap` bytes of a private plain file. `None` if it can't be trusted (a link, a pipe, another user's, group-writable, unreadable, or
    /// not the file that was listed: its device, inode or link count, from the opened descriptor, differ from `entry`'s, which a pin and the `Shared`
    /// rule use, so its bytes would place one file while those describe another); `Gone` if it isn't there any more.
    fn read<'b>(&self, entry: &Entry, name: &str, cap: usize, buffer: &'b mut Vec<u8>) -> Result<Option<&'b [u8]>, Gone> {
        #[cfg(test)]
        if let Some(hook) = &self.caps.before_open {
            hook(&self.path.join(name));
        }
        let file = match self.folder.open_private_in(OsStr::new(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(Gone),
            Err(_) => return Ok(None),
        };
        if !is_the_listed(&file, entry) {
            return Ok(None);
        }
        buffer.clear();
        if file.take(cap as u64).read_to_end(buffer).is_err() {
            return Ok(None);
        }
        Ok(Some(buffer))
    }

    /// A journal by its first line: the version of its file and its path must both be found, and what a `saving` version adds is added.
    fn journal(&self, bytes: &[u8]) -> Placed {
        let Some(header) = journal_header(bytes) else { return Placed::unknown() };
        if header.version > MARKED_JOURNAL_VERSION {
            return Placed {
                sides: None,
                format: Format::Foreign,
            };
        }
        let Some(path) = header.path.to_path() else { return Placed::unknown() };
        let Some(file) = header.file else { return Placed::unknown() };
        let Some(mut sides) = self.maps.place(path.as_os_str().as_encoded_bytes(), file.inode, file.size, file.mtime_ns) else {
            return Placed::unknown();
        };
        // A `saving` version names the file a save was writing when the journal was last written. If the lineage never saw it (the save didn't
        // finish, so no link names it) or its inode is 0 (unknown), it adds nothing: it can't name a side we know of, and the journal is already
        // placed by its own file and path. Adding nothing can only leave a journal less owned, never more, so it fails toward refusing.
        if let Some(saving) = header.saving.and_then(|s| self.maps.version(s.inode, s.size, s.mtime_ns)) {
            sides = sides.union(saving);
        }
        Placed {
            sides: Some(sides),
            format: Format::Current,
        }
    }

    /// A sidecar by its own header and its journal in the same folder.
    fn sidecar(&self, name: &str, bytes: &[u8], journals: &HashMap<String, Option<Sides>>) -> Option<Sides> {
        let header = sidecar::header_of(bytes)?;
        let journal = format!("journal-{}.log", names::sidecar_stem(name)?);
        if header.journal != journal {
            return None;
        }
        let owner = (*journals.get(&journal)?)?;
        Some(self.maps.version(header.inode, header.size, header.mtime_ns).map_or(owner, |v| owner.union(v)))
    }
}

/// Whether an opened file is the one that was listed: its device and inode are the entry's and it has one link. Where there is no device and inode to
/// compare (the stand-in off the Mac) nothing is trusted, and the artifact is `Unknown`.
#[cfg(unix)]
fn is_the_listed(file: &std::fs::File, entry: &Entry) -> bool {
    file.metadata()
        .is_ok_and(|opened| opened.dev() == entry.dev && opened.ino() == entry.ino && opened.nlink() == 1)
}

#[cfg(not(unix))]
fn is_the_listed(_file: &std::fs::File, _entry: &Entry) -> bool {
    false
}

#[derive(PartialEq)]
enum Pass {
    Journals,
    Rest,
}

fn store_of(kind: Kind) -> Store {
    match kind {
        Kind::Journal => Store::Journal,
        Kind::Variant | Kind::Other => Store::JournalVariant,
        Kind::PageIds => Store::PageIds,
        Kind::SleepFile => Store::SleepFile,
        Kind::KeptPages | Kind::KeptTemp => Store::KeptPages,
    }
}

#[cfg(test)]
pub(crate) mod tests;
