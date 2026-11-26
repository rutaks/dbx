#!/usr/bin/env bash
# Journey config (slice 22): a config file sets the prefix to ctrl+b. With the
# fixture connection, `ctrl+b l` focuses the editor, `ctrl+b r` runs a
# statement, and the mode bar shows ctrl+b, not ctrl+g. Then a second file with
# two mistakes: the client still starts, a notice says how many problems there
# are, and the help lists each with its line and column and the file's path.
# The file is named by DBPANE_CONFIG, under the journey's scratch directory;
# HOME is the scratch one too, so no real config file is read.
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

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 pagila

# ---- a file that sets the prefix ------------------------------------------------
printf '[keys]\nprefix = "ctrl+b"\n' >"$OUT/prefix.toml"
start_shell
tm send-keys -t "$SESSION" "DBPANE_CONFIG=$OUT/prefix.toml $CLIENT; echo EXIT=\$?" Enter
wait_for 00-listed 'local pg'
# A clean file says nothing at startup.
expect_not listed 'config file'

key Enter
wait_for 01-databases '▸ pagila'
goto 'pagila'
key u
wait_for 02-used '1 local pg / pagila'

# The new prefix focuses the editor, and the bar shows it.
key C-b l
wait_for 03-editor-focused 'EDITOR'
expect editor-focused 'ctrl\+b prefix'
expect_not editor-focused 'ctrl\+g'

# ctrl+b r runs the statement.
tm send-keys -t "$SESSION" -l 'select 41 + 1 as answer'
key C-b r
wait_for 04-ran 'answer'
expect ran '42'
expect ran '1 row'
expect ran 'ctrl\+b prefix'
expect_not ran 'ctrl\+g'

key C-b q
wait_for 05-quit 'EXIT=0' 3
expect_terminal_restored quit

# ---- a file with two mistakes ----------------------------------------------------
printf '[run]\nrow_cap = 0\nbogus = 1\n' >"$OUT/mistakes.toml"
start_shell
tm send-keys -t "$SESSION" "DBPANE_CONFIG=$OUT/mistakes.toml $CLIENT; echo EXIT=\$?" Enter
wait_for 10-listed 'local pg'
wait_for 11-notice 'The config file has 2 problems'
expect notice 'ctrl\+g \? lists them'

key C-g '?'
wait_for 12-help 'Config problems \(2\)'
expect help 'line 2, column 11: \[run\] row_cap: expected a whole number'
expect help 'line 3, column 1: unknown key "bogus" in \[run\]'
expect help 'mistakes.toml'
# The rest of the help is the defaults: the prefix is still ctrl+g.
key Escape
key C-g q
wait_for 13-quit 'EXIT=0' 3
expect_terminal_restored quit-2

if grep -l -e "$DBX_TUI_SEED_PASSWORD" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen"
fi
echo "OK config; captures in $OUT"
