//! `recto highlight`: FILL IN what it does, in a line. Made by `tools/new-feature.sh highlight`: the path to done is `docs/FEATURES.md`, and Find
//! (`commands/finding.rs`) is a whole one.
//!
//! REGISTER (step 1): one line in the `family!` of `commands/mod.rs` it belongs to, `Highlight(highlight::Highlight),`; then `RECTO_BLESS=1` for
//! the goldens, and the command in `every_command()` in `policy_guards.rs`. Until then it has no wire, no CLI and no help.

use super::{Cmd, Cx, row_access};
use crate::table::*;
use crate::*;
use serde_json::{Value, json};

/// FILL IN what it asks. A choice of words, or of flags that can't go together, is a type read from a `line` (`finding.rs`).
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Highlight {
    #[serde(default)]
    pub tab: TabRef,
}

// Each property is a decision with no default: `Cmd`'s docs in `commands/mod.rs` say what each guards. The values here are placeholders until
// `fill_me_in_highlight_gate_level_and_route` is replaced.
impl Cmd for Highlight {
    const TAG: &'static str = "highlight";
    const ROWS: &'static [Spec] = ROWS_HIGHLIGHT;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&TabRef> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        json!({ "tab": super::log_tab(&self.tab) })
    }
    /// FILL IN: the feature's own function, given `cx`; what differs between the person and an agent is asked of `cx.actor`.
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let id = cx.access.with(|s| select(s, &self.tab))?;
        Ok(json!({ "tab": id.0 }))
    }
    fn human(data: &Value) -> String {
        format!("FILL IN: tab {}.", data["tab"])
    }
}

pub(crate) const ROWS_HIGHLIGHT: &[Spec] = &[Spec {
    name: "highlight",
    tag: "highlight",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "FILL IN: what it does, as `recto help` says it.",
    args: &[TAB],
    data: "`{ tab }`.",
    examples: &["recto highlight"],
    ..BASE
}];

#[cfg(test)]
mod tests {
    /// FILL IN (unit tier): the rule this command decides, tested here without the app, named for the failure it catches.
    #[test]
    fn fill_me_in_highlight_rule() {
        panic!("fill me in: the rule `highlight` decides, as a unit test beside it (docs/FEATURES.md, step 10)");
    }

    /// FILL IN: the placeholders above are decisions. Its level (and why not one lower), its route, whose document it reads (`ABOUT`), whether it
    /// may act on a protected document (`PROTECTED_OK`), whether it draws, reads pages or waits. Say each in the brief; then delete this test.
    #[test]
    fn fill_me_in_highlight_gate_level_and_route() {
        panic!("fill me in: decide `highlight`'s level, route, ABOUT, PROTECTED_OK, DRAWS, READS_PAGES and WAITS: they are placeholders");
    }
}
