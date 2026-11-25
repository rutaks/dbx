#!/usr/bin/env bash
# Journey export (slice 19): against a scratch store holding the Docker fixture
# connection (database `edge`), with the client started in a scratch directory
# (so every default path is in it, and the typed ones are relative to it):
#   1. read `text_values` (NULL, '', 'NULL', a tab, a multi-line value) and open
#      the export overlay with `e`: five formats, the default path in the
#      scratch directory, the losses of CSV;
#   2. `enter` writes the CSV: the notice says so, and the file holds NULL as
#      \N and '' as "";
#   3. a path that is there already asks first, naming it; `enter` is no yes;
#      `esc` goes back and the file is as it was; `y` replaces it;
#   4. a directory that is not there is refused by name, the overlay stays;
#   5. JSON and Markdown files, and copy as Markdown (`C`, `3`);
#   6. a row copy that is lossy says so, and a plain one does not;
#   7. quit; the terminal is restored; nothing was written but the files above.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
# type_text TEXT: types TEXT literally, a `;` as its byte (tmux reads an
# argument ending in `;` as a command separator).
type_text() {
  local rest="$1" part
  while [ -n "$rest" ]; do
    part="${rest%%;*}"
    [ -n "$part" ] && tm send-keys -t "$SESSION" -l "$part"
    [ "$part" = "$rest" ] && break
    tm send-keys -t "$SESSION" -H 3b
    rest="${rest#*;}"
  done
}
# wait_gone NAME REGEX [SECONDS]: until REGEX is no longer on screen (a notice
# lasts a few seconds), so the next wait cannot be answered by the last one.
wait_gone() {
  local name="$1" re="$2" tries=$(( ${3:-8} * 10 ))
  while [ "$tries" -gt 0 ]; do
    capture "$name"
    grep -Eq -- "$re" "$LAST_CAPTURE" || return 0
    sleep 0.1; tries=$((tries - 1))
  done
  fail "still on screen: /$re/ ($name)"
}
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 edge
start_shell

X="$OUT/exports"
rm -rf "$X"; mkdir -p "$X"
tm send-keys -t "$SESSION" "cd '$X' && $CLIENT; echo EXIT=\$?" Enter
wait_for 00-listed 'local pg'
key u
wait_for 01-used '1 local pg / edge'
key C-g l
wait_for 02-editor 'EDITOR'
type_text 'select id, label, v from text_values order by id'
key F5
wait_for 03-rows '8 rows'
key C-g j
wait_for 04-results 'RESULTS'
expect hints 'e export'
expect hints 'C copy as'

# ---- 1. the overlay -------------------------------------------------------------
key e
wait_for 05-overlay 'Export result'
expect overlay '● CSV'
expect overlay '○ JSON'
expect overlay '○ Excel'
expect overlay '○ Markdown'
expect overlay '○ SQL INSERT'
expect overlay "result-[0-9]{8}-[0-9]{6}\.csv"
expect overlay 'Writes the 8 rows held'
expect overlay 'NULL is written as'
expect overlay "gets a ' in front"

# ---- 2. writing the CSV ---------------------------------------------------------
key Enter
wait_for 06-exported 'Exported 8 rows to' 10
expect_not exported-overlay 'Export result'
CSV="$(ls "$X"/result-*.csv 2>/dev/null | head -n1)"
[ -n "$CSV" ] || fail "no CSV file in $X"
[ "$(ls "$X" | wc -l | tr -d ' ')" = 1 ] || fail "more than the one file in $X: $(ls "$X")"
head -n1 "$CSV" | grep -Fxq '"id","label","v"' || fail "the CSV header: $(head -n1 "$CSV")"
grep -Fq '"1","null",\N' "$CSV" || fail "NULL is not \\N in the CSV: $(sed -n 2p "$CSV")"
grep -Fq '"empty",""' "$CSV" || fail "the empty string is not \"\" in the CSV: $(sed -n 3p "$CSV")"
grep -Fq '"text NULL","NULL"' "$CSV" || fail "the text NULL is not itself in the CSV: $(sed -n 4p "$CSV")"
wait_gone 06b-notice-gone 'Exported 8 rows to'

# ---- 3. a file that is there already ---------------------------------------------
printf 'mine' >"$X/mine.txt"
key e
wait_for 07-overlay-again 'Export result'
key Tab C-u
type_text "mine.txt"
key Enter
wait_for 08-replace 'Replace the file\?'
expect replace 'mine\.txt'
expect replace 'y replace'
key Enter
sleep 0.3
capture 08b-enter-is-no
expect enter-is-no 'Replace the file\?'
[ "$(cat "$X/mine.txt")" = mine ] || fail "enter replaced the file"
key Escape
wait_for 09-back 'Export result'
expect_not back 'Replace the file\?'
expect back 'mine\.txt'
[ "$(cat "$X/mine.txt")" = mine ] || fail "esc replaced the file"
key Enter
wait_for 10-replace-again 'Replace the file\?'
key y
wait_for 11-replaced 'Exported 8 rows to mine\.txt' 10
head -n1 "$X/mine.txt" | grep -Fxq '"id","label","v"' || fail "the file was not replaced: $(head -c 80 "$X/mine.txt")"
wait_gone 11b-notice-gone 'Exported 8 rows to'

# ---- 4. a directory that is not there --------------------------------------------
key e
wait_for 12-overlay-third 'Export result'
key Tab C-u
type_text "nope/out.csv"
key Enter
wait_for 13-no-directory 'Not written to' 10
expect no-directory 'Export result'
expect no-directory 'nope'
[ ! -e "$X/nope" ] || fail "the directory was made"
key Escape
wait_for 14-closed 'RESULTS'
expect_not closed 'Export result'

# ---- 5. JSON and Markdown, and copy as Markdown ------------------------------------
key e
wait_for 15-overlay-json 'Export result'
key 2
key Tab C-u
type_text "rows.json"
key Enter
wait_for 16-json 'Exported 8 rows to rows\.json' 10
grep -Fq '"v": null' "$X/rows.json" || fail "NULL is not null in the JSON"
grep -Fq '"v": ""' "$X/rows.json" || fail "the empty string is not \"\" in the JSON"
grep -Fq '"v": "NULL"' "$X/rows.json" || fail "the text NULL is not itself in the JSON"
wait_gone 16b-notice-gone 'Exported 8 rows to'
key e
wait_for 17-overlay-md 'Export result'
key 4
capture 17b-md-losses
expect md-losses 'line breaks become <br>'
expect_not md-losses "gets a ' in front"
key Tab C-u
type_text "rows.md"
key Enter
wait_for 18-md 'Exported 8 rows to rows\.md' 10
grep -Fq '| NULL' "$X/rows.md" || fail "the Markdown has no NULL cell"
wait_gone 18b-notice-gone 'Exported 8 rows to'
key C
wait_for 19-copy-as 'Copy as'
expect copy-as '1 CSV'
expect copy-as '3 Markdown'
expect_not copy-as 'Excel'
key 3
wait_for 20-copied 'Sent Markdown \(8 rows\)'
wait_gone 20b-notice-gone 'Sent Markdown'

# ---- 6. a lossy row ------------------------------------------------------------------
# Row 5 holds a tab. Row 3 is the text NULL: nothing lossy.
key j j j j
key Y
wait_for 21-lossy 'Lossy:'
wait_gone 21b-notice-gone 'Lossy:'
key g j j
key Y
wait_for 22-plain 'Sent row to the terminal clipboard \(16 B\)'
expect_not plain 'Lossy'

# ---- 7. nothing else was written -------------------------------------------------------
key C-g q
wait_for 23-quit 'EXIT=0' 3
expect_terminal_restored quit
FILES="$(ls "$X" | sort | tr '\n' ' ')"
case "$FILES" in
  "mine.txt result-"*".csv rows.json rows.md ") ;;
  *) fail "the files in $X: $FILES" ;;
esac
ls -a "$X" | grep -q '\.tmp$' && fail "a temporary file was left in $X"
echo "OK export; captures in $OUT"
