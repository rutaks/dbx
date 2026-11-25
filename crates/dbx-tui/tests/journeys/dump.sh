#!/usr/bin/env bash
# Journey dump (slice 20): against a scratch store holding the Docker fixture
# connection, with the client started in a scratch directory (so every default
# path is in it). dbx records the destination directory in the store it runs
# with: here, the scratch store. The fixtures are only read.
#   1. `D` on `pagila` opens the overlay: structure and data, the whole
#      database, the default path; `enter` dumps it: the mode bar shows the
#      progress, then the notice says where, how big, how many rows;
#   2. the same path again asks first, naming it; `esc` goes back and the file
#      is as it was;
#   3. a dump of `edge` (a million rows) is cancelled from its progress
#      overlay: `cancelling…`, then cancelled, and nothing is at the path;
#   4. quitting during a dump asks first, naming the file; `y` cancels it and
#      quits; the terminal is restored; only the pagila dump was written.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 pagila
start_shell

X="$OUT/dumps"
rm -rf "$X"; mkdir -p "$X"
tm send-keys -t "$SESSION" "cd '$X' && $CLIENT; echo EXIT=\$?" Enter
wait_for 00-listed 'local pg'
key Enter
wait_for 01-databases 'pagila'
# The databases are listed by name: edge, fixtures_ready, pagila, ...
key j j j
wait_for 02-on-pagila 'D dump'

# ---- 1. dump pagila ---------------------------------------------------------------
key D
wait_for 03-overlay 'Dump pagila'
expect overlay '● structure and data'
expect overlay '● whole database'
expect overlay '○ drop tables'
expect overlay "pagila-[0-9]{8}-[0-9]{6}\.sql"
expect overlay '↵ export'
expect overlay 'known not to restore whole'
expect overlay 'tsvector columns'
expect overlay 'See README "Dump"'
key Enter
wait_for 04-dumping 'dumping pagila' 10
wait_for 05-dumped 'Wrote the dump of pagila to' 60
expect dumped 'dbx reported no'
expect dumped 'known not to restore'
expect_not dumped 'Dumped pagila'
[ -z "$(ls -a "$X" | grep -F '.tmp')" ] || fail "a temporary file is left in $X: $(ls -a "$X")"
F="$(ls "$X"/pagila-*.sql 2>/dev/null | head -n1)"
[ -n "$F" ] || fail "no dump file in $X: $(ls -a "$X")"
head -n1 "$F" | grep -Fxq -- '-- Database export: pagila' || fail "the dump's first line: $(head -n1 "$F")"
grep -q '^CREATE TABLE "public"."actor"' "$F" || fail "no actor table in the dump"
SUM="$(cksum <"$F")"

# ---- 2. the same path again --------------------------------------------------------
key D
wait_for 06-overlay-again 'Dump pagila'
key Tab Tab Tab C-u
tm send-keys -t "$SESSION" -l "$(basename "$F")"
key Enter
wait_for 07-replace 'Replace the file\?' 10
expect replace 'is there already'
expect replace 'y replace'
key Escape
wait_for 08-back 'Dump pagila'
expect_not back 'Replace the file\?'
[ "$(cksum <"$F")" = "$SUM" ] || fail "the file changed"
key Escape
wait_for 09-closed 'SIDEBAR'
expect_not closed 'Dump pagila'

# ---- 3. cancel a dump of edge --------------------------------------------------------
key k k
wait_for 10-on-edge 'D dump'
key D
wait_for 11-overlay-edge 'Dump edge'
key Enter
# dbx's first progress closes the overlay.
wait_for 12-dumping-edge 'dumping edge: .* rows' 10
expect_not dumping-edge 'Dump edge'
key D
wait_for 13-progress 'c cancel dump'
key c
wait_for_soft 14-cancelling 'cancelling' 5 || true
wait_for 15-cancelled 'cancelled after' 30
expect cancelled 'Nothing was written to'
expect_not cancelled 'Dumped edge'
[ -z "$(ls "$X"/edge-*.sql 2>/dev/null)" ] || fail "a cancelled dump of the whole database left a file: $(ls "$X")"
key Escape

# ---- 4. quit while dumping --------------------------------------------------------------
key D
wait_for 16-overlay-edge-again 'Dump edge'
key Enter
wait_for 17-dumping-again 'dumping edge: .* rows' 10
key C-g q
wait_for 18-quit-question 'Quit while dumping\?'
expect quit-question 'cancel it and quit'
expect quit-question 'edge-[0-9]{8}-[0-9]{6}\.sql'
key y
wait_for 19-quit 'EXIT=0' 10
expect_terminal_restored quit
FILES="$(ls -a "$X" | grep -v '^\.\.\?$' | tr '\n' ' ')"
[ "$FILES" = "$(basename "$F") " ] || fail "the files in $X: $FILES"
echo "OK dump; captures in $OUT"
