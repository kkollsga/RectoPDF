#!/bin/sh
# One release/notarization archive recipe. The updater validates __MACOSX metadata but never unpacks it into the signed bundle.
set -eu
[ "$#" -eq 2 ] || { echo "usage: archive.sh <app> <zip>" >&2; exit 2; }
exec /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$1" "$2"
