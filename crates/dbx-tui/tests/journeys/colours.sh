#!/usr/bin/env bash
# Journey colours (slice 10): against a scratch store holding the Docker
# fixture connection, give the tab `pagila`, type two statements in the editor
# and check, in the escape sequences tmux reports (`capture-pane -e`), that the
# keyword cells carry another colour from the identifier cells, that the gutter
# bar (the statement that would run) is on the second statement, and that it
# moves to the first when the caret does. Quit; the terminal is restored.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

# fg_of LINE_REGEX WORD: the foreground escape in force on the first cell of
# WORD in the first screen line (of the last capture with escapes) matching
# LINE_REGEX. Prints it; prints nothing when there is no such line or word.
fg_of() {
  python3 -I - "$ESC_CAPTURE" "$1" "$2" <<'PY'
import re, sys
path, line_re, word = sys.argv[1:4]
sgr = re.compile(r'\x1b\[([0-9;]*)m')
for raw in open(path, encoding='utf-8'):
    plain = sgr.sub('', raw.rstrip('\n'))
    if not re.search(line_re, plain):
        continue
    at = plain.find(word)
    if at < 0:
        continue
    fg, pos, i = '', 0, 0
    while i < len(raw):
        m = sgr.match(raw, i)
        if m:
            codes = m.group(1)
            if codes in ('', '0'):
                fg = ''
            else:
                for part in re.findall(r'38;2;\d+;\d+;\d+|38;5;\d+|\b3[0-7]\b|\b9[0-7]\b|\b39\b', codes):
                    fg = '' if part == '39' else part
            i = m.end()
            continue
        if pos == at:
            print(fg)
            sys.exit(0)
        pos += 1
        i += 1
PY
}
# esc_capture NAME: the pane with its escape sequences.
esc_capture() {
  ESC_CAPTURE="$OUT/$1.esc"
  tm capture-pane -e -p -t "$SESSION" >"$ESC_CAPTURE"
  capture "$1"
}
# bar_text: the text of the screen line that carries the gutter bar.
bar_text() { grep -F '▎' "$LAST_CAPTURE" | head -1; }

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 pagila
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 00-listed 'local pg'
key Enter
wait_for 01-databases '▸ pagila'
n=0
while [ "$n" -lt 40 ]; do
  tm capture-pane -e -p -t "$SESSION" | grep -E -- 'pagila' | grep -q '48;2;49;50;68' && break
  key j; sleep 0.12; n=$((n + 1))
done
key u
wait_for 02-used 'editor · local pg / pagila'
key C-g l
wait_for 03-focused 'EDITOR'

tm send-keys -t "$SESSION" -l 'select film_id from film'
tm send-keys -t "$SESSION" -H 3b
key Enter
tm send-keys -t "$SESSION" -l 'select actor_id from actor'
tm send-keys -t "$SESSION" -H 3b
wait_for 04-typed 'select actor_id from actor'
esc_capture 04-typed

kw="$(fg_of 'select film_id from film' 'select')"
id="$(fg_of 'select film_id from film' 'film_id')"
from="$(fg_of 'select film_id from film' 'from')"
[ -n "$kw" ] || fail "no colour found on the keyword select: $(cat "$ESC_CAPTURE" | head -5)"
[ -n "$id" ] || fail "no colour found on the identifier film_id"
[ "$kw" != "$id" ] || fail "the keyword and the identifier have the same colour ($kw)"
[ "$kw" = "$from" ] || fail "select and from differ ($kw, $from)"
echo "keyword colour $kw, identifier colour $id"

# The caret is in the second statement: the bar is on its row, not the first.
expect typed '1 .*select film_id from film|^ *1'
bar="$(bar_text)"
case "$bar" in *actor_id*) ;; *) fail "the bar is not on the second statement: [$bar]";; esac

# Up to the first statement: the bar moves to it.
key Up
sleep 0.4
capture 05-up
bar="$(bar_text)"
case "$bar" in *film_id*) ;; *) fail "the bar did not move to the first statement: [$bar]";; esac
case "$bar" in *actor_id*) fail "the bar is still on the second statement: [$bar]";; esac

key C-g q
wait_for 06-quit 'EXIT=0'
expect_terminal_restored quit
echo "PASS colours"
