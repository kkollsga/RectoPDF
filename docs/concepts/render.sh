#!/bin/sh
# Renders every concept screen to docs/concepts/*.png at 2x, in headless Chrome.
# Quiet: no window, no focus change, its own temporary browser profile.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
profile=$(mktemp -d)
trap 'rm -rf "$profile"' EXIT

shot() { # name query
  "$chrome" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=2 \
    --force-color-profile=srgb --no-first-run --no-default-browser-check \
    --user-data-dir="$profile" --window-size=1560,1000 --virtual-time-budget=1500 \
    --screenshot="$here/$1.png" "file://$here/$2" >/dev/null 2>&1
}

for screen in home reader menu markup compress edited menufile menumap find ask sign focus; do
  for theme in light dark; do
    shot "$screen-$theme" "src/concept.html?theme=$theme&screen=$screen"
  done
done
shot "reader-dark-white-paper" "src/concept.html?theme=dark&screen=paperwhite"
shot "sidebar-states" "src/concept.html?screen=states"
# Overview of every screen, light on the left and dark on the right, built from the renders above.
"$chrome" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
  --force-color-profile=srgb --no-first-run --no-default-browser-check --allow-file-access-from-files \
  --user-data-dir="$profile" --window-size=1600,6590 --virtual-time-budget=4000 \
  --screenshot="$here/overview.png" "file://$here/src/overview.html" >/dev/null 2>&1
ls -la "$here"/*.png
