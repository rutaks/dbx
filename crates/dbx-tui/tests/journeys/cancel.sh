#!/usr/bin/env bash
# Journey cancel (slice 06): cancel a running statement and carry on.
#
# 1. ctrl+c: after `select 1` has given a row, run `select pg_sleep(30)`,
#    cancel it with ctrl+c; the tab says cancelling, then cancelled, and still
#    shows the earlier row marked as the earlier run's; pg_stat_activity
#    (through `docker exec` into the fixture) shows no such statement; the next
#    statement in the same session gives its row.
# 2. The same with the prefix then c, from the sidebar.
# 3. A cancel that does not land: the connection goes through a local proxy
#    that forwards Postgres connections but swallows cancel requests. The tab
#    says cancelling, then that the statement has not stopped yet (and
#    refuses a new run in that session); a second ctrl+c asks before
#    closing the session; yes stops waiting, the statement is (as the tab
#    says) still running on the server until it ends, and the tab runs again
#    at once in a new session.
# 4. A connection with a 1 s query timeout: `select pg_sleep(30)` is timed out,
#    named as the connection's limit, and left running nowhere.
# 5. Quit while a cancel has not landed and the confirm is open (by SIGTERM,
#    since the open question takes the keys): the client exits within its
#    bound, restores the terminal and says on stderr where a statement may
#    still be running. The statement the server never got a
#    cancel for runs to its end (the proxy half-closes the connection, which
#    Postgres does not notice during pg_sleep), and then nothing is left.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
type_sql() { tm send-keys -t "$SESSION" -l "$1"; }
# A `;` alone is a command separator to tmux: it is sent escaped.
semicolon() { tm send-keys -t "$SESSION" '\;'; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"
TAG="dbxtui_cancel_$$"

# running TAG: how many backends are running a statement containing TAG.
running() {
  docker exec dbxtui-fixtures-postgres-1 psql -U postgres -d postgres --no-psqlrc -tA -c \
    "select count(*) from pg_stat_activity where query like '%$1%' and pid <> pg_backend_pid() and state = 'active'"
}
# expect_gone LABEL TAG SECONDS: nothing runs TAG within SECONDS.
expect_gone() {
  local tries=$(( $3 * 10 ))
  while [ "$tries" -gt 0 ]; do
    [ "$(running "$2")" = 0 ] && return 0
    sleep 0.1; tries=$((tries - 1))
  done
  fail "$1: $2 is still running on the server"
}

goto() {
  local n=0
  while [ "$n" -lt 40 ]; do
    if tm capture-pane -e -p -t "$SESSION" | grep -E -- "$1" | grep -q '\[48;'; then return 0; fi
    key j; sleep 0.12; n=$((n + 1))
  done
  capture "goto-failed"; fail "the cursor never reached /$1/"
}

# A proxy to the fixture that never passes on a cancel request (protocol code
# 80877102 at the start of a connection): those connections are held open,
# unanswered, so the client's cancel attempt waits until it gives up.
PROXY_DIR="$(mktemp -d)"
cat >"$PROXY_DIR/proxy.py" <<'PY'
import socket, sys, threading
CANCEL = (80877102).to_bytes(4, "big")
held = []
def pump(src, dst, inspect):
    seen = b""
    try:
        while True:
            data = src.recv(65536)
            if not data:
                break
            if inspect and len(seen) < 64:
                seen += data
                if CANCEL in seen[:64]:
                    held.append((src, dst))
                    return
            dst.sendall(data)
    except OSError:
        pass
    try:
        dst.shutdown(socket.SHUT_WR)
    except OSError:
        pass
def serve(client):
    upstream = socket.create_connection(("127.0.0.1", 55433))
    threading.Thread(target=pump, args=(client, upstream, True), daemon=True).start()
    threading.Thread(target=pump, args=(upstream, client, False), daemon=True).start()
listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", 0))
listener.listen(16)
print(listener.getsockname()[1], flush=True)
while True:
    conn, _ = listener.accept()
    threading.Thread(target=serve, args=(conn,), daemon=True).start()
PY
python3 -I "$PROXY_DIR/proxy.py" >"$PROXY_DIR/port" &
PROXY_PID=$!
trap 'kill $PROXY_PID 2>/dev/null; rm -rf "$PROXY_DIR"; cleanup' EXIT
for _ in $(seq 50); do [ -s "$PROXY_DIR/port" ] && break; sleep 0.1; done
PROXY_PORT="$(cat "$PROXY_DIR/port")"
[ -n "$PROXY_PORT" ] || fail "the proxy did not start"

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 pagila
seed "proxied pg" postgres 127.0.0.1 "$PROXY_PORT" pagila
DBX_TUI_SEED_QUERY_TIMEOUT=1 seed "timeout pg" postgres 127.0.0.1 55433 pagila
start_shell
tm send-keys -t "$SESSION" "sh -c 'echo \$\$ >$OUT/client.pid; exec $CLIENT'; echo EXIT=\$?" Enter
wait_for 01-listed 'timeout pg'

# ---- 1. ctrl+c ---------------------------------------------------------------
key Enter
wait_for 02-databases '▸ pagila'
goto 'pagila'
key u
key C-g l
type_sql "select 1 as earlier_06"
semicolon
key F5
wait_for 03-earlier '1 row'
key Enter
type_sql "select pg_sleep(30) as ${TAG}_key"
key F5
wait_for 10-running 'running 1s' 4
expect running-hint 'ctrl\+c cancel'
[ "$(running "${TAG}_key")" = 1 ] || fail "the statement is not running on the server"
key C-c
capture 11-cancelling
wait_for 12-cancelled 'cancelled after' 4
expect cancelled 'earlier run: select 1 as earlier_06'
expect cancelled '● cancelled after'
expect_not cancelled 'ctrl\+c cancel'
expect_gone ctrl-c "${TAG}_key" 3
semicolon
key Enter
type_sql "select 'after ctrl+c' as after_06"
key F5
wait_for 13-after '1 row' 4
expect after "^ *│ +1 +after ctrl\+c"  # row 1: the gutter comes first
expect_not after 'earlier run'

# ---- 2. the prefix, then c, from the sidebar -----------------------------------
semicolon
key Enter
type_sql "select pg_sleep(30) as ${TAG}_prefix"
key F5
wait_for 20-running 'running 1s' 4
key C-g h
wait_for 21-sidebar-hint 'ctrl\+g c cancel'
key C-g c
wait_for 22-cancelled 'cancelled after' 4
expect_gone prefix-c "${TAG}_prefix" 3
key C-g l
semicolon
key Enter
type_sql "select 'after the prefix' as after_06"
key F5
wait_for 23-after '1 row' 4
expect after-prefix "^ *│ +1 +after the prefix"

# ---- 3. a cancel that does not land ----------------------------------------------
key C-g h
goto 'proxied pg'
key Enter
wait_for 30-proxied-databases 'proxied pg' 4
goto 'pagila'
key u
wait_for 31-proxied-target 'editor · proxied pg / pagila'
key C-g l
semicolon
key Enter
type_sql "select pg_sleep(12) as ${TAG}_deaf"
key F5
wait_for 32-running 'running 1s' 4
key C-c
wait_for 33-cancelling 'cancelling…' 2
wait_for 34-still-running 'not stopped yet' 4
expect still-running 'ctrl\+c stop waiting'
[ "$(running "${TAG}_deaf")" = 1 ] || fail "the swallowed cancel stopped the statement after all"
key F5
wait_for 35-refused 'cancelled statement has not stopped' 2
key C-c
wait_for 36-confirm "Close this tab's session" 2
expect confirm '↵ new session'
expect confirm 'esc keep waiting'
key Enter
wait_for 37-stopped-waiting 'stopped waiting' 2
expect stopped-waiting 'abandoned statement may still be running on proxied pg / pagila'
[ "$(running "${TAG}_deaf")" = 1 ] || fail "expected the statement still running, as the tab says"
semicolon
key Enter
type_sql "select 'fresh_session_06' as after_06"
key F5
wait_for 38-new-session "1 row" 4
expect new-session 'fresh_session_06'
expect_gone deaf "${TAG}_deaf" 12

# ---- 4. the connection's own timeout ------------------------------------------------
key C-g h
goto 'timeout pg'
key Enter
wait_for 40-timeout-databases 'timeout pg' 4
goto 'pagila'
key u
key C-g l
semicolon
key Enter
type_sql "select pg_sleep(30) as ${TAG}_timeout"
key F5
wait_for 41-timed-out 'timed out after' 5
expect timed-out "query timeout of 1 s"
expect_not timed-out 'SQL error'
expect_gone timeout "${TAG}_timeout" 3

# ---- 5. quit while the cancel has not landed and the question is open ------------
key C-g h
key g
goto 'proxied pg'
goto 'pagila'
key u
key C-g l
semicolon
key Enter
type_sql "select pg_sleep(8) as ${TAG}_quit"
key F5
wait_for 50-running 'running 1s' 4
key C-c
wait_for 51-still-running 'not stopped yet' 4
key C-c
wait_for 52-confirm "Close this tab's session" 2
kill -TERM "$(cat "$OUT/client.pid")"
wait_for 53-quit 'EXIT=0' 4
expect quit 'dbx-tui: A statement may still be running on proxied pg / pagila\.'
expect_terminal_restored quit
expect_gone quit "$TAG" 9

echo "OK cancel; captures in $OUT"
