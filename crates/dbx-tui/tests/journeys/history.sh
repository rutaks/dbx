#!/usr/bin/env bash
# Journey history (slice 18): against a scratch store holding the Docker
# fixture connection, run `select 'needle'`, clear the editor, open the
# history (ctrl+alt+e), type `needle` and press enter: the statement is back
# in the editor. Then the same through the prefix key, with F5: it runs again
# from the history. The history is the scratch store's own: the real desktop
# store is never opened.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

# goto REGEX: press j until the cursor row (the one drawn on a background) matches.
goto() {
  local n=0
  while [ "$n" -lt 40 ]; do
    if tm capture-pane -e -p -t "$SESSION" | grep -E -- "$1" | grep -q '48;2;49;50;68'; then return 0; fi
    key j; sleep 0.12; n=$((n + 1))
  done
  capture "goto-failed"; fail "the cursor never reached /$1/"
}

# editor_has TEXT: the text is in the editor (above the results pane's title),
# not in the results.
editor_has() { awk '/ results /{exit} {print}' "$LAST_CAPTURE" | grep -Fq -- "$1"; }

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 pagila
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 00-listed 'local pg'
key Enter
wait_for 01-databases '▸ pagila'
goto 'pagila'
key u
wait_for 02-used '1 local pg / pagila'
key C-g l
wait_for 03-editor 'EDITOR'

# Run it, then clear the editor: only the history knows it now.
tm send-keys -t "$SESSION" -l "select 'needle' as found"
key F5
wait_for 04-ran '1 row'
expect ran 'found'
key C-g a
key BSpace
sleep 0.3
capture 05-cleared
editor_has "select 'needle'" && fail "the editor was not cleared"

# Open the history: this run is in it, with where it ran and that it worked.
key C-M-e
wait_for 06-open 'History'
expect open 'search'
tm send-keys -t "$SESSION" -l 'needle'
wait_for 07-found "local pg / pagila +✓ +select 'needle' as found"
expect found 'this connection'
expect found '↵ insert'
expect found 'f5 run'
# enter and f5 wait for a pending search: let the pause and the search pass.
sleep 0.8

# enter puts the statement back in the editor, and the overlay closes.
key Enter
wait_for 08-inserted "select 'needle' as found"
expect_not inserted 'History'
editor_has "select 'needle' as found" || fail "the statement is not in the editor"

# The prefix key opens it too; f5 runs the entry again, in this tab.
key C-g a
key BSpace
key C-g e
wait_for 09-open-again 'History'
tm send-keys -t "$SESSION" -l 'needle'
wait_for 10-found-again "select 'needle' as found"
sleep 0.8
key F5
wait_for 11-ran-again '1 row'
expect ran-again 'found'
expect_not ran-again 'History'

# Both runs are in the history now (the second one was saved too).
key C-M-e
wait_for 12-two 'History'
tm send-keys -t "$SESSION" -l 'needle'
wait_for 13-two-entries '2 of 2'
key Escape
wait_for 14-closed 'EDITOR'
expect_not closed 'History'

key C-g q
wait_for 15-quit 'EXIT=0' 3
expect_terminal_restored quit

if grep -l -e "$DBX_TUI_SEED_PASSWORD" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen"
fi
echo "OK history; captures in $OUT"
