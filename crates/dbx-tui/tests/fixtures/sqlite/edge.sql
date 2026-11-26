-- Edge cases for the SQLite ground truth and the SQLite journeys. build.sh
-- loads this into edge.db. Every table has an integer `id` (except the ones
-- that are about having no key), so a read can order by it.

-- NULL, the empty string and the text 'NULL' must stay three different things.
CREATE TABLE text_values (id INTEGER PRIMARY KEY, label TEXT, v TEXT);
INSERT INTO text_values VALUES
  (1, 'null', NULL),
  (2, 'empty string', ''),
  (3, 'the text NULL', 'NULL'),
  (4, 'one space', ' '),
  (5, 'the text null', 'null'),
  (6, 'quotes and backslash', 'it''s a "quote" \ and \n not a newline'),
  (7, 'a pipe and a backtick', 'a|b`c'),
  (8, 'leading and trailing space', '  padded  ');

-- Integers past 2^53 and at the i64 limits.
CREATE TABLE integers (id INTEGER PRIMARY KEY, label TEXT, v INTEGER);
INSERT INTO integers VALUES
  (1, '2^53', 9007199254740992),
  (2, '2^53 + 1', 9007199254740993),
  (3, 'i64 max', 9223372036854775807),
  (4, 'i64 min', -9223372036854775808),
  (5, 'zero', 0),
  (6, 'minus one', -1),
  (7, 'null', NULL),
  (8, 'i64 max - 1', 9223372036854775806);

-- Reals. SQLite has no NaN: an expression that would give one stores NULL. It
-- does store infinity (a literal too large for a double).
CREATE TABLE reals (id INTEGER PRIMARY KEY, label TEXT, v REAL);
INSERT INTO reals VALUES
  (1, 'one in ten million', 1e-7),
  (2, 'largest double', 1.7976931348623157e308),
  (3, 'smallest subnormal', 5e-324),
  (4, 'a tenth', 0.1),
  (5, 'negative zero', -0.0),
  (6, '1e15', 1e15),
  (7, '1e16', 1e16),
  (8, 'many digits', 123456789.123456789),
  (9, 'infinity', 9e999),
  (10, 'minus infinity', -9e999),
  (11, 'NaN stored', 0e0 / 0),
  (12, 'whole', 3.0),
  (13, 'null', NULL);

-- Text: emoji, a ZWJ sequence, CJK, full-width forms, line breaks, a tab.
CREATE TABLE texts (id INTEGER PRIMARY KEY, label TEXT, v TEXT);
INSERT INTO texts VALUES
  (1, 'emoji', '😀 grin'),
  (2, 'zwj family', '👨‍👩‍👧'),
  (3, 'cjk', '日本語のテキスト'),
  (4, 'full width', 'ａｂｃ１２３'),
  (5, 'combining accent', 'e' || char(769)),
  (6, 'line break', 'first
second'),
  (7, 'carriage return', 'a' || char(13) || 'b'),
  (8, 'tab', 'a' || char(9) || 'b'),
  (9, 'right to left', 'שלום'),
  (10, 'flag', '🇰🇪'),
  (11, 'long', (SELECT group_concat('row ' || n, ' ') FROM (WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 2000) SELECT n FROM c)));

-- Blobs and a JSON text column.
CREATE TABLE blobs (id INTEGER PRIMARY KEY, label TEXT, b BLOB);
INSERT INTO blobs VALUES
  (1, 'empty', x''),
  (2, 'one zero byte', x'00'),
  (3, 'reads as text', x'68656c6c6f'),
  (4, 'not utf-8', x'ff00fe'),
  (5, 'null', NULL),
  (6, '1 kB of zeros', zeroblob(1024));

CREATE TABLE documents (id INTEGER PRIMARY KEY, label TEXT, doc JSON);
INSERT INTO documents VALUES
  (1, 'object', '{"b": 1,  "a": [1, 2, {"c": null}]}'),
  (2, 'array', '[1, "two", 3.5]'),
  (3, 'not json', 'plain text'),
  (4, 'unicode escape', '{"k": "é"}'),
  (5, 'null', NULL);

-- Dates stored as text, as integers and as reals, under date-like type names.
CREATE TABLE dates (id INTEGER PRIMARY KEY, label TEXT, d DATE, ts DATETIME, stamp TIMESTAMP, unix INTEGER, julian REAL);
INSERT INTO dates VALUES
  (1, 'iso', '2024-03-05', '2024-03-05 13:45:12', '2024-03-05 13:45:12.123', 1709646312, 2460374.0),
  (2, 'with zone', '2024-03-05', '2024-03-05T13:45:12+05:30', '2024-03-05 13:45:12Z', 0, 0.5),
  (3, 'epoch', '1970-01-01', '1970-01-01 00:00:00', '1970-01-01 00:00:00', 0, 2440587.5),
  (4, 'not a date', 'tomorrow', '', 'NULL', -1, NULL),
  (5, 'null', NULL, NULL, NULL, NULL, NULL);

-- Declared types dbx decodes differently from each other.
CREATE TABLE typed (id INTEGER PRIMARY KEY, b BOOLEAN, n NUMERIC, dec DECIMAL(10,5), vc VARCHAR(5), fl FLOAT, dbl DOUBLE PRECISION, big BIGINT, small SMALLINT, ch CHAR(3), cl CLOB);
INSERT INTO typed VALUES
  (1, 1, 12.50, 3.14159, 'abc', 1.5, 2.5, 9007199254740993, 7, 'ab', 'x'),
  (2, 0, 9007199254740993, 0.00001, 'a longer text than five', 1e-7, 1e300, -9223372036854775808, -7, 'abcdef', 'long clob'),
  (3, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  (4, 2, '12.50', '1.0', '', 0.1, 0.1, 0, 0, '', ''),
  (5, 'true', 'text in a numeric', 'text in a decimal', 5, 'text in a float', 'text in a double', 'text in a bigint', 'text', 5, 5);

-- A column without a type holds whatever each row gives it.
CREATE TABLE untyped (id INTEGER PRIMARY KEY, label TEXT, v);
INSERT INTO untyped VALUES
  (1, 'integer', 1),
  (2, 'real', 1.5),
  (3, 'text', 'x'),
  (4, 'blob', x'00ff'),
  (5, 'null', NULL),
  (6, 'big integer', 9007199254740993),
  (7, 'number as text', '42'),
  (8, 'infinity', 9e999),
  (9, 'i64 min', -9223372036854775808);

-- No primary key (a rowid table), and a composite key.
CREATE TABLE no_pk (a TEXT, b INTEGER);
INSERT INTO no_pk VALUES ('x', 1), ('x', 1), ('y', NULL), (NULL, 2);

CREATE TABLE composite_key (a INTEGER, b TEXT, v TEXT, PRIMARY KEY (a, b));
INSERT INTO composite_key VALUES (1, 'one', 'first'), (1, 'two', 'second'), (2, 'one', 'third'), (2, 'two', NULL);

-- A quoted, mixed-case table and columns that are reserved words or hold spaces.
CREATE TABLE "Mixed Case" ("select" TEXT, "Order Id" INTEGER PRIMARY KEY, "from" TEXT);
INSERT INTO "Mixed Case" VALUES ('reserved', 1, 'also reserved'), ('words', 2, NULL);

-- A table named with quotes and a second statement: a name must never be
-- spliced into SQL unquoted.
CREATE TABLE "x""; select 1 --" (id INTEGER PRIMARY KEY, v TEXT);
INSERT INTO "x""; select 1 --" VALUES (1, 'one known row');

-- Tables named `"x"` and `[x]`, quote characters and brackets in the name,
-- beside a plain `x`: SQLite reads `"x"` and `[x]` as quoted spellings of `x`,
-- so a name passed through unescaped would read the wrong table, not fail.
CREATE TABLE x (id INTEGER PRIMARY KEY, v TEXT);
INSERT INTO x VALUES (1, 'the plain x: the wrong table');
CREATE TABLE """x""" (id INTEGER PRIMARY KEY, v TEXT);
INSERT INTO """x""" VALUES (1, 'the table named "x"'), (2, 'its second row');
CREATE TABLE "[x]" (id INTEGER PRIMARY KEY, v TEXT);
INSERT INTO "[x]" VALUES (1, 'the table named [x]'), (2, 'its second row'), (3, 'its third row');

CREATE TABLE empty_table (id INTEGER PRIMARY KEY, v TEXT);

-- A table with a foreign key, for `.join` completion.
CREATE TABLE parent (id INTEGER PRIMARY KEY, name TEXT);
INSERT INTO parent VALUES (1, 'p1'), (2, 'p2');
CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parent (id), note TEXT);
INSERT INTO child VALUES (1, 1, 'c1'), (2, 1, 'c2'), (3, 2, 'c3'), (4, NULL, 'orphan');

CREATE VIEW edge_view AS SELECT t.id, t.label, i.v AS big FROM text_values t JOIN integers i ON i.id = t.id;

-- A large table for paging and cancelling.
CREATE TABLE numbers_10k (id INTEGER PRIMARY KEY, txt TEXT);
WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 10000)
INSERT INTO numbers_10k SELECT n, 'row ' || n FROM c;
