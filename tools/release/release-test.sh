#!/usr/bin/env bash
# release-test: proves the release scripts' refusals and their output in throwaway folders, with made files and no build, network,
# signing identity or keychain. Seconds. Run after any change to tools/release/ or app/release-zip.sh (tools/check.sh --full does, as
# tools/check-scope.sh maps them to "release").
#
#   tools/release/release-test.sh
#
# What it doesn't do: check-draft.sh's codesign, Gatekeeper and stapler checks of a real signed bundle, and 0.1.0's own verdict, which need
# the downloaded 0.1.0 zip and a build. `tools/release/check-draft.sh --self-test <RectoPDF-0.1.0.zip>` runs those (docs/RELEASING.md).
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/../.." && pwd)"
X=$(mktemp -d "${TMPDIR:-/tmp}/release-test.XXXXXX")
trap 'rm -rf "$X"' EXIT
unset NOTES_ROOT BUMP_ROOT
fails=0
check() { # name, expected, actual
  if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi
}
has() { # name, text wanted, in what
  case "$3" in *"$2"*) echo "ok    $1" ;; *) echo "FAIL  $1: no [$2] in [$3]"; fails=$((fails + 1)) ;; esac
}
hasnt() { # name, text not wanted, in what
  case "$3" in *"$2"*) echo "FAIL  $1: [$2] in [$3]"; fails=$((fails + 1)) ;; *) echo "ok    $1" ;; esac
}
NOTES="$SOURCE/tools/release/notes.sh"
BUMP="$SOURCE/tools/release/bump.sh"
SUM=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

# ---- notes.sh: the draft's body from docs/releases/X.Y.Z.md ----
N="$X/notes"; mkdir -p "$N/docs/releases"
notes() { NOTES_ROOT="$N" "$NOTES" "$@" 2>&1; }
good_notes() {
  cat > "$N/docs/releases/1.2.3.md" <<EOF
RectoPDF 1.2.3 fixes things found in 1.2.2.

## Install

macOS 15 or later, Apple silicon. Download [RectoPDF-1.2.3.zip](https://github.com/kkollsga/RectoPDF/releases/download/v1.2.3/RectoPDF-1.2.3.zip), open it.

## Fixes

- A fix, which 1.2.2 got wrong.

## Checksums

\`\`\`
(written by the release workflow)
\`\`\`
EOF
}
printf '%s  RectoPDF-1.2.3.zip\n' "$SUM" > "$X/SHA256SUMS"

# Failure: a release goes out with no notes at all, or the draft step's body is made some other way and the tag is withdrawn an hour in.
out=$(notes --check 1.2.3); rc=$?
check "notes: no file for the version is refused" 1 "$rc"
has "notes: and it says which file to write" "docs/releases/1.2.3.md" "$out"

# Failure: the body is the notes with the workflow's stand-in where the checksums go, or the Install part twice.
good_notes
body=$(NOTES_ROOT="$N" "$NOTES" 1.2.3 "$X/SHA256SUMS" 2>/dev/null); rc=$?
check "notes: good notes make a body" 0 "$rc"
has "notes: the checksums are filled in" "$SUM  RectoPDF-1.2.3.zip" "$body"
hasnt "notes: the stand-in is gone" "written by the release workflow" "$body"
check "notes: one Install part" 1 "$(printf '%s\n' "$body" | grep -c '^## Install')"
has "notes: the rest is the file's" "A fix, which 1.2.2 got wrong." "$body"
out=$(notes --check 1.2.3); rc=$?
check "notes: --check passes good notes" 0 "$rc"

# Failure: the notes of another version are published as this one's (a copied file, a link to the old zip that every reader would download).
sed 's/^RectoPDF 1.2.3 fixes/RectoPDF 1.2.2 fixes/' "$N/docs/releases/1.2.3.md" > "$X/t" && mv "$X/t" "$N/docs/releases/1.2.3.md"
out=$(notes --check 1.2.3); rc=$?
check "notes: a first line naming another version is refused" 1 "$rc"
has "notes: and says so" "first line doesn't say it's RectoPDF 1.2.3" "$out"
good_notes
sed 's#download/v1.2.3/RectoPDF-1.2.3.zip#download/v1.2.2/RectoPDF-1.2.2.zip#' "$N/docs/releases/1.2.3.md" > "$X/t" && mv "$X/t" "$N/docs/releases/1.2.3.md"
out=$(notes --check 1.2.3); rc=$?
check "notes: a link to another version's zip is refused" 1 "$rc"
has "notes: and names it" "1.2.2" "$out"

# Failure: a paragraph marked to be put right is published as it stands. The marker is an HTML comment, which the page wouldn't show.
good_notes
printf '\n<!-- PLACEHOLDER: the next line is to be confirmed -->\n- A limit.\n' >> "$N/docs/releases/1.2.3.md"
out=$(notes --check 1.2.3); rc=$?
check "notes: the placeholder marker is refused" 1 "$rc"
has "notes: and its line is named" "placeholder, line" "$out"
good_notes
printf -- '- [Placeholder, to be confirmed] A limit.\n' >> "$N/docs/releases/1.2.3.md"
out=$(notes --check 1.2.3); rc=$?
check "notes: a bracketed placeholder is refused too" 1 "$rc"

# Failure: the body is over GitHub's limit, so the draft step fails after the signing and the tag is withdrawn.
good_notes
python3 -I -c 'import sys; open(sys.argv[1], "a").write("- x" * 34000 + "\n")' "$N/docs/releases/1.2.3.md"
out=$(notes --check 1.2.3); rc=$?
check "notes: a body over the bound is refused" 1 "$rc"
has "notes: and says the bound" "over the 100,000" "$out"

# Failure: the checksums block names another file, or two, and the body tells people to check the wrong thing.
good_notes
printf '%s  RectoPDF-1.2.2.zip\n' "$SUM" > "$X/SUMS-other"
out=$(NOTES_ROOT="$N" "$NOTES" 1.2.3 "$X/SUMS-other" 2>&1 >/dev/null); rc=$?
check "notes: SHA256SUMS of another zip is refused" 1 "$rc"
cat "$X/SHA256SUMS" "$X/SHA256SUMS" > "$X/SUMS-two"
out=$(NOTES_ROOT="$N" "$NOTES" 1.2.3 "$X/SUMS-two" 2>&1 >/dev/null); rc=$?
check "notes: SHA256SUMS of two lines is refused" 1 "$rc"

# Failure: notes written without an Install or a Checksums part publish a body with no way to download it or check it.
printf 'RectoPDF 1.2.3 is a release.\n\n## Fixes\n\n- One.\n' > "$N/docs/releases/1.2.3.md"
body=$(NOTES_ROOT="$N" "$NOTES" 1.2.3 "$X/SHA256SUMS" 2>/dev/null); rc=$?
check "notes: notes with neither part still make a body" 0 "$rc"
has "notes: with 0.1.0's Install part added" "Download [RectoPDF-1.2.3.zip](https://github.com/kkollsga/RectoPDF/releases/download/v1.2.3/RectoPDF-1.2.3.zip)" "$body"
has "notes: and its Checksums part" '```'$'\n'"$SUM  RectoPDF-1.2.3.zip" "$body"
check "notes: the opening paragraph stays first" "RectoPDF 1.2.3 is a release." "$(printf '%s\n' "$body" | head -1)"

# Failure: the real notes in this repository drift from what the script takes (a wrong link, the wrong version), found only at the tag.
# Each docs/releases/X.Y.Z.md here is taken as it is, but for its placeholder lines, which are the one thing allowed until the release.
for real in "$SOURCE"/docs/releases/[0-9]*.md; do
  v=$(basename "$real" .md); case "$v" in *[!0-9.]*) continue ;; esac
  mkdir -p "$X/real-$v/docs/releases"
  grep -v '<!-- PLACEHOLDER:' "$real" > "$X/real-$v/docs/releases/$v.md"
  out=$(NOTES_ROOT="$X/real-$v" "$NOTES" --check "$v" 2>&1); rc=$?
  check "notes: docs/releases/$v.md passes but for its marked placeholders ($out)" 0 "$rc"
done

# ---- bump.sh: every file that names the version ----
B="$X/bump"
tree() { # a tree as the repository has it, at version $1, with every file the bump knows
  rm -rf "$B"; mkdir -p "$B/crates/recto-core" "$B/crates/recto-headings" "$B/fuzz" "$B/spikes/sandbox"
  printf '[workspace]\nmembers = ["crates/*"]\n\n[workspace.package]\nversion = "%s"\nedition = "2024"\n\n[workspace.dependencies]\nserde = { version = "1.0.0" }\n' "$1" > "$B/Cargo.toml"
  printf '[package]\nname = "recto-core"\nversion.workspace = true\n' > "$B/crates/recto-core/Cargo.toml"
  printf '[package]\nname = "recto-headings"\nversion = "%s"\n' "$1" > "$B/crates/recto-headings/Cargo.toml"
  lock() { printf '# @generated\nversion = 4\n\n[[package]]\nname = "byteorder-lite"\nversion = "%s"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n\n[[package]]\nname = "recto-core"\nversion = "%s"\ndependencies = [\n "byteorder-lite",\n]\n\n[[package]]\nname = "%s"\nversion = "0.0.0"\n' "$1" "$1" "$2"; }
  lock "$1" recto-fuzz-targets > "$B/Cargo.lock"
  { lock "$1" recto-fuzz-targets; printf '\n[[package]]\nname = "recto-headings"\nversion = "%s"\n' "$1"; } > "$B/crates/recto-headings/Cargo.lock"
  lock "$1" recto-fuzz-targets > "$B/fuzz/Cargo.lock"
  lock "$1" recto-sandbox-spike > "$B/spikes/sandbox/Cargo.lock"
  printf 'RectoPDF %s: third-party software notices\n\nships byteorder-lite %s\n' "$1" "$1" > "$B/THIRD-PARTY-NOTICES.txt"
  printf '# Privacy\n\nThis describes RectoPDF %s.\n' "$1" > "$B/PRIVACY.md"
  printf '1. Download `RectoPDF-%s.zip` from the latest release.\n- In %s, deleting pages is done with recto.\n' "$1" "$1" > "$B/README.md"
}
bump() { BUMP_ROOT="$B" "$BUMP" "$@" 2>&1; }

# Failure: --check calls a tree consistent that isn't, or not one that is.
tree 0.1.0
out=$(bump --check); rc=$?
check "bump: --check passes a tree at one version" 0 "$rc"
has "bump: and names it" "names 0.1.0" "$out"

# Failure: a bump misses a file (the headings lockfile, built --locked, fails the release build), or changes a third-party package of
# the same version, or the fuzz or spike workspace's own 0.0.0.
out=$(bump 0.1.1 --no-cargo); rc=$?
check "bump: 0.1.0 to 0.1.1 succeeds" 0 "$rc"
out=$(bump --check 0.1.1); rc=$?
check "bump: and the tree is then at 0.1.1 ($out)" 0 "$rc"
check "bump: the headings lockfile's own packages follow" 2 "$(grep -c '^version = "0.1.1"' "$B/crates/recto-headings/Cargo.lock")"
check "bump: a third-party package of the same version stays" 'version = "0.1.0"' "$(grep -A1 'name = "byteorder-lite"' "$B/Cargo.lock" | tail -1)"
check "bump: the fuzz workspace's own package stays" 'version = "0.0.0"' "$(grep -A1 'name = "recto-fuzz-targets"' "$B/fuzz/Cargo.lock" | tail -1)"
check "bump: the spike's own package stays" 'version = "0.0.0"' "$(grep -A1 'name = "recto-sandbox-spike"' "$B/spikes/sandbox/Cargo.lock" | tail -1)"
check "bump: a dependency's version in Cargo.toml stays" 'serde = { version = "1.0.0" }' "$(grep '^serde' "$B/Cargo.toml")"
check "bump: the notices' third-party line stays" "ships byteorder-lite 0.1.0" "$(sed -n 3p "$B/THIRD-PARTY-NOTICES.txt")"
check "bump: README's prose about a version stays" "- In 0.1.0, deleting pages is done with recto." "$(sed -n 2p "$B/README.md")"
check "bump: README's zip follows" '1. Download `RectoPDF-0.1.1.zip` from the latest release.' "$(sed -n 1p "$B/README.md")"

# Failure: a tree bumped by hand in part is called consistent, or a bump goes ahead over it and hides which file was behind.
tree 0.1.0
sed 's/^version = "0.1.0"$/version = "0.1.1"/' "$B/Cargo.toml" > "$X/t" && mv "$X/t" "$B/Cargo.toml"
out=$(bump --check); rc=$?
check "bump: --check refuses a tree bumped in part" 1 "$rc"
has "bump: and names each file behind" "crates/recto-headings/Cargo.lock: names 0.1.0 where 0.1.1 is wanted" "$out"
before=$(cat "$B/PRIVACY.md")
out=$(bump 0.1.2 --no-cargo); rc=$?
check "bump: a bump over a tree in part is refused" 1 "$rc"
check "bump: and changes nothing" "$before" "$(cat "$B/PRIVACY.md")"

# Failure: a version is bumped backwards or to itself (a version once published is spent), or to something the updater won't read.
tree 0.1.1
out=$(bump 0.1.1 --no-cargo); rc=$?
check "bump: to the same version is refused" 1 "$rc"
out=$(bump 0.1.0 --no-cargo); rc=$?
check "bump: to an older version is refused" 1 "$rc"
has "bump: and says why" "spent" "$out"
for bad in v0.1.2 0.1.2.1 01.1.2 0.1; do
  out=$(bump "$bad" --no-cargo); rc=$?
  check "bump: $bad isn't a release version" 2 "$rc"
done
check "bump: 0.1.10 is newer than 0.1.9 (by number, not text)" 0 "$(tree 0.1.9; bump 0.1.10 --no-cargo >/dev/null; echo $?)"

# Failure: a file the bump knows is gone or renamed, and --check passes with it unchecked.
tree 0.1.0
rm "$B/PRIVACY.md"
out=$(bump --check); rc=$?
check "bump: a missing file fails --check" 1 "$rc"
has "bump: and is named" "PRIVACY.md: missing" "$out"
tree 0.1.0
printf '# Privacy\n' > "$B/PRIVACY.md"
out=$(bump --check); rc=$?
check "bump: a file that no longer names the version fails --check" 1 "$rc"

# Failure: this repository's own tree isn't at one version, or the bump's table no longer fits its files.
out=$("$BUMP" --check 2>&1); rc=$?
check "bump: this repository is at one version ($out)" 0 "$rc"

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
