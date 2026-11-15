#!/usr/bin/env bash
# release/notes: the release's notes, as the draft's body, from docs/releases/X.Y.Z.md. Never from the commit log: 0.1.1 would have been
# 1,660 subjects (about 241 KB, over GitHub's limit), some describing the security faults in more detail than the notes do.
#
#   tools/release/notes.sh X.Y.Z <SHA256SUMS>   Prints the body: the file, with an Install part if it has none, and its Checksums block
#                                               filled from SHA256SUMS (the part added if it has none).
#   tools/release/notes.sh --check X.Y.Z        The same refusals, before anything is built (the release gate): no checksums needed.
#   NOTES_ROOT=<dir>                            The tree to read docs/releases/ from (default: this repository). The tests use a throwaway one.
#
# Refused, with the reason, and nothing printed:
#   - there's no docs/releases/X.Y.Z.md;
#   - it names another version as this release: its first line doesn't say "RectoPDF X.Y.Z", or a zip or download link names another
#     version (RectoPDF-A.B.C.zip, /releases/download/vA.B.C/);
#   - it still holds a placeholder: the marker is a line `<!-- PLACEHOLDER: what is to be put right -->`, and any other "placeholder" counts
#     too, as an HTML comment doesn't show on the page and a bracketed one does;
#   - SHA256SUMS doesn't hold exactly one line, for RectoPDF-X.Y.Z.zip, with a 64-digit checksum;
#   - the body is over 100,000 characters. GitHub refuses a release body over 125,000 ("body is too long (maximum is 125000 characters)",
#     its API's validation message, as reported in github.com/intuit/auto/issues/2418 and handled by softprops/action-gh-release 2.5.2 and
#     release-it). GitHub's REST reference for creating or updating a release (docs.github.com/en/rest/releases/releases) states no limit, so
#     the number isn't in GitHub's own documentation; checked 2026-11-15. 100,000 leaves a fifth of it spare.
set -euo pipefail
ROOT="${NOTES_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
usage() { sed -n '2,8p' "$0"; exit 2; }
case "${1:-}" in
  --check) [ $# = 2 ] || usage; version="$2" sums="" ;;
  -h | --help | "") usage ;;
  *) [ $# = 2 ] || usage; version="$1" sums="$2" ;;
esac
python3 -I - "$ROOT" "$version" "$sums" <<'PY'
import os, re, sys

root, version, sums = sys.argv[1], sys.argv[2], sys.argv[3]
LIMIT = 100_000
REPO = "https://github.com/kkollsga/RectoPDF"

def refuse(why):
    print(f"notes: {why}", file=sys.stderr)
    sys.exit(1)

if not re.fullmatch(r"(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})", version):
    refuse(f"{version!r} isn't a release version (X.Y.Z)")
rel = f"docs/releases/{version}.md"
try:
    with open(os.path.join(root, rel), encoding="utf-8") as f:
        text = f.read()
except FileNotFoundError:
    refuse(f"there are no notes for {version}: write {rel} (docs/RELEASING.md)")
zip_name = f"RectoPDF-{version}.zip"

first = next((line for line in text.splitlines() if line.strip()), "")
if not re.search(r"\bRectoPDF " + re.escape(version) + r"\b", first):
    refuse(f"{rel}'s first line doesn't say it's RectoPDF {version}: {first[:120]!r}")
others = sorted({v for v in re.findall(r"RectoPDF-([0-9]+\.[0-9]+\.[0-9]+)\.zip", text) + re.findall(r"/releases/download/v([0-9]+\.[0-9]+\.[0-9]+)/", text) if v != version})
if others:
    refuse(f"{rel} links to another version's zip or download ({', '.join(others)}), not {version}'s")
for number, line in enumerate(text.splitlines(), 1):
    if "placeholder" in line.lower():
        refuse(f"{rel} still holds a placeholder, line {number}: {line.strip()[:160]}")

parts = re.split(r"(?m)^(?=## )", text)
def part(title):
    return next((i for i, p in enumerate(parts) if re.match(r"## " + re.escape(title) + r"\s*$", p.splitlines()[0] if p else "")), None)

install = part("Install")
if install is None:
    # The Install part 0.1.0's body had, before everything else but the opening paragraph.
    block = (f"## Install\n\nmacOS 15 or later, Apple silicon. Download [{zip_name}]({REPO}/releases/download/v{version}/{zip_name}), "
             "open it and drag RectoPDF to Applications. It's signed with a Developer ID and notarized by Apple.\n\n"
             "RectoPDF updates itself from here on (RectoPDF menu, Settings).\n\n")
    parts.insert(1 if len(parts) > 1 or not parts[0].startswith("## ") else 0, block)
elif f"/releases/download/v{version}/{zip_name}" not in parts[install]:
    refuse(f"{rel}'s Install part has no link to {REPO}/releases/download/v{version}/{zip_name}")

if sums:
    try:
        with open(sums, encoding="utf-8") as f:
            lines = [line for line in f.read().splitlines() if line.strip()]
    except FileNotFoundError:
        refuse(f"no SHA256SUMS at {sums}")
    if len(lines) != 1 or not re.fullmatch(r"[0-9a-f]{64}  " + re.escape(zip_name), lines[0]):
        refuse(f"SHA256SUMS should be one line, a checksum and {zip_name}: {lines!r}")
    fenced = f"## Checksums\n\n```\n{lines[0]}\n```\n"
    checksums = part("Checksums")
    if checksums is None:
        parts.append(("\n" if not parts[-1].endswith("\n\n") else "") + fenced)
    else:
        if not re.search(r"(?ms)^```\n.*?^```\s*$", parts[checksums]):
            refuse(f"{rel}'s Checksums part has no ``` block to fill")
        parts[checksums] = re.sub(r"(?ms)^```\n.*?^```[ \t]*\n?", lambda _: f"```\n{lines[0]}\n```\n", parts[checksums], count=1)

body = "".join(parts)
if len(body) > LIMIT:
    refuse(f"the body is {len(body):,} characters, over the {LIMIT:,} this allows (GitHub refuses over 125,000)")
if sums:
    sys.stdout.write(body)
else:
    print(f"notes: {rel} is ready to be the body of v{version} ({len(body):,} characters before the checksums).", file=sys.stderr)
PY
