#!/usr/bin/env bash
# release/app-entitlements: the entitlements a release's main executable claims, so the Find key can live in the data-protection keychain
# in the app's own access group (the owner's decision, 2026-11-15), and the check that a signed bundle claims them as it should. The team
# and the bundle identifier are read from crates/recto-update/src/lib.rs, where they're defined once.
#
#   tools/release/app-entitlements.sh --write <file>             Writes the entitlements plist: the App ID, the team, and the keychain
#                                                                group, which is the App ID.
#   tools/release/app-entitlements.sh --check <RectoPDF.app>     Says each problem and exits 1 unless: the bundle embeds a provisioning
#                                                                profile for the App ID that allows the group; the main executable claims
#                                                                these three and nothing else; and no other code in the bundle (MacOS,
#                                                                Helpers, Frameworks, PlugIns, XPCServices, Library) claims any of them.
#                                                                Prints no part of the profile but the App ID.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
team=$(sed -n 's/^pub const TEAM_ID: &str = "\([A-Z0-9]*\)";$/\1/p' "$root/crates/recto-update/src/lib.rs")
bundle_id=$(sed -n 's/^pub const BUNDLE_ID: &str = "\(.*\)";$/\1/p' "$root/crates/recto-update/src/lib.rs")
[ -n "$team" ] && [ -n "$bundle_id" ] || { echo "the team or the bundle identifier couldn't be read from crates/recto-update/src/lib.rs" >&2; exit 1; }
app_id="$team.$bundle_id"

case "${1:-}" in
  --write)
    out="${2:?usage: tools/release/app-entitlements.sh --write <file>}"
    cat > "$out" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.application-identifier</key><string>$app_id</string>
  <key>com.apple.developer.team-identifier</key><string>$team</string>
  <key>keychain-access-groups</key><array><string>$app_id</string></array>
</dict></plist>
EOF
    ;;
  --check)
    app="${2:?usage: tools/release/app-entitlements.sh --check <RectoPDF.app>}"
    work="$(mktemp -d "${TMPDIR:-/tmp}/app-entitlements.XXXXXX")"
    trap 'rm -rf "$work"' EXIT
    problems=0
    say() { echo "entitlements: $1" >&2; problems=1; }
    profile="$app/Contents/embedded.provisionprofile"
    if [ ! -f "$profile" ]; then
      say "the bundle has no Contents/embedded.provisionprofile"
    elif ! security cms -D -i "$profile" > "$work/profile.plist" 2>/dev/null; then
      say "Contents/embedded.provisionprofile isn't a signed provisioning profile"
    else
      python3 -I - "$work/profile.plist" "$app_id" "$team" <<'PY' || problems=1
import plistlib, sys
path, app_id, team = sys.argv[1:4]
with open(path, "rb") as file:
    entitlements = plistlib.load(file).get("Entitlements", {})
groups = entitlements.get("keychain-access-groups", [])
wrong = []
if entitlements.get("com.apple.application-identifier") != app_id:
    wrong.append(f"the embedded profile isn't for {app_id}")
if app_id not in groups and f"{team}.*" not in groups:
    wrong.append(f"the embedded profile doesn't allow the keychain group {app_id}")
for line in wrong:
    print(f"entitlements: {line}", file=sys.stderr)
sys.exit(1 if wrong else 0)
PY
    fi
    claimed() { codesign -d --entitlements - --xml "$1" 2>/dev/null || true; }
    claimed "$app" > "$work/main.plist"
    python3 -I - "$work/main.plist" "$app_id" "$team" <<'PY' || problems=1
import plistlib, sys
path, app_id, team = sys.argv[1:4]
raw = open(path, "rb").read()
claims = plistlib.loads(raw) if raw.strip() else {}
want = {"com.apple.application-identifier": app_id, "com.apple.developer.team-identifier": team, "keychain-access-groups": [app_id]}
wrong = False
for key, value in want.items():
    if claims.get(key) != value:
        print(f"entitlements: the app's main executable claims {key} = {claims.get(key)!r}, not {value!r}", file=sys.stderr)
        wrong = True
# Exactly these three: anything more is a power the release would have that nobody decided it should.
for key in sorted(set(claims) - set(want)):
    print(f"entitlements: the app's main executable claims {key} as well, which a release doesn't", file=sys.stderr)
    wrong = True
sys.exit(1 if wrong else 0)
PY
    while IFS= read -r program; do
      [ "$program" = "$app/Contents/MacOS/RectoPDF" ] && continue
      if claimed "$program" | grep -q -e com.apple.application-identifier -e keychain-access-groups -e com.apple.developer.team-identifier; then
        say "${program#"$app/"} claims the app's identity or keychain group; only the main executable may"
      fi
    done < <(find "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Frameworks" "$app/Contents/PlugIns" "$app/Contents/XPCServices" \
      "$app/Contents/Library" -type f 2>/dev/null)
    [ "$problems" = 0 ] || exit 1
    echo "entitlements: $app_id claimed by the main executable alone, with its profile embedded"
    ;;
  *)
    sed -n '6,12p' "$0" >&2
    exit 2
    ;;
esac
