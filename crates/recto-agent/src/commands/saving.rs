//! `save` and `save-as`: a tab's edits into its file, or its document as a new one. The writing is the saving module's.

use super::{Cmd, Cx, row_access};
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

/// Saves a tab: into its file, or with `out` as a new one (Save As).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Save {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    /// Save As: the new file. Without it, the file itself.
    #[serde(default)]
    pub out: Option<PathBuf>,
    /// Save As over a file that's there, which the person chose to replace.
    #[serde(default)]
    pub replace: bool,
}

impl Cmd for Save {
    const TAG: &'static str = "save";
    const ROWS: &'static [Spec] = ROWS_SAVE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab, out, replace } = self;
        match out {
            Some(out) => json!({ "tab": super::log_tab(tab), "replace": replace, "untrusted": super::log_file(out) }),
            None => json!({ "tab": super::log_tab(tab) }),
        }
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab, out, replace } = self;
        let to = match out {
            Some(out) => saving::SaveTo::As {
                out: out.clone(),
                replace: *replace,
            },
            None if *replace => return Err(AgentError::new("usage", "`--replace` goes with Save As's `--out`.", exit::USAGE)),
            None => saving::SaveTo::Same,
        };
        saving::save(cx.access, cx.level, tab, &to, cx.events)
    }
    /// Its answer, every field, as it has always been put.
    fn human(data: &Value) -> String {
        serde_json::to_string_pretty(data).unwrap_or_default()
    }
}

pub(crate) const ROWS_SAVE: &[Spec] = &[
    Spec {
        name: "save",
        tag: "save",
        group: GROUP_EDIT,
        access: Access::At(Level::Control),
        summary: "Saves a tab's edits into its file. Turns, moves and added pages are added to the end of the file, so everything that was in it stays. The first save of a file keeps one backup of the original. A save that deletes pages writes the file anew, so what was deleted is gone from it, and needs Full. A certified PDF's pages aren't saved over. Nothing from the edit history goes in the file.",
        args: &[TAB],
        data: SAVED,
        emits: &["saved", "edits_changed"],
        examples: &["recto save", "recto save --tab report --json"],
        client_wait_s: 600,
        ..BASE
    },
    Spec {
        name: "save-as",
        tag: "save",
        group: GROUP_EDIT,
        access: Access::At(Level::Control),
        summary: "Saves a tab's document as a new file, and the tab goes over to it. The original is left as it was. Replacing a file that's there needs `--replace` and Full access.",
        args: &[
            Arg {
                key: "out",
                flag: Some("--out"),
                kind: Kind::Path,
                required: true,
                help: "The new file.",
            },
            Arg {
                key: "replace",
                flag: Some("--replace"),
                kind: Kind::Switch,
                required: false,
                help: "Replace a file that's there. Needs Full.",
            },
            TAB,
        ],
        data: SAVED,
        emits: &["saved", "edits_changed"],
        examples: &["recto save-as --out ~/Documents/report-final.pdf"],
        client_wait_s: 600,
        ..BASE
    },
];
