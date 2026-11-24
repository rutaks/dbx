#!/usr/bin/env bash
# Journey start-and-quit (slice 01): start, see three panes, move focus, zoom,
# resize, quit; the shell must come back with the cursor visible.
# Captures go to $JOURNEY_OUT (default: a fresh temp dir, printed at the end).
. "$(dirname "$0")/lib.sh"

build_bin
scratch_store
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 01-initial 'No connections yet'
expect initial 'connections'
expect initial '1 no target'
expect initial 'results'
expect initial 'SIDEBAR'
expect initial 'ctrl\+g prefix'
[ "$(wc -l <"$LAST_CAPTURE" | tr -d ' ')" -eq 35 ] || fail "initial capture is not 35 rows"
tail -n 1 "$LAST_CAPTURE" | grep -q SIDEBAR || fail "mode bar is not on row 35"
[ "$(tm display -p -t "$SESSION" '#{alternate_on}')" = 1 ] || fail "not on the alternate screen"

# prefix: the mode bar shows the keys that work
tm send-keys -t "$SESSION" C-g
wait_for 02-prefix 'PREFIX'
expect prefix 'h/j/k/l focus'
expect prefix 'z zoom'
expect prefix 'q quit'
expect prefix 'esc back'

# prefix then l: focus moves to the editor
tm send-keys -t "$SESSION" l
wait_for 03-focus-editor 'EDITOR'
expect_not focus 'PREFIX'

# prefix then z: zoom hides the other panes; again restores
tm send-keys -t "$SESSION" C-g z
wait_for 04-zoomed '\(zoom\)'
expect_not zoomed 'connections'
tm send-keys -t "$SESSION" C-g z
wait_for 05-unzoomed 'connections'
expect unzoomed 'results'

# resize while running: the next capture fills the new size
tm resize-window -t "$SESSION" -x 90 -y 28
sleep 0.5
wait_for 06-resized 'EDITOR'
[ "$(wc -l <"$LAST_CAPTURE" | tr -d ' ')" -eq 28 ] || fail "resized capture is not 28 rows"
tail -n 1 "$LAST_CAPTURE" | grep -q 'EDITOR' || fail "mode bar is not on the last of 28 rows"
perl -CSD -ne 'exit 1 if length($_) > 91' "$LAST_CAPTURE" || fail "a row is wider than 90 columns"
grep -q 'results' "$LAST_CAPTURE" || fail "results title missing after resize"
# The app, not just tmux, fills the new width: the divider row reaches column 90.
perl -Mutf8 -CSD -ne 'if (/├─ results ─+$/) { chomp; $len = length($_) } END { exit(($len // 0) == 90 ? 0 : 1) }' "$LAST_CAPTURE" \
  || fail "the divider row does not reach the last column after the resize"

# prefix then q: exit 0, prompt back, screen restored, cursor visible
tm send-keys -t "$SESSION" C-g q
wait_for 07-after-quit 'EXIT=0'
expect after-quit 'PROMPT>'
expect_not after-quit 'No connections yet'
expect_terminal_restored quit

echo "OK start-and-quit; captures in $OUT"
