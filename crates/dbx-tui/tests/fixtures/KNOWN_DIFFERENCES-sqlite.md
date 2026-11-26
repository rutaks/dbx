# Known differences (SQLite): what the client shows against what `sqlite3` prints

`tests/ground_truth_sqlite.rs` reads every cell of every table and view of
`edge.db` (`tests/fixtures/sqlite/edge.sql`) and of Sakila (`sakila.db`)
through `DbxCore::run` and through `sqlite3 -quote` (3.51.0; it keeps NULL, `''`
and `'NULL'` apart), and fails on a difference that is not listed here, on a
listed one that no longer occurs, on one whose count or example changed, and on
a reason that is still `UNREVIEWED`. `GT_WRITE=1 cargo test -p dbx-tui --test
ground_truth_sqlite` rewrites the table from what the run finds (new or changed
rows get `UNREVIEWED`) and then FAILS on purpose; a person writes the reasons.

About 358,000 cells are compared. Everything not listed is identical: NULL,
`''`, `'NULL'`, integers past 2^53 and at both i64 limits, text with emoji, ZWJ
sequences, CJK, right-to-left text, line breaks, tabs and a 2,000-word value,
JSON text, dates stored as text, integers and reals, a column with no type
holding every storage class, quoted mixed-case names, and every Sakila table
and view.

A row is one group: a table, a column, a class and the shape of the two texts
(digits folded to 9, letters to a), with how many cells have it. `FORMAT` is the
same value in another spelling. `LOSS` is a value the client shows wrongly.

A REAL is compared bit for bit: sqlite3 is asked for its eight bytes
(`hex(ieee754_to_blob(x))`) and the client's text must parse to exactly that
double, or it is a LOSS, however close. sqlite3's printed spelling (`printf('%.17g')`,
since `quote()` is one step off for 1e300) is only what the table shows. dbx
hands the client the exact double and the client shows its shortest
round-trip spelling, which is why some rows show more digits than sqlite3
prints and others fewer.

The grid cannot tell a blob from text: both a blob and a text value that
starts with `0x` read `0x…` (the blob rows below). The column's declared
type (`BLOB`) is the only hint, and a column with no type gives none.

<!-- gt:differences:start -->
| table | column | class | shape (ours -> sqlite3) | cells | first example | why |
|---|---|---|---|---|---|---|
| edge.blobs | b | FORMAT | `'0x<hex>' -> X'<hex>'` | 5 | row 1: ours `'0x'`, sqlite3 `X''` | A blob is shown as `0x` and its bytes in hex: the same bytes. The grid cannot tell it from a text value that starts with `0x`: both read `0x…`; the column type (BLOB) is the only hint. |
| edge.dates | julian | FORMAT | `'9.9' -> 9` | 1 | row 1: ours `'2460374.0'`, sqlite3 `2460374` | A whole REAL keeps its `.0`, which says it is a real, not an integer. |
| edge.reals | v | FORMAT | `'9.9' -> 9` | 3 | row 5: ours `'0.0'`, sqlite3 `0` | Whole reals (0.0, -0.0, 3.0) keep `.0`. The row inserted as `-0.0` shows `0.0` because SQLite stores +0.0 for it (its bytes are `0000000000000000`): nothing is lost on the way. |
| edge.reals | v | FORMAT | `'9.9' -> 9.9` | 1 | row 8: ours `'123456789.12345679'`, sqlite3 `123456789.1234568` | The client shows all the digits the double needs (17); sqlite3 cuts at 16. |
| edge.reals | v | FORMAT | `'9.9a+9' -> 9.9a+9` | 1 | row 2: ours `'1.7976931348623157e+308'`, sqlite3 `1.797693134862316e+308` | Same, for the largest double. |
| edge.reals | v | FORMAT | `'9a+9' -> 9` | 1 | row 7: ours `'1e+16'`, sqlite3 `10000000000000000` | Exponent spelling for 1e16; the same double. |
| edge.reals | v | FORMAT | `'9a-9' -> 9.9a-9` | 1 | row 3: ours `'5e-324'`, sqlite3 `4.940656458412465e-324` | The smallest subnormal: shortest spelling against sqlite3's long one. |
| edge.reals | v | FORMAT | `'9a-9' -> 9a-9` | 1 | row 1: ours `'1e-7'`, sqlite3 `1e-07` | Exponent without the leading zero; the same double. |
| edge.reals | v | LOSS | `a -> -a` | 1 | row 10: ours `NULL`, sqlite3 `-Inf` | Minus infinity is shown as NULL: dbx turns a non-finite double into JSON null (`serde_json::Number::from_f64`). A decoding loss in dbx; not patched. |
| edge.reals | v | LOSS | `a -> a` | 1 | row 9: ours `NULL`, sqlite3 `Inf` | Infinity is shown as NULL, as above. SQLite has no NaN (it stores NULL), so only infinity is affected. |
| edge.typed | dec | FORMAT | `'9.9' -> 9a-9` | 1 | row 2: ours `'0.00001'`, sqlite3 `1e-05` | A DECIMAL column holds a REAL here; shortest spelling against exponent form. |
| edge.typed | fl | FORMAT | `'9a-9' -> 9a-9` | 1 | row 2: ours `'1e-7'`, sqlite3 `1e-07` | As for reals. |
| edge.untyped | v | FORMAT | `'0x<hex>' -> X'<hex>'` | 1 | row 4: ours `'0x00ff'`, sqlite3 `X'00ff'` | A blob in a column with no type, as for blobs. |
| edge.untyped | v | LOSS | `a -> a` | 1 | row 8: ours `NULL`, sqlite3 `Inf` | Infinity is shown as NULL, as above. |
| sakila.payment | amount | FORMAT | `'9.9' -> 9.9` | 1 | row 2059: ours `'8.97'`, sqlite3 `8.970000000000001` | sqlite3's own `%.17g` adds noise digits; the client shows the shortest spelling of the stored double. |
| sakila.sales_by_film_category | total_sales | FORMAT | `'9.9' -> 9.9` | 3 | row 12: ours `'3417.7200000000003'`, sqlite3 `3417.72` | A SUM of reals: the double really is 3417.7200000000003; sqlite3 rounds to 15 digits. The client is the exact one. |
| sakila.sales_by_store | total_sales | FORMAT | `'9.9' -> 9.9` | 1 | row 2: ours `'33726.770000000004'`, sqlite3 `33726.77` | As above. |
<!-- gt:differences:end -->

## What the export and the dump lose (files written, not cells shown)

Pinned by `tests/sqlite.rs` (the SQL INSERT export of every edge table into a
scratch copy, and a dump of `edge.db` loaded into a new file). All LOSS:

- Infinity is written as NULL (the same decoding loss).
- A blob is written as the text `'0x..'`, not `X'..'`: the file loads, and the
  column then holds text.
- An integer past 2^53 (either sign) in a column with no declared type is
  written as text: dbx gives it to the client as text, so it cannot be told
  from a text value. The other numbers of such a column are written as
  numbers (and JSON numbers): dbx says per value that it is a number, and the
  client keeps that for the cells of a column with no type name.
- The dump (not the export) drops a time zone from a date-time
  (`2024-03-05T13:45:12+05:30` becomes `2024-03-05 13:45:12`).
