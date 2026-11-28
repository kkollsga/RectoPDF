//! **The record from before the last update:** offered once to fall back on, put back as a notice says, and dropped after its week. Moved out of `updater.rs`
//! for its size, unchanged but for the removal record's check inside `drop_an_old_record_copy`.

use super::*;

impl Updater {
    /// The record from before the update, if this launch is the version before coming back from one that kept ending, and it's there: for when what that one left can't be read here. Taken once.
    #[must_use]
    pub fn take_record_to_fall_back_on(&self) -> Option<PathBuf> {
        lock(&self.st).fall_back.take()
    }

    /// The record from before the update was put back, as what the version that didn't work left couldn't be read: the notice says so.
    pub fn fell_back(&self) {
        let mut st = lock(&self.st);
        if let Status::WentBack { version, back_on } = &st.status {
            let text = format!(
                "{APP_NAME} {version} kept quitting unexpectedly, and what it left couldn't be read, so {back_on} came back with your session from before the update."
            );
            st.notice = Some(Notice { text, fades: true });
        }
        drop(st);
        self.log("what the version that didn't work left couldn't be read, so the record from before the update was put back");
    }

    /// What's kept of the record from before the last update goes once it's a week old, counted from `.whole`, which is made at the install (not from a refusal), as PRIVACY.md says. One look at one file's time. **Never while an unfinished removal's record is there:** the copy is something its plan pinned, and it is left as it is.
    pub(super) fn drop_an_old_record_copy(&self, now_ms: u64, offered_to_fall_back_on: bool) {
        if crate::removal_record::anything_there(&self.files.root) {
            return;
        }
        let whole = self.files.before().join(".whole");
        let made_ms = std::fs::metadata(&whole)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| since.as_millis());
        if !offered_to_fall_back_on && made_ms.is_some_and(|made| u128::from(now_ms).saturating_sub(made) > u128::from(KEEP_RECORD_COPY_MS)) {
            let _ = std::fs::remove_dir_all(self.files.before());
        }
    }
}
