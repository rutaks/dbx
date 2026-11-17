#!/usr/bin/env bash
# Journey quit-mid-run (slice 05 review): quitting while a statement runs
# cancels it on the server. Twice: quitting with ctrl+g q, and with SIGTERM.
# Each time `select pg_sleep(30)` is running in the client; within 3 s of the
# quit, pg_stat_activity (read through `docker exec` into the fixture
# container) shows no such query, and the terminal is restored.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"
PROBE="dbxtui_quit_probe_$$"

# still_running: how many backends run the probe statement (not this query).
still_running() {
  docker exec dbxtui-fixtures-postgres-1 psql -U postgres -d postgres --no-psqlrc -tA -c \
    "select count(*) from pg_stat_activity where query like '%${PROBE}%' and pid <> pg_backend_pid() and state = 'active'"
}

# expect_gone LABEL: the probe stops running within 3 s.
expect_gone() {
  local tries=30
  while [ "$tries" -gt 0 ]; do
    [ "$(still_running)" = 0 ] && return 0
    sleep 0.1; tries=$((tries - 1))
  done
  fail "$1: the statement is still running on the server 3 s after quitting"
}

goto() {
  local n=0
  while [ "$n" -lt 40 ]; do
    if tm capture-pane -e -p -t "$SESSION" | grep -E -- "$1" | grep -q '\[48;'; then return 0; fi
    key j; sleep 0.12; n=$((n + 1))
  done
  capture "goto-failed"; fail "the cursor never reached /$1/"
}

# start_running NAME: starts the client, gives the tab pagila, runs the probe.
start_running() {
  tm send-keys -t "$SESSION" "sh -c 'echo \$\$ >$OUT/client.pid; exec $CLIENT'; echo EXIT=\$?" Enter
  wait_for "$1-listed" 'local pg'
  key Enter
  wait_for "$1-databases" '▸ pagila'
  goto 'pagila'
  key u
  key C-g l
  tm send-keys -t "$SESSION" -l "select pg_sleep(30) as ${PROBE}"
  key F5
  wait_for "$1-running" 'running 1s' 4
  [ "$(still_running)" = 1 ] || fail "$1: the probe is not running on the server"
}

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 pagila
start_shell

start_running 20-key
key C-g q
wait_for 21-key-quit 'EXIT=0' 4
expect_terminal_restored key-quit
expect_gone key-quit

tm send-keys -t "$SESSION" clear Enter
start_running 22-term
kill -TERM "$(cat "$OUT/client.pid")"
wait_for 23-term-quit 'EXIT=0' 4
expect_terminal_restored term-quit
expect_gone term-quit

echo "OK quit-mid-run; captures in $OUT"
