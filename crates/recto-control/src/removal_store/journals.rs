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
use names::Kind;
use recto_core::removal::{ArtifactKey, Format, Found, Lineage, Owner, Refusal, Side, Sides, Store, Survey, TargetMode};
use recto_platform::FileStamp;
use recto_platform::folder::{Entry, Folder, Opened};
use recto_session::edit::identity::sidecar;
use recto_session::record::{MARKED_JOURNAL_VERSION, journal_file_name, journal_header};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read as _;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Arc;

/// The most of a journal's start read for its header line, in bytes. Also what the ledger's adoption reads.
pub(super) const HEADER_BYTES: usize = 8 * 1024;
/// The most entries looked at in one folder. Design limit, not a measurement.
const MAX_ENTRIES: usize = 8_192;
/// The two sessions' folders.
const FLAVORS: [&str; 2] = ["session", "session-windowless"];

/// What a test runs just before a path is opened.
#[cfg(test)]
pub(crate) type Hook = Arc<dyn Fn(&Path) + Send + Sync>;

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

pub(crate) fn survey_journals_with(root: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey, caps: &Caps) -> Result<(), Refusal> {
    let maps = Maps::of(lineage);
    let mut buffer = Vec::new();
    for flavor in FLAVORS {
        let session_path = root.join(flavor);
        let session = match Folder::open(&session_path) {
            Opened::Missing => continue,
            Opened::Other => return Ok(()),
            Opened::Folder(folder) => folder,
        };
        let folder = match session.open_in(OsStr::new("journals")) {
            Opened::Missing => continue,
            Opened::Other => return Ok(()),
            Opened::Folder(folder) => folder,
        };
        let run = Run {
            flavor,
            #[cfg(test)]
            path: session_path.join("journals"),
            folder: &folder,
            maps: &maps,
            mode,
            caps,
        };
        if !run.survey(survey, &mut buffer)? {
            return Ok(());
        }
    }
    for store in [Store::Journal, Store::JournalVariant, Store::PageIds, Store::SleepFile, Store::KeptPages] {
        survey.mark_complete(store);
    }
    Ok(())
}

/// What the lineage says about versions and paths, as the journals name them.
struct Maps {
    /// By the 16-digit key a name carries (from inode, size and mtime: the device is a mount number and the journal doesn't hold the volume).
    versions: HashMap<String, Sides>,
    /// By the path's bytes.
    paths: HashMap<Vec<u8>, Sides>,
}

impl Maps {
    fn of(lineage: &Lineage<'_>) -> Self {
        let mut versions: HashMap<String, Sides> = HashMap::new();
        let current = lineage.current();
        let all = std::iter::once((current, Side::Target)).chain(lineage.versions());
        for (version, side) in all.filter(|(version, _)| version.inode != 0) {
            let sides = versions.entry(key_of(version.inode, version.size, version.mtime_ns)).or_default();
            *sides = sides.with(side);
        }
        let mut paths: HashMap<Vec<u8>, Sides> = HashMap::new();
        for (path, side) in lineage.paths() {
            let sides = paths.entry(path.as_os_str().as_encoded_bytes().to_vec()).or_default();
            *sides = sides.with(side);
        }
        Self { versions, paths }
    }

    /// The sides of the version a header names, if its inode is real and the lineage saw it.
    fn version(&self, inode: u64, size: u64, mtime_ns: i64) -> Option<Sides> {
        (inode != 0).then(|| self.versions.get(&key_of(inode, size, mtime_ns)).copied()).flatten()
    }
}

/// The key a journal's name carries for a file version, by the one function that names journals.
fn key_of(inode: u64, size: u64, mtime_ns: i64) -> String {
    let stamp = FileStamp {
        size,
        mtime_ns,
        device: 0,
        inode,
    };
    let name = journal_file_name(Some(stamp), Path::new(""), None);
    name.strip_prefix("journal-")
        .and_then(|rest| rest.strip_suffix(".log"))
        .unwrap_or_default()
        .to_string()
}

/// One folder's survey.
struct Run<'a> {
    flavor: &'a str,
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
    fn survey(&self, survey: &mut Survey, buffer: &mut Vec<u8>) -> Result<bool, Refusal> {
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
                    match self.place(name, kind, &journals, buffer) {
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
                survey.add(Found {
                    store: store_of(kind),
                    key: self.key(entry),
                    owner,
                    format,
                })?;
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
        let Ok(entries) = self.folder.entries() else { return Ok(None) };
        let mut listed = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else { return Ok(None) };
            if listed.len() >= self.caps.entries {
                return Err(Refusal::TooLarge);
            }
            listed.push(entry);
        }
        Ok(Some(listed))
    }

    /// Root-relative, with the flavor: the same name in both folders is two artifacts.
    fn key(&self, entry: &Entry) -> ArtifactKey {
        let mut name = format!("{}/journals/", self.flavor).into_bytes();
        name.extend_from_slice(entry.name.as_encoded_bytes());
        ArtifactKey::of(store_of(names::kind_of(&entry.name)), &name)
    }

    fn place(&self, name: Option<&str>, kind: Kind, journals: &HashMap<String, Option<Sides>>, buffer: &mut Vec<u8>) -> Result<Placed, Gone> {
        let Some(name) = name else { return Ok(Placed::unknown()) };
        match kind {
            Kind::Journal | Kind::Variant => {
                let Some(bytes) = self.read(name, self.caps.header_bytes, buffer)? else {
                    return Ok(Placed::unknown());
                };
                Ok(self.journal(bytes))
            }
            Kind::PageIds => {
                let Some(bytes) = self.read(name, sidecar::HEADER_BYTES, buffer)? else {
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

    /// The first `cap` bytes of a private plain file. `None` if it can't be trusted (a link, a pipe, another user's, group-writable, unreadable);
    /// `Gone` if it isn't there any more.
    fn read<'b>(&self, name: &str, cap: usize, buffer: &'b mut Vec<u8>) -> Result<Option<&'b [u8]>, Gone> {
        #[cfg(test)]
        if let Some(hook) = &self.caps.before_open {
            hook(&self.path.join(name));
        }
        let file = match self.folder.open_file(OsStr::new(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(Gone),
            Err(_) => return Ok(None),
        };
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
        let found = (
            self.maps.version(file.inode, file.size, file.mtime_ns),
            self.maps.paths.get(path.as_os_str().as_encoded_bytes()).copied(),
        );
        let (Some(version), Some(at)) = found else { return Placed::unknown() };
        let mut sides = version.union(at);
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
mod tests;
