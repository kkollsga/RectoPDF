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

pub(super) fn signed_as_a_release(_team: &str) -> bool {
    false
}

pub(super) fn load_or_create(_service: &str, _account: &str) -> Result<FindKey, Refused> {
    Err(Refused::Unavailable)
}
