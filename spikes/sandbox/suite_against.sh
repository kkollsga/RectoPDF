#!/usr/bin/env bash
# Runs the production renderer test binaries (already built with `cargo test --release -p recto-render-service --no-run`) against a worker from bundle.sh, with no test changed: the
# path they were built to use becomes a link to the signed worker, and goes back afterwards. Wrap in tools/sysguard/heavy.sh.
#
#   tools/sysguard/heavy.sh spikes/sandbox/suite_against.sh <Spike.app or "plain">
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
release="$root/target/release"
app="$1"
if [ "$app" != plain ]; then
  mv "$release/recto-render-service" "$release/recto-render-service.spike-keep"
  trap 'rm -f "$release/recto-render-service" "$release/libpdfium.dylib"; mv "$release/recto-render-service.spike-keep" "$release/recto-render-service"' EXIT
  ln -s "$app/Contents/MacOS/recto-render-service" "$release/recto-render-service"
  ln -s "$app/Contents/MacOS/libpdfium.dylib" "$release/libpdfium.dylib"
fi
cd "$root"
status=0
for name in worker hostile handles sequence memory corpus; do
  bin="$(ls -t "$release/deps/$name"-* | grep -v '\.d$' | head -1)"
  echo "== $name"
  "$bin" --test-threads=4 2>&1 | grep -E "^test .*FAILED|^test result|panicked at|^    [a-z_]+$" | cut -c1-200 || status=1
done
exit $status
