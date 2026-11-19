//! The installer operation and its single real process launch.

use recto_platform::local;
use std::path::Path;

/// Which operation the installer carries out on its plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallerMode {
    Install,
    Back,
}

impl InstallerMode {
    /// Starts the helper through the one real launch implementation.
    ///
    /// # Errors
    ///
    /// The helper could not be started.
    pub fn start(self, helper: &Path, plan: &Path, log: &Path) -> std::io::Result<u32> {
        let command = match self {
            Self::Install => "install",
            Self::Back => "back",
        };
        local::spawn_detached(helper, &[command.to_owned(), plan.display().to_string()], &[], log)
    }
}
