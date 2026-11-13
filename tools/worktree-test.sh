#!/usr/bin/env bash
# worktree-test: tools/worktree.sh's seeding, on a throwaway repository. Run it after any change to tools/worktree.sh.
#
# Failure it catches: `worktree.sh seed` on a worktree checked out before the main checkout's last build leaves the worktree's sources
# older than the cloned build, so cargo takes the main checkout's crates as this worktree's and the build links the wrong code. And
# `worktree.sh remove` deleting a worktree whose app copies stay registered with Launch Services, each a RectoPDF in Open With. And a seed
# that stops at a file it can't clone (one a build is writing, say) before PDFium, so every test that draws a page fails in the new worktree
# with render_library_missing; or one that fails when the main checkout has no PDFium to give.
set -euo pipefail
tool="$(cd "$(dirname "$0")" && pwd)/worktree.sh"
work=$(cd "$(mktemp -d "${TMPDIR:-/tmp}/worktree-test.XXXXXX")" && pwd)
trap 'rm -rf "$work"' EXIT
cd "$work"
git init -q -b main repo
cd repo
git config user.email test@example.invalid
git config user.name test
mkdir -p crates/a
echo 'fn main() {}' > crates/a/lib.rs
git add . && git commit -q -m one
git worktree add -q --no-track -b behind "$work/behind" main
# The main checkout builds after that worktree was checked out: its build files are newer than the worktree's sources.
mkdir -p target/debug/deps
sleep 1
echo built > target/debug/built
echo dep > target/debug/deps/a.rlib
"$tool" seed "$work/behind" >/dev/null 2>&1
if [ ! -f "$work/behind/target/debug/built" ]; then
  echo "worktree-test: skipped, the seed didn't clone here (not one APFS volume)"
  exit 0
fi
seeded=$(stat -f %m "$work/behind/target/debug/built")
source=$(stat -f %m "$work/behind/crates/a/lib.rs")
if [ "$source" -lt "$seeded" ]; then
  echo "worktree-test: FAIL: after seed, the worktree's sources ($source) are older than the seeded build ($seeded), so cargo would reuse the main checkout's crates"
  exit 1
fi
# PDFium is seeded even when a file in a profile can't be cloned, and the seed goes on.
git worktree add -q --no-track -b drawing "$work/drawing" main
mkdir -p target/pdfium
echo library > target/pdfium/libpdfium.dylib
echo half-written > target/debug/unreadable
chmod 000 target/debug/unreadable
if ! "$tool" seed "$work/drawing" >"$work/said" 2>&1; then
  echo "worktree-test: FAIL: a file that can't be cloned failed the seed: $(cat "$work/said")"
  exit 1
fi
chmod 600 target/debug/unreadable
if [ ! -f "$work/drawing/target/pdfium/libpdfium.dylib" ]; then
  echo "worktree-test: FAIL: the seed left PDFium out when a file in a profile couldn't be cloned, so the worker can't draw there: $(cat "$work/said")"
  exit 1
fi
if [ ! -f "$work/drawing/target/debug/built" ]; then
  echo "worktree-test: FAIL: the seed stopped at a file it couldn't clone, and left the rest of the profile out"
  exit 1
fi
# With no PDFium in the main checkout, the seed says so plainly, and doesn't fail.
rm -rf target/pdfium
git worktree add -q --no-track -b bare "$work/bare" main
if ! "$tool" seed "$work/bare" >"$work/said" 2>&1 || ! grep -q "tools/pdfium.sh" "$work/said"; then
  echo "worktree-test: FAIL: with no PDFium to seed, the seed failed or didn't say how to get it: $(cat "$work/said")"
  exit 1
fi
# Removing a worktree forgets every app bundle in it, nested ones included, before the folder goes.
mkdir -p "$work/behind/app/build.noindex/RectoPDF.app/Contents/Helpers/RectoRender.app"
cat > "$work/lsregister" <<STUB
#!/usr/bin/env bash
echo "\$*" >> "$work/forgot"
STUB
chmod +x "$work/lsregister"
WORKTREE_LSREGISTER="$work/lsregister" "$tool" remove "$work/behind" >/dev/null 2>&1
for bundle in app/build.noindex/RectoPDF.app app/build.noindex/RectoPDF.app/Contents/Helpers/RectoRender.app; do
  if ! grep -qxF -- "-u $work/behind/$bundle" "$work/forgot" 2>/dev/null; then
    echo "worktree-test: FAIL: remove didn't forget $bundle before deleting the worktree"
    exit 1
  fi
done
echo "worktree-test: pass"
