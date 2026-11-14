//! **What the updater remembers, in its own file** (`updates.json`): written whole, off the lock the UI thread asks through, and never before it's been read.

use super::{Arc, Saved, Updater, lock};

impl Updater {
    /// Writes what's remembered, whatever it is by the time the last write's done, off the lock the UI thread asks through. **Never before it's been read:**
    /// until the launch is told ([`Self::launched`], which reads `updates.json` and sets the facts in the same step) what's held is a fresh file's defaults
    /// and any switch chosen meanwhile, and writing that would put defaults over what an update proved, what it's watching and the versions that failed. A
    /// switch chosen then is kept and goes to disk on top of what's read ([`chosen_before`]). Nor by a second copy, whose state folder is another's.
    pub(super) fn save(&self) {
        let _writing = lock(&self.writing);
        let saved = {
            let st = lock(&self.st);
            (st.facts.is_some() && !st.second_copy).then(|| st.saved.clone())
        };
        if let Some(saved) = saved {
            saved.write(&self.files.saved);
        }
    }

    /// Writes what's remembered on a thread of its own, as this may be the UI thread.
    pub(super) fn save_soon(self: &Arc<Self>) {
        let me = Arc::clone(self);
        let written = std::thread::Builder::new()
            .name("recto-update-save".into())
            .stack_size(128 * 1024)
            .spawn(move || me.save());
        if written.is_err() {
            self.save();
        }
    }
}

/// **The switches chosen before the launch was told, kept on top of what's read** ([`Updater::save`] writes nothing before then): `before` is what the updater
/// held until now, a fresh file's defaults with only what was chosen set; `read` is `updates.json`. *Update automatically* as chosen; *Allow development
/// builds* as chosen, with the way back to a release it sets ([`Updater::choose_dev_builds`]: off on a development build starts it, on ends it). Says whether
/// that changed what was read, so it's written.
pub(super) fn chosen_before(before: &Saved, read: &mut Saved, current: &str) -> bool {
    let was = (read.auto_update, read.dev_builds, read.back_to_release);
    if before.auto_update.is_some() {
        read.auto_update = before.auto_update;
    }
    if let Some(on) = before.dev_builds {
        read.dev_builds = Some(on);
        read.back_to_release = !on && crate::version::is_development(current);
    }
    was != (read.auto_update, read.dev_builds, read.back_to_release)
}
