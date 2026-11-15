//! No Keychain here: every run's Find key is in memory.

use super::FindKey;

pub(super) const SERVICE: &str = "io.github.kkollsga.RectoPDF.find-text";
pub(super) const ACCOUNT: &str = "find-text-key-1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refused {
    NotTheTeam,
    Unavailable,
}

impl Refused {
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::NotTheTeam => "not_signed_by_the_team",
            Self::Unavailable => "keychain_unavailable",
        }
    }
}

/// Whether this copy is the team's release: never, where there's no Keychain. The three answers the Mac's check gives, as `find_key` names them all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) enum Release {
    Yes,
    No,
    Unknown,
}

pub(super) fn signed_as_a_release(_team: &str) -> Release {
    Release::No
}

pub(super) fn prompts_switched_off() -> bool {
    true
}

pub(super) fn load_or_create(_service: &str, _account: &str, _group: &str) -> Result<FindKey, Refused> {
    Err(Refused::Unavailable)
}

#[cfg(feature = "keychain-probe")]
pub(super) fn remove(_service: &str, _account: &str, _group: &str) -> i32 {
    -1
}
