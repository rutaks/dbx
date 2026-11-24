#!/usr/bin/env bash
# Journey complete (slice 11): against a scratch store holding the Docker
# fixture connection, give the query tab `pagila`, type `select * from fi` and
# see the popup list `film` (and the view `film_list`) with the keys it takes
# in the mode bar; accept `film` with enter, run it, see rows. Then, in a
# second statement, type `select f.` ahead of `from film f` and, with the
# caret after the dot, open the popup by key: the columns of `film` with their
# types. `esc` closes it and the text stays. Quit; the terminal is restored.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
# type_sql TEXT: types TEXT literally. tmux reads an argument ending in `;` as
# a command separator, so each `;` is sent as its byte (0x3b).
type_sql() {
  local rest="$1" part
  while [ -n "$rest" ]; do
    part="${rest%%;*}"
    [ -n "$part" ] && tm send-keys -t "$SESSION" -l "$part"
    [ "$part" = "$rest" ] && break
    tm send-keys -t "$SESSION" -H 3b
    rest="${rest#*;}"
  done
}
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

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 pagila
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 00-listed 'local pg'

# Give the tab `pagila`, and focus the editor.
key Enter
wait_for 01-databases '▸ pagila'
goto 'pagila'
key u
wait_for 02-used '1 local pg / pagila'
key C-g l
wait_for 03-editor-focused 'EDITOR'
expect editor-focused 'ctrl\+space complete'

# Typing an identifier after FROM opens the popup: tables and views of the
# schema (the first answer takes a round trip), with their kind on the right.
type_sql 'select * from fi'
wait_for 04-popup 'film f +table'
expect popup 'film_list fl +view'
expect popup 'film_actor fa +table'
expect_not popup 'film_id'
expect_not popup 'lateral'
# The popup's keys are in the mode bar, and it is over the editor, not an overlay.
expect popup 'enter insert'
expect popup 'tab replace'
expect popup 'esc close'
expect popup 'select \* from fi'
expect_not popup 'ctrl\+space complete'

# enter inserts the first suggestion, `film`, with its generated alias; the popup is gone.
key Enter
wait_for 05-accepted 'ctrl\+space complete'
expect accepted 'select \* from film f'
expect_not accepted 'film_list fl +view'
# Run it: rows.
key F5
wait_for 06-rows '(1,000\+|[0-9,]+) rows +[0-9.]+m?s'
expect rows 'film_id'
expect rows 'title'

# A second statement: the table's columns after an alias and a dot.
type_sql ';'
key Enter Enter
type_sql 'select f. from film f'
wait_for 07-typed 'select f\. from film f'
key Home
for _ in 1 2 3 4 5 6 7 8 9; do key Right; done
key C-Space
wait_for 08-columns 'film_id +integer +column'
expect columns 'title +(text|character varying)'
expect columns 'description'
expect columns 'enter insert'
# esc closes the popup and leaves the text; the next esc is the editor's.
key Escape
wait_for 09-closed 'ctrl\+space complete'
expect_not closed 'film_id +integer'
expect closed 'select f\. from film f'

key C-g q
wait_for 10-quit 'EXIT=0' 3
expect_terminal_restored quit

if grep -l -e "$DBX_TUI_SEED_PASSWORD" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen"
fi
echo "OK complete; captures in $OUT"
