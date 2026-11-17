#!/usr/bin/env bash
# Journey store-error (slice 02): a store that cannot be read opens the
# blocking Error overlay, which says what to do; panes take no input behind
# it; retry fails again; q quits with the terminal restored.
. "$(dirname "$0")/lib.sh"

build_bin
scratch_store
printf 'this is not a sqlite database\n' >"$STORE/dbx.db"
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 40-overlay 'Store unavailable'
expect overlay 'not a valid SQLite'
expect overlay 'DBX_DATA_DIR'
expect overlay '↵ retry'
expect overlay 'q quit'

# Behind the overlay nothing moves: prefix then l would focus the editor.
tm send-keys -t "$SESSION" C-g l
sleep 0.3
capture 41-still-blocked
expect still-blocked 'Store unavailable'
expect still-blocked '^ SIDEBAR'
expect_not still-blocked 'EDITOR'

tm send-keys -t "$SESSION" Enter
sleep 0.5
wait_for 42-retried 'Store unavailable'

tm send-keys -t "$SESSION" q
wait_for 43-after-quit 'EXIT=0'
expect_terminal_restored quit
echo "OK store-error; captures in $OUT"
