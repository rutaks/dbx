-- The edge-case schema for layer 1 (docs/process.md, Testing): values whose
-- exact text is known, so a test can compare what the client shows with what
-- was stored. These are the first tables; the 100+ column and million-row
-- tables arrive with the result grid (slice 07). Loaded into database `edge`.

-- Text: NULL vs '' vs the text 'NULL', whitespace, multi-line, wide characters.
CREATE TABLE text_values (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    v     text
);
INSERT INTO text_values VALUES
    (1, 'null',            NULL),
    (2, 'empty',           ''),
    (3, 'text NULL',       'NULL'),
    (4, 'one space',       ' '),
    (5, 'tab',             E'\t'),
    (6, 'multi-line',      E'line1\nline2\n\tindented'),
    (7, 'wide',            '日本語 😀 👨‍👩‍👧 ａｂｃ'),
    (8, 'trailing spaces', 'abc   ');

-- Numbers: integers beyond 2^53, exact decimals, floats including NaN and
-- Infinity (which dbx returns as NULL: design section 2, known inaccuracy).
CREATE TABLE numbers (
    id       integer PRIMARY KEY,
    label    text NOT NULL,
    big      bigint,
    exact    numeric,
    approx   double precision
);
INSERT INTO numbers VALUES
    (1, '2^53 + 1',     9007199254740993,     12345678901234567890.123456789, 0.1),
    (2, 'bigint min',   -9223372036854775808, 0.000000000000000000001,        0.30000000000000004),
    (3, 'bigint max',   9223372036854775807,  -1.10,                          'NaN'),
    (4, 'infinities',   0,                    1e-30,                          'Infinity'),
    (5, 'nulls',        NULL,                 NULL,                           NULL);

-- Time: zones, precision, dates outside the common range.
CREATE TABLE temporal (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    tstz  timestamptz,
    ts    timestamp,
    d     date,
    iv    interval
);
INSERT INTO temporal VALUES
    (1, 'offset +02',  '2024-03-05 13:45:12.123456+02', '2024-03-05 13:45:12.123456', '2024-03-05', '1 day 02:03:04'),
    (2, 'epoch',       '1970-01-01 00:00:00+00',        '1970-01-01 00:00:00',        '1970-01-01', '0'),
    (3, 'far',         '9999-12-31 23:59:59+00',        '0001-01-01 00:00:00',        '4713-01-01 BC', '-1 year');

-- Other families: bool, json, binary, uuid, arrays.
CREATE TABLE other_types (
    id    integer PRIMARY KEY,
    flag  boolean,
    doc   jsonb,
    bin   bytea,
    uid   uuid,
    arr   integer[]
);
INSERT INTO other_types VALUES
    (1, true,  '{"big": 12345678901234567890, "dec": 1.10}', '\xdeadbeef00ff', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', '{1,2}'),
    (2, false, '[]',                                          '\x',             NULL,                                    '{}'),
    (3, NULL,  'null',                                        NULL,             NULL,                                    NULL);

-- Shapes: empty, no primary key, composite key.
CREATE TABLE empty_table (id integer PRIMARY KEY, v text);

CREATE TABLE no_pk (v text, n integer);
INSERT INTO no_pk VALUES ('dup', 1), ('dup', 1), (NULL, NULL);

CREATE TABLE composite_key (
    a integer,
    b text,
    v text,
    PRIMARY KEY (a, b)
);
INSERT INTO composite_key VALUES (1, 'x', 'first'), (1, 'y', 'second'), (2, 'x', 'third');

-- Odd identifiers: mixed case, spaces, reserved words.
CREATE TABLE "Mixed Case" ("Id" integer PRIMARY KEY, "select" text, "two words" text);
INSERT INTO "Mixed Case" VALUES (1, 'reserved', 'spaced');

-- A second schema.
CREATE SCHEMA other;
CREATE TABLE other.things (id integer PRIMARY KEY, name text);
INSERT INTO other.things VALUES (1, 'in the second schema');
