//! The copies of the session record the app keeps beside the live one (`<root>/session.v<N>`, `<root>/session-windowless.v<N>` and
//! `<root>/<flavor>.damaged[.<k>]`), for `Store::LegacySession`, and the one function that surveys any copy folder, shared with the update's copy
//! (`docs/design/removal.md` §3, §4 and §10 "D2c-3 as built").
//!
//! **Read-only, and nothing in a copy is decoded.** A copy's record slots and `places.log` are always `Unknown` and are never opened: fields are added to
//! record version 1 without a bump, so an older build would silently miss a newer build's text, and nothing can rewrite a copy yet. Its `journals/` folder
//! goes through the journals adapter (`survey_journals_folder`), with every entry in the copy's store and every key starting with the copy's place under
//! the state root. Only what is proven to hold no document data is left out: Finder's plain `.DS_Store`, and one plain marker of one link that the app
//! writes and reads by its name alone (`went-back`, or the update's `.whole`). Every other entry (a temp, a folder, a link, a pipe, an unknown name) is
//! `Unknown`, so a plan refuses instead of reading "none found".

use super::backups::{self, Finder};
use super::journals::{self, Caps};
use super::placing::Maps;
#[cfg(test)]
use super::testing::Hook;
use recto_core::removal::{ArtifactKey, Format, Found, Lineage, Owner, Refusal, Store, Survey, TargetMode};
use recto_platform::folder::{Entry, Folder, Opened};
use recto_session::record::CURRENT_VERSION;
use std::ffi::OsStr;
use std::path::Path;

/// The most entries looked at in the state root. Design limit, not a measurement.
const MAX_ENTRIES: usize = 8_192;
/// The most copies opened. Design limit: a copy is made at each version going forward and one for each damaged state kept (a handful).
const MAX_COPIES: usize = 64;
/// The two sessions' names.
const FLAVORS: [&str; 2] = ["session", "session-windowless"];
/// The word a damaged state's folder is named by (`<flavor>.damaged`, then `<flavor>.damaged.<k>`). A constant, so the line that reads it isn't mistaken
/// for a decision by a refusal's code (`recto-agent/tests/codes.rs`): it is a folder name.
const DAMAGED: &[u8] = b"damaged";
/// The one-byte marker the record store writes beside a record after a copy went back, and reads only for whether it is there.
const WENT_BACK: &[u8] = b"went-back";

/// The caps, so a test can make them small.
#[derive(Clone)]
pub(crate) struct LegacyCaps {
    /// The state root's entries.
    pub entries: usize,
    /// The copy folders opened.
    pub copies: usize,
    /// For each copy folder and its `journals` (its entries, the header bytes read, and the journals' hooks).
    pub folder: Caps,
    /// Called with a copy's path just before it is opened: a test's way to change the root under the listing.
    #[cfg(test)]
    pub before_open: Option<Hook>,
    /// Called with the root's path once it is checked and before it is listed.
    #[cfg(test)]
    pub before_list: Option<Hook>,
}

impl Default for LegacyCaps {
    fn default() -> Self {
        Self {
            entries: MAX_ENTRIES,
            copies: MAX_COPIES,
            folder: Caps::default(),
            #[cfg(test)]
            before_open: None,
            #[cfg(test)]
            before_list: None,
        }
    }
}

/// What a name in the state root is, as far as this store goes (a pure function of the name's bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Name {
    /// Not this store's: another store's, or the live `session` or `session-windowless` (surveyed elsewhere).
    NotOurs,
    /// A copy this build can read the shape of: `<flavor>.v<N>` with N up to this build's record version, or `<flavor>.damaged[.<k>]`.
    Copy,
    /// A copy from a newer build (`<flavor>.v<N>`, N past this build's): one `Foreign` artifact, nothing inside is opened.
    Newer,
    /// Starts with `session` but is no name the app writes: `Unknown`, not opened.
    Other,
}

/// A decimal number with no sign, no leading zero and at least 1, that fits a `u32`.
fn counted(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() || digits[0] == b'0' || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse::<u32>().ok()
}

/// The name's kind. The only writers of copies are `RecordStore::version_folder` and `make_room_for_damaged`, which name from the flavor's folder
/// name (`fold_name`): `<flavor>.v<N>` and `<flavor>.damaged` and `<flavor>.damaged.<k>`.
pub(super) fn legacy_kind(name: &[u8]) -> Name {
    if FLAVORS.iter().any(|flavor| flavor.as_bytes() == name) {
        return Name::NotOurs;
    }
    if !name.starts_with(b"session") {
        return Name::NotOurs;
    }
    for flavor in FLAVORS {
        let Some(rest) = name.strip_prefix(flavor.as_bytes()).and_then(|rest| rest.strip_prefix(b".")) else {
            continue;
        };
        if let Some(version) = rest.strip_prefix(b"v").and_then(counted) {
            return if version <= CURRENT_VERSION { Name::Copy } else { Name::Newer };
        }
        let numbered = rest.strip_prefix(DAMAGED).and_then(|rest| rest.strip_prefix(b".")).and_then(counted);
        if rest == DAMAGED || numbered.is_some() {
            return Name::Copy;
        }
    }
    Name::Other
}

/// Adds every copy of the session record in the state root to `survey`, and marks `Store::LegacySession` complete only after the root and every copy
/// were listed to the end.
///
/// A root that isn't there is complete and empty. A root that isn't a real folder of this user's (a link, a file), one that can't be listed, a copy
/// that vanishes between the listing and its opening, a copy's `journals` that is no folder, and a folder that can't be listed leave the store
/// unmarked, so the plan says `Incomplete` rather than "none found". A root of more than 8,192 entries, or more than 64 copies, is `TooLarge`.
/// `add`'s refusal passes through.
pub fn survey_legacy_sessions(root: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey) -> Result<(), Refusal> {
    survey_legacy_sessions_with(root, lineage, mode, survey, &LegacyCaps::default())
}

pub(crate) fn survey_legacy_sessions_with(root: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey, caps: &LegacyCaps) -> Result<(), Refusal> {
    let folder = match Folder::open(root) {
        Opened::Missing => {
            survey.mark_complete(Store::LegacySession);
            return Ok(());
        }
        Opened::Other => return Ok(()),
        Opened::Folder(folder) => folder,
    };
    #[cfg(test)]
    if let Some(hook) = &caps.before_list {
        hook(root);
    }
    let Ok(entries) = folder.entries() else { return Ok(()) };
    let mut listed: Vec<Entry> = Vec::new();
    let mut seen = 0usize;
    for entry in entries {
        let Ok(entry) = entry else { return Ok(()) };
        // Every entry the listing returns counts, this store's or not: the root holds other stores' entries, and a root of a million of them is not
        // read to its end (one `fstatat` each). Only this store's are kept.
        seen += 1;
        if seen > caps.entries {
            return Err(Refusal::TooLarge);
        }
        if legacy_kind(entry.name.as_encoded_bytes()) != Name::NotOurs {
            listed.push(entry);
        }
    }
    let maps = Maps::of(lineage);
    let mut copies = 0usize;
    for entry in &listed {
        let name = entry.name.as_encoded_bytes();
        let kind = legacy_kind(name);
        if kind == Name::Copy {
            copies += 1;
            if copies > caps.copies {
                return Err(Refusal::TooLarge);
            }
            #[cfg(test)]
            if let Some(hook) = &caps.before_open {
                hook(&root.join(&entry.name));
            }
            match folder.open_in(&entry.name) {
                Opened::Missing => return Ok(()),
                Opened::Folder(copy) => {
                    let mut prefix = name.to_vec();
                    prefix.push(b'/');
                    if !survey_copy(
                        &copy,
                        &prefix,
                        Store::LegacySession,
                        Some(WENT_BACK),
                        &root.join(&entry.name),
                        &maps,
                        mode,
                        &caps.folder,
                        survey,
                    )? {
                        return Ok(());
                    }
                    continue;
                }
                Opened::Other => {}
            }
        }
        // A copy from a newer build, a name nobody writes, and a copy that is no folder of ours (a link, a file, another user's): one artifact each,
        // never opened.
        survey.add(Found {
            store: Store::LegacySession,
            key: ArtifactKey::of(Store::LegacySession, name),
            owner: Owner::Unknown,
            format: if kind == Name::Newer { Format::Foreign } else { Format::Current },
        })?;
    }
    survey.mark_complete(Store::LegacySession);
    Ok(())
}

/// One copy folder's entries added to `survey` in `store`, and its `journals` folder through the journals adapter. Whether it was listed to the end
/// and every entry added (`false` leaves the store unmarked).
///
/// `prefix` is the copy's place under the state root with its closing `/` (every key starts with it). `marker` is the one plain file of one link that
/// is left out as the app's own marker, if the copy has one. `at` is where the folder was when opened, for a test's seams; it is never opened. No entry
/// of the copy is opened: a slot, `places.log`, a temp and anything else is `Unknown`.
#[allow(clippy::too_many_arguments)]
pub(super) fn survey_copy(
    folder: &Folder,
    prefix: &[u8],
    store: Store,
    marker: Option<&[u8]>,
    at: &Path,
    maps: &Maps,
    mode: TargetMode,
    caps: &Caps,
    survey: &mut Survey,
) -> Result<bool, Refusal> {
    #[cfg(test)]
    if let Some(hook) = &caps.before_list {
        hook(at);
    }
    let Ok(entries) = folder.entries() else { return Ok(false) };
    let mut listed: Vec<Entry> = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { return Ok(false) };
        if listed.len() >= caps.entries {
            return Err(Refusal::TooLarge);
        }
        listed.push(entry);
    }
    for entry in &listed {
        let name = entry.name.as_encoded_bytes();
        if name == b"journals" {
            continue;
        }
        let owner = match backups::finder_file(name, entry.is_file, entry.links) {
            Some(Finder::Skip) => continue,
            Some(Finder::Owner(owner)) => owner,
            None if marker == Some(name) && entry.is_file && entry.links == 1 => continue,
            None => Owner::Unknown,
        };
        let mut key = prefix.to_vec();
        key.extend_from_slice(name);
        survey.add(Found {
            store,
            key: ArtifactKey::of(store, &key),
            owner,
            format: Format::Current,
        })?;
    }
    let journals = match folder.open_in(OsStr::new("journals")) {
        Opened::Missing => return Ok(true),
        Opened::Other => return Ok(false),
        Opened::Folder(journals) => journals,
    };
    let mut journals_prefix = prefix.to_vec();
    journals_prefix.extend_from_slice(b"journals/");
    journals::survey_journals_folder(&journals, &journals_prefix, Some(store), &at.join("journals"), maps, mode, caps, survey)
}

#[cfg(test)]
mod tests;
