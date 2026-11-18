#!/usr/bin/env bash
# Journey list-connections (slice 02): start against a scratch store seeded
# with two connections, see both in the sidebar with their kind, move the
# cursor, quit; the shell comes back restored.
. "$(dirname "$0")/lib.sh"

build_bin
build_example seed_store
scratch_store
seed "local pg" postgres 127.0.0.1 55433 pagila
seed "shop" mysql 127.0.0.1 3306 shop
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 30-listed 'shop'
expect listed '○ local pg +postgres'
expect listed '○ shop +mysql'
expect_not listed 'loading'
expect listed 'j/k move'

# The cursor row is drawn on selection_bg: the escape-coded capture shows the
# background moving from the first row to the second.
row_bg() { tm capture-pane -e -p -t "$SESSION" | grep -- "$1" | grep -c '48;2;49;50;68' || true; }
[ "$(row_bg 'local pg')" -ge 1 ] || fail "cursor not on the first row"
[ "$(row_bg 'shop')" -eq 0 ] || fail "second row highlighted before moving"
tm send-keys -t "$SESSION" j
sleep 0.3
capture 31-cursor-moved
[ "$(row_bg 'shop')" -ge 1 ] || fail "cursor did not move to the second row"
[ "$(row_bg 'local pg')" -eq 0 ] || fail "first row still highlighted"
tm send-keys -t "$SESSION" k
sleep 0.3
[ "$(row_bg 'local pg')" -ge 1 ] || fail "cursor did not move back up"

tm send-keys -t "$SESSION" C-g q
wait_for 32-after-quit 'EXIT=0'
expect after-quit 'PROMPT>'
expect_terminal_restored quit
echo "OK list-connections; captures in $OUT"
