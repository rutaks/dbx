#!/usr/bin/env bash
# Journey panic-restores-terminal (slice 01): a panic while the terminal is in
# the client's mode must leave the shell usable, whether it happens on the main
# thread or inside a spawned task, and must end the process with code 101.
. "$(dirname "$0")/lib.sh"

build_example_in_profile panic_probe
PROBE="$TARGET_DIR/$PROFILE/examples/panic_probe"

for mode in main task; do
  start_shell
  arg=""; want="probe panic"
  [ "$mode" = task ] && { arg=task; want="probe panic in task"; }
  tm send-keys -t "$SESSION" "$PROBE $arg; echo EXIT=\$?" Enter
  wait_for "10-panic-$mode" 'EXIT=[0-9]' 6
  expect "panic ($mode)" 'EXIT=101'
  expect "panic ($mode)" "$want"
  expect_terminal_restored "panic ($mode)"
done
echo "OK panic-restores-terminal; captures in $OUT"
