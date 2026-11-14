#!/usr/bin/env bash
# new-feature: a feature's first files, each with a test that fails saying what to fill in, and the checklist of docs/FEATURES.md (R4.2).
#
#   tools/new-feature.sh <name>            e.g. page_labels: a command `page_labels`, `recto page labels`
#   tools/new-feature.sh --remove <name>   takes out what <name> made, and nothing else
#
# Makes two files from the templates in tools/new-feature/ and adds one marked line to each of two shared files, so the tree compiles and
# `tools/check.sh --quick` fails only on the `fill_me_in_<name>_…` tests:
#   crates/recto-agent/src/commands/<name>.rs        the command: payload, Cmd (no defaults), its row, its words, tests to fill in
#   crates/recto-app/src/core/headless/<name>.rs      the headless app's tests, the window tier's and the shots', to fill in
#   + `pub mod <name>;` in crates/recto-agent/src/commands/mod.rs, `mod <name>;` in crates/recto-app/src/core/headless/mod.rs
# The command isn't registered in a family yet: that line, the goldens and `every_command()` are the first step of the checklist, because they're
# where the guards start to hold the command. The templates are compiled by recto-agent's and recto-app's own tests (`scaffold_template`), so a change
# that would break what this makes fails there. Small on purpose: generated text is debt; docs/FEATURES.md and Find are the example.
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
made="Made by \`tools/new-feature.sh $name\`"

if [ "$remove" = 1 ]; then
  # Only what this made: a file that says so in its first lines, and a line that ends with this name's own mark.
  for file in "$command_file" "$tests_file"; do
    if [ -e "$file" ] && head -3 "$file" | grep -qF -- "$made"; then
      rm -f "$file"
    elif [ -e "$file" ]; then
      echo "new-feature: $file wasn't made by this, so it stays." >&2
    fi
  done
  for file in "$commands_mod" "$headless_mod"; do
    awk -v mark="$mark" '{ if (length($0) >= length(mark) && substr($0, length($0) - length(mark) + 1) == mark) next; print }' "$file" > "$file.new"
    mv "$file.new" "$file"
  done
  echo "new-feature: $name removed."
  exit 0
fi
# A name that can't be a module, or is one already.
case " as async await break const continue crate dyn else enum extern false fn for gen if impl in let loop match mod move mut pub ref return self static struct super trait true type unsafe use where while abstract become box do final macro override priv try typeof unsized virtual yield scaffold_template " in
  *" $name "*) echo "new-feature: $name is a word Rust keeps, or the template's own name." >&2; exit 2 ;;
esac
for file in "$commands_mod" "$headless_mod"; do
  if grep -qE "^(pub(\(crate\))? )?mod $name;" "$file"; then
    echo "new-feature: $file already has a module $name." >&2
    exit 1
  fi
done
for path in "$command_file" "$tests_file" "${command_file%.rs}" "${tests_file%.rs}"; do
  [ ! -e "$path" ] || { echo "new-feature: $path is already there." >&2; exit 1; }
done
camel=$(printf '%s' "$name" | awk -F_ '{ for (i = 1; i <= NF; i++) printf "%s%s", toupper(substr($i, 1, 1)), substr($i, 2) }')
upper=$(printf '%s' "$name" | tr '[:lower:]' '[:upper:]')
spaced=$(printf '%s' "$name" | tr '_' ' ')
# The template with this name in it, less what's only the template's.
from_template() { # TEMPLATE OUT
  sed -e '/template-only: begin/,/template-only: end/d' -e '/^\/\/! (This file is the scaffold/,/copied\.)$/d' \
    -e "s/NewFeature/$camel/g" -e "s/NEW_FEATURE/$upper/g" -e "s/new_feature/$name/g" -e "s/new feature/$spaced/g" "$1" > "$2"
}
from_template tools/new-feature/command.rs "$command_file"
from_template tools/new-feature/headless.rs "$tests_file"

# The one line each shared file takes, in its place in the sorted list of `mod` lines (before an attribute it would go under), marked so --remove
# takes exactly it. In the C locale: a Norwegian one sorts `aa` last.
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
\`tools/check.sh --quick\` now fails on the five fill_me_in_${name}_ tests and nothing else. The path, each step in docs/FEATURES.md:
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
