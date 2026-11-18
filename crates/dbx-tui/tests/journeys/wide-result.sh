#!/usr/bin/env bash
# Journey wide-result (slice 07): against a scratch store holding the Docker
# fixture connection (database `edge`):
#   1. select over the 120-column table, move right 60 columns and down five
#      pages, capture: every visible header names the column its cells belong to
#      (each value encodes its own column and row), and the gutter's row
#      number is the row the values encode;
#   2. a long multi-line value in the cell viewer, then a JSON cell pretty and
#      raw, and NULL;
#   3. wide characters: columns stay aligned by display width;
#   4. load more on the million-row table: the loading state with the rows so
#      far still navigable, then the cap doubled (`2,000+`) with the cursor on
#      the same row;
#   5. quit; the terminal is restored.
# Needs the fixtures: tests/fixtures/up.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
# type_sql TEXT: types TEXT literally. tmux reads an argument ending in `;` as
# a command separator, so each `;` is sent as its byte (0x3b).
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
# repeat N KEY: presses KEY N times in one go.
repeat() {
  local n="$1" k="$2" args=() i
  for ((i = 0; i < n; i++)); do args+=("$k"); done
  key "${args[@]}"
}
# run_sql SQL: focus the editor, replace its text, run it.
run_sql() {
  key C-g k
  key C-a
  # select all, delete: shift+ctrl+end from the start of the buffer
  key C-Home S-C-End BSpace
  type_sql "$1"
  key F5
}
nc -z 127.0.0.1 55433 2>/dev/null || fail "the fixture Postgres is not up: run tests/fixtures/up.sh"

build_bin
build_example seed_store
scratch_store
export DBX_TUI_SEED_PASSWORD=dbxtui
seed "local pg" postgres 127.0.0.1 55433 edge
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 00-listed 'local pg'
key u
wait_for 01-used 'editor · local pg / edge'

# ---- 1. the 120-column table ----------------------------------------------------
key C-g l
type_sql 'select * from wide_120 order by id;'
key F5
wait_for 02-wide-first 'c001'
expect wide-first '400 rows'
expect wide-first 'c 1/121'
key C-g j
wait_for 03-results-focused 'RESULTS'
expect results-focused 'enter view'
repeat 60 l
repeat 5 PageDown
wait_for 04-wide-scrolled 'c 61/121'
expect wide-scrolled 'c060'
expect wide-scrolled 'r [0-9]+/400'
expect_not wide-scrolled 'c001'
cp "$LAST_CAPTURE" "$OUT/wide-scrolled.txt" 2>/dev/null || true
python3 -I - "$LAST_CAPTURE" <<'PY' || fail "the visible headers do not name the columns of the cells under them"
import re, sys
lines = open(sys.argv[1], encoding="utf-8").read().split("\n")
def pane(line):
    return line.split("│", 1)[1] if "│" in line else ""
def tokens(text, start=0):
    return [(start + m.start(), start + m.end(), m.group()) for m in re.finditer(r"\S+(?: \S+)*", text[start:])]
header = None
for i, line in enumerate(lines):
    toks = [t for t in tokens(pane(line)) if re.fullmatch(r"c\d{3}", t[2])]
    if len(toks) >= 3:
        header, hdr_line = toks, i
        break
assert header, "no header row with three or more column names"
cols = [int(t[2][1:]) for t in header]
assert cols == list(range(cols[0], cols[0] + len(cols))), f"header columns are not consecutive: {cols}"
assert cols[0] >= 40 and 60 in cols, f"not scrolled to column 60: {cols}"
def decode(token):
    """(column, row) a cell value encodes."""
    m = re.fullmatch(r"c(\d{3})-r(\d{3})", token)
    if m: return int(m[1]), int(m[2])
    m = re.fullmatch(r"(\d+)\.50", token)
    if m: v = int(m[1]); return v // 1000, v % 1000
    m = re.fullmatch(r'\{"c": (\d+), "r": (\d+)\}', token)
    if m: return int(m[1]), int(m[2])
    m = re.fullmatch(r"(\d+)", token)
    if m: v = int(m[1]); return v // 1000, v % 1000
    return None
checked = 0
for line in lines[hdr_line + 1:]:
    text = pane(line)
    g = re.match(r"\s*(\d+) ", text)
    if not g:
        continue
    gutter = int(g[1])
    # The gutter is one space from the first column: tokens start after it.
    toks = tokens(text, g.end())
    for (hs, he, name) in header:
        over = [t for t in toks if t[0] < he and t[1] > hs]
        assert over, f"row {gutter}: nothing under {name}"
        got = decode(over[0][2])
        assert got is not None, f"row {gutter}: cannot read {over[0][2]!r} under {name}"
        assert got == (int(name[1:]), gutter), f"row {gutter}: under {name} is {over[0][2]!r}, which says column/row {got}"
        checked += 1
assert checked >= 3 * 5, f"only {checked} cells checked"
print(f"aligned: {checked} cells under {len(header)} headers, columns {cols[0]}..{cols[-1]}")
PY

# ---- 2. the cell viewer ---------------------------------------------------------
run_sql "select 1 as id, repeat('abcdefghij ', 200) || chr(10) || 'second line' as long_text, '{\"z\":[1,2,{\"b\":null}],\"a\":\"d\"}'::jsonb as doc, null::text as nothing;"
wait_for 05-long-ran 'c 1/4'
key C-g j
key l
key Enter
wait_for 06-viewer-long 'long_text · text · row 1'
expect viewer-long 'abcdefghij abcdefghij'
expect viewer-long 'esc close'
expect viewer-long 'y copy'
key j j j j j j j j j j j j j j j j
sleep 0.3
capture 07-viewer-long-scrolled
expect viewer-long-scrolled 'second line'
key Escape
wait_for 08-viewer-closed 'RESULTS'
expect_not viewer-closed 'y copy  +esc close'
key l Enter
wait_for 09-viewer-json 'doc · jsonb · row 1 · 36 chars, 36 B · pretty'
expect viewer-json '"z": \['
expect viewer-json '"a": "d"'
key r
wait_for 10-viewer-json-raw 'stored text'
expect viewer-json-raw '\{"z": \[1, 2, \{"b": null\}\], "a": "d"\}|\{"a": "d", "z"'
key Escape
key l Enter
wait_for 11-viewer-null 'nothing · text · row 1'
expect viewer-null '∅  NULL: no value'
key y
wait_for 12-copied-null 'Sent an empty string to the terminal clipboard'
key Escape
sleep 4.5 # the notice (4 s) would sit over the last row of the next result

# ---- 3. wide characters ---------------------------------------------------------
run_sql "select id, label, v, 'END' as e, 0 as z from width_text where id in (1,2,3,4,5,7,8,9) order by id;"
wait_for 13-wide-text 'c 1/5'
key C-g j
sleep 0.3
capture 13-wide-text
expect wide-text '日本語のテキスト'
python3 -I - "$LAST_CAPTURE" <<'PY' || fail "the columns of wide text are not aligned by display width"
import re, sys, unicodedata
def width(s):
    w = 0
    for ch in s:
        if unicodedata.combining(ch) or ch in "‍" or unicodedata.category(ch) in ("Mn", "Me", "Cf"):
            continue
        w += 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1
    return w
lines = open(sys.argv[1], encoding="utf-8").read().split("\n")
xs = {}
start = next(i for i, l in enumerate(lines) if "├─ results" in l)
for line in lines[start + 1:]:
    if "│" not in line: continue
    pane = line.split("│", 1)[1]
    if re.match(r"\s*\d+ ", pane) and re.search(r"\bEND\b", pane):
        before = pane[: re.search(r"\bEND\b", pane).start()]
        xs[pane.strip()[:12]] = width(before)
assert len(xs) == 8, f"expected 8 rows with END, got {len(xs)}: {xs}"
assert len(set(xs.values())) == 1, f"END is not in one column: {xs}"
print(f"aligned: END at display column {next(iter(xs.values()))} on {len(xs)} rows")
PY

# ---- 4. load more on the million-row table --------------------------------------
# Row 1,001 sleeps 2 s (the server cannot send anything before it, and dbx
# reads that row to know the result was cut), so the first run takes about
# 2 s and so does loading the second thousand: long enough to be seen.
run_sql "select id, txt, num, case when id = 1001 then pg_sleep(2)::text else '' end as slow from million_rows order by id;"
wait_for 14-million 'c 1/4' 30
expect million 'row 1 '
key C-g j
key j j j
sleep 0.3
capture 15-million-cursor
expect million-cursor 'm more'
expect million-cursor 'r 4/1,000\+'
key m
# The statement calls a function (pg_sleep), which dbx cannot prove safe to repeat: it asks first.
wait_for 15b-rerun-question 'Run the statement again' 5
expect rerun-question 'may change data'
key Enter
wait_for 16-loading-more 'loading more' 5
expect loading-more 'row 4 '
expect_not loading-more 'earlier run'
key j
sleep 0.3
capture 17-loading-more-navigated
expect loading-more-navigated 'r 5/1,000\+'
wait_for 18-loaded-more '2,000\+ rows' 40
expect loaded-more 'r 5/2,000\+'
expect_not loaded-more 'loading more'
cp "$LAST_CAPTURE" "$OUT/loaded-more.txt" 2>/dev/null || true

key C-g q
wait_for 19-quit 'EXIT=0' 5
expect_terminal_restored quit

if grep -l -e "$DBX_TUI_SEED_PASSWORD" "$OUT"/*.txt 2>/dev/null | grep -q .; then
  fail "a password appeared on screen"
fi
echo "OK wide-result; captures in $OUT"
