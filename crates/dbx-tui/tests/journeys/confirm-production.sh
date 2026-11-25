#!/usr/bin/env bash
# Journey confirm-production (slice 17): a connection to `edge` marked
# production shows a red PROD; a delete on a scratch table stops at a red
# "Run on production?" box naming the connection, the database and the exact
# statement. `esc` runs nothing (psql shows the row is still there) and says
# `Not run.`; `enter` does nothing; `y` runs it (psql shows the row gone).
# Uses only a scratch schema it creates and drops. Quits cleanly.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

psql_edge() { docker exec -e PGOPTIONS='-c client_min_messages=warning' dbxtui-fixtures-postgres-1 psql -U postgres -d edge --no-psqlrc -v ON_ERROR_STOP=1 -tA -c "$1"; }
SCHEMA="scratch_17_j$$"
drop_schema() { psql_edge "drop schema if exists $SCHEMA cascade" >/dev/null 2>&1 || true; }
trap 'cleanup; drop_schema' EXIT
psql_edge "drop schema if exists $SCHEMA cascade; create schema $SCHEMA; create table $SCHEMA.t (id int primary key, v text); insert into $SCHEMA.t values (1, 'one'), (2, 'two')" >/dev/null \
  || fail "could not create the scratch table"
rows() { psql_edge "select string_agg(id::text, ',' order by id) from $SCHEMA.t"; }

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
DBX_TUI_SEED_PRODUCTION=1 seed "prod pg" postgres 127.0.0.1 55433 edge
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 00-listed 'prod pg'

# The connection's own database: `u` on the connection row.
key u
wait_for 01-used '1 prod pg / edge'
expect used '1 prod pg / edge PROD'
expect used ' PROD '

key C-g l
wait_for 02-editor 'EDITOR'
tm send-keys -t "$SESSION" -l "delete from $SCHEMA.t where id = 1"
key F5
wait_for 03-asked 'Run on production\?'
expect asked 'On prod pg / edge'
expect asked 'it writes data'
expect asked "delete from $SCHEMA.t where id = 1"
expect asked 'y run'
expect asked 'esc cancel'
[ "$(rows)" = "1,2" ] || fail "something ran before the yes: $(rows)"

# `enter` cannot confirm.
key Enter
sleep 0.5
capture 04-enter
expect enter 'Run on production\?'
[ "$(rows)" = "1,2" ] || fail "enter ran it: $(rows)"

# `esc` runs nothing.
key Escape
wait_for 05-not-run 'Not run\.'
expect_not not-run 'Run on production\?'
[ "$(rows)" = "1,2" ] || fail "esc ran it: $(rows)"

# Asked again, and `y` runs exactly it.
key F5
wait_for 06-asked-again 'Run on production\?'
sleep 0.5  # a `y` in the first 300 ms is ignored
key y
wait_for 07-ran '1 row affected' 10
[ "$(rows)" = "2" ] || fail "after the yes, expected only row 2: $(rows)"

key C-g q
wait_for 08-quit 'EXIT=0' 4
expect_terminal_restored quit

if grep -l -e "$DBX_TUI_SEED_PASSWORD" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen"
fi
echo "OK confirm-production; captures in $OUT"
