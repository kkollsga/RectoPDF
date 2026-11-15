#!/usr/bin/env bash
# release/bump: moves every file that names the release's version to a new one, consistently, and commits nothing.
#
#   tools/release/bump.sh X.Y.Z [--no-cargo]    Rewrites the files below to X.Y.Z, then checks the tree is at X.Y.Z (and, unless --no-cargo,
#                                               that cargo takes every lockfile as it is, with --locked --offline).
#   tools/release/bump.sh --check [X.Y.Z]       Says whether the tree is at one version (at X.Y.Z, if given) and which files disagree. Exit 1 if not.
#   BUMP_ROOT=<dir>                             The tree to work on (default: this repository). The tests use a throwaway one.
#
# The files, each with what in it names the version (docs/RELEASING.md, "The version"):
#   Cargo.toml                         [workspace.package] version: the one source. The release gate compares the tag with it, and app/build.sh
#                                      writes it into the app's and the renderer helper's Info.plist (CFBundleShortVersionString), which the updater compares.
#   crates/recto-headings/Cargo.toml   [package] version: its own workspace, so it doesn't inherit; nothing compares it, it's kept with the rest.
#   Cargo.lock, crates/recto-headings/Cargo.lock, fuzz/Cargo.lock, spikes/sandbox/Cargo.lock
#                                      each package of this repository's crates/ (no `source` line, named by a crates/*/Cargo.toml). Third-party
#                                      packages whose own version happens to be the same (byteorder-lite 0.1.0) are left alone, and so are the fuzz
#                                      and spike workspaces' own packages (0.0.0, versions of their own). The headings lockfile is built --locked by
#                                      app/build.sh and read --locked by tools/notices.py, so a stale one fails the release build.
#   THIRD-PARTY-NOTICES.txt            its first line, "RectoPDF X.Y.Z: third-party software notices", which ships as the app's Licenses.txt
#                                      (tools/notices.py writes it; release-zip.sh compares the copy in the app with it byte for byte).
#   PRIVACY.md                         "This describes RectoPDF X.Y.Z."
#   README.md                          the zip's name in Install, "RectoPDF-X.Y.Z.zip".
# Not rewritten, by design: prose about a version ("In 0.1.0, deleting ... pages"), the release notes, and the tests' "0.1.0", which are
# fixtures for an older version (update_bed.rs OLD, restore/update.rs OLD, the update goldens); a test that needs the crate's version reads
# CARGO_PKG_VERSION.
set -euo pipefail
ROOT="${BUMP_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
usage() { sed -n '2,7p' "$0"; exit 2; }
mode=bump want="" cargo=1
for a in "$@"; do
  case "$a" in
    --check) mode=check ;;
    --no-cargo) cargo=0 ;;
    -h | --help) usage ;;
    -*) echo "bump: unknown option $a" >&2; usage ;;
    *) [ -z "$want" ] || usage; want="$a" ;;
  esac
done
[ "$mode" = check ] || [ -n "$want" ] || usage

# The Python below does both: reads every file's version(s), and with a target, rewrites them. One table, so --check can't miss a file the bump writes.
python3 -I - "$ROOT" "$mode" "$want" <<'PY'
import os, re, sys

root, mode, want = sys.argv[1], sys.argv[2], sys.argv[3]
PLAIN = re.compile(r"^(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$")

def die(message, code=1):
    print(f"bump: {message}", file=sys.stderr)
    sys.exit(code)

def read(rel):
    try:
        with open(os.path.join(root, rel), encoding="utf-8") as f:
            return f.read()
    except FileNotFoundError:
        return None

def ours():
    """The package names of this repository's crates: what a lockfile's path packages may be."""
    names = set()
    crates = os.path.join(root, "crates")
    for entry in sorted(os.listdir(crates)) if os.path.isdir(crates) else []:
        text = read(os.path.join("crates", entry, "Cargo.toml"))
        if text is None:
            continue
        found = re.search(r'^\[package\][^\[]*?^name\s*=\s*"([^"]+)"', text, re.M | re.S)
        if found:
            names.add(found.group(1))
    return names

# Each rule: (file, what names the version, how it reads and writes). A section rule finds `version = "..."` inside one [section] only.
def section(name):
    def find(text):
        m = re.search(r'^\[' + re.escape(name) + r'\][^\[]*?^version\s*=\s*"([^"]*)"', text, re.M | re.S)
        return [(m.start(1), m.end(1), m.group(1))] if m else []
    return find

def line(pattern):
    rx = re.compile(pattern, re.M)
    def find(text):
        return [(m.start(1), m.end(1), m.group(1)) for m in rx.finditer(text)]
    return find

def lockfile(names):
    def find(text):
        spots = []
        for block in re.finditer(r'^\[\[package\]\]\n(.*?)(?=^\[\[package\]\]|\Z)', text, re.M | re.S):
            body = block.group(1)
            name = re.search(r'^name = "([^"]+)"$', body, re.M)
            version = re.search(r'^version = "([^"]+)"$', body, re.M)
            if not name or not version or re.search(r'^source = ', body, re.M) or name.group(1) not in names:
                continue
            start = block.start(1) + version.start(1)
            spots.append((start, start + len(version.group(1)), version.group(1)))
        return spots
    return find

names = ours()
RULES = [
    ("Cargo.toml", "[workspace.package] version", section("workspace.package")),
    ("crates/recto-headings/Cargo.toml", "[package] version", section("package")),
    ("Cargo.lock", "this repository's packages", lockfile(names)),
    ("crates/recto-headings/Cargo.lock", "this repository's packages", lockfile(names)),
    ("fuzz/Cargo.lock", "this repository's packages", lockfile(names)),
    ("spikes/sandbox/Cargo.lock", "this repository's packages", lockfile(names)),
    ("THIRD-PARTY-NOTICES.txt", "its first line", line(r'\ARectoPDF ([0-9][0-9.]*): third-party software notices$')),
    ("PRIVACY.md", '"This describes RectoPDF X.Y.Z."', line(r'^This describes RectoPDF ([0-9][0-9.]*[0-9])\.$')),
    ("README.md", "the zip's name, RectoPDF-X.Y.Z.zip", line(r'RectoPDF-([0-9][0-9.]*[0-9])\.zip')),
]

def survey():
    """{file: (what, [versions found])}; a file missing or naming nothing is a disagreement of its own."""
    seen = {}
    for rel, what, find in RULES:
        text = read(rel)
        seen[rel] = (what, None if text is None else [v for _, _, v in find(text)])
    return seen

def report(seen, target):
    versions = {}
    problems = []
    for rel, (what, found) in seen.items():
        if found is None:
            problems.append(f"{rel}: missing")
        elif not found:
            problems.append(f"{rel}: names no version where it should ({what})")
        else:
            for v in found:
                versions.setdefault(v, set()).add(rel)
    main = read("Cargo.toml")
    source = (seen.get("Cargo.toml") or (None, None))[1]
    source = source[0] if source else None
    goal = target or source
    for v, files in sorted(versions.items()):
        if v != goal:
            for rel in sorted(files):
                problems.append(f"{rel}: names {v} where {goal} is wanted ({seen[rel][0]})")
    return goal, problems

if mode == "check":
    if want and not PLAIN.match(want):
        die(f"{want} isn't a release version: X.Y.Z, plain numbers, no v and no fourth part", 2)
    goal, problems = report(survey(), want or None)
    if problems:
        print(f"bump: the tree is not consistently at {goal}:")
        for p in problems:
            print(f"  {p}")
        sys.exit(1)
    print(f"bump: every file that names the version names {goal} ({len(RULES)} files).")
    sys.exit(0)

# The bump.
if not PLAIN.match(want):
    die(f"{want} isn't a release version: X.Y.Z, plain numbers, no v and no fourth part", 2)
seen = survey()
current, problems = report(seen, None)
if problems:
    print("bump: the tree isn't at one version to start from, so nothing was changed:", file=sys.stderr)
    for p in problems:
        print(f"  {p}", file=sys.stderr)
    sys.exit(1)
key = lambda v: tuple(int(p) for p in v.split("."))
if key(want) == key(current):
    die(f"the tree is already at {want}")
if key(want) < key(current):
    die(f"{want} is older than {current}: a version, once published, is spent, so a bump only goes forward")
changed = []
for rel, what, find in RULES:
    text = read(rel)
    spots = find(text)
    for start, end, _ in sorted(spots, reverse=True):
        text = text[:start] + want + text[end:]
    with open(os.path.join(root, rel), "w", encoding="utf-8") as f:
        f.write(text)
    changed.append(f"{rel} ({len(spots)})")
print(f"bump: {current} -> {want} in {', '.join(changed)}")
_, after = report(survey(), want)
if after:
    die("after the bump the tree still disagrees: " + "; ".join(after))
PY
[ "$mode" = bump ] || exit 0
if [ "$cargo" = 1 ]; then
  # That cargo takes each lockfile as it now is: what app/build.sh's --locked build and tools/notices.py's --locked read will meet. Resolves, builds nothing.
  for manifest in Cargo.toml crates/recto-headings/Cargo.toml; do
    cargo metadata --format-version 1 --locked --offline --manifest-path "$ROOT/$manifest" >/dev/null \
      || { echo "bump: cargo doesn't take $manifest's lockfile as it is (--locked --offline)" >&2; exit 1; }
  done
  echo "bump: cargo takes the main and the headings lockfile as they are (--locked --offline)."
fi
echo "bump: nothing is committed. Look at \`git diff --stat\`, then commit it."
