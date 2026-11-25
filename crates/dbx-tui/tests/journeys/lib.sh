# Shared helpers for layer 5 journeys. Source it; do not run it.
# Uses a private tmux server so no other tmux session is ever touched.

set -u
JOURNEY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$JOURNEY_DIR/../../../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
# PROFILE=release runs the journey against the release binary
# (target/release/dbpane); the default is the debug one.
PROFILE="${PROFILE:-debug}"
case "$PROFILE" in
  debug) PROFILE_FLAG="" ;;
  release) PROFILE_FLAG="--release" ;;
  *) echo "PROFILE must be debug or release" >&2; exit 2 ;;
esac
BIN="$TARGET_DIR/$PROFILE/dbpane"
# Captures and scratch stores live in OUT. A journey that passes removes it
# unless JOURNEY_OUT names the directory to keep them in; a failing journey
# always keeps it (its last screen is printed first).
if [ -n "${JOURNEY_OUT:-}" ]; then KEEP_OUT=1; else KEEP_OUT=0; fi
FAILED=0
OUT="${JOURNEY_OUT:-$(mktemp -d)}"
# One tmux server per journey run, so journeys run side by side cannot kill
# each other's sessions.
SOCK="dbxtui-$(basename "$0" .sh)-$$"
SESSION="journey-$$"
mkdir -p "$OUT"

# Every journey runs the client against a scratch store with a scratch key:
# never the real store, never the keychain (dbx reads DBX_SECRET_KEY_FILE
# before any key policy).
STORE="$OUT/store"
KEYFILE="$OUT/secret.key"
SCRATCH_HOME="$OUT/home"
# HOME too, so dbx never looks for a key file in the real user's home.
CLIENT="env -u DBX_SECRET_KEY HOME=$SCRATCH_HOME DBX_DATA_DIR=$STORE DBX_SECRET_KEY_FILE=$KEYFILE $BIN"
scratch_store() {
  rm -rf "$STORE"; mkdir -p "$STORE" "$SCRATCH_HOME"
  [ -f "$KEYFILE" ] || { openssl rand -hex 32 >"$KEYFILE"; chmod 600 "$KEYFILE"; }
}
# seed NAME KIND HOST PORT [DATABASE]: saves a connection into the scratch store
seed() {
  env -u DBX_SECRET_KEY HOME="$SCRATCH_HOME" DBX_SECRET_KEY_FILE="$KEYFILE" "$TARGET_DIR/debug/examples/seed_store" "$STORE" "$@" >/dev/null \
    || fail "seed $1"
}

tm() { tmux -L "$SOCK" "$@"; }
# Ends this journey's tmux server (and so its socket file under /tmp).
kill_tmux() {
  tm kill-server >/dev/null 2>&1 || true
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$SOCK"
}
# The journey's exit: its tmux server and, when it passed, its scratch directory.
cleanup() {
  kill_tmux
  if [ "$KEEP_OUT" = 0 ] && [ "$FAILED" = 0 ]; then rm -rf "$OUT"; fi
}
trap cleanup EXIT

fail() { FAILED=1; echo "FAIL: $*" >&2; [ -n "${LAST_CAPTURE:-}" ] && { echo "--- last screen ---" >&2; cat "$LAST_CAPTURE" >&2; }; exit 1; }

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

# wait_for_soft NAME REGEX [SECONDS]: like wait_for, but returns 1 on timeout.
wait_for_soft() {
  local name="$1" re="$2" tries=$(( ${3:-10} * 10 ))
  while [ "$tries" -gt 0 ]; do
    capture "$name"
    grep -Eq -- "$re" "$LAST_CAPTURE" && return 0
    sleep 0.1; tries=$((tries - 1))
  done
  return 1
}

expect() { grep -Eq -- "$2" "$LAST_CAPTURE" || fail "$1: expected /$2/"; }
expect_not() { grep -Eq -- "$2" "$LAST_CAPTURE" && fail "$1: did not expect /$2/"; true; }

# start_shell: a plain sh with a recognisable prompt, 120x35
start_shell() {
  kill_tmux
  tm new-session -d -s "$SESSION" -x 120 -y 35 "env PS1='PROMPT>' sh"
  tm set-option -t "$SESSION" -g status off >/dev/null
  wait_for shell-ready 'PROMPT>'
}

# Always build: cargo makes this a no-op when nothing changed.
build_bin() { (cd "$ROOT" && cargo build -p dbx-tui $PROFILE_FLAG) >/dev/null 2>&1 || fail "build"; }
# Examples are test tools (seeding a store): always the debug build.
build_example() { (cd "$ROOT" && cargo build -p dbx-tui --example "$1") >/dev/null 2>&1 || fail "build example $1"; }
# An example built in the journey's own profile (`panic_probe`, which must
# behave as the release binary does under panic = "abort").
build_example_in_profile() { (cd "$ROOT" && cargo build -p dbx-tui $PROFILE_FLAG --example "$1") >/dev/null 2>&1 || fail "build example $1"; }

# expect_terminal_restored LABEL: the pane is back to a normal shell
expect_terminal_restored() {
  [ "$(tm display -p -t "$SESSION" '#{alternate_on}')" = 0 ] || fail "$1: still on the alternate screen"
  [ "$(tm display -p -t "$SESSION" '#{cursor_flag}')" = 1 ] || fail "$1: cursor is hidden"
  [ "$(tm display -p -t "$SESSION" '#{mouse_any_flag}')" = 0 ] || fail "$1: mouse reporting still on"
}
