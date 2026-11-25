#!/usr/bin/env bash
# Journey browse-table (slice 16): on `pagila`, `o` on the table `rental`
# (16,044 rows) opens a tab named `rental` headed `rows 1–1,000`; `]` shows
# `rows 1,001–2,000` and `[` goes back; `#` counts and the header says
# `of 16,044`; `[` on the first page does nothing; `o` on the table again goes
# to its tab; editing the select and running it makes an ordinary query tab
# (the paging keys leave the mode bar). Quits cleanly.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

# goto REGEX: press j until the cursor row (the one drawn on a background) matches.
goto() {
  local n=0
  while [ "$n" -lt 80 ]; do
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

# Down to the table `rental`: connection, database, schema.
goto 'local pg'
key Enter
wait_for 01-databases '▸ pagila'
goto 'pagila'
key Enter
wait_for 02-schemas '▸ public'
goto 'public'
key Enter
wait_for 03-tables 'rental'
goto 'rental([^_a-z]|$)'
# The hint is on a table row, and only there.
wait_for 04-hint 'o browse'

# 1. `o` opens the table in a tab of its own, and its first page runs.
key o
wait_for 05-opened 'rental · rows 1–1,000' 15
expect opened '2 rental'
expect opened 'film_id|rental_id'
expect opened '\] next page'
expect opened '# count'
expect_not opened '\[ prev page'
expect_not opened 'no primary key'
expect_not opened ' more'

# 2. `]` shows the next page, `[` goes back.
key ']'
wait_for 06-page-2 'rental · rows 1,001–2,000' 10
expect page-2 '\[ prev page'
key '['
wait_for 07-page-1 'rental · rows 1–1,000' 10
# `[` on the first page does nothing.
key '['
sleep 0.5
capture 08-still-first
expect still-first 'rental · rows 1–1,000'

# 3. `#` counts, on request: the header says `of 16,044` once it is known.
key '#'
wait_for 09-counted 'rental · rows 1–1,000 of 16,044' 10
expect_not counted '# count'

# 4. The last page is short: `]` goes there and then stops.
for _ in $(seq 1 16); do key ']'; sleep 0.4; done
wait_for 10-last 'rental · rows 16,001–16,044 of 16,044' 10
expect_not last '\] next page'

# 5. `o` on the table again goes to its tab; no third tab.
key C-g h
wait_for 11-sidebar 'o browse'
key o
sleep 0.5
capture 12-again
expect again '1 no target|1 local pg'
expect_not again '3 rental'
expect again '2 rental'

# 6. Editing the text and running it makes an ordinary query tab.
key C-g k
tm send-keys -t "$SESSION" -l -- '-- mine '
key F5
wait_for 13-ran-edited 'local pg / pagila|rental' 10
key C-g j
sleep 0.5
capture 14-ordinary
expect_not ordinary '\] next page'
expect_not ordinary '\[ prev page'
expect_not ordinary 'rental · rows'

key C-g q
wait_for 15-quit 'EXIT=0' 4
expect_terminal_restored quit

if grep -l -e "$DBX_TUI_SEED_PASSWORD" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen"
fi
echo "OK browse-table; captures in $OUT"
