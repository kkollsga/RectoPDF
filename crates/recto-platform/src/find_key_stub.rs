//! No Keychain here: every run's Find key is in memory.

use super::FindKey;

pub const SERVICE: &str = "io.github.kkollsga.RectoPDF.find-text";
pub const ACCOUNT: &str = "find-text-key-1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    NotTheTeam,
    Unavailable,
}

impl Refused {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotTheTeam => "not_signed_by_the_team",
            Self::Unavailable => "keychain_unavailable",
        }
    }
}

pub fn signed_by_team(_team: &str) -> bool {
    false
}

pub fn load_or_create(_service: &str, _account: &str) -> Result<FindKey, Refused> {
    Err(Refused::Unavailable)
}
