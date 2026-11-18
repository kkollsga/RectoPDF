#!/bin/sh
# One app archive producer. Releases/notarization sequester metadata outside the signed bundle; development builds keep their explicit stripping policy.
set -eu
usage() { echo "usage: archive.sh [--no-metadata] <app> <zip>" >&2; exit 2; }
case "${1:-}" in
    --no-metadata)
        [ "$#" -eq 3 ] || usage
        shift
        set -- -c -k --norsrc --noextattr --keepParent "$1" "$2"
        ;;
    --*) usage ;;
    *)
        [ "$#" -eq 2 ] || usage
        set -- -c -k --sequesterRsrc --keepParent "$1" "$2"
        ;;
esac
exec /usr/bin/ditto "$@"
