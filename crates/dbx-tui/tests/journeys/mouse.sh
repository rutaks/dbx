#!/usr/bin/env bash
# Journey mouse (slice 09): against a scratch store holding the Docker fixture
# connection, drive the client with SGR mouse reports written to the pane with
# `tmux send-keys -l` (tmux injects mouse events with `send-keys -M` only
# inside key tables, which is awkward; the bytes an application receives are
# the same). Drag the sidebar divider and see it move; resize from the keyboard
# (prefix, R); double-click the divider to reset it; double-click a connection
# and a database to open them; click a table, put the caret in the editor with
# a click, run a query, click a cell (the title row says where the cursor is),
# double-click it to open the viewer, close it with a click; the wheel scrolls
# the results while the editor keeps focus; drag the results scrollbar; drag the
# editor/results divider; quit, and see the layout come back on the next start.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

# type_sql TEXT: types TEXT literally (a `;` is sent as its byte: tmux reads
# an argument ending in `;` as a command separator).
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

# pos REGEX: "COLUMN ROW" (1-based, as SGR reports count) of the first match on
# the last capture, or fail.
pos() {
  python3 -I -c '
import re, sys
rx = re.compile(sys.argv[1])
for y, line in enumerate(open(sys.argv[2], encoding="utf-8").read().split("\n"), 1):
    m = rx.search(line)
    if m:
        print(m.start() + 1, y)
        sys.exit(0)
sys.exit(1)
' "$1" "$LAST_CAPTURE" || { capture "pos-failed"; fail "no /$1/ on screen"; }
}

# SGR mouse reports: button, then 1-based column and row.
sgr() { tm send-keys -t "$SESSION" -l "$(printf "$@")"; }
press()   { sgr '\033[<0;%d;%dM' "$1" "$2"; }
release() { sgr '\033[<0;%d;%dm' "$1" "$2"; }
motion()  { sgr '\033[<32;%d;%dM' "$1" "$2"; }
click()   { press "$1" "$2"; release "$1" "$2"; sleep 0.15; }
# Two presses inside the double-click window (350 ms): both reports are written at once.
dclick()  { sgr '\033[<0;%d;%dM\033[<0;%d;%dm\033[<0;%d;%dM\033[<0;%d;%dm' "$1" "$2" "$1" "$2" "$1" "$2" "$1" "$2"; sleep 0.3; }
dragto()  { press "$1" "$2"; motion "$3" "$4"; release "$3" "$4"; sleep 0.3; }
wheel_down() { sgr '\033[<65;%d;%dM' "$1" "$2"; sleep 0.15; }

# col_of_separator: the 1-based column of the sidebar's `│` (first row).
sep_col() { capture "sep"; pos '│' | cut -d' ' -f1; }

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 pagila
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 00-listed 'local pg'
expect listed 'connections'
start_sep="$(sep_col)"
echo "sidebar separator starts at column $start_sep"

# ---- dividers ----
# Drag the sidebar divider 14 columns to the right.
dragto "$start_sep" 3 "$((start_sep + 14))" 3
now_sep="$(sep_col)"
[ "$now_sep" = "$((start_sep + 14))" ] || fail "dragging moved the divider from $start_sep to $now_sep, wanted $((start_sep + 14))"
# Dragged far left: it stops at its minimum (14 columns, separator included).
dragto "$now_sep" 3 2 3
[ "$(sep_col)" = 14 ] || fail "minimum: the divider is at column $(sep_col), wanted 14"
# A double-click on it puts it back.
dclick 14 3
[ "$(sep_col)" = "$start_sep" ] || fail "double-click: the divider is at column $(sep_col), wanted $start_sep"

# Resize from the keyboard: prefix, R, l three times; the mode bar says so.
key C-g R
wait_for 10-resize-mode 'RESIZE'
expect resize-mode 'h/l sidebar'
expect resize-mode 'j/k split'
expect resize-mode 'esc done'
key l l l
sleep 0.3
[ "$(sep_col)" = "$((start_sep + 3))" ] || fail "resize mode: the divider is at column $(sep_col), wanted $((start_sep + 3))"
key Escape
sleep 0.3
capture 11-resize-left
expect_not resize-left 'RESIZE'
expect resize-left 'SIDEBAR'
dclick "$((start_sep + 3))" 3
[ "$(sep_col)" = "$start_sep" ] || fail "reset after the keyboard resize"

# ---- the tree ----
# Double-click a connection, then a database: they open.
read -r c r < <(pos 'local pg')
dclick "$c" "$r"
wait_for 20-databases '▸ pagila'
read -r c r < <(pos 'pagila')
dclick "$c" "$r"
wait_for 21-schemas '▸ public'
read -r c r < <(pos 'public')
dclick "$c" "$r"
wait_for 22-tables 'film_list'
# Click a table, then `u` gives its database to the editor.
read -r c r < <(pos '▸ film +│')
click "$((c + 3))" "$r"
capture 23-table-selected
expect table-selected 'u use'
key u
wait_for 24-used 'editor · local pg / pagila'

# ---- the editor ----
# A click puts the caret in the editor and focuses it.
read -r c r < <(pos 'Type SQL')
click "$((c + 2))" "$r"
wait_for 30-editor-focused 'EDITOR'
type_sql 'select film_id, title from film order by film_id limit 5'
key F5
wait_for 31-five-rows 'ADAPTATION HOLES'
# A click inside the text places the caret there: typing goes in at that column.
read -r c r < <(pos 'select film_id, title from film')
click "$((c + 6))" "$r"
type_sql 'X'
capture 32-caret
expect caret 'selectX film_id'
key BSpace

# ---- the grid ----
read -r c r < <(pos 'ADAPTATION')
click "$((c + 2))" "$r"
wait_for 40-cell 'r 3/5  c 2/2'
expect cell 'RESULTS'
# A double-click opens the cell in the viewer; a click on `esc close` closes it.
dclick "$((c + 2))" "$r"
wait_for 41-viewer 'esc close'
expect viewer 'ADAPTATION HOLES'
read -r c r < <(pos 'esc close')
click "$c" "$r"
sleep 0.3
capture 42-viewer-closed
expect_not viewer-closed 'esc close'

# ---- the wheel and the scrollbar ----
# Back to the editor with a click, run a longer result.
read -r c r < <(pos 'select film_id, title from film')
click "$((c + 60))" "$r"
wait_for 50-editor-again 'EDITOR'
type_sql ';'
key Enter Enter
type_sql 'select generate_series(1, 300) as n;'
key F5
wait_for 51-300-rows '300 rows'
# The wheel over the results scrolls them while the editor keeps focus.
read -r c r < <(pos ' results ')
for _ in 1 2 3 4 5; do wheel_down "$((c + 10))" "$((r + 4))"; done
capture 52-wheeled
expect wheeled 'r 16/300'
expect wheeled 'EDITOR'
# A click in the scrollbar column (the right edge) near the bottom jumps near the end.
cols="$(tm display -p -t "$SESSION" '#{pane_width}')"
rows="$(tm display -p -t "$SESSION" '#{pane_height}')"
click "$cols" "$((rows - 2))"
capture 53-scrollbar
expect scrollbar 'r 2[6-9][0-9]/300'
expect scrollbar 'RESULTS'

# ---- the editor/results divider ----
read -r c r < <(pos ' results ')
dragto "$((c + 20))" "$r" "$((c + 20))" "$((r + 4))"
capture 54-split-dragged
read -r c2 r2 < <(pos ' results ')
[ "$r2" = "$((r + 4))" ] || fail "the editor/results divider moved from row $r to $r2, wanted $((r + 4))"

# ---- the layout is kept ----
dragto "$start_sep" 3 "$((start_sep + 10))" 3
[ "$(sep_col)" = "$((start_sep + 10))" ] || fail "the second drag"
key C-g q
wait_for 60-quit 'EXIT=0' 3
expect_terminal_restored quit
prefs="$STORE/dbx-tui-prefs.json"
[ -f "$prefs" ] || fail "no preferences file at $prefs"
grep -q 'sidebar_permille' "$prefs" || fail "the sidebar proportion was not saved: $(cat "$prefs")"
grep -q 'editor_permille' "$prefs" || fail "the editor proportion was not saved: $(cat "$prefs")"
# The next start comes back as it was left.
tm send-keys -t "$SESSION" "clear; $CLIENT; echo EXIT=\$?" Enter
wait_for 61-restarted 'local pg'
[ "$(sep_col)" = "$((start_sep + 10))" ] || fail "restart: the divider is at column $(sep_col), wanted $((start_sep + 10))"
# A corrupt file means the defaults, with no error on screen.
key C-g q
wait_for 62-quit-again 'EXIT=0' 3
echo '{ not json' >"$prefs"
tm send-keys -t "$SESSION" "clear; $CLIENT; echo EXIT=\$?" Enter
wait_for 63-corrupt 'local pg'
[ "$(sep_col)" = "$start_sep" ] || fail "a corrupt preferences file: the divider is at column $(sep_col), wanted the default $start_sep"
expect_not corrupt 'rror|nvalid|corrupt'
key C-g q
wait_for 64-final-quit 'EXIT=0' 3
expect_terminal_restored final-quit

if grep -l -e "$DBX_TUI_SEED_PASSWORD" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen"
fi
echo "OK mouse; captures in $OUT"
