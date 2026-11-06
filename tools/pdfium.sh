#!/usr/bin/env bash
# pdfium: puts PDFium in target/pdfium/, the one place the worker and the tests look for it. Run it any time: when the file is already
# there and its digest matches, it does nothing.
#
#   tools/pdfium.sh            Check, and fetch if needed. Prints the library's path.
#   tools/pdfium.sh --attest   Also verify the tarball's build attestation with gh (needs gh and a network).
#
# Source:  bblanchon/pdfium-binaries, release chromium/8125 (PDFium 158.0.8125.0, no V8, no XFA).
# Why here: `target/` is untracked and belongs to one checkout, so no worktree shares state with another. `cargo clean` removes it and
#          this puts it back. Nothing outside the checkout is written. A downloaded binary is untrusted: both digests must match.
#
# RECTO_PDFIUM_TARBALL=<file> uses a copy you already have, instead of downloading. It goes through the same digest check.
# Mac on Apple silicon only for now. Elsewhere this says so and exits 0, because the Windows compile check needs no library.
set -euo pipefail

URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/8125/pdfium-mac-arm64.tgz"
TARBALL_SHA256="4939bd935ac83ddc09a3a4f038bc43a19cea1660316a3667f26cb07a070e139d"
DYLIB_SHA256="a45c44645523968525c710462691dd974f4efb1013ce1c249919a0939cfb373a"

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="${CARGO_TARGET_DIR:-$root/target}/pdfium"
lib="$dest/libpdfium.dylib"
attest=0
case "${1:-}" in
  "") ;;
  --attest) attest=1 ;;
  *) sed -n '2,13p' "$0"; exit 2 ;;
esac

if [ "$(uname -s)-$(uname -m)" != "Darwin-arm64" ]; then
  echo "pdfium: no pinned build for $(uname -s)-$(uname -m) yet; skipping." >&2
  exit 0
fi

sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

if [ -f "$lib" ] && [ "$(sha "$lib")" = "$DYLIB_SHA256" ] && [ "$attest" = 0 ]; then
  echo "$lib"
  exit 0
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/recto-pdfium.XXXXXX")"
trap 'rm -rf "$work"' EXIT
tarball="$work/pdfium-mac-arm64.tgz"
if [ -n "${RECTO_PDFIUM_TARBALL:-}" ]; then
  cp "$RECTO_PDFIUM_TARBALL" "$tarball"
else
  echo "pdfium: fetching $URL" >&2
  curl --fail --silent --show-error --location --proto '=https' --output "$tarball" "$URL"
fi

got="$(sha "$tarball")"
if [ "$got" != "$TARBALL_SHA256" ]; then
  echo "pdfium: tarball digest is $got, expected $TARBALL_SHA256. Stopping." >&2
  exit 1
fi
if [ "$attest" = 1 ]; then
  gh attestation verify "$tarball" --repo bblanchon/pdfium-binaries >&2
fi
# No absolute paths and no climbing out, before anything is unpacked.
if tar -tzf "$tarball" | grep -E '(^/|(^|/)\.\.(/|$))' >&2; then
  echo "pdfium: the tarball has unsafe paths. Stopping." >&2
  exit 1
fi
mkdir "$work/unpacked"
tar -xzf "$tarball" -C "$work/unpacked" lib/libpdfium.dylib LICENSE VERSION args.gn
got="$(sha "$work/unpacked/lib/libpdfium.dylib")"
if [ "$got" != "$DYLIB_SHA256" ]; then
  echo "pdfium: library digest is $got, expected $DYLIB_SHA256. Stopping." >&2
  exit 1
fi
grep -q 'pdf_enable_v8 = false' "$work/unpacked/args.gn" && grep -q 'pdf_enable_xfa = false' "$work/unpacked/args.gn"

mkdir -p "$dest"
cp "$work/unpacked/LICENSE" "$work/unpacked/VERSION" "$work/unpacked/args.gn" "$dest/"
# Into place in one step, so a run beside this one sees the old file or the whole new one.
cp "$work/unpacked/lib/libpdfium.dylib" "$lib.partial"
mv -f "$lib.partial" "$lib"
echo "$lib"
