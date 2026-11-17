#!/usr/bin/env bash
# Journey browse-pagila (slice 04): against a scratch store holding the Docker
# fixture connection, expand it to `pagila`, `public` and `film`; the capture
# shows `film_id` with its key marker and `rating` with its enum type;
# collapse; quit. Also: a connection that fails shows dbx's message under it
# and `r` retries; one that hangs shows `loading…`, and collapsing it gives up;
# a click on a mode-bar hint does what its key does; quitting with a listing in
# flight against a server that never answers restores the terminal promptly.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

# goto REGEX: press j until the cursor row (the one drawn on a background) matches.
goto() {
  local n=0
  while [ "$n" -lt 80 ]; do
    if tm capture-pane -e -p -t "$SESSION" | grep -E -- "$1" | grep -q '\[48;'; then return 0; fi
    key j; sleep 0.12; n=$((n + 1))
  done
  capture "goto-failed"; fail "the cursor never reached /$1/"
}

# click_hint TEXT: left-click the mode bar (last row) where TEXT is, as an SGR mouse report.
click_hint() {
  capture click-source
  local bar col
  bar="$(tail -n 1 "$LAST_CAPTURE")"
  col="$(awk -v t="$1" '{print index($0, t)}' <<<"$bar")"
  [ "${col:-0}" -gt 0 ] || fail "no hint /$1/ on the mode bar: $bar"
  local rows; rows="$(wc -l <"$LAST_CAPTURE" | tr -d ' ')"
  tm send-keys -t "$SESSION" -l "$(printf '\033[<0;%d;%dM\033[<0;%d;%dm' "$((col + 1))" "$rows" "$((col + 1))" "$rows")"
}

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 pagila
seed "dead" postgres 127.0.0.1 1 pagila
# A server that accepts and never answers, for a listing that hangs.
SILENT=55998
nc -z 127.0.0.1 $SILENT 2>/dev/null && fail "port $SILENT is in use"
python3 -I -c '
import socket, sys
server = socket.socket()
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind(("127.0.0.1", int(sys.argv[1])))
server.listen(16)
held = []
while True:
    held.append(server.accept()[0])
' $SILENT &
SILENT_PID=$!
trap 'kill $SILENT_PID 2>/dev/null; cleanup' EXIT
sleep 0.5
seed "silent" postgres 127.0.0.1 $SILENT pagila
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 70-listed 'silent'
expect listed '▸ ○ dead'
expect listed '▸ ○ local pg'
expect listed 'enter expand'
expect listed 'r refresh'
expect listed 'e edit'
expect listed 'ctrl\+g n new'

# A click on a mode-bar hint does what its key does: `e edit` opens the form.
click_hint 'e edit'
wait_for 71-click-edit 'Edit connection'
key Escape
sleep 0.3
capture 72-click-closed
expect_not click-closed 'Edit connection'

# A connection that refuses: dbx's message under the node, red glyph; r retries.
key Enter
wait_for 73-failed 'refused|failed'
expect failed '▾ ● dead'
expect failed 'h collapse'
key r
sleep 1
capture 74-retried
expect retried 'connection failed|refused|error'
key h
sleep 0.3
capture 75-failed-collapsed
expect_not failed-collapsed 'connection failed|refused'
expect failed-collapsed '▸ ● dead'

# The fixture: connection, database, schema, table.
key j
goto 'local pg'
key Enter
wait_for 76-databases '▸ edge'
expect databases '▾ ● local pg'
expect databases 'postgres'
expect databases 'edge'
goto 'pagila'
key Enter
wait_for 77-schemas '▸ public'
expect schemas 'editor · local pg / pagila'
goto 'public'
key Enter
wait_for 78-tables 'film_list +view'
expect tables 'actor'
expect tables 'film_list +view'
expect tables 'rental_by_.* +matview'
expect_not tables 'pg_catalog|information_schema'
expect_not tables 'payment_p2022'
expect tables 'payment( |$)'
goto 'film([^_a-z]|$)'
key Enter
wait_for 79-columns '◆ film_id'
expect columns '◆ film_id +integer'
expect columns 'rating +mpaa_rating \?'
expect columns 'title +text'
expect columns 'special.* +text\[\] \?'
expect_not columns 'title +text \?'
# The look: the key marker is on the key column only.
[ "$(grep -c '◆' "$LAST_CAPTURE")" -eq 1 ] || fail "one key marker expected"
# the tree is longer than the pane: the scrollbar is drawn
expect columns '▕'

# a long list scrolled to its end: the top rows are gone, the bar remains
key G
sleep 0.3
capture 79b-scrolled
expect scrolled '▕'
expect scrolled 'silent'
expect_not scrolled '▸ ● dead'

# collapse: the table closes, then h goes up a level at a time
key g
goto 'film([^_a-z]|$)'
key h
sleep 0.3
capture 80-collapsed
expect_not collapsed 'film_id'
expect collapsed 'film_list'
key h
sleep 0.3
capture 81-parent
expect parent 'h collapse'

# A listing that hangs shows `loading…`; collapsing it gives up.
key g
key j
key h
goto 'silent'
key Enter
wait_for 82-loading 'loading…'
expect loading '▾ ● silent|▾ ○ silent'
key h
sleep 0.3
capture 83-gave-up
expect_not gave-up 'loading…'
expect gave-up '▸ ○ silent'

# Quitting with a listing still in flight against a server that never
# answers: the terminal is restored at once and the process exits, without
# waiting for the listing (the cursor is still on `silent`).
key Enter
wait_for 83b-in-flight 'loading…'
key C-g q
wait_for 84-quit 'EXIT=0' 3
expect_terminal_restored quit

if grep -l -e "$DBX_TUI_SEED_PASSWORD" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen"
fi
echo "OK browse-pagila; captures in $OUT"
