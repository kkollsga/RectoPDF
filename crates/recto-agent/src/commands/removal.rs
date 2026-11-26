//! **Clear leftovers** (`docs/design/removal.md` §10, D5): `removal check` and `removal clear`. What makes a removal refuse today, the unfinished saves a crashed
//! writer left beside a document and the copies of the session kept for going back, is moved to the Trash or deleted after one confirmation. The app answers
//! them (`recto-app`'s `core/removal.rs`, with the rules and the words in `recto-control`'s `removal_store/leftovers`); a session with no window says it has none.
//!
//! - **`removal check <tab>`** reads only, at ReadOnly. It says what a clear would do and what it leaves, and gives a plan to confirm only to one who may clear.
//! - **`removal clear <tab> --confirm <plan>`** needs Full, and the person may run it too: it is the one command the window's sheet will send. The plan is the
//!   asker's own, for this tab's document, and good for ten minutes while nothing it was about has changed.

use super::{Cmd, Cx, OnlyAsks, row_access};
use crate::Code;
use crate::human::text;
use crate::table::*;
use crate::*;
use recto_core::removal::PlanId;
use serde::Deserialize;
use serde_json::{Value, json};

/// What clearing the leftovers beside a tab's document would do.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemovalCheck {
    #[serde(default)]
    pub tab: TabRef,
}

/// Clears what `removal check` offered: the plan it gave, confirmed.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
#[serde(into = "line::RemovalClear")]
pub struct RemovalClear {
    pub tab: TabRef,
    pub confirm: PlanId,
}

/// `removal clear` as it's written on the wire.
pub mod line {
    use super::TabRef;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub struct RemovalClear {
        #[serde(default)]
        pub tab: TabRef,
        pub confirm: String,
    }
}

/// A plan's number as `removal check` gives it: 16 hex digits, so no client's number type can round it.
pub fn plan_word(id: PlanId) -> String {
    format!("{:016x}", id.get())
}

/// What the check's words say how to confirm, and what the line must be for [`RemovalClear`].
const BAD_PLAN: &str = "The plan is the 16 letters and digits `removal check` gave.";

impl TryFrom<line::RemovalClear> for RemovalClear {
    type Error = AgentError;
    fn try_from(line: line::RemovalClear) -> Result<Self, AgentError> {
        let usage = || AgentError::new(Code::USAGE, BAD_PLAN, exit::USAGE);
        let word = line.confirm.trim();
        let digits = word.len() == 16 && word.bytes().all(|b| b.is_ascii_hexdigit());
        let id = digits
            .then(|| u64::from_str_radix(word, 16).ok())
            .flatten()
            .and_then(PlanId::from_raw)
            .ok_or_else(usage)?;
        Ok(Self { tab: line.tab, confirm: id })
    }
}

impl From<&RemovalClear> for line::RemovalClear {
    fn from(asked: &RemovalClear) -> Self {
        Self {
            tab: asked.tab.clone(),
            confirm: plan_word(asked.confirm),
        }
    }
}

impl From<RemovalClear> for line::RemovalClear {
    fn from(asked: RemovalClear) -> Self {
        Self::from(&asked)
    }
}

impl std::fmt::Debug for RemovalClear {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        line::RemovalClear::from(self).fmt(f)
    }
}

impl Cmd for RemovalCheck {
    const TAG: &'static str = "removal_check";
    const ROWS: &'static [Spec] = ROWS_REMOVAL_CHECK;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    /// It lists other documents' unfinished saves in the folder of a document opened with its password, so an agent isn't given it.
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    /// It lists folders: on a slow or stopped volume that takes as long as the volume likes.
    const WAITS: bool = true;
    fn starts_from(&self) -> super::FromPlace {
        super::FromPlace::No
    }
    fn tab(&self) -> Option<&TabRef> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::App
    }
    fn log_details(&self) -> Value {
        json!({ "tab": super::log_tab(&self.tab) })
    }
    /// Only the app has what is to be cleared.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(removal_needs_app())
    }
    fn human(data: &Value) -> String {
        let mut out = text(&data["words"]).to_string();
        if let Some(id) = data["plan"].as_str() {
            out.push_str(&format!("\n\nTo do it: recto removal clear --confirm {id}"));
        }
        out
    }
}

impl Cmd for RemovalClear {
    const TAG: &'static str = "removal_clear";
    const ROWS: &'static [Spec] = ROWS_REMOVAL_CLEAR;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = true;
    fn starts_from(&self) -> super::FromPlace {
        super::FromPlace::No
    }
    fn tab(&self) -> Option<&TabRef> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::App
    }
    /// The plan's number is a secret of its asker, so it isn't logged.
    fn log_details(&self) -> Value {
        json!({ "tab": super::log_tab(&self.tab) })
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(removal_needs_app())
    }
    fn human(data: &Value) -> String {
        text(&data["words"]).to_string()
    }
}

impl super::Removal {
    /// A check only asks; a clear changes what an update carries across and is waited for.
    pub fn only_asks(&self) -> bool {
        match self {
            Self::RemovalCheck(command) => command.only_asks(),
            Self::RemovalClear(command) => command.only_asks(),
        }
    }
}

impl OnlyAsks for RemovalCheck {
    fn only_asks(&self) -> bool {
        true
    }
}

impl OnlyAsks for RemovalClear {
    fn only_asks(&self) -> bool {
        false
    }
}

/// What a session with no window says: leftovers are the app's.
pub fn removal_needs_app() -> AgentError {
    AgentError::new(
        Code::NEEDS_APP,
        "Clearing leftovers belongs to the app, and this session has no window.",
        exit::FAILED,
    )
    .then("Open RectoPDF, and `recto` will talk to that.")
}

pub(crate) const ROWS_REMOVAL_CHECK: &[Spec] = &[Spec {
    name: "removal check",
    tag: "removal_check",
    group: GROUP_APP,
    summary: "Says what clearing the leftovers beside a tab's document would do, and what it would leave. Leftovers are what makes a removal refuse: unfinished saves a crashed writer left (in the document's folder and the other folders it was saved to) go to the Trash, and the copies of the session kept for going back to an earlier version are deleted whole. A save still being written, or on a volume with no Trash, is left where it is and said. It reads only and takes nothing. Needs Full to be given a plan to confirm; below that it says what it found. Only in the app.",
    args: &[TAB],
    data: "`{ words, button, found: { saves_here, saves_elsewhere, other_folders, copies, waiting, no_trash }, plan, offered, refused, untrusted: { document, folder } }`. `plan` is 16 hex digits for `removal clear --confirm`, and only when there is something to clear and the asker may clear. `untrusted` is the person's.",
    examples: &["recto removal check", "recto removal check --tab report"],
    client_wait_s: 60,
    ..BASE
}];

pub(crate) const ROWS_REMOVAL_CLEAR: &[Spec] = &[Spec {
    name: "removal clear",
    tag: "removal_clear",
    group: GROUP_APP,
    access: Access::At(Level::Full),
    summary: "Clears the leftovers `removal check` showed, for the plan it gave: moves the unfinished saves to the Trash (empty the Trash to remove them for good) and deletes the copies of the session kept for going back, after which an update that goes back may open without the person's tabs. The plan is the asker's own, for this tab's document, and good for 10 minutes; a save of the document, or any change to what was found, refuses it with nothing done. It waits for no one: an update being installed or a `recto session` running is `removal_busy`. It never touches the document, a journal, a backup or the session in use. Stops after 10 seconds where it is, and says how far it got. Only in the app.",
    args: &[
        Arg {
            key: "confirm",
            flag: Some("--confirm"),
            kind: Kind::Text,
            required: true,
            help: "The plan `removal check` gave: 16 hex digits.",
        },
        TAB,
    ],
    data: "`{ words, outcome, cleared: { trashed, copies }, acted, planned, stopped }`. `outcome` is `complete`, `partly_cleared` or `unchanged`.",
    examples: &["recto removal clear --confirm 3f9a0c51d27be864"],
    client_wait_s: 60,
    ..BASE
}];
