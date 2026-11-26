# Known differences (MySQL): what the client shows against what `mysql` prints

`tests/ground_truth_mysql.rs` reads every cell of every table and view of
`edge` (`tests/fixtures/mysql/edge/01-edge.sql`) and of `sakila` through
`DbxCore::run` and through `mysql --xml` in the fixture container (MySQL
8.4.13; XML keeps NULL, `''` and the text `NULL` apart), and fails on a
difference that is not listed here, on a listed one that no longer occurs, on
one whose count or example changed, and on a reason that is still
`UNREVIEWED`. `GT_WRITE=1 cargo test -p dbx-tui --test ground_truth_mysql`
rewrites the table from what the run finds (new or changed rows get
`UNREVIEWED`) and then FAILS on purpose; a person writes the reasons.

A row is one group: a table, a column, a class and the shape of the two texts
(digits folded to 9, letters to a), with how many cells have it. `FORMAT` is
the same value in another spelling. `LOSS` is a value the client shows wrongly.

<!-- gt:differences:start -->
| table | column | class | shape (ours -> mysql) | cells | first example | why |
|---|---|---|---|---|---|---|
| edge.gt_floats | f4 | LOSS | `'-9' -> '-9'` | 1 | row 9: ours `'-123457'`, mysql `'-123457'` (exact -123456.7890625) | The text is the same as `mysql` prints, and it is not the number stored. A FLOAT is single precision (about 7 digits) and the server writes it with six significant digits; dbx's driver gives the client that text. `mysql` shows it the same way, so only the `cast(.. as double)` column, which holds the stored value, tells it. An export or a dump of this cell holds the six digits (pinned in `tests/mysql.rs`). |
| edge.gt_floats | f4 | LOSS | `'9' -> '9'` | 1 | row 6: ours `'16777200'`, mysql `'16777200'` (exact 16777216) | Same cause: six significant digits for a FLOAT. The stored value is 2^24; the text says 16777200, a different single-precision number. |
| edge.gt_floats | f4 | LOSS | `'9.9' -> '9.9'` | 1 | row 3: ours `'0.333333'`, mysql `'0.333333'` (exact 0.3333333432674408) | Same cause: six significant digits for a FLOAT. |
| edge.gt_floats | f4 | LOSS | `'9.9a-9' -> '9.9a-9'` | 1 | row 8: ours `'1.17549e-38'`, mysql `'1.17549e-38'` (exact 1.1754943508222875e-38) | Same cause: six significant digits for a FLOAT. |
| edge.gt_floats | f4 | LOSS | `'9.9a9' -> '9.9a9'` | 1 | row 7: ours `'3.40282e38'`, mysql `'3.40282e38'` (exact 3.4028234663852886e38) | Same cause: six significant digits for a FLOAT. The largest FLOAT is shown as a smaller number. A DOUBLE column is not affected: every one of its cells reads back as the same double, bit for bit. |
| edge.gt_misc | b64 | FORMAT | `'<bits>' -> hex` | 3 | row 1: ours `'00000000000000000000000000000000000000000000000001000001010…(66 chars)`, mysql `'4142'` | A BIT column is shown as its binary digits, one per bit, from the left; the server's `hex()` is the same number (compared as numbers: 0x4142). `mysql` itself prints the raw bytes. |
| edge.gt_misc | b8 | FORMAT | `'<bits>' -> hex` | 3 | row 1: ours `'01000001'`, mysql `'41'` | Same: eight digits for BIT(8), the same number as `hex()`. |
| edge.gt_misc | bin | FORMAT | `'0x<hex>' -> hex` | 3 | row 1: ours `'0x68656c6c'`, mysql `'68656C6C'` | Bytes are shown as `0x` and their hex, the same bytes. The grid cannot tell them from a text value that starts with `0x`: the column type (BINARY) is the only hint. |
| edge.gt_misc | bl | FORMAT | `'0x<hex>' -> hex` | 3 | row 1: ours `'0x68656c6c6f'`, mysql `'68656C6C6F'` | Bytes as `0x` and hex (BLOB), the same bytes. An empty blob is `0x`. |
| edge.gt_misc | vbin | FORMAT | `'0x<hex>' -> hex` | 3 | row 1: ours `'0x68656c6c6f'`, mysql `'68656C6C6F'` | Bytes as `0x` and hex (VARBINARY), the same bytes. |
| edge.gt_temporal | dt6 | FORMAT | `'9-9-9 9:9:9' -> '9-9-9 9:9:9.9'` | 1 | row 4: ours `'2000-02-29 00:00:00'`, mysql `'2000-02-29 00:00:00.000000'` | A DATETIME(6) with no fraction is shown without it: the same instant. Other fractions keep their digits (`.123456`). |
| edge.gt_temporal | t6 | FORMAT | `'9:9:9' -> '9:9:9.9'` | 1 | row 4: ours `'00:00:00'`, mysql `'00:00:00.000000'` | A TIME(6) with no fraction is shown without it: the same time. |
| edge.gt_temporal | ts6 | FORMAT | `'9-9-9 9:9:9.9' -> '9-9-9 9:9:9.9'` | 1 | row 4: ours `'2000-02-29 00:00:00.500'`, mysql `'2000-02-29 00:00:00.500000'` | A TIMESTAMP(6) of half a second is shown as `.500`: trailing zeros of the fraction are cut at three digits, the same instant. |
<!-- gt:differences:end -->
