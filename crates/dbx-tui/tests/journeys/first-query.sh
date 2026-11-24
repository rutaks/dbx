#!/usr/bin/env bash
# Journey first-query (slice 05): against a scratch store holding the Docker
# fixture connection, give the query tab `pagila` (`u` in the sidebar), type a
# query in the editor and run it with F5: three rows. Then: the running state
# in the mode bar; a statement without a result set; NULL, '' and 'NULL' side
# by side; a syntax error with its position and the statement that ran. Quit;
# the terminal is restored.
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

# F5 with no database for the tab: a notice says which key gives it one.
key F5
wait_for 01-no-target 'press u on one'

# Give the tab `pagila`.
key Enter
wait_for 02-databases '▸ pagila'
goto 'pagila'
key u
wait_for 03-used '1 local pg / pagila'

# Focus the editor, type, run with F5: three rows.
key C-g l
wait_for 04-editor-focused 'EDITOR'
expect editor-focused 'f5 run'
expect_not editor-focused 'u use'
type_sql 'select title, rating from film order by film_id limit 3'
key F5
wait_for 05-three-rows 'ACADEMY DINOSAUR'
expect three-rows 'title +rating'
expect three-rows 'ACE GOLDFINGER'
expect three-rows 'ADAPTATION HOLES'
expect three-rows '3 rows  [0-9<]+ms'
expect_not three-rows 'AFFAIR PREJUDICE'

# A second statement after it: only it runs, and the mode bar counts the
# seconds while it does. (dbx splits statements at `;`; after two or more
# blank lines it also splits before a DDL/DML keyword, but not before SELECT
# or WITH, so the `;` is needed here.)
type_sql ';'
key Enter Enter
type_sql 'select pg_sleep(2.5) as slept;'
key F5
wait_for 06-running 'running 1s' 3
key F5
wait_for 06b-refused 'still running' 2
wait_for 07-slept '1 row  [0-9.]+s' 5
expect slept 'slept'
expect_not slept 'ACADEMY DINOSAUR'

# A statement without a result set.
key Enter
type_sql 'set search_path to public;'
key F5
wait_for 08-no-result-set '0 rows affected'

# NULL, the empty string and the text NULL, side by side, and a number.
key Enter
type_sql "select null as a, '' as b, 'NULL' as c, 42 as n;"
key F5
wait_for 09-nulls '∅'
expect nulls 'NULL'
expect nulls '42'

# A syntax error over two lines: the message, its position, the statement.
key Enter
type_sql 'select title'
key Enter
type_sql '  fro film;'
key F5
wait_for 10-syntax-error 'SQL error'
expect syntax-error 'line 2, column 7'
expect syntax-error 'syntax error at or near "film"'
expect syntax-error 'fro film'
expect syntax-error 'failed'

key C-g q
wait_for 11-quit 'EXIT=0' 3
expect_terminal_restored quit

if grep -l -e "$DBX_TUI_SEED_PASSWORD" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen"
fi
echo "OK first-query; captures in $OUT"
