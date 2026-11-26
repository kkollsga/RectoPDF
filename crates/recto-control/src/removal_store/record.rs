//! The session records (`<root>/session` and `<root>/session-windowless`): which of their files are the target's, for `Store::SessionRecord`
//! (`docs/design/removal.md` §3, §4 and §10 "D2c-2a as built").
//!
//! **Read-only.** Each flavor's folder is opened once by its descriptor (no link followed, this user's) and listed and read through it. The two
//! slots `record.a.json` and `record.b.json` are each opened with `open_private_in` (no link followed, no pipe waited on, a plain private file), read
//! whole up to the record store's own cap and decoded by its own decoder. `places.log` is never opened. Nothing in a tab's path is opened.
//!
//! **Placing.** Each tab is placed by the journal rule (`Maps::place`): the version of its file and its path must both be found, and their sides
//! are joined. A slot is `Unknown` if any tab is, else target-side if any tab is, else `Unrelated` (a slot with no tabs too); more than one hard link
//! makes it `Shared`. A record from a newer build is `Unknown` and `Foreign`, since its tabs can't be read; one damaged, over the cap or not
//! private is `Unknown`. `places.log` names only a slot's tabs, by id and position, so it takes the strictest owner of the slots found (Unknown, then
//! Shared, then target-side, then Unrelated); no slot at all makes it `Unknown`. Every other entry is `Unknown` except a plain `.DS_Store`, a plain
//! `went-back` marker and `journals` (the journals adapter's), so one nobody can place refuses the plan instead of being skipped.

use super::backups::{self, Finder};
use super::placing::Maps;
use recto_core::removal::{ArtifactKey, Format, Found, Lineage, Owner, Refusal, Store, Survey, TargetMode};
use recto_platform::folder::{Entry, Folder, Opened};
use recto_session::record::{CURRENT_VERSION, MAX_RECORD_BYTES, RecordError, decode_record};
use std::ffi::OsStr;
use std::io::Read as _;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Arc;

/// The most entries looked at in one folder. Design limit, not a measurement.
const MAX_ENTRIES: usize = 8_192;
/// The two sessions' folders.
const FLAVORS: [&str; 2] = ["session", "session-windowless"];
const SLOTS: [&str; 2] = ["record.a.json", "record.b.json"];
const PLACES: &str = "places.log";
/// The one-byte marker the record store writes beside a record after a copy went back.
const WENT_BACK: &[u8] = b"went-back";

/// What a test runs with a path just before it is used.
#[cfg(test)]
pub(crate) type Hook = Arc<dyn Fn(&Path) + Send + Sync>;

/// The caps, so a test can make them small.
#[derive(Clone)]
pub(crate) struct RecordCaps {
    pub entries: usize,
    pub record_bytes: usize,
    /// Called with a slot's path just before it is opened: a test's way to change the folder under the listing.
    #[cfg(test)]
    pub before_open: Option<Hook>,
    /// Called with a folder's path once it is checked and before it is listed.
    #[cfg(test)]
    pub before_list: Option<Hook>,
}

impl Default for RecordCaps {
    fn default() -> Self {
        Self {
            entries: MAX_ENTRIES,
            record_bytes: MAX_RECORD_BYTES,
            #[cfg(test)]
            before_open: None,
            #[cfg(test)]
            before_list: None,
        }
    }
}

/// Adds every file of both session folders to `survey`, and marks `Store::SessionRecord` complete only after both were listed to the end.
///
/// A folder that isn't there is complete and empty. One that isn't a real folder of this user's (a link, a file), one that can't be listed, and a
/// slot that vanishes between the listing and its opening leave the store unmarked, so the plan says `Incomplete` rather than "none found". The
/// adapter does not check the lineage's gap: the plan's `HistoryIncomplete` gate (`Survey::history`) does. A folder of more than 8,192 entries is
/// `TooLarge`. `add`'s refusal passes through.
pub fn survey_record(root: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey) -> Result<(), Refusal> {
    survey_record_with(root, lineage, mode, survey, &RecordCaps::default())
}

pub(crate) fn survey_record_with(root: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey, caps: &RecordCaps) -> Result<(), Refusal> {
    let maps = Maps::of(lineage);
    let mut buffer = Vec::new();
    for flavor in FLAVORS {
        let path = root.join(flavor);
        let folder = match Folder::open(&path) {
            Opened::Missing => continue,
            Opened::Other => return Ok(()),
            Opened::Folder(folder) => folder,
        };
        let run = Run {
            flavor,
            #[cfg(test)]
            path,
            folder: &folder,
            maps: &maps,
            mode,
            caps,
        };
        if !run.survey(survey, &mut buffer)? {
            return Ok(());
        }
    }
    survey.mark_complete(Store::SessionRecord);
    Ok(())
}

/// How strict an owner is for `places.log`: the strictest of the slots is what it takes.
fn strictness(owner: Owner) -> u8 {
    match owner {
        Owner::Unrelated => 0,
        Owner::Target | Owner::SaveAsSource => 1,
        Owner::Shared => 2,
        Owner::Unknown => 3,
    }
}

/// What reading a slot decided.
struct Placed {
    owner: Owner,
    format: Format,
}

impl Placed {
    fn unknown() -> Self {
        Self {
            owner: Owner::Unknown,
            format: Format::Current,
        }
    }
}

/// The file went away between the listing and the opening, so the folder changed under the survey.
struct Gone;

struct Run<'a> {
    flavor: &'a str,
    /// Where the folder was when opened, for a test's seams; never opened again.
    #[cfg(test)]
    path: PathBuf,
    folder: &'a Folder,
    maps: &'a Maps,
    mode: TargetMode,
    caps: &'a RecordCaps,
}

impl Run<'_> {
    /// Whether the folder was listed and every entry added; `false` leaves the store unmarked.
    fn survey(&self, survey: &mut Survey, buffer: &mut Vec<u8>) -> Result<bool, Refusal> {
        let Some(listed) = self.list()? else { return Ok(false) };
        // The slots first: `places.log` takes the strictest of them.
        let mut strictest: Option<Owner> = None;
        let mut places: Option<&Entry> = None;
        for entry in &listed {
            let name = entry.name.as_encoded_bytes();
            let owner = if name == b"journals" || name == WENT_BACK && entry.is_file && entry.links == 1 {
                continue;
            } else if name == PLACES.as_bytes() {
                places = Some(entry);
                continue;
            } else if let Some(finder) = backups::finder_file(name, entry.is_file, entry.links) {
                match finder {
                    Finder::Skip => continue,
                    Finder::Owner(owner) => Placed {
                        owner,
                        format: Format::Current,
                    },
                }
            } else if let Some(slot) = SLOTS.iter().find(|slot| slot.as_bytes() == name) {
                let mut placed = if entry.is_file {
                    match self.place(slot, buffer) {
                        Ok(placed) => placed,
                        Err(Gone) => return Ok(false),
                    }
                } else {
                    Placed::unknown()
                };
                if entry.links != 1 && placed.owner != Owner::Unknown {
                    placed.owner = Owner::Shared;
                }
                strictest = Some(strictest.map_or(placed.owner, |s| if strictness(placed.owner) > strictness(s) { placed.owner } else { s }));
                placed
            } else {
                Placed::unknown()
            };
            self.add(survey, entry, owner)?;
        }
        if let Some(entry) = places {
            let owner = match strictest {
                _ if !entry.is_file => Owner::Unknown,
                None => Owner::Unknown,
                Some(_) if entry.links != 1 && strictest != Some(Owner::Unknown) => Owner::Shared,
                Some(owner) => owner,
            };
            self.add(
                survey,
                entry,
                Placed {
                    owner,
                    format: Format::Current,
                },
            )?;
        }
        Ok(true)
    }

    fn add(&self, survey: &mut Survey, entry: &Entry, placed: Placed) -> Result<(), Refusal> {
        let mut name = format!("{}/", self.flavor).into_bytes();
        name.extend_from_slice(entry.name.as_encoded_bytes());
        survey.add(Found {
            store: Store::SessionRecord,
            key: ArtifactKey::of(Store::SessionRecord, &name),
            owner: placed.owner,
            format: placed.format,
        })
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

    /// A slot by its tabs. `Gone` if it isn't there any more.
    fn place(&self, name: &str, buffer: &mut Vec<u8>) -> Result<Placed, Gone> {
        #[cfg(test)]
        if let Some(hook) = &self.caps.before_open {
            hook(&self.path.join(name));
        }
        let file = match self.folder.open_private_in(OsStr::new(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(Gone),
            Err(_) => return Ok(Placed::unknown()),
        };
        let cap = self.caps.record_bytes as u64;
        match file.metadata() {
            Ok(meta) if meta.len() <= cap => {}
            _ => return Ok(Placed::unknown()),
        }
        buffer.clear();
        if file.take(cap + 1).read_to_end(buffer).is_err() || buffer.len() as u64 > cap {
            return Ok(Placed::unknown());
        }
        Ok(match decode_record(buffer, CURRENT_VERSION) {
            Ok(decoded) => Placed {
                owner: self.tabs(&decoded.record.tabs),
                format: Format::Current,
            },
            Err(RecordError::Newer(_)) => Placed {
                owner: Owner::Unknown,
                format: Format::Foreign,
            },
            Err(RecordError::Damaged(_)) => Placed::unknown(),
        })
    }

    /// Unknown if any tab can't be placed; else the target's if any tab is; else unrelated.
    fn tabs(&self, tabs: &[recto_session::record::TabEntry]) -> Owner {
        let mut owner = Owner::Unrelated;
        for tab in tabs {
            let sides = tab
                .file
                .as_ref()
                .zip(tab.path.to_path())
                .and_then(|(file, path)| self.maps.place(path.as_os_str().as_encoded_bytes(), file.inode, file.size, file.mtime_ns));
            let Some(sides) = sides else { return Owner::Unknown };
            match sides.owner(self.mode) {
                Owner::Unknown => return Owner::Unknown,
                Owner::Unrelated => {}
                target_side => owner = target_side,
            }
        }
        owner
    }
}

#[cfg(test)]
mod tests;
