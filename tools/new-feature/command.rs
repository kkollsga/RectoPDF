//! `recto new feature`: FILL IN what it does, in a line. Made by `tools/new-feature.sh new_feature`: the path to done is `docs/FEATURES.md`, and Find
//! (`commands/finding.rs`) is a whole one.
//!
//! REGISTER (step 1): one line in the `family!` of `commands/mod.rs` it belongs to, `NewFeature(new_feature::NewFeature),`; then `RECTO_BLESS=1` for
//! the goldens, and the command in `every_command()` in `policy_guards.rs`. Until then it has no wire, no CLI and no help.
//! (This file is the scaffold's template: `recto-agent`'s tests compile it as `commands::scaffold_template`, so a change to `Cmd` that breaks it fails
//! there. What's between `template-only` lines isn't copied.)

use super::{Cmd, Cx, row_access};
use crate::table::*;
use crate::*;
use serde_json::{Value, json};

/// FILL IN what it asks. A choice of words, or of flags that can't go together, is a type read from a `line` (`finding.rs`).
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NewFeature {
    #[serde(default)]
    pub tab: TabRef,
}

// Each property is a decision with no default: `Cmd`'s docs in `commands/mod.rs` say what each guards. The values here are placeholders until
// `fill_me_in_new_feature_gate_level_and_route` is replaced.
impl Cmd for NewFeature {
    const TAG: &'static str = "new_feature";
    const ROWS: &'static [Spec] = ROWS_NEW_FEATURE;
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

pub(crate) const ROWS_NEW_FEATURE: &[Spec] = &[Spec {
    name: "new feature",
    tag: "new_feature",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "FILL IN: what it does, as `recto help` says it.",
    args: &[TAB],
    data: "`{ tab }`.",
    examples: &["recto new feature"],
    ..BASE
}];

#[cfg(test)]
mod tests {
    /// FILL IN (unit tier): the rule this command decides, tested here without the app, named for the failure it catches.
    #[test]
    fn fill_me_in_new_feature_rule() {
        // template-only: begin
        if module_path!().contains("scaffold_template") {
            return;
        }
        // template-only: end
        panic!("fill me in: the rule `new_feature` decides, as a unit test beside it (docs/FEATURES.md, step 10)");
    }

    /// FILL IN: the placeholders above are decisions. Its level (and why not one lower), its route, whose document it reads (`ABOUT`), whether it
    /// may act on a protected document (`PROTECTED_OK`), whether it draws, reads pages or waits. Say each in the brief; then delete this test.
    #[test]
    fn fill_me_in_new_feature_gate_level_and_route() {
        // template-only: begin
        if module_path!().contains("scaffold_template") {
            return;
        }
        // template-only: end
        panic!("fill me in: decide `new_feature`'s level, route, ABOUT, PROTECTED_OK, DRAWS, READS_PAGES and WAITS: they are placeholders");
    }
}
