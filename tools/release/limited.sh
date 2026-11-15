# shellcheck shell=bash
# release/limited: a command under a time limit that names itself when it waits, for the preflight's signing and launching steps
# (tools/release/dp-keychain-probe.sh, tools/release/preflight-app.sh). Sourced, not run:
#
#   . tools/release/limited.sh
#   limited <seconds> <what> <command...>    Runs the command with its milliseconds said. Past its limit it hung, which is said as
#                                            "<what> didn't finish in <seconds> s: it hung", with the keychains as they are then, and the
#                                            caller exits 1; a failure says its exit code and exits 1 too.
#
# What tells apart why a signing would wait (run 43751288571 hung at its first team signing for the job's whole 20 minutes): the keychains
# searched and the default, the signing keychain's settings (asked under a limit, as a locked keychain may wait to be unlocked), and how many
# signing identities are found. Paths and counts only. The limit is perl's alarm, as there's no `timeout` on macOS: it ends the command it
# starts (exit 142), not that command's own children.
now_ms() { perl -MTime::HiRes=time -e 'printf "%d", time * 1000'; }
keychains() {
  echo "  keychains searched: $(security list-keychains -d user | tr -d '"' | tr -s ' \n' ' ')"
  echo "  default keychain: $(security default-keychain -d user | tr -d '" ')"
  if [ -n "${PREFLIGHT_KEYCHAIN:-}" ]; then
    case " $(security list-keychains -d user | tr -d '"' | tr -s ' \n' ' ') " in
      *" $PREFLIGHT_KEYCHAIN "*) echo "  the signing keychain is searched" ;;
      *) echo "  the signing keychain isn't searched" ;;
    esac
    echo "  the signing keychain's settings: $(perl -e 'alarm shift; exec @ARGV' 10 security show-keychain-info "$PREFLIGHT_KEYCHAIN" 2>&1 | tr '\n' ' ' || true)"
  fi
  echo "  signing identities: $(perl -e 'alarm shift; exec @ARGV' 10 security find-identity -v -p codesigning 2>/dev/null | tail -1 || true)"
}
# limited <seconds> <what> <command...>: the command under a limit, with its milliseconds; one that passes its limit hung, which is said with
# the keychains as they are then, and ends the run.
limited() {
  local limit="$1" what="$2" started code=0 ms
  shift 2
  started=$(now_ms)
  perl -e 'alarm shift; exec @ARGV' "$limit" "$@" || code=$?
  ms=$(($(now_ms) - started))
  if [ "$code" -eq 142 ]; then
    echo "::error::$what didn't finish in $limit s: it hung"
    keychains
    exit 1
  elif [ "$code" -ne 0 ]; then
    echo "::error::$what failed (exit $code, after $ms ms)"
    exit 1
  fi
  echo "  ok  $what ($ms ms)"
}
