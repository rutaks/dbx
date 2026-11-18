# Known differences: what the client shows against what `psql` prints

This is the list of the ways the client can mislead you about a stored value.
`tests/ground_truth.rs` reads it: it runs every column of every table in the
`edge` fixture database through `DbxCore::run` and through `psql -At`, and
fails on a difference that is not listed here, on a listed one that no longer
occurs, on a listed one whose text changed, and on a reason that is still
`UNREVIEWED`. `GT_WRITE=1 cargo test -p dbx-tui --test ground_truth` rewrites
the tables below from what the harness finds, keeps the reason of a row that is
unchanged, gives every new or changed row the reason `UNREVIEWED`, and then
FAILS on purpose (it is refused when `CI` is set): a person reads the diff of
this file, writes each real reason, and runs the test again without `GT_WRITE`.

Upstream issue: not filed yet (see `UPSTREAM-PROPOSAL.md`; nothing is sent
without Samuel's say-so).

## The session both sides ran in

The harness reads these settings through the Core and through `psql` at the
start of a run and fails unless each pair is equal (they decide how a value
prints). The `psql` side runs with `PGCLIENTENCODING=UTF8` and with `PGTZ` and
`PGDATESTYLE` cleared. `gt_temporal` is run a second time, on both sides, after
`SET TIME ZONE 'Asia/Kolkata'` (UTC+05:30); it appears below as
`gt_temporal @ Asia/Kolkata`.

<!-- gt:settings:start -->
| setting | value |
|---|---|
| TimeZone | Etc/UTC |
| DateStyle | ISO, MDY |
| IntervalStyle | postgres |
| extra_float_digits | 1 |
| bytea_output | hex |
| lc_monetary | en_US.utf8 |
| client_encoding | UTF8 |
| TimeZone, run of gt_temporal @ Asia/Kolkata | Asia/Kolkata |
<!-- gt:settings:end -->

## How to read the list

- **Row** is the label the fixture row carries, saying what it probes.
- **Mode.** dbx decodes a result as binary or as text, depending on its
  other columns (design section 17). The harness runs each column twice:
  `alone` (`select col from t`) and `beside tstz` (the same, plus a
  `timestamptz` column, which makes dbx decode the whole result as text). A
  column of a date or time type is decoded as text even alone. Where a
  difference appears alone and not beside a timestamptz column, it is the binary
  decoder's.
- **We show / psql shows** are the texts, escaped (`\n`, `\t`, `\u{..}` for a
  control character, `\x7c` for a pipe), cut at 70 characters; `NULL` without
  quotes is NULL.
- **FORMAT**: the same value spelled differently. **LOSS**: a wrong or missing
  value: what is shown is not what the database stores.
- A text is judged the same value, by type, as `tests/ground_truth/normalise.rs`
  says (floats by their bits, booleans `t`/`true`, `bytea` `\x`/`0x`, ISO 8601
  against Postgres's timestamp text, `money` without `$` and `,`, arrays element
  by element, and so on). Everything else, every integer, `numeric` and text
  value included, must be identical, character for character.

## Exact: nothing to list

Identical to `psql`, alone and beside a timestamptz column, for every
fixture row: integers of every width and at their limits (beyond 2^53 too),
`numeric` (30 digits, scale 20, negative zero, `NaN`, `Infinity`), text (empty,
`NULL`, a space, whitespace, emoji, combining marks, right to left, a ZWJ
sequence, a flag, tabs and line breaks, control characters, 100 kB), `date`,
`time`, `timetz`, `timestamp` (including infinity, BC and the far ends of the
range), `uuid`, `json` and `jsonb` (key order, duplicate keys, numbers like
`1E+400`), enum, domain, composite, `inet`, `cidr`, `macaddr`, bit strings, `xml`,
`tsvector`, `timestamp(3)` and `time(3)` (including a value that rounds up to
`24:00:00`), `regtype`, arrays of enum and of composite, computed columns
(`count(*)`, `sum` of numeric and bigint, `avg`, `row(...)`, `json_agg`,
`jsonb_agg`, in the `gt_computed` view and as queries the harness runs, listed
as `query: ...`), and the 100-column table. The million-row table is compared by
count and by a sample of rows, and also agrees. (`timestamptz` is a FORMAT
difference on both paths, below.)

## Differences by cause

**LOSS** (binary path only, so `alone`; beside a `timestamptz` column dbx reads
the same value right, except where said):

1. `NaN`, `Infinity` and `-Infinity` in a `float4` or `float8` column, or inside
   a float array, come back as NULL. A client cannot tell it from a NULL.
2. A `float4` is rounded to six decimals: `1e-7` is `0.0`, `-1.5e-7` is `-0.0`,
   `1.17549435e-38` and the subnormal `1e-45` are `0.0`, `1/3` is `0.333333`.
3. Types dbx has no binary decoder for come back as their wire bytes (as text if
   they happen to be UTF-8, which draws as `�`, else as `0x` hex): ranges and
   multiranges, `tsquery`, `box`, `tid`, `pg_lsn` (a value that is not UTF-8
   shows as hex, `0/0` as NUL bytes), `line`, `lseg`, `path`, `polygon`,
   `circle`, `jsonpath` (a hidden `\u{1}` before the text) and `xid8`.
4. `"char"` (the internal one-byte type) comes back as the byte's number.
5. An array whose lower bound is not 1 loses it, a `bytea[]`'s elements come
   back as the bytes decoded as text, and a `json[]`'s elements are written back
   compactly (`{"b": 1,  "a": 2}` is `{"b":1,"a":2}`: same value, not the text
   Postgres keeps).

**FORMAT** (the same value):

- `timestamptz`, on both paths: ISO 8601 (`2024-03-05T11:45:12+00:00`) for
  psql's `2024-03-05 11:45:12+00`. Under a session zone that is not UTC
  (`gt_temporal @ Asia/Kolkata`) dbx shows the instant in that zone, offset
  `+05:30` included, so it honours the session and only the spelling differs.
- `bool`: `true`/`false` for `t`/`f`; `bytea`: `0xdead` for `\xdead` (alone).
- `float8`, alone: a whole number gets `.0`, `1e-7` for `1e-07`; `float4`,
  alone: rounded to six decimals and printed as an `f64`; where the rounding
  lands on the same float4 (`1234.567749` for `1234.5677`, `100000000.0`) only the
  spelling differs, and where it does not it is the LOSS above. Inside a `real[]`
  each element is widened instead, so `0.1` shows as `0.10000000149011612`.
- `interval`, alone: `3 days 00:00:00` for `3 days`, `1 year` for `-1 years`.
- `money`, alone: no `$` and no grouping (and the decimals are always two; see `UPSTREAM-PROPOSAL.md` for a locale that has other).
- Arrays, alone: JSON text (`[1,null]`) for `{1,NULL}`, and a 2-D array of
  `float8` as `{{1.0,2.0}}`.
- `point`, alone: the coordinates in full decimal; `pg_lsn` as hex, but only for a value whose bytes are not UTF-8 (dbx has no decoder: see the LOSS list).

## The grid and the cell viewer

The harness also draws every column's first 40 rows through the client's own
view (one column per frame, through `GridProbe`, a thin test-support wrapper
around `App`) and requires each drawn cell to equal what the grid is documented
to draw for the Core's text: NULL as `∅`, a line break as `⏎`, a tab as
`⇥`, a control character as `�`, the bidirectional overrides dropped, and
the text cut with `…` at 40 cells. Spaces at the start or end of a cell barely show
in the grid, so the comparison trims both sides, and cells that differ only in
spaces are listed as look-alikes below. The cell
viewer (`enter`) is where such values are told apart: `empty string`, `∅ NULL:
no value`, `⊘ no cell`, a trailing space as `·`.

Always told apart, grid and viewer, by test: NULL, the empty string, the text
`NULL` (grid: `∅`, blank, `NULL`), and a cell the row lacks (grid `⊘`, viewer
`no cell`). The empty string and a single space are both blank in the grid:
the viewer tells them apart (`empty string` and `·`).

### Look-alikes in the grid

Cells of one column that the grid draws the same but that hold different
values. The last column says whether the cell viewer shows them differently;
`NO` is a defect of the viewer, here only for raw control bytes (two different
control characters both draw as `�`).

<!-- gt:lookalikes:start -->
| table | column | the grid draws | the values behind it | the cell viewer shows | viewer tells them apart |
|---|---|---|---|---|---|
| gt_catalog_types | x8 | `��������` | plain: `"\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}"`; array type, beyond 2^32: `"\u{0}\u{0}\u{0}\u{1}\u{0}\u{0}\u{0}\u{0}"` | `��������` / `��������` | NO |
| gt_text | t | (blank) | empty string: `""`; one space: `" "` | `empty string` / `·` | yes |
| gt_text | v | `∅` | null: `NULL`; the glyphs the grid uses: `"∅"` | `∅  NULL: no value` / `∅` | yes |
| gt_text | v | (blank) | empty string: `""`; one space: `" "` | `empty string` / `·` | yes |
| gt_text | v | `x` | leading and trailing: `" x "`; bidi override: `"\u{2066}x\u{2069}"` | ` x·` / `x` | yes |
| gt_types | ir | `�` | quoted enum, unbounded ranges: `"\u{18}"`; apostrophe enum, empty ranges: `"\u{1}"` | `�` / `�` | NO |
| gt_types | tr | `�` | quoted enum, unbounded ranges: `"\u{18}"`; apostrophe enum, empty ranges: `"\u{1}"` | `�` / `�` | NO |
| text_values | v | (blank) | empty: `""`; one space: `" "` | `empty string` / `·` | yes |
<!-- gt:lookalikes:end -->

## Every difference, cell by cell

<!-- gt:differences:start -->
| table | row | column | mode | we show | psql shows | class | why |
|---|---|---|---|---|---|---|---|
| gt_arrays | simple | ai | alone | `"[1,2]"` | `"{1,2}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | empty | ai | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | with NULL elements | ai | alone | `"[1,null,3]"` | `"{1,NULL,3}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | lower bound not 1 | ai | alone | `"[1,2,3]"` | `"[0:2]={1,2,3}"` | LOSS | binary path: the array's lower bound is dropped (`[0:2]=` in psql) |
| gt_arrays | big integers | ai | alone | `"[2147483647,-2147483648]"` | `"{2147483647,-2147483648}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | simple | at | alone | `"[\"a\",\"b\"]"` | `"{a,b}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | empty | at | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | with NULL elements | at | alone | `"[\"a\",null,\"NULL\"]"` | `"{a,NULL,\"NULL\"}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | elements that need quotes | at | alone | `"[\"a b\",\"c,d\",\"e\\\"f\",\"g\\\\h\",\"\",\"{}\",\"NULL\",null]"` | `"{\"a b\",\"c,d\",\"e\\\"f\",\"g\\\\h\",\"\",\"{}\",\"NULL\",NULL}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | lower bound not 1 | at | alone | `"[\"x\",\"y\"]"` | `"[-1:0]={x,y}"` | LOSS | binary path: the array's lower bound is dropped (`[0:2]=` in psql) |
| gt_arrays | simple | an | alone | `"[\"1.10\",\"2\"]"` | `"{1.10,2}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | empty | an | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | with NULL elements | an | alone | `"[null,\"1\"]"` | `"{NULL,1}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | NaN and Infinity | an | alone | `"[\"NaN\"]"` | `"{NaN}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | big integers | an | alone | `"[\"123456789012345678901234567890\"]"` | `"{123456789012345678901234567890}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | simple | ab | alone | `"[true,false]"` | `"{t,f}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | empty | ab | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | with NULL elements | ab | alone | `"[true,null]"` | `"{t,NULL}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | simple | af | alone | `"[1.5,2.0]"` | `"{1.5,2}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | empty | af | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | with NULL elements | af | alone | `"[null,1.0]"` | `"{NULL,1}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | nested 2-D | af | alone | `"{{1.0,2.0},{3.0,4.0}}"` | `"{{1,2},{3,4}}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | NaN and Infinity | af | alone | `"[null,null,null]"` | `"{NaN,Infinity,-Infinity}"` | LOSS | binary path: NaN and infinities inside a float array are NULL (UPSTREAM-PROPOSAL.md, issue 1) |
| gt_arrays | simple | ats | alone | `"[\"2024-03-05T11:45:12+00:00\"]"` | `"{\"2024-03-05 11:45:12+00\"}"` | FORMAT | on both paths dbx gives a timestamptz array as JSON text with ISO 8601 elements; psql prints `{"2024-03-05 11:45:12+00"}`; the same instants |
| gt_arrays | empty | ats | alone | `"[]"` | `"{}"` | FORMAT | on both paths dbx gives a timestamptz array as JSON text with ISO 8601 elements; psql prints `{"2024-03-05 11:45:12+00"}`; the same instants |
| gt_arrays | with NULL elements | ats | alone | `"[null]"` | `"{NULL}"` | FORMAT | on both paths dbx gives a timestamptz array as JSON text with ISO 8601 elements; psql prints `{"2024-03-05 11:45:12+00"}`; the same instants |
| gt_arrays | simple | ats | beside tstz | `"[\"2024-03-05T11:45:12+00:00\"]"` | `"{\"2024-03-05 11:45:12+00\"}"` | FORMAT | on both paths dbx gives a timestamptz array as JSON text with ISO 8601 elements; psql prints `{"2024-03-05 11:45:12+00"}`; the same instants |
| gt_arrays | empty | ats | beside tstz | `"[]"` | `"{}"` | FORMAT | on both paths dbx gives a timestamptz array as JSON text with ISO 8601 elements; psql prints `{"2024-03-05 11:45:12+00"}`; the same instants |
| gt_arrays | with NULL elements | ats | beside tstz | `"[null]"` | `"{NULL}"` | FORMAT | on both paths dbx gives a timestamptz array as JSON text with ISO 8601 elements; psql prints `{"2024-03-05 11:45:12+00"}`; the same instants |
| gt_arrays | simple | ay | alone | `"[\"ޭ\"]"` | `"{\"\\\\xdead\"}"` | LOSS | binary path: a bytea array's elements are shown as the bytes decoded as text, not as hex |
| gt_arrays | empty | ay | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | with NULL elements | ay | alone | `"[null]"` | `"{NULL}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | simple | au | alone | `"[\"a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11\"]"` | `"{a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | empty | au | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | with NULL elements | au | alone | `"[null]"` | `"{NULL}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | simple | aj | alone | `"[\"{\\\"a\\\":1}\"]"` | `"{\"{\\\"a\\\": 1}\"}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | empty | aj | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | with NULL elements | aj | alone | `"[null]"` | `"{NULL}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | simple | a4 | alone | `"[0.10000000149011612,1.0000000116860974e-7,16777216.0]"` | `"{0.1,1e-07,1.6777216e+07}"` | FORMAT | binary path: dbx widens each float4 element to f64 and prints that, so `0.1` shows as `0.10000000149011612`; the same float4 |
| gt_arrays | with NULL elements | a4 | alone | `"[null]"` | `"{NULL}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| gt_arrays | NaN and Infinity | a4 | alone | `"[null,null]"` | `"{NaN,Infinity}"` | LOSS | binary path: NaN and infinities inside a float array are NULL (UPSTREAM-PROPOSAL.md, issue 1) |
| gt_arrays | simple | ajs | alone | `"[\"{\\\"b\\\":1,\\\"a\\\":2}\"]"` | `"{\"{\\\"b\\\": 1,  \\\"a\\\": 2}\"}"` | LOSS | binary path: a json[] element is parsed and written back compactly (`{"b": 1,  "a": 2}` comes back as `{"b":1,"a":2}`, `[1 ,2]` as `[1,2]`); the JSON value and the key order are the same but the stored text is not, and Postgres keeps json text as written (the json column itself is exact) |
| gt_arrays | empty | ajs | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives the empty json[] as JSON text, `[]` for psql's `{}` |
| gt_arrays | with NULL elements | ajs | alone | `"[null,\"null\",\"[1,2]\"]"` | `"{NULL,\"null\",\"[1 ,2]\"}"` | LOSS | binary path: a json[] element is parsed and written back compactly (`{"b": 1,  "a": 2}` comes back as `{"b":1,"a":2}`, `[1 ,2]` as `[1,2]`); the JSON value and the key order are the same but the stored text is not, and Postgres keeps json text as written (the json column itself is exact) |
| gt_arrays_more | simple | ad | alone | `"[\"2024-03-05\",\"1970-01-01\"]"` | `"{2024-03-05,1970-01-01}"` | FORMAT | on both paths dbx gives a date array as JSON text with the same elements as strings (`["2024-03-05",null]`); psql prints `{2024-03-05,NULL}`; element for element the same text |
| gt_arrays_more | empty | ad | alone | `"[]"` | `"{}"` | FORMAT | on both paths dbx gives a date array as JSON text with the same elements as strings (`["2024-03-05",null]`); psql prints `{2024-03-05,NULL}`; element for element the same text |
| gt_arrays_more | with NULL elements | ad | alone | `"[null,\"2024-02-29\"]"` | `"{NULL,2024-02-29}"` | FORMAT | on both paths dbx gives a date array as JSON text with the same elements as strings (`["2024-03-05",null]`); psql prints `{2024-03-05,NULL}`; element for element the same text |
| gt_arrays_more | infinity and BC | ad | alone | `"[\"infinity\",\"-infinity\",\"4713-01-01 BC\"]"` | `"{infinity,-infinity,\"4713-01-01 BC\"}"` | FORMAT | on both paths dbx gives a date array as JSON text with the same elements as strings (`["2024-03-05",null]`); psql prints `{2024-03-05,NULL}`; element for element the same text |
| gt_arrays_more | simple | ad | beside tstz | `"[\"2024-03-05\",\"1970-01-01\"]"` | `"{2024-03-05,1970-01-01}"` | FORMAT | on both paths dbx gives a date array as JSON text with the same elements as strings (`["2024-03-05",null]`); psql prints `{2024-03-05,NULL}`; element for element the same text |
| gt_arrays_more | empty | ad | beside tstz | `"[]"` | `"{}"` | FORMAT | on both paths dbx gives a date array as JSON text with the same elements as strings (`["2024-03-05",null]`); psql prints `{2024-03-05,NULL}`; element for element the same text |
| gt_arrays_more | with NULL elements | ad | beside tstz | `"[null,\"2024-02-29\"]"` | `"{NULL,2024-02-29}"` | FORMAT | on both paths dbx gives a date array as JSON text with the same elements as strings (`["2024-03-05",null]`); psql prints `{2024-03-05,NULL}`; element for element the same text |
| gt_arrays_more | infinity and BC | ad | beside tstz | `"[\"infinity\",\"-infinity\",\"4713-01-01 BC\"]"` | `"{infinity,-infinity,\"4713-01-01 BC\"}"` | FORMAT | on both paths dbx gives a date array as JSON text with the same elements as strings (`["2024-03-05",null]`); psql prints `{2024-03-05,NULL}`; element for element the same text |
| gt_arrays_more | simple | ats | alone | `"[\"2024-03-05 13:45:12.123456\",\"1970-01-01 00:00:00\"]"` | `"{\"2024-03-05 13:45:12.123456\",\"1970-01-01 00:00:00\"}"` | FORMAT | on both paths dbx gives a timestamp array as JSON text with the same elements as strings (`["2024-03-05 13:45:12",null]`); psql prints `{"2024-03-05 13:45:12",NULL}` (quoted, as it has a space); element for element the same text |
| gt_arrays_more | empty | ats | alone | `"[]"` | `"{}"` | FORMAT | on both paths dbx gives a timestamp array as JSON text with the same elements as strings (`["2024-03-05 13:45:12",null]`); psql prints `{"2024-03-05 13:45:12",NULL}` (quoted, as it has a space); element for element the same text |
| gt_arrays_more | with NULL elements | ats | alone | `"[null]"` | `"{NULL}"` | FORMAT | on both paths dbx gives a timestamp array as JSON text with the same elements as strings (`["2024-03-05 13:45:12",null]`); psql prints `{"2024-03-05 13:45:12",NULL}` (quoted, as it has a space); element for element the same text |
| gt_arrays_more | infinity and BC | ats | alone | `"[\"infinity\",\"-infinity\",\"4713-01-01 00:00:00 BC\"]"` | `"{infinity,-infinity,\"4713-01-01 00:00:00 BC\"}"` | FORMAT | on both paths dbx gives a timestamp array as JSON text with the same elements as strings (`["2024-03-05 13:45:12",null]`); psql prints `{"2024-03-05 13:45:12",NULL}` (quoted, as it has a space); element for element the same text |
| gt_arrays_more | simple | ats | beside tstz | `"[\"2024-03-05 13:45:12.123456\",\"1970-01-01 00:00:00\"]"` | `"{\"2024-03-05 13:45:12.123456\",\"1970-01-01 00:00:00\"}"` | FORMAT | on both paths dbx gives a timestamp array as JSON text with the same elements as strings (`["2024-03-05 13:45:12",null]`); psql prints `{"2024-03-05 13:45:12",NULL}` (quoted, as it has a space); element for element the same text |
| gt_arrays_more | empty | ats | beside tstz | `"[]"` | `"{}"` | FORMAT | on both paths dbx gives a timestamp array as JSON text with the same elements as strings (`["2024-03-05 13:45:12",null]`); psql prints `{"2024-03-05 13:45:12",NULL}` (quoted, as it has a space); element for element the same text |
| gt_arrays_more | with NULL elements | ats | beside tstz | `"[null]"` | `"{NULL}"` | FORMAT | on both paths dbx gives a timestamp array as JSON text with the same elements as strings (`["2024-03-05 13:45:12",null]`); psql prints `{"2024-03-05 13:45:12",NULL}` (quoted, as it has a space); element for element the same text |
| gt_arrays_more | infinity and BC | ats | beside tstz | `"[\"infinity\",\"-infinity\",\"4713-01-01 00:00:00 BC\"]"` | `"{infinity,-infinity,\"4713-01-01 00:00:00 BC\"}"` | FORMAT | on both paths dbx gives a timestamp array as JSON text with the same elements as strings (`["2024-03-05 13:45:12",null]`); psql prints `{"2024-03-05 13:45:12",NULL}` (quoted, as it has a space); element for element the same text |
| gt_arrays_more | simple | ai8 | alone | `"[\"9007199254740993\",\"-9223372036854775808\"]"` | `"{9007199254740993,-9223372036854775808}"` | FORMAT | binary path: dbx gives a bigint[] as JSON text; an element beyond 2^53 is a quoted string and a smaller one a number, with the same digits as psql's `{9007199254740993,1}` |
| gt_arrays_more | empty | ai8 | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives a bigint[] as JSON text; an element beyond 2^53 is a quoted string and a smaller one a number, with the same digits as psql's `{9007199254740993,1}` |
| gt_arrays_more | with NULL elements | ai8 | alone | `"[null,1]"` | `"{NULL,1}"` | FORMAT | binary path: dbx gives a bigint[] as JSON text; an element beyond 2^53 is a quoted string and a smaller one a number, with the same digits as psql's `{9007199254740993,1}` |
| gt_arrays_more | infinity and BC | ai8 | alone | `"[\"9223372036854775807\"]"` | `"{9223372036854775807}"` | FORMAT | binary path: dbx gives a bigint[] as JSON text; an element beyond 2^53 is a quoted string and a smaller one a number, with the same digits as psql's `{9007199254740993,1}` |
| gt_bool_uuid_bytea | true, a uuid, bytes | flag | alone | `"true"` | `"t"` | FORMAT | dbx gives a JSON boolean, shown as `true`/`false`; psql prints `t`/`f |
| gt_bool_uuid_bytea | false, nil uuid, empty | flag | alone | `"false"` | `"f"` | FORMAT | dbx gives a JSON boolean, shown as `true`/`false`; psql prints `t`/`f |
| gt_bool_uuid_bytea | true, a uuid, bytes | flag | beside tstz | `"true"` | `"t"` | FORMAT | dbx gives a JSON boolean, shown as `true`/`false`; psql prints `t`/`f |
| gt_bool_uuid_bytea | false, nil uuid, empty | flag | beside tstz | `"false"` | `"f"` | FORMAT | dbx gives a JSON boolean, shown as `true`/`false`; psql prints `t`/`f |
| gt_bool_uuid_bytea | true, a uuid, bytes | bin | alone | `"0xdeadbeef00ff"` | `"\\xdeadbeef00ff"` | FORMAT | dbx shows `0x` and the hex; psql `\x` and the hex |
| gt_bool_uuid_bytea | false, nil uuid, empty | bin | alone | `"0x"` | `"\\x"` | FORMAT | dbx shows `0x` and the hex; psql `\x` and the hex |
| gt_bool_uuid_bytea | NULL, max uuid, one NUL | bin | alone | `"0x00"` | `"\\x00"` | FORMAT | dbx shows `0x` and the hex; psql `\x` and the hex |
| gt_bool_uuid_bytea | all 256 byte values | bin | alone | `"0x000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f2021"…(514 chars)` | `"\\x000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f2021"…(514 chars)` | FORMAT | dbx shows `0x` and the hex; psql `\x` and the hex |
| gt_bool_uuid_bytea | bytes that read as text | bin | alone | `"0x68656c6c6f"` | `"\\x68656c6c6f"` | FORMAT | dbx shows `0x` and the hex; psql `\x` and the hex |
| gt_catalog_types | plain | jp | alone | `"\u{1}$.\"a\"[*]?(@ > 1)"` | `"$.\"a\"[*]?(@ > 1)"` | LOSS | binary path: dbx has no decoder for jsonpath and shows the wire bytes, which are a version byte (1) then the text, so the text shows with a hidden `\u{1}` in front (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_catalog_types | extremes | jp | alone | `"\u{1}strict $.\"a b\".**{1 to last}"` | `"strict $.\"a b\".**{1 to last}"` | LOSS | binary path: dbx has no decoder for jsonpath and shows the wire bytes, which are a version byte (1) then the text, so the text shows with a hidden `\u{1}` in front (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_catalog_types | array type, beyond 2^32 | jp | alone | `"\u{1}$"` | `"$"` | LOSS | binary path: dbx has no decoder for jsonpath and shows the wire bytes, which are a version byte (1) then the text, so the text shows with a hidden `\u{1}` in front (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_catalog_types | plain | x8 | alone | `"\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}"` | `"0"` | LOSS | binary path: dbx has no decoder for xid8 and shows its 8 wire bytes, big-endian: `0` as eight NUL bytes, `4294967296` as the bytes 0 0 0 1 0 0 0 0, and a value whose bytes are not valid UTF-8 as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_catalog_types | extremes | x8 | alone | `"0xffffffffffffffff"` | `"18446744073709551615"` | LOSS | binary path: dbx has no decoder for xid8 and shows its 8 wire bytes, big-endian: `0` as eight NUL bytes, `4294967296` as the bytes 0 0 0 1 0 0 0 0, and a value whose bytes are not valid UTF-8 as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_catalog_types | array type, beyond 2^32 | x8 | alone | `"\u{0}\u{0}\u{0}\u{1}\u{0}\u{0}\u{0}\u{0}"` | `"4294967296"` | LOSS | binary path: dbx has no decoder for xid8 and shows its 8 wire bytes, big-endian: `0` as eight NUL bytes, `4294967296` as the bytes 0 0 0 1 0 0 0 0, and a value whose bytes are not valid UTF-8 as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_computed | group 2 | sum_float | alone | `"1e-7"` | `"1e-07"` | FORMAT | the same float8 (a sum, so a computed column); dbx writes `1e-7`, psql `1e-07` |
| gt_computed | group 1 | agg_int | alone | `"[100,200]"` | `"{100,200}"` | FORMAT | binary path: array_agg of integers as JSON text, the same elements as psql's `{100,200}` |
| gt_computed | group 2 | agg_int | alone | `"[2147483647,2147483647]"` | `"{2147483647,2147483647}"` | FORMAT | binary path: array_agg of integers as JSON text, the same elements as psql's `{100,200}` |
| gt_computed | group 3 | agg_int | alone | `"[null]"` | `"{NULL}"` | FORMAT | binary path: array_agg of integers as JSON text, the same elements as psql's `{100,200}` |
| gt_computed | group 1 | agg_numeric | alone | `"[\"1.10\",\"2.20\"]"` | `"{1.10,2.20}"` | FORMAT | binary path: array_agg of numerics as JSON text; each element is a quoted string with all its digits and scale (`"1.10"`), the same text as psql's `{1.10,2.20}` |
| gt_computed | group 2 | agg_numeric | alone | `"[\"12345678901234567890.123456789\",null]"` | `"{12345678901234567890.123456789,NULL}"` | FORMAT | binary path: array_agg of numerics as JSON text; each element is a quoted string with all its digits and scale (`"1.10"`), the same text as psql's `{1.10,2.20}` |
| gt_computed | group 3 | agg_numeric | alone | `"[null]"` | `"{NULL}"` | FORMAT | binary path: array_agg of numerics as JSON text; each element is a quoted string with all its digits and scale (`"1.10"`), the same text as psql's `{1.10,2.20}` |
| gt_computed | group 1 | agg_text | alone | `"[\"a\",\"b\"]"` | `"{a,b}"` | FORMAT | binary path: array_agg of text as JSON text, the same elements as psql's `{a,b}` |
| gt_computed | group 2 | agg_text | alone | `"[\"c\",null]"` | `"{c,NULL}"` | FORMAT | binary path: array_agg of text as JSON text, the same elements as psql's `{a,b}` |
| gt_computed | group 3 | agg_text | alone | `"[null]"` | `"{NULL}"` | FORMAT | binary path: array_agg of text as JSON text, the same elements as psql's `{a,b}` |
| gt_documents | simple | tsq | alone | `"\u{0}\u{0}\u{0}\u{3}\u{2}\u{2}\u{1}\u{0}\u{0}fox\u{0}\u{1}\u{0}\u{0}quick\u{0}"` | `"'quick' & 'fox'"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_documents | declaration | tsq | alone | `"\u{0}\u{0}\u{0}\u{3}\u{2}\u{3}\u{1}\u{0}\u{0}b\u{0}\u{1}\u{0}\u{0}a\u{0}"` | `"'a' \x7c 'b'"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_documents | entities | tsq | alone | `"\u{0}\u{0}\u{0}\u{2}\u{2}\u{1}\u{1}\u{0}\u{0}a\u{0}"` | `"!'a'"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_floats | zero | f4 | alone | `"0.0"` | `"0"` | FORMAT | the same float4: dbx rounds a scalar float4 to six decimals (the rule that loses digits in the LOSS rows above) and prints the f64, and for this value that rounding lands on the same float4; only the spelling differs (`.0`, a long exponent), psql prints the shortest digits that identify the float4 |
| gt_floats | negative zero | f4 | alone | `"-0.0"` | `"-0"` | FORMAT | the same float4: dbx rounds a scalar float4 to six decimals (the rule that loses digits in the LOSS rows above) and prints the f64, and for this value that rounding lands on the same float4; only the spelling differs (`.0`, a long exponent), psql prints the shortest digits that identify the float4 |
| gt_floats | one third | f4 | alone | `"0.333333"` | `"0.33333334"` | LOSS | float4 rounded to six decimals by dbx's binary decoder (UPSTREAM-PROPOSAL.md, issue 2) |
| gt_floats | one in ten million | f4 | alone | `"0.0"` | `"1e-07"` | LOSS | float4 rounded to six decimals by dbx's binary decoder (UPSTREAM-PROPOSAL.md, issue 2) |
| gt_floats | one in a million | f4 | alone | `"1e-6"` | `"1e-06"` | FORMAT | the same float4: dbx rounds a scalar float4 to six decimals (the rule that loses digits in the LOSS rows above) and prints the f64, and for this value that rounding lands on the same float4; only the spelling differs (`.0`, a long exponent), psql prints the shortest digits that identify the float4 |
| gt_floats | 2^24 + 1 | f4 | alone | `"16777216.0"` | `"1.6777216e+07"` | FORMAT | the same float4: dbx rounds a scalar float4 to six decimals (the rule that loses digits in the LOSS rows above) and prints the f64, and for this value that rounding lands on the same float4; only the spelling differs (`.0`, a long exponent), psql prints the shortest digits that identify the float4 |
| gt_floats | largest finite | f4 | alone | `"3.4028234663852886e+38"` | `"3.4028235e+38"` | FORMAT | the same float4: dbx rounds a scalar float4 to six decimals (the rule that loses digits in the LOSS rows above) and prints the f64, and for this value that rounding lands on the same float4; only the spelling differs (`.0`, a long exponent), psql prints the shortest digits that identify the float4 |
| gt_floats | smallest normal | f4 | alone | `"0.0"` | `"1.1754944e-38"` | LOSS | float4 rounded to six decimals by dbx's binary decoder (UPSTREAM-PROPOSAL.md, issue 2) |
| gt_floats | subnormal | f4 | alone | `"0.0"` | `"1e-45"` | LOSS | float4 rounded to six decimals by dbx's binary decoder (UPSTREAM-PROPOSAL.md, issue 2) |
| gt_floats | NaN | f4 | alone | `NULL` | `"NaN"` | LOSS | NaN or an infinity: dbx's binary decoder returns NULL (UPSTREAM-PROPOSAL.md, issue 1) |
| gt_floats | Infinity | f4 | alone | `NULL` | `"Infinity"` | LOSS | NaN or an infinity: dbx's binary decoder returns NULL (UPSTREAM-PROPOSAL.md, issue 1) |
| gt_floats | -Infinity | f4 | alone | `NULL` | `"-Infinity"` | LOSS | NaN or an infinity: dbx's binary decoder returns NULL (UPSTREAM-PROPOSAL.md, issue 1) |
| gt_floats | integral, 1e8 | f4 | alone | `"100000000.0"` | `"1e+08"` | FORMAT | the same float4: dbx rounds a scalar float4 to six decimals (the rule that loses digits in the LOSS rows above) and prints the f64, and for this value that rounding lands on the same float4; only the spelling differs (`.0`, a long exponent), psql prints the shortest digits that identify the float4 |
| gt_floats | integral, 1e15 | f4 | alone | `"999999986991104.0"` | `"1e+15"` | FORMAT | the same float4: dbx rounds a scalar float4 to six decimals (the rule that loses digits in the LOSS rows above) and prints the f64, and for this value that rounding lands on the same float4; only the spelling differs (`.0`, a long exponent), psql prints the shortest digits that identify the float4 |
| gt_floats | plain fraction | f4 | alone | `"1234.567749"` | `"1234.5677"` | FORMAT | dbx rounds a scalar float4 to six decimals, `1234.567749`; that happens to land on the same float4 as psql's shortest digits `1234.5677`, so it is spelled differently but is not the widened value (a real widening would show `1234.5677490234375`) |
| gt_floats | small negative | f4 | alone | `"-0.0"` | `"-1.5e-07"` | LOSS | float4 rounded to six decimals by dbx's binary decoder (UPSTREAM-PROPOSAL.md, issue 2) |
| gt_floats | zero | f8 | alone | `"0.0"` | `"0"` | FORMAT | the same float8; dbx prints a whole number with `.0` and an exponent as `e-7`, psql as `1` and `e-07 |
| gt_floats | negative zero | f8 | alone | `"-0.0"` | `"-0"` | FORMAT | the same float8; dbx prints a whole number with `.0` and an exponent as `e-7`, psql as `1` and `e-07 |
| gt_floats | one in ten million | f8 | alone | `"1e-7"` | `"1e-07"` | FORMAT | the same float8; dbx prints a whole number with `.0` and an exponent as `e-7`, psql as `1` and `e-07 |
| gt_floats | one in a million | f8 | alone | `"1e-6"` | `"1e-06"` | FORMAT | the same float8; dbx prints a whole number with `.0` and an exponent as `e-7`, psql as `1` and `e-07 |
| gt_floats | 2^24 + 1 | f8 | alone | `"16777217.0"` | `"16777217"` | FORMAT | the same float8; dbx prints a whole number with `.0` and an exponent as `e-7`, psql as `1` and `e-07 |
| gt_floats | NaN | f8 | alone | `NULL` | `"NaN"` | LOSS | NaN or an infinity: dbx's binary decoder returns NULL (UPSTREAM-PROPOSAL.md, issue 1) |
| gt_floats | Infinity | f8 | alone | `NULL` | `"Infinity"` | LOSS | NaN or an infinity: dbx's binary decoder returns NULL (UPSTREAM-PROPOSAL.md, issue 1) |
| gt_floats | -Infinity | f8 | alone | `NULL` | `"-Infinity"` | LOSS | NaN or an infinity: dbx's binary decoder returns NULL (UPSTREAM-PROPOSAL.md, issue 1) |
| gt_floats | integral, 1e8 | f8 | alone | `"100000000.0"` | `"100000000"` | FORMAT | the same float8; dbx prints a whole number with `.0` and an exponent as `e-7`, psql as `1` and `e-07 |
| gt_floats | integral, 1e15 | f8 | alone | `"1000000000000000.0"` | `"1e+15"` | FORMAT | the same float8; dbx prints a whole number with `.0` and an exponent as `e-7`, psql as `1` and `e-07 |
| gt_floats | small negative | f8 | alone | `"-1.5e-7"` | `"-1.5e-07"` | FORMAT | the same float8; dbx prints a whole number with `.0` and an exponent as `e-7`, psql as `1` and `e-07 |
| gt_geometric | plain | ln | alone | `"0x3ff0000000000000bff00000000000000000000000000000"` | `"{1,-1,0}"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | closed path, fractions | ln | alone | `"0x3fe00000000000004000000000000000c00a000000000000"` | `"{0.5,2,-3.25}"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | degenerate and extreme | ln | alone | `"0x7e37e43c8800759c3ff00000000000000000000000000000"` | `"{1e+300,1,0}"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | plain | ls | alone | `"0x000000000000000000000000000000003ff00000000000003ff0000000000000"` | `"[(0,0),(1,1)]"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | closed path, fractions | ls | alone | `"0x3ff8000000000000c00000000000000000000000000000003fb999999999999a"` | `"[(1.5,-2),(0,0.1)]"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | degenerate and extreme | ls | alone | `"\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}"` | `"[(0,0),(0,0)]"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | plain | pa | alone | `"0x0000000003000000000000000000000000000000003ff00000000000003ff0000000"…(108 chars)` | `"[(0,0),(1,1),(2,0)]"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | closed path, fractions | pa | alone | `"0x0100000003000000000000000000000000000000003ff00000000000003ff0000000"…(108 chars)` | `"((0,0),(1,1),(2,0))"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | degenerate and extreme | pa | alone | `"0x00000000013ff00000000000003ff0000000000000"` | `"[(1,1)]"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | plain | po | alone | `"\u{0}\u{0}\u{0}\u{3}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}@\u{10}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}@\u{10}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}@\u{8}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}"` | `"((0,0),(4,0),(4,3))"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | closed path, fractions | po | alone | `"0x000000033fb999999999999a3fc999999999999a4202a05f20000000000000000000"…(106 chars)` | `"((0.1,0.2),(10000000000,0),(0,1))"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | degenerate and extreme | po | alone | `"0x000000013ff00000000000003ff0000000000000"` | `"((1,1))"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | plain | ci | alone | `"0x3ff000000000000040000000000000004008000000000000"` | `"<(1,2),3>"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | closed path, fractions | ci | alone | `"0x000000000000000000000000000000003fe0000000000000"` | `"<(0,0),0.5>"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_geometric | degenerate and extreme | ci | alone | `"0x7e37e43c8800759c000000000000000001a56e1fc2f8f359"` | `"<(1e+300,0),1e-300>"` | LOSS | binary path: dbx has no decoder for this geometric type and shows the wire bytes (big-endian float8 coordinates, with a count or flag where the type has one): as `0x` hex when they are not valid UTF-8, else as raw characters, NULs drawn as `�` (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_money_bits | plain | m | alone | `"12.50"` | `"$12.50"` | FORMAT | binary path: dbx drops the currency symbol and the grouping; psql prints `$1,234.56 |
| gt_money_bits | thousands | m | alone | `"1234567.89"` | `"$1,234,567.89"` | FORMAT | binary path: dbx drops the currency symbol and the grouping; psql prints `$1,234.56 |
| gt_money_bits | negative | m | alone | `"-0.01"` | `"-$0.01"` | FORMAT | binary path: dbx drops the currency symbol and the grouping; psql prints `$1,234.56 |
| gt_money_bits | largest | m | alone | `"92233720368547758.07"` | `"$92,233,720,368,547,758.07"` | FORMAT | binary path: dbx drops the currency symbol and the grouping; psql prints `$1,234.56 |
| gt_money_bits | smallest | m | alone | `"-92233720368547758.08"` | `"-$92,233,720,368,547,758.08"` | FORMAT | binary path: dbx drops the currency symbol and the grouping; psql prints `$1,234.56 |
| gt_other_types | extremes | pt | alone | `"(100000000000000000000000000000000000000000000000000000000000000000000"…(305 chars)` | `"(1e+300,0)"` | FORMAT | binary path: dbx prints the coordinates in full decimal; psql in shortest form |
| gt_other_types | plain | bx | alone | `"0x400800000000000040100000000000003ff00000000000004000000000000000"` | `"(3,4),(1,2)"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_other_types | extremes | bx | alone | `"\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}"` | `"(0,0),(0,0)"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_other_types | plain | lsn | alone | `"0x00000016b374d848"` | `"16/B374D848"` | FORMAT | dbx has no decoder for pg_lsn: it shows the 8 wire bytes, and for this value they are not valid UTF-8, so they come out as `0x` hex, which reads as the same lsn; that is luck, not a decoder (the extremes row shows `0/0` as NUL bytes) |
| gt_other_types | extremes | lsn | alone | `"\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}"` | `"0/0"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_other_types | plain | ch | alone | `"120"` | `"x"` | LOSS | binary path: dbx shows the single byte of a "char" as its number; psql shows the character |
| gt_other_types | extremes | ch | alone | `"0"` | `""` | LOSS | binary path: dbx shows the single byte of a "char" as its number; psql shows the character |
| gt_other_types | plain | tid_v | alone | `"\u{0}\u{0}\u{0}\u{0}\u{0}\u{1}"` | `"(0,1)"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_other_types | extremes | tid_v | alone | `"0xffffffffffff"` | `"(4294967295,65535)"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_temporal | epoch | tstz | alone | `"1970-01-01T00:00:00+00:00"` | `"1970-01-01 00:00:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | microseconds | tstz | alone | `"2024-03-05T11:45:12.123456+00:00"` | `"2024-03-05 11:45:12.123456+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | year one | tstz | alone | `"0001-01-01T00:00:00+00:00"` | `"0001-01-01 00:00:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | leap day | tstz | alone | `"2024-02-29T23:59:59+00:00"` | `"2024-02-29 23:59:59+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | offset -05:30 | tstz | alone | `"2024-06-01T05:30:00+00:00"` | `"2024-06-01 05:30:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | offset +14 | tstz | alone | `"2024-05-31T10:00:00+00:00"` | `"2024-05-31 10:00:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | DST change in New York | tstz | alone | `"2024-03-10T07:30:00+00:00"` | `"2024-03-10 07:30:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | whole-second values | tstz | alone | `"2000-01-01T12:34:56+00:00"` | `"2000-01-01 12:34:56+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | epoch | tstz | beside tstz | `"1970-01-01T00:00:00+00:00"` | `"1970-01-01 00:00:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | microseconds | tstz | beside tstz | `"2024-03-05T11:45:12.123456+00:00"` | `"2024-03-05 11:45:12.123456+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | year one | tstz | beside tstz | `"0001-01-01T00:00:00+00:00"` | `"0001-01-01 00:00:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | leap day | tstz | beside tstz | `"2024-02-29T23:59:59+00:00"` | `"2024-02-29 23:59:59+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | offset -05:30 | tstz | beside tstz | `"2024-06-01T05:30:00+00:00"` | `"2024-06-01 05:30:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | offset +14 | tstz | beside tstz | `"2024-05-31T10:00:00+00:00"` | `"2024-05-31 10:00:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | DST change in New York | tstz | beside tstz | `"2024-03-10T07:30:00+00:00"` | `"2024-03-10 07:30:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | whole-second values | tstz | beside tstz | `"2000-01-01T12:34:56+00:00"` | `"2000-01-01 12:34:56+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| gt_temporal | far future | iv | alone | `"178000000 years 00:00:00"` | `"178000000 years"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval |
| gt_temporal | year one | iv | alone | `"-178000000 years 00:00:00"` | `"-178000000 years"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval |
| gt_temporal | before the common era | iv | alone | `"-1 year -2 mons -3 days -04:05:06"` | `"-1 years -2 mons -3 days -04:05:06"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval |
| gt_temporal | leap day | iv | alone | `"1 mon -1 day 00:00:00"` | `"1 mon -1 days"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval |
| gt_temporal | whole-second values | iv | alone | `"3 days 00:00:00"` | `"3 days"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval |
| gt_types | plain values | ir | alone | `"\u{2}\u{0}\u{0}\u{0}\u{4}\u{0}\u{0}\u{0}\u{1}\u{0}\u{0}\u{0}\u{4}\u{0}\u{0}\u{0}\n"` | `"[1,10)"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | quoted enum, unbounded ranges | ir | alone | `"\u{18}"` | `"(,)"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | apostrophe enum, empty ranges | ir | alone | `"\u{1}"` | `"empty"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | extreme bounds | ir | alone | `"0x020000000480000000000000047fffffff"` | `"[-2147483648,2147483647)"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | plain values | nr | alone | `"0x060000000c0002000000000001000113880000000c000200000000000100021388"` | `"[1.5,2.5]"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | quoted enum, unbounded ranges | nr | alone | `"0x080000000c000200000000000100021388"` | `"(,2.5)"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | apostrophe enum, empty ranges | nr | alone | `"\u{1}"` | `"empty"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | extreme bounds | nr | alone | `"0x060000000a0001fffa0000001503e80000000a00010007000000000064"` | `"[0.000000000000000000001,1000000000000000000000000000000]"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | plain values | tr | alone | `"0x02000000080002b5e724bf2200000000080002b5f16898a000"` | `"[\"2024-03-05 11:45:12+00\",\"2024-03-06 00:00:00+00\")"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | quoted enum, unbounded ranges | tr | alone | `"\u{18}"` | `"(,)"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | apostrophe enum, empty ranges | tr | alone | `"\u{1}"` | `"empty"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | extreme bounds | tr | alone | `"0x06000000088000000000000000000000087fffffffffffffff"` | `"[-infinity,infinity]"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | plain values | mr | alone | `"\u{0}\u{0}\u{0}\u{2}\u{0}\u{0}\u{0}\u{11}\u{2}\u{0}\u{0}\u{0}\u{4}\u{0}\u{0}\u{0}\u{1}\u{0}\u{0}\u{0}\u{4}\u{0}\u{0}\u{0}\u{3}\u{0}\u{0}\u{0}\u{11}\u{2}\u{0}\u{0}\u{0}\u{4}\u{0}\u{0}\u{0}\u{5}\u{0}\u{0}\u{0}\u{4}\u{0}\u{0}\u{0}\u{7}"` | `"{[1,3),[5,7)}"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | quoted enum, unbounded ranges | mr | alone | `"\u{0}\u{0}\u{0}\u{0}"` | `"{}"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| gt_types | apostrophe enum, empty ranges | mr | alone | `"\u{0}\u{0}\u{0}\u{1}\u{0}\u{0}\u{0}\u{11}\u{2}\u{0}\u{0}\u{0}\u{4}\u{0}\u{0}\u{0}\u{1}\u{0}\u{0}\u{0}\u{4}\u{0}\u{0}\u{0}\u{3}"` | `"{[1,3)}"` | LOSS | binary path: dbx has no decoder for this type and shows the wire bytes, as text when they happen to be valid UTF-8, else as `0x` hex (UPSTREAM-PROPOSAL.md, issue 3) |
| numbers | bigint max | approx | alone | `NULL` | `"NaN"` | LOSS | NaN or an infinity: dbx's binary decoder returns NULL (UPSTREAM-PROPOSAL.md, issue 1) |
| numbers | infinities | approx | alone | `NULL` | `"Infinity"` | LOSS | NaN or an infinity: dbx's binary decoder returns NULL (UPSTREAM-PROPOSAL.md, issue 1) |
| other_types | row 1 | flag | alone | `"true"` | `"t"` | FORMAT | dbx gives a JSON boolean, shown as `true`/`false`; psql prints `t`/`f |
| other_types | row 2 | flag | alone | `"false"` | `"f"` | FORMAT | dbx gives a JSON boolean, shown as `true`/`false`; psql prints `t`/`f |
| other_types | row 1 | flag | beside tstz | `"true"` | `"t"` | FORMAT | dbx gives a JSON boolean, shown as `true`/`false`; psql prints `t`/`f |
| other_types | row 2 | flag | beside tstz | `"false"` | `"f"` | FORMAT | dbx gives a JSON boolean, shown as `true`/`false`; psql prints `t`/`f |
| other_types | row 1 | bin | alone | `"0xdeadbeef00ff"` | `"\\xdeadbeef00ff"` | FORMAT | dbx shows `0x` and the hex; psql `\x` and the hex |
| other_types | row 2 | bin | alone | `"0x"` | `"\\x"` | FORMAT | dbx shows `0x` and the hex; psql `\x` and the hex |
| other_types | row 1 | arr | alone | `"[1,2]"` | `"{1,2}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| other_types | row 2 | arr | alone | `"[]"` | `"{}"` | FORMAT | binary path: dbx gives an array as JSON text (`[1,null]`), with its elements spelled as above; psql as `{1,NULL} |
| temporal | offset +02 | tstz | alone | `"2024-03-05T11:45:12.123456+00:00"` | `"2024-03-05 11:45:12.123456+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| temporal | epoch | tstz | alone | `"1970-01-01T00:00:00+00:00"` | `"1970-01-01 00:00:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| temporal | far | tstz | alone | `"9999-12-31T23:59:59+00:00"` | `"9999-12-31 23:59:59+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| temporal | offset +02 | tstz | beside tstz | `"2024-03-05T11:45:12.123456+00:00"` | `"2024-03-05 11:45:12.123456+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| temporal | epoch | tstz | beside tstz | `"1970-01-01T00:00:00+00:00"` | `"1970-01-01 00:00:00+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| temporal | far | tstz | beside tstz | `"9999-12-31T23:59:59+00:00"` | `"9999-12-31 23:59:59+00"` | FORMAT | dbx reformats timestamptz to ISO 8601 (`T`, `+00:00`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+00`; the same instant |
| temporal | far | iv | alone | `"-1 year 00:00:00"` | `"-1 years"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval |
| query: array_agg of bigint | row 1 | c | alone | `"[0,-1,\"-9223372036854775808\",\"9223372036854775807\",2147483648,42949672"…(170 chars)` | `"{0,-1,-9223372036854775808,9223372036854775807,2147483648,4294967296,9"…(158 chars)` | FORMAT | binary path: a bigint[] as JSON text; an element beyond 2^53 is a quoted string and a smaller one a number, with the same digits as psql |
| gt_temporal @ Asia/Kolkata | epoch | tstz | alone | `"1970-01-01T05:30:00+05:30"` | `"1970-01-01 05:30:00+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | microseconds | tstz | alone | `"2024-03-05T17:15:12.123456+05:30"` | `"2024-03-05 17:15:12.123456+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | leap day | tstz | alone | `"2024-03-01T05:29:59+05:30"` | `"2024-03-01 05:29:59+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | offset -05:30 | tstz | alone | `"2024-06-01T11:00:00+05:30"` | `"2024-06-01 11:00:00+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | offset +14 | tstz | alone | `"2024-05-31T15:30:00+05:30"` | `"2024-05-31 15:30:00+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | DST change in New York | tstz | alone | `"2024-03-10T13:00:00+05:30"` | `"2024-03-10 13:00:00+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | whole-second values | tstz | alone | `"2000-01-01T18:04:56+05:30"` | `"2000-01-01 18:04:56+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | epoch | tstz | beside tstz | `"1970-01-01T05:30:00+05:30"` | `"1970-01-01 05:30:00+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | microseconds | tstz | beside tstz | `"2024-03-05T17:15:12.123456+05:30"` | `"2024-03-05 17:15:12.123456+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | leap day | tstz | beside tstz | `"2024-03-01T05:29:59+05:30"` | `"2024-03-01 05:29:59+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | offset -05:30 | tstz | beside tstz | `"2024-06-01T11:00:00+05:30"` | `"2024-06-01 11:00:00+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | offset +14 | tstz | beside tstz | `"2024-05-31T15:30:00+05:30"` | `"2024-05-31 15:30:00+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | DST change in New York | tstz | beside tstz | `"2024-03-10T13:00:00+05:30"` | `"2024-03-10 13:00:00+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | whole-second values | tstz | beside tstz | `"2000-01-01T18:04:56+05:30"` | `"2000-01-01 18:04:56+05:30"` | FORMAT | under the session zone Asia/Kolkata dbx shows the instant in that zone (`+05:30`, not UTC) and reformats it to ISO 8601 (`T`) on both paths; psql prints `YYYY-MM-DD HH:MM:SS+05:30`; the same instant and offset |
| gt_temporal @ Asia/Kolkata | far future | iv | alone | `"178000000 years 00:00:00"` | `"178000000 years"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval (as in the UTC run) |
| gt_temporal @ Asia/Kolkata | year one | iv | alone | `"-178000000 years 00:00:00"` | `"-178000000 years"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval (as in the UTC run) |
| gt_temporal @ Asia/Kolkata | before the common era | iv | alone | `"-1 year -2 mons -3 days -04:05:06"` | `"-1 years -2 mons -3 days -04:05:06"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval (as in the UTC run) |
| gt_temporal @ Asia/Kolkata | leap day | iv | alone | `"1 mon -1 day 00:00:00"` | `"1 mon -1 days"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval (as in the UTC run) |
| gt_temporal @ Asia/Kolkata | whole-second values | iv | alone | `"3 days 00:00:00"` | `"3 days"` | FORMAT | dbx pads a bare year or day count with ` 00:00:00` and writes `1 year` for -1 too; psql does neither; the same interval (as in the UTC run) |
<!-- gt:differences:end -->
