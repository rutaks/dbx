#!/usr/bin/env bash
# Journey kitty-user-var (slice 13): in a terminal that looks like kitty
# (KITTY_WINDOW_ID set) the client sets the user variable `dbx_tui` on start and
# clears it, and pops the keyboard protocol and modifyOtherKeys, on quit and on
# SIGTERM. Read from the raw bytes the client writes, through a pty.
. "$(dirname "$0")/lib.sh"

build_bin
scratch_store

run() { timeout 30 python3 -I "$JOURNEY_DIR/pty_probe.py" "$1" "$BIN" "$STORE" "$KEYFILE" "$SCRATCH_HOME"; }

for mode in quit term; do
  text="$(run "$mode")"
  case "$text" in *'<ESC>]1337;SetUserVar=dbx_tui=MQ==<ESC>\'*) ;; *) fail "$mode: the variable was never set";; esac
  case "$text" in *'<ESC>]1337;SetUserVar=dbx_tui<ESC>\'*) ;; *) fail "$mode: the variable was never cleared";; esac
  case "$text" in *'<ESC>[>4;0m'*) ;; *) fail "$mode: modifyOtherKeys was not reset";; esac
  # Cleared after it was set.
  set_at="${text%%SetUserVar=dbx_tui=MQ==*}"; clear_at="${text##*SetUserVar=dbx_tui=MQ==}"
  case "$clear_at" in *'SetUserVar=dbx_tui<ESC>\'*) ;; *) fail "$mode: cleared before it was set";; esac
  [ -n "$set_at" ] || true
done
echo "OK kitty-user-var"
