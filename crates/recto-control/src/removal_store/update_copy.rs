//! The copy of the session record an update keeps for a week in case it has to go back (`<root>/update/before/`), for `Store::UpdateCopy`
//! (`docs/design/removal.md` §3, §4 and §10 "D2c-3 as built"). Surveyed by the same function as the other copies (`legacy_session::survey_copy`): its
//! slots and `places.log` are `Unknown` and never opened, its `journals/` go through the journals adapter in this store, and the only entry left out is
//! the plain `.whole` marker of one link (the installer writes it as an empty file and reads only whether it is there and when it was made).
//!
//! **A missing `.whole` never means empty.** A copy killed partway has no `.whole`, and nothing deletes it before the next install, so it is surveyed
//! exactly as a whole one is.
//!
//! **Read-only.** The root, `update` and `before` are opened one from the other without following a link, each parent dropped as the next is opened, so
//! at most three descriptors are open (`before`, its `journals` and the journal file whose header is being read). A step that isn't there is complete and empty; one that is anything but a folder of this user's leaves the store
//! unmarked.

use super::journals::Caps;
use super::legacy_session::survey_copy;
use super::placing::Maps;
use crate::record_store::UPDATE_COPY;
use recto_core::removal::{Lineage, Refusal, Store, Survey, TargetMode};
use recto_platform::folder::{Folder, Opened};
use std::ffi::OsStr;
use std::path::Path;

#[cfg(test)]
use super::testing::Hook;

/// The marker `copy_record` writes last, once every file is copied.
const WHOLE: &[u8] = b".whole";

/// The caps, so a test can make them small.
#[derive(Clone, Default)]
pub(crate) struct UpdateCaps {
    /// For `before` and its `journals` (its entries, the header bytes read, and the journals' hooks).
    pub folder: Caps,
    /// Called with a path (`root/update`, then `root/update/before`) just before it is opened: a test's way to change it under the survey.
    #[cfg(test)]
    pub before_open: Option<Hook>,
}

/// Adds every file of the update's copy of the record to `survey`, and marks `Store::UpdateCopy` complete only after it was listed to the end.
///
/// Missing at any step (no `update`, no `before`) is complete and empty. Anything but a folder of this user's at any step, a folder that can't be
/// listed, and a `journals` that is no folder leave the store unmarked, so the plan says `Incomplete`. A folder of more than 8,192 entries is `TooLarge`.
/// `add`'s refusal passes through.
pub fn survey_update_copy(root: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey) -> Result<(), Refusal> {
    survey_update_copy_with(root, lineage, mode, survey, &UpdateCaps::default())
}

pub(crate) fn survey_update_copy_with(root: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey, caps: &UpdateCaps) -> Result<(), Refusal> {
    let mut folder = match Folder::open(root) {
        Opened::Missing => {
            survey.mark_complete(Store::UpdateCopy);
            return Ok(());
        }
        Opened::Other => return Ok(()),
        Opened::Folder(folder) => folder,
    };
    let mut at = root.to_path_buf();
    for part in UPDATE_COPY {
        at.push(part);
        #[cfg(test)]
        if let Some(hook) = &caps.before_open {
            hook(&at);
        }
        // The parent is dropped as the next folder takes its place.
        folder = match folder.open_in(OsStr::new(part)) {
            Opened::Missing => {
                survey.mark_complete(Store::UpdateCopy);
                return Ok(());
            }
            Opened::Other => return Ok(()),
            Opened::Folder(next) => next,
        };
    }
    let mut prefix = String::new();
    for part in UPDATE_COPY {
        prefix.push_str(part);
        prefix.push('/');
    }
    if survey_copy(
        &folder,
        prefix.as_bytes(),
        Store::UpdateCopy,
        Some(WHOLE),
        &at,
        &Maps::of(lineage),
        mode,
        &caps.folder,
        survey,
    )? {
        survey.mark_complete(Store::UpdateCopy);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
