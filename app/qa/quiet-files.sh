# Sourced by the shot scripts once they've set $root (their mktemp folder) and $out (OUT_DIR). A quiet run opens and writes files only in the
# system's temp folders (2026-11-13), so a PDF named on the command line is opened as a copy in the run's folder (`inside FILE` prints the copy's
# path, an APFS clone where the volume allows), and shots are written into the run's folder and copied to OUT_DIR as the script ends, however it
# ends (`quiet_files_out`, in the script's EXIT trap).
final_out="$out"
out="$root/out"
mkdir -p "$out" "$root/in"
inside() {
    local copy
    copy="$root/in/$(basename "$1")"
    [ -e "$copy" ] || cp -c "$1" "$copy" 2>/dev/null || cp "$1" "$copy"
    printf '%s\n' "$copy"
}
quiet_files_out() {
    local shot
    for shot in "$out"/*; do
        [ -f "$shot" ] && cp -f "$shot" "$final_out"/
    done
    return 0
}
