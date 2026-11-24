#!/usr/bin/env bash
# Journey tabs (slice 15b): two query tabs on the fixtures. Tab 1 is given
# `pagila` with `u` and runs `select 1`; prefix `t` opens tab 2 (on pagila
# too), which runs `select 2`; prefix `[` shows tab 1's result again, and the
# bar shows both tabs. Then tab 2 is given `edge`; tab 1 runs a three-second
# statement, and tab 2's quick one shows while tab 1 still runs (its glyph in
# the bar). Last, quitting while tab 1 runs leaves no statement on the server
# (pg_stat_activity through `docker exec`, as quit-mid-run does).
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
# type_sql TEXT: types TEXT literally, each `;` as its byte (tmux reads a
# trailing `;` as a command separator).
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
PROBE="tabs_probe_$$"

still_running() {
  docker exec dbxtui-fixtures-postgres-1 psql -U postgres -d postgres --no-psqlrc -tA -c \
    "select count(*) from pg_stat_activity where query like '%${PROBE}%' and pid <> pg_backend_pid() and state = 'active'"
}

expect_gone() {
  local tries=30
  while [ "$tries" -gt 0 ]; do
    [ "$(still_running)" = 0 ] && return 0
    sleep 0.1; tries=$((tries - 1))
  done
  fail "$1: the statement is still running on the server 3 s after quitting"
}

goto() {
  local n=0 dir="${2:-j}"
  while [ "$n" -lt 40 ]; do
    if tm capture-pane -e -p -t "$SESSION" | grep -E -- "$1" | grep -q '48;2;49;50;68'; then return 0; fi
    key "$dir"; sleep 0.12; n=$((n + 1))
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
expect listed '1 no target'

# 1. Tab 1 gets pagila with `u` and runs `select 1`.
key Enter
wait_for 01-databases '▸ pagila'
goto 'pagila'
key u
wait_for 02-used '1 local pg / pagila'
key C-g l
type_sql "select 1 + 1000 as one"
key F5
wait_for 03-tab1-ran '1001'

# 2. Prefix `t` opens tab 2 on the same database; it runs `select 2`.
key C-g t
wait_for 04-tab2 '2 local pg / pagila'
expect tab2 '1 local pg / pagila.*2 local pg / pagila'
expect_not tab2 '1001'
type_sql "select 2 + 2000 as two"
key F5
wait_for 05-tab2-ran '2002'
expect_not tab2-ran '1001'
# Prefix `[` shows tab 1 and its result; the bar shows both tabs.
key C-g '['
wait_for 06-back-to-tab1 '1001'
expect back-to-tab1 '1 local pg / pagila.*2 local pg / pagila'
expect_not back-to-tab1 '2002'

# 3. Tab 2 is given edge; tab 1 runs a slow statement and tab 2 a quick one,
#    whose result shows while tab 1 still runs.
key C-g ']'
wait_for 07-tab2-again '2002'
key C-g h
goto 'edge' k
key u
wait_for 08-tab2-edge '2 local pg / edge'
key C-g '['
key C-g l
wait_for 09-tab1-editor '1001'
type_sql ';'
key Enter Enter
type_sql "select pg_sleep(3), 'a' || 'qq' as slow;"
key F5
wait_for 10-tab1-running 'running'
key C-g ']'
wait_for 11-tab2-shown '2 local pg / edge'
type_sql ';'
key Enter Enter
type_sql "select 'b' || 'xyz' as quick;"
key F5
wait_for 12-tab2-quick 'bxyz' 2
# Tab 1 is still running: its glyph is in the bar, and its result is not here.
expect tab2-quick '1 local pg / pagila ●'
expect_not tab2-quick 'aqq'
key C-g '['
wait_for 13-tab1-done 'aqq' 5
expect tab1-done '1 row'

# 4. Tab 1 runs a long statement; quitting from tab 2 leaves nothing running.
type_sql ';'
key Enter Enter
type_sql "select pg_sleep(30) as ${PROBE};"
key F5
wait_for 14-probe-running 'running 1s' 4
[ "$(still_running)" = 1 ] || fail "the probe is not running on the server"
key C-g ']'
wait_for 15-on-tab2 'bxyz'
key C-g q
wait_for 16-quit 'EXIT=0' 4
expect_terminal_restored quit
expect_gone quit

if grep -l -e "$DBX_TUI_SEED_PASSWORD" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen"
fi
echo "OK tabs; captures in $OUT"
