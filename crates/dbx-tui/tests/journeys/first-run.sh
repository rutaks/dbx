#!/usr/bin/env bash
# Journey first-run (slice 24a): a stranger's first start. A scratch HOME with
# no dbx desktop store and no DBX_DATA_DIR: dbpane picks its own store under
# that HOME, and the main area welcomes the stranger, naming the store and
# the keys that add a connection, open help and quit. The stranger adds a
# connection to the fixture and runs `select 1`; the welcome is gone. A second
# start finds the connection in the same own store.
# Only the scratch HOME is ever reached; the key comes from a scratch key file.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
typ() { tm send-keys -t "$SESSION" -l "$1"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

build_bin
rm -rf "$SCRATCH_HOME"; mkdir -p "$SCRATCH_HOME"
[ -f "$KEYFILE" ] || { openssl rand -hex 32 >"$KEYFILE"; chmod 600 "$KEYFILE"; }
case "$(uname -s)" in
  Darwin) OWN="$SCRATCH_HOME/Library/Application Support/dbpane"; DESKTOP="$SCRATCH_HOME/Library/Application Support/com.dbx.app" ;;
  *) OWN="$SCRATCH_HOME/.local/share/dbpane"; DESKTOP="$SCRATCH_HOME/.local/share/com.dbx.app" ;;
esac
[ ! -e "$DESKTOP" ] || fail "the scratch HOME has a dbx store already"
# No DBX_DATA_DIR (that is the point), and no XDG_DATA_HOME to point elsewhere.
FIRST="env -u DBX_SECRET_KEY -u DBX_DATA_DIR -u XDG_DATA_HOME HOME=$SCRATCH_HOME DBX_SECRET_KEY_FILE=$KEYFILE $BIN"

start_shell
# Wide, so the long scratch path is on one row.
tm resize-window -t "$SESSION" -x 220 -y 35
sleep 0.3
tm send-keys -t "$SESSION" "$FIRST; echo EXIT=\$?" Enter
wait_for 60-welcome 'Welcome to dbpane'
expect welcome "dbpane's own store"
expect welcome "$OWN"
expect welcome 'Add your first connection: ctrl\+g n'
expect welcome 'Help and all keys: ctrl\+g \?'
expect welcome 'Quit: ctrl\+g q'
expect welcome 'No connections yet'
# The same keys are in the mode bar, as clickable hints.
tail -n 1 "$LAST_CAPTURE" | grep -Eq 'ctrl\+g n new +ctrl\+g \? help +ctrl\+g q quit' || fail "the mode bar lacks the welcome's hints"
[ -f "$OWN/dbx.db" ] || fail "no store was made at $OWN"
[ ! -e "$DESKTOP" ] || fail "dbpane touched the dbx desktop directory"

# Add the connection to the fixture: prefix, n.
key C-g n
wait_for 61-form 'New connection'
typ 'fixture pg'
key Tab; typ '127.0.0.1'
key Tab C-u; typ '55433'
key Tab; typ 'postgres'
key Tab; typ 'dbxtui'
key Tab; typ 'pagila'
wait_for 62-filled 'pagila'
key Enter
wait_for 63-saved 'fixture pg +postgres'
expect_not saved 'Welcome to dbpane'
expect saved 'Results appear here'

# Give the tab a database, then run `select 1`.
key Enter
wait_for 64-databases '▸ pagila'
n=0
until tm capture-pane -e -p -t "$SESSION" | grep -E -- 'pagila' | grep -q '48;2;49;50;68'; do
  key j; sleep 0.12; n=$((n + 1)); [ "$n" -lt 40 ] || { capture 64b; fail "the cursor never reached pagila"; }
done
key u
wait_for 65-used '1 fixture pg / pagila'
key C-g l
wait_for 66-editor 'EDITOR'
typ 'select 1 as one'
key F5
wait_for 67-result '1 row +[0-9<]+ms'
expect result 'one'
key C-g q
wait_for 68-quit 'EXIT=0'
expect_terminal_restored quit

# Second start: the connection is in the same own store; no welcome.
start_shell
tm resize-window -t "$SESSION" -x 220 -y 35
sleep 0.3
tm send-keys -t "$SESSION" "$FIRST; echo EXIT=\$?" Enter
wait_for 69-again 'fixture pg +postgres'
expect again 'Results appear here'
expect_not again 'Welcome to dbpane'
key C-g q
wait_for 70-quit 'EXIT=0'
[ ! -e "$DESKTOP" ] || fail "dbpane touched the dbx desktop directory"

echo "OK first-run; captures in $OUT"
