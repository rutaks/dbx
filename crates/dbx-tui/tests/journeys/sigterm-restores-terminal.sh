#!/usr/bin/env bash
# Journey sigterm-restores-terminal (slice 01): being killed by SIGTERM or
# SIGHUP must leave the shell usable, not on the alternate screen with the
# cursor hidden and mouse reporting on.
. "$(dirname "$0")/lib.sh"

build_bin
scratch_store
for sig in TERM HUP; do
  start_shell
  tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
  wait_for "20-running-$sig" 'No connections yet'
  pane_pid="$(tm display -p -t "$SESSION" '#{pane_pid}')"
  client_pid="$(pgrep -P "$pane_pid" -f dbpane | head -n 1)"
  [ -n "$client_pid" ] || fail "client process not found"
  kill -"$sig" "$client_pid"
  wait_for "21-after-$sig" 'EXIT=[0-9]' 6
  expect_terminal_restored "SIG$sig"
done
echo "OK sigterm-restores-terminal; captures in $OUT"
