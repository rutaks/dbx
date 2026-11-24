#!/usr/bin/env bash
# Journey editor-keys (slice 13): in tmux started with `extended-keys on`, run
# with ctrl+enter, comment a line with ctrl+/, duplicate it with ctrl+d, undo
# with ctrl+z, find text with ctrl+f and go to a line with the prefix. What
# arrives is whatever tmux sends the client: Cmd never does (tmux has no Cmd),
# and without the keyboard protocol the client falls back to what it can read.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

goto() {
  local n=0
  while [ "$n" -lt 40 ]; do
    if tm capture-pane -e -p -t "$SESSION" | grep -E -- "$1" | grep -q '48;2;49;50;68'; then return 0; fi
    key j; sleep 0.12; n=$((n + 1))
  done
  capture "goto-failed"; fail "the cursor never reached /$1/"
}

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 pagila

start_shell
# The server option the slice names: tmux passes ctrl+enter and ctrl+/ on only
# when it is on.
tm set -s extended-keys on
tm set -s extended-keys-format csi-u 2>/dev/null || true

key "$CLIENT; echo EXIT=\$?" Enter
wait_for 01-start 'connections'
key Enter
wait_for 02-open 'pagila'
goto 'pagila'
key u
wait_for 03-used '1 local pg / pagila'
key C-g l
wait_for 04-editor 'EDITOR'

# ctrl+enter runs.
key -l 'select title from film order by film_id limit 2'
key C-Enter
if ! wait_for_soft 05-ctrl-enter 'ACADEMY DINOSAUR' 4; then
  capture 05-what-arrived
  fail "ctrl+enter did not run under tmux extended-keys: the editor shows: $(grep -m3 -E 'select|^ *[0-9]+ ' "$LAST_CAPTURE" | tr '\n' '|')"
fi
expect ctrl-enter 'ACE GOLDFINGER'

# ctrl+/ comments the line, and again uncomments it.
key C-/
if ! wait_for_soft 06-comment '-- select title' 3; then
  capture 06-what-arrived
  fail "ctrl+/ did not comment the line: the editor shows: $(grep -m3 -E 'select|^ *[0-9]+ ' "$LAST_CAPTURE" | tr '\n' '|')"
fi
key C-/
wait_for 07-uncomment 'select title from film'
expect_not uncomment '[-][-] select title'

# ctrl+d duplicates the line; ctrl+z undoes it in one step.
key C-d
wait_for 08-duplicate ' 2 +select title from film'
key C-z
wait_for 09-undo 'EDITOR'
expect_not undo ' 2 +select title from film'

# Find: ctrl+f opens the bar, typing finds, esc closes.
key C-f
wait_for 10-find 'find'
key -l 'order'
wait_for 11-found '1/1'
key Escape
wait_for 12-closed 'EDITOR'
expect_not closed 'find +order'

# Go to line: the prefix is the key that always arrives.
key Enter -l 'select 1'
key C-g g
wait_for 13-goto 'line'
key -l '1' Enter
wait_for 14-went 'EDITOR'
expect went 'select title from film'

key C-g q
wait_for 15-quit 'EXIT=0' 3
expect_terminal_restored quit
echo "OK editor-keys; captures in $OUT"
