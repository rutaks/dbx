# Proposal for dbx: Postgres values that come back wrong

Draft for dbx's maintainers. Not sent anywhere; Samuel decides whether and
where it is filed. Found by comparing every cell of `tests/fixtures/edge/`
against `psql -At` (`tests/ground_truth.rs`); the full list is
`KNOWN_DIFFERENCES.md`. Checked against dbx at `c21f156` ("fix(postgres):
recover queries rejected for tuple locators", with our `crates/dbx-tui` on top)
and Postgres 16, through `QueryExecutionOptions` with `max_rows` and a
`client_session_id` and nothing else.

## The shape of the problem

`dbx-driver-postgres` decodes a result one of two ways, chosen per result
(`prepared_select_metadata`, `postgres.rs:2026-2030`, calling
`pg_type_requires_text_protocol`, `postgres.rs:1026`): binary, unless some
column needs text (a date or time type, `reg*`, `record`, or a type
`tokio_postgres::Type::from_oid` does not know). The text path returns what
Postgres prints. The binary path decodes each value by hand and loses some. So
**the same value reads differently depending on the other columns in the
query**, and every reproduction below is right when a `timestamptz` column
(`now()`) is added beside it.

Nothing a frontend can set reaches the text path. The only state that forces
it for Postgres is a private process-wide map, `postgres_text_result_connections`
(`crates/dbx-driver-postgres/src/postgres.rs:2846`), filled by
`mark_postgres_connection_text_results` (`postgres.rs:2851`) at one place, when a
connection is made to a server whose version string contains "TBase"
(`postgres_server_ignores_binary_result_format`, `postgres.rs:1903`; the call is
at `postgres.rs:3101`). The other switch, `prefer_text_protocol`, is computed in
dbx-core from the database type alone, `db_type == Redshift`
(`postgres_prefers_text_protocol`, `crates/dbx-core/src/query/mod.rs:1831-1833`,
used at `query/mod.rs:2127`). No option, connection field or environment
variable reaches either, so a frontend cannot ask for exact text results.

## Losses by cause

66 entries in `KNOWN_DIFFERENCES.md` are LOSS (a wrong or missing value), all
on the binary path (`alone`); each is right beside a `timestamptz` column.

| cause | entries | what |
|---|---|---|
| no binary decoder: wire bytes shown | 44 | `int4range`, `numrange`, `tstzrange` (4 each), `int4multirange` (3), `tsquery` (3), `box` (2), `tid` (2), `pg_lsn` extremes (1), and, found by the second round of fixtures, `line`, `lseg`, `path`, `polygon`, `circle` (3 each), `jsonpath` (3), `xid8` (3) |
| float handling | 13 | `NaN` and the infinities as NULL (`float4` 3, `float8` 3, `numbers.approx` 2), `float4` rounded to six decimals (5) |
| arrays | 7 | lower bound dropped (2), `NaN` and infinities inside `float4[]` and `float8[]` as NULL (2), `bytea[]` elements decoded as text (1), `json[]` elements re-serialised compactly (2) |
| `"char"` | 2 | the byte's number for the character |

(`pg_lsn` is not decoded either: the value `16/B374D848` happens to read as hex
because its 8 bytes are not valid UTF-8, which is luck, and `0/0` shows as NUL
bytes. `money` also has no decoder in the strict sense but has a hand-written
one with its own problem; see below.)

### 1. `NaN`, `Infinity` and `-Infinity` come back as NULL

```sql
select 'NaN'::float8 as f;            -- dbx: null          psql: NaN
select 'Infinity'::float4 as f;       -- dbx: null          psql: Infinity
select array['NaN'::float8, 1] as f;  -- dbx: [null,1.0]    psql: {NaN,1}
select 'NaN'::float8 as f, now() as t;-- dbx: "NaN", ...    (text path: right)
```

A NULL and a NaN cannot be told apart, and `NaN` is a value people store.
`serde_json::Number::from_f64` returns `None` for a non-finite value, and the
code turns that into `Value::Null`:

- `postgres.rs:1473-1477` (`pg_fallback_value_to_json`, `f64`);
- `postgres.rs:1478-1484` (the same, `f32`);
- `postgres.rs:761-763` (`pg_float_number`, used for `float4[]` and `float8[]`
  at `postgres.rs:929` and `932`, and for scalar floats at `postgres.rs:461`
  and `466`).

### 2. `float4` is rounded to six decimals

```sql
select 1e-7::real as f;               -- dbx: 0.0           psql: 1e-07
select (1.0/3)::real as f;            -- dbx: 0.333333      psql: 0.33333334
select 1.17549435e-38::real as f;     -- dbx: 0.0           psql: 1.1754944e-38
select 1e-7::real as f, now() as t;   -- dbx: "1e-07", ...  (text path: right)
```

`postgres.rs:1478-1484` computes `(v as f64 * 1_000_000.0).round() / 1_000_000.0`.
Everything below 5e-7 becomes `0.0` (and `-1.5e-7` becomes `-0.0`), including
the smallest normal and subnormal `float4`. The same file already fixed this
for pgvector, `pg_vector_element_number` at `postgres.rs:1451` (issue #3931:
"casting through `f64`... truncates remaining single-precision digits"), with a
test, `pgvector_element_number_round_trips_full_f32_precision`. A `float4[]`
goes the other way: `select array[0.1::real]` gives `[0.10000000149011612]`
(widened with `v as f64`, `postgres.rs:929`), the same float4 spelled with
digits it does not have.

### 3. Types with no binary decoder come back as their wire bytes

```sql
select '[1,10)'::int4range as r;     -- dbx: "\u0002\0\0\0\u0004\0\0\0\u0001\0\0\0\u0004\0\0\0\n"   psql: [1,10)
select to_tsquery('simple', 'quick & fox');  -- dbx: "\0\0\0\u0003\u0002\u0002\u0001\0\0fox\0\u0001\0\0quick\0"   psql: 'quick' & 'fox'
select '(3,4),(1,2)'::box;           -- dbx: "0x4008000000000000..."   psql: (3,4),(1,2)
select '(0,1)'::tid;                 -- dbx: "\0\0\0\0\0\u0001"       psql: (0,1)
select 'x'::"char";                  -- dbx: "120"                    psql: x
select '16/B374D848'::pg_lsn;        -- dbx: "0x00000016b374d848"     psql: 16/B374D848
```

Also `int4multirange`, `numrange` and `tstzrange` (`daterange` is decoded), and
the types the review added: `line`, `lseg`, `path`, `polygon` and `circle` (their
big-endian `float8` coordinates, as `0x` hex or raw characters), `jsonpath` (a
version byte `1` before the text, so the text shows with a hidden `\u{1}`) and
`xid8` (its 8 bytes: `0` is eight NUL bytes, `18446744073709551615` is
`0xffffffffffffffff`).
Each is right beside a `timestamptz` column. The bytes reach the client as a
string, as text when they are valid UTF-8 (control characters) and as `0x` hex
otherwise (`binary_value_to_json`, in the last `or_else`s of
`pg_fallback_value_to_json`, `postgres.rs:1488-1490`). The cause is
`pg_type_requires_text_protocol` (`postgres.rs:1026-1050`): for a simple type
that `Type::from_oid` knows (all of the above) it returns false, though
`pg_fallback_value_to_json` has no decoder for them.

### 4. Smaller ones

- An array whose lower bound is not 1 loses it: `select '[0:2]={1,2,3}'::int4[]`
  gives `[1,2,3]`; psql `[0:2]={1,2,3}`.
- A `bytea[]`'s elements are decoded as text: `'{"\\xdead"}'::bytea[]` gives
  `["ޭ"]`; psql `{"\\xdead"}` (`pg_array_to_json_value`, the `PgAnyString`
  arm, `postgres.rs:933-935`).
- A `json[]`'s elements are parsed and written back compactly
  (`pg_json_array_values_to_json`, `postgres.rs:757-759`, called from `pg_array_to_json_value`, `postgres.rs:887`): `'{"b": 1,  "a": 2}'`
  comes back as `{"b":1,"a":2}`. The value and the key order survive; the text
  Postgres keeps does not. A `json` column on its own is exact.
- `money` (`format_pg_money`, `postgres.rs:303-311`) always divides the stored
  integer by 100. The stored integer is in the smallest unit of the database's
  `lc_monetary` currency, so a database whose locale has 0 or 3 fractional
  digits (a yen or a dinar locale) would show a money value wrong by a factor
  (100 or 10). Not reproducible on the fixture, whose `lc_monetary` is
  `en_US.utf8` (2 digits); stated as read from the code.

Not losses, only spelling (listed in `KNOWN_DIFFERENCES.md` as FORMAT): `money`
without `$` and grouping, `interval` as `3 days 00:00:00` and `-1 year`, `bytea`
as `0x...`, floats as `1.0` and `1e-7`, `timestamptz` as ISO 8601 on both
paths, arrays as JSON text.

## Two possible fixes

**A. A text-protocol option (the smallest change).** Add
`pub prefer_text_protocol: bool` (default false) to `QueryExecutionOptions`
(`dbx-core/src/query/mod.rs:1322`) and carry it to the driver's existing
prepared text-fallback path. In `start_postgres_select_stream`
(`postgres.rs:2077`) that path already runs when the server is known to ignore
binary results (`postgres.rs:2083-2089`) or when a column's type needs text
(`prepared_select_metadata`, `postgres.rs:2026-2030`): it **prepares** the
statement, keeps the **column types**, and returns `TextFallback`, after which
`finish_prepared_select` reads the rows as text through `execute_select_text`
(`postgres.rs:2344-2349`). The change is to treat the option like that first
condition (`if prefer_text || postgres_client_requires_text_results(client)`),
threaded through `execute_select_query_with_progress` and
`execute_select_prepared`. Because it prepares first, a multi-statement string
is refused by the server's Parse step as it is today, and nothing in dbx-core
(its read-only proof, row cap, timeouts, cancel) is bypassed: the change is
inside the driver, after those guards.

Do **not** reuse the Redshift route as it stands. `prefer_text_protocol` today
(`query/mod.rs:2127`) goes to `execute_select_text` with no prepare and no
column types (`postgres.rs:10083-10084`), which is a simple-protocol query: it
accepts several statements and gives the text path no type names, so the grid
would lose its column types.

Cheap, and exact for every case above: the harness's "beside a timestamptz
column" mode is this prepared text path, and it matches `psql` apart from
spelling. Cost: every value is a string, so the desktop app's numeric
formatting would not apply to a result that opts in; a client that wants typed
numbers must not opt in.

**B. Keep the binary path and fix its decoders.**

1. Floats: where `from_f64` gives `None`, return the string `"NaN"`,
   `"Infinity"` or `"-Infinity"`; dbx already returns strings where JSON cannot
   hold a number (`safe_i64_to_json`). `pg_float_number` at `postgres.rs:761`
   and the two sites at `1473-1484` are the whole change.
2. `float4`: use `v.to_string().parse::<serde_json::Number>()` as
   `pg_vector_element_number` does, for both the scalar and the array case.
3. Types without a decoder: have `pg_type_requires_text_protocol` return true
   for them (ranges, multiranges, `tsquery`, `box`, `tid`, `pg_lsn`, `"char"`,
   `line`, `lseg`, `path`, `polygon`, `circle`, `jsonpath`, `xid8` and any other
   type `pg_fallback_value_to_json` cannot decode), so they
   take the text path, which already works. That one change fixes section 3
   without writing a decoder for each.

A needs one flag and the plumbing described above; B keeps JSON numbers for the common case and
fixes the cause. B1 and B3 on their own would remove every LOSS except the
float4 rounding (B2), the array lower bound, a `bytea[]`, a `json[]` and the
`money` scale, which need a decoder each.

Test to add: like the one at `postgres.rs:12245-12262`, which already runs one
statement through `execute_query_with_max_rows_inner` with
`prefer_text_protocol` false and then true, run the statements of sections 1 to
3 both ways and assert that the two results agree.
