# Shared helpers for layer 5 journeys. Source it; do not run it.
# Uses a private tmux server so no other tmux session is ever touched.

set -u
JOURNEY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$JOURNEY_DIR/../../../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET_DIR/debug/dbx-tui"
OUT="${JOURNEY_OUT:-$(mktemp -d)}"
SOCK="dbxtui-test"
SESSION="journey-$$"
mkdir -p "$OUT"

tm() { tmux -L "$SOCK" "$@"; }
cleanup() { tm kill-server >/dev/null 2>&1 || true; }
trap cleanup EXIT

fail() { echo "FAIL: $*" >&2; [ -n "${LAST_CAPTURE:-}" ] && { echo "--- last screen ---" >&2; cat "$LAST_CAPTURE" >&2; }; exit 1; }

# capture NAME -> saves the pane text to $OUT/NAME.txt and sets LAST_CAPTURE
capture() {
  LAST_CAPTURE="$OUT/$1.txt"
  tm capture-pane -p -t "$SESSION" >"$LAST_CAPTURE"
}

# wait_for NAME REGEX [SECONDS]: capture until REGEX appears (fixed-string grep -E)
wait_for() {
  local name="$1" re="$2" tries=$(( ${3:-10} * 10 ))
  while [ "$tries" -gt 0 ]; do
    capture "$name"
    grep -Eq -- "$re" "$LAST_CAPTURE" && return 0
    sleep 0.1; tries=$((tries - 1))
  done
  fail "timed out waiting for /$re/ ($name)"
}

expect() { grep -Eq -- "$2" "$LAST_CAPTURE" || fail "$1: expected /$2/"; }
expect_not() { grep -Eq -- "$2" "$LAST_CAPTURE" && fail "$1: did not expect /$2/"; true; }

# start_shell: a plain sh with a recognisable prompt, 120x35
start_shell() {
  cleanup
  tm new-session -d -s "$SESSION" -x 120 -y 35 "env PS1='PROMPT>' sh"
  tm set-option -t "$SESSION" -g status off >/dev/null
  wait_for shell-ready 'PROMPT>'
}

# Always build: cargo makes this a no-op when nothing changed.
build_bin() { (cd "$ROOT" && cargo build -p dbx-tui) >/dev/null 2>&1 || fail "build"; }
build_example() { (cd "$ROOT" && cargo build -p dbx-tui --example "$1") >/dev/null 2>&1 || fail "build example $1"; }

# expect_terminal_restored LABEL: the pane is back to a normal shell
expect_terminal_restored() {
  [ "$(tm display -p -t "$SESSION" '#{alternate_on}')" = 0 ] || fail "$1: still on the alternate screen"
  [ "$(tm display -p -t "$SESSION" '#{cursor_flag}')" = 1 ] || fail "$1: cursor is hidden"
  [ "$(tm display -p -t "$SESSION" '#{mouse_any_flag}')" = 0 ] || fail "$1: mouse reporting still on"
}
