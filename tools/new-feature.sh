#!/usr/bin/env bash
# new-feature: a feature's first files, each with a test that fails saying what to fill in, and the checklist of docs/FEATURES.md (R4.2).
#
#   tools/new-feature.sh <name>            e.g. page_labels: a command `page_labels`, `recto page labels`
#   tools/new-feature.sh --remove <name>   takes out what <name> made, and nothing else
#
# Makes two files and adds one marked line to each of two shared files, so the tree compiles and `tools/check.sh --quick` fails only on the
# `fill_me_in_<name>_…` tests:
#   crates/recto-agent/src/commands/<name>.rs        the command: payload, Cmd (no defaults), its row, its words, a unit test to fill in
#   crates/recto-app/src/core/headless/<name>.rs      the headless app's tests, the window tier's and the shots', to fill in
#   + `pub mod <name>;` in crates/recto-agent/src/commands/mod.rs, `mod <name>;` in crates/recto-app/src/core/headless/mod.rs
# The command isn't registered in a family yet: that line, the goldens and `every_command()` are the first step of the checklist, because they're
# where the guards start to hold the command. Small on purpose: generated text is debt; docs/FEATURES.md and Find are the example.
set -euo pipefail
cd "$(dirname "$0")/.."
remove=0
if [ "${1:-}" = --remove ]; then
  remove=1
  shift
fi
name=${1:-}
if ! [[ $name =~ ^[a-z][a-z0-9]*(_[a-z0-9]+)*$ ]]; then
  echo "new-feature: give a name in snake_case, as its command's tag (page_labels)." >&2
  exit 2
fi
command_file=crates/recto-agent/src/commands/$name.rs
tests_file=crates/recto-app/src/core/headless/$name.rs
commands_mod=crates/recto-agent/src/commands/mod.rs
headless_mod=crates/recto-app/src/core/headless/mod.rs
mark="// new-feature: $name"

if [ "$remove" = 1 ]; then
  rm -f "$command_file" "$tests_file"
  for file in "$commands_mod" "$headless_mod"; do
    grep -vF -- "$mark" "$file" > "$file.new" || true
    mv "$file.new" "$file"
  done
  echo "new-feature: $name removed."
  exit 0
fi
for file in "$command_file" "$tests_file"; do
  [ ! -e "$file" ] || { echo "new-feature: $file is already there." >&2; exit 1; }
done
camel=$(printf '%s' "$name" | awk -F_ '{ for (i = 1; i <= NF; i++) printf "%s%s", toupper(substr($i, 1, 1)), substr($i, 2) }')
upper=$(printf '%s' "$name" | tr '[:lower:]' '[:upper:]')
spaced=$(printf '%s' "$name" | tr '_' ' ')

cat > "$command_file" <<EOF
//! \`recto $spaced\`: FILL IN what it does, in a line. Made by \`tools/new-feature.sh $name\`: the path to done is \`docs/FEATURES.md\`, and Find
//! (\`commands/finding.rs\`) is a whole one.
//!
//! REGISTER (step 1): one line in the \`family!\` of \`commands/mod.rs\` it belongs to, \`$camel($name::$camel),\`; then \`RECTO_BLESS=1\` for the goldens,
//! and the command in \`every_command()\` in \`policy_guards.rs\`. Until then it has no wire, no CLI and no help.

use super::{Cmd, Cx, row_access};
use crate::table::*;
use crate::*;
use serde_json::{Value, json};

/// FILL IN what it asks. A choice of words, or of flags that can't go together, is a type read from a \`line\` (\`finding.rs\`).
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct $camel {
    #[serde(default)]
    pub tab: TabRef,
}

// Each property is a decision with no default: \`Cmd\`'s docs in \`commands/mod.rs\` say what each guards.
impl Cmd for $camel {
    const TAG: &'static str = "$name";
    const ROWS: &'static [Spec] = ROWS_$upper;
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
    /// FILL IN: the feature's own function, given \`cx\`; what differs between the person and an agent is asked of \`cx.actor\`.
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let id = cx.access.with(|s| select(s, &self.tab))?;
        Ok(json!({ "tab": id.0 }))
    }
    fn human(data: &Value) -> String {
        format!("FILL IN: tab {}.", data["tab"])
    }
}

pub(crate) const ROWS_$upper: &[Spec] = &[Spec {
    name: "$spaced",
    tag: "$name",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "FILL IN: what it does, as \`recto help\` says it.",
    args: &[TAB],
    data: "\`{ tab }\`.",
    examples: &["recto $spaced"],
    ..BASE
}];

#[cfg(test)]
mod tests {
    /// FILL IN (unit tier): the rule this command decides, tested here without the app, named for the failure it catches.
    #[test]
    fn fill_me_in_${name}_rule() {
        panic!("fill me in: the rule \`$name\` decides, as a unit test in $command_file (docs/FEATURES.md, step 10)");
    }
}
EOF

cat > "$tests_file" <<EOF
//! \`$name\` through the app core, with no window (\`docs/FEATURES.md\`, step 10). Made by \`tools/new-feature.sh $name\`: each test fails saying what
//! goes in its place. Replace each with tests that name the failure they catch; Find's are \`find.rs\` and \`finding.rs\` beside this file.

use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;

/// FILL IN (headless tier): what the person's control and an agent's command do, through the one executor, read from what the app publishes.
#[test]
fn fill_me_in_${name}_through_the_app_core() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("$name.pdf", 2);
    app.open(&file);
    let tabs = app.agent(Level::Control, r#"{"cmd":"tabs"}"#).expect("tabs");
    assert_eq!(tabs["count"], 1, "the kit is up: {tabs}");
    panic!("fill me in: \`$name\` sent by an agent and by the person's control (\`app.act\`), and what the window is told, in $tests_file");
}

/// FILL IN (window tier): what only the real app shows of it goes into a launch that is already there; the ratchet holds the number of launches.
#[test]
fn fill_me_in_${name}_window_checks_join_a_launch() {
    panic!("fill me in: what only the real app shows of \`$name\` (drawing, AppKit), added to a test in crates/recto-cli/tests/app.rs that already launches it; then delete this test");
}

/// FILL IN (shots): the feature once, light and dark, on the contact sheet.
#[test]
fn fill_me_in_${name}_shots_light_and_dark() {
    panic!("fill me in: one step for \`$name\` in app/qa/contact-sheet.sh, run, and its sheets looked at; then delete this test");
}
EOF

# The one line each shared file takes, in its place in the sorted list of `mod` lines (before an attribute it would go under), marked so --remove
# takes exactly it.
insert_mod() { # FILE PREFIX
  LC_ALL=C awk -v prefix="$2" -v name="$name" -v line="$2$name; $mark" '
    { lines[NR] = $0 }
    index($0, prefix) == 1 && at == 0 { mod = substr($0, length(prefix) + 1); sub(/;.*/, "", mod); if (mod > name) at = (NR > 1 && lines[NR - 1] ~ /^#\[/) ? NR - 1 : NR }
    index($0, prefix) == 1 { last = NR }
    END { if (at == 0) at = last + 1; for (i = 1; i <= NR; i++) { if (i == at) print line; print lines[i] } if (at == NR + 1) print line }
  ' "$1" > "$1.new"
  mv "$1.new" "$1"
}
insert_mod "$commands_mod" "pub mod "
insert_mod "$headless_mod" "mod "
rustfmt --edition 2024 "$command_file" "$tests_file"

cat <<EOF
new-feature: made $command_file and $tests_file, and one marked line in $commands_mod and $headless_mod.
\`tools/check.sh --quick\` now fails on the four fill_me_in_${name}_ tests and nothing else. The path, each step in docs/FEATURES.md:
   1. Register the command: \`$camel($name::$camel),\` in a family! of $commands_mod; RECTO_BLESS=1 for the goldens; every_command() in policy_guards.rs.
   2. Its typed payload and words; a new refusal's Code in crates/recto-agent/src/code.rs.
   3. Its state's home: a component of Session, through crates/recto-session/src/parts.rs (listed in its tests/parts.rs).
   4. Its executor: run() calls the feature's own function; person and agent differ only through Actor.
   5. The CLI comes from its row: read the help golden's diff.
   6. The window: a UiAction arm in crates/recto-app/src/core/controls.rs and a pure translation in crates/recto-app/src/core/$name.rs; a menu row in core/menu_rows.rs' ROWS.
   7. What it publishes: a view model in crates/recto-app/src/model.rs or its own module.
   8. The bridge forwards (crates/recto-ffi), Swift draws (app/Sources/RectoPDF).
   9. Its QA: qa state, and a verb only for what only the host has (crates/recto-agent/src/commands/qa/rows_features.rs).
  10. Its tests by tier, replacing the four fill_me_in tests.
  11. Its shots: a step in app/qa/contact-sheet.sh.
  12. Its notes: docs/engineering/$name.md and a line in docs/engineering/README.md.
The brief and the hand-back: docs/agent-instructions/feature-brief.md. To take it all out: tools/new-feature.sh --remove $name
EOF
