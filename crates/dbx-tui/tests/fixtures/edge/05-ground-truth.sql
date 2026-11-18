-- Tables for slice 08 (ground truth), in database `edge`: one table per type
-- family, every row labelled with what it probes, all data deterministic (no
-- random(), no now()). `tests/ground_truth.rs` compares every cell of every
-- table here with what `psql` prints, and `KNOWN_DIFFERENCES.md` lists the
-- cells where the client differs.
--
-- Apply to the running fixtures with:
--   docker exec -i dbxtui-fixtures-postgres-1 psql -U postgres -d edge \
--     -v ON_ERROR_STOP=1 -f - < tests/fixtures/edge/05-ground-truth.sql

-- Integers at every width and at their limits, including beyond 2^53 (a
-- double holds integers exactly only up to 2^53).
CREATE TABLE gt_integers (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    i2    smallint,
    i4    integer,
    i8    bigint,
    o     oid
);
INSERT INTO gt_integers VALUES
    (1,  'zeros',                  0,      0,           0,                    0),
    (2,  'minus one, one',         -1,     -1,          -1,                   1),
    (3,  'minimums',               -32768, -2147483648, -9223372036854775808, 0),
    (4,  'maximums',               32767,  2147483647,  9223372036854775807,  4294967295),
    (5,  '2^31',                   NULL,   NULL,        2147483648,           2147483648),
    (6,  '2^32',                   NULL,   NULL,        4294967296,           NULL),
    (7,  '2^53 - 1',               NULL,   NULL,        9007199254740991,     NULL),
    (8,  '2^53',                   NULL,   NULL,        9007199254740992,     NULL),
    (9,  '2^53 + 1',               NULL,   NULL,        9007199254740993,     NULL),
    (10, '-(2^53) - 1',            NULL,   NULL,        -9007199254740993,    NULL),
    (11, '2^63 - 2',               NULL,   NULL,        9223372036854775806,  NULL),
    (12, 'all NULL',               NULL,   NULL,        NULL,                 NULL);

-- numeric: precision and scale, negative zero, NaN and the infinities.
CREATE TABLE gt_numerics (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    n     numeric,
    n102  numeric(10,2),
    n4020 numeric(40,20),
    n300  numeric(30,0)
);
INSERT INTO gt_numerics VALUES
    (1,  '30 digits, no scale',      123456789012345678901234567890,                 NULL,        NULL,                                          999999999999999999999999999999),
    (2,  '20.20 digits',             12345678901234567890.12345678901234567890,      12345678.90, 12345678901234567890.12345678901234567890,     NULL),
    (3,  'negative 30 digits',       -123456789012345678901234567890.123456789,      -0.01,       -0.00000000000000000001,                       -1),
    (4,  'negative zero input',      '-0',                                           '-0.00',     '-0',                                          '-0'),
    (5,  'zero with scale',          0.000,                                          0,           0,                                             0),
    (6,  'NaN',                      'NaN',                                          'NaN',       'NaN',                                         'NaN'),
    (7,  'Infinity',                 'Infinity',                                     NULL,        NULL,                                          NULL),
    (8,  '-Infinity',                '-Infinity',                                    NULL,        NULL,                                          NULL),
    (9,  'tiny',                     0.000000000000000000000000000001,               0.001,       0.00000000000000000001,                        NULL),
    (10, 'huge (1e100)',             10::numeric ^ 100,                              NULL,        NULL,                                          NULL),
    (11, 'trailing zeros',           1.500000,                                       1.5,         1.50,                                          NULL),
    (12, 'rounds half up',           0.005,                                          0.005,       NULL,                                          NULL),
    (13, 'all NULL',                 NULL,                                           NULL,        NULL,                                          NULL);

-- float4 and float8: non-finite values, subnormals, values float4 cannot hold.
CREATE TABLE gt_floats (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    f4    real,
    f8    double precision
);
INSERT INTO gt_floats VALUES
    (1,  'zero',                     0,                  0),
    (2,  'negative zero',            '-0',               '-0'),
    (3,  'one tenth',                0.1,                0.1),
    (4,  'one third',                1.0/3,              1.0/3),
    (5,  'one in ten million',       1e-7,               1e-7),
    (6,  'one in a million',         1e-6,               1e-6),
    (7,  '2^24 + 1',                 16777217,           16777217),
    (8,  'largest finite',           3.4028235e38,       1.7976931348623157e308),
    (9,  'smallest normal',          1.17549435e-38,     2.2250738585072014e-308),
    (10, 'subnormal',                1e-45,              5e-324),
    (11, 'NaN',                      'NaN',              'NaN'),
    (12, 'Infinity',                 'Infinity',         'Infinity'),
    (13, '-Infinity',                '-Infinity',        '-Infinity'),
    (14, 'integral, 1e8',            100000000,          100000000),
    (15, 'integral, 1e15',           1e15,               1e15),
    (16, 'plain fraction',           1234.5678,          1234.5678),
    (17, 'small negative',           -1.5e-7,            -1.5e-7),
    (18, 'sum that is not 0.3',      0.3,                0.1::float8 + 0.2),
    (19, 'all NULL',                 NULL,               NULL);

-- Text: everything the grid and the viewer must keep apart or draw safely.
CREATE TABLE gt_text (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    t     text,
    v     varchar(30),
    c     char(6)
);
INSERT INTO gt_text VALUES
    (1,  'null',                    NULL,                             NULL,       NULL),
    (2,  'empty string',            '',                               '',         ''),
    (3,  'the text NULL',           'NULL',                           'NULL',     'NULL'),
    (4,  'the text null',           'null',                           'null',     'null'),
    (5,  'one space',               ' ',                              ' ',        ' '),
    (6,  'whitespace only',         E'  \t  ',                        E'\t',      E'\t'),
    (7,  'padded char',             'ab',                             'ab',       'ab'),
    (8,  'leading and trailing',    '  both  ',                       ' x ',      ' y'),
    (9,  'emoji',                   '😀 🎉',                          '😀',       '😀'),
    (10, 'combining marks',         E'é ä č',  E'é', 'é'),
    (11, 'right to left',           'שלום עולם مرحبا',                'مرحبا',    'שלום'),
    (12, 'ZWJ sequence',            E'\U0001F468‍\U0001F469‍\U0001F467', E'\U0001F468‍\U0001F469', 'x'),
    (13, 'flag',                    E'\U0001F1EF\U0001F1F5',          E'\U0001F1EF\U0001F1F5', 'JP'),
    (14, 'tabs and newlines',       E'a\tb\nc\r\nd\re',               E'x\ny',    E'p\tq'),
    (15, 'backslash and quotes',    E'\\ '' "',                       E'\\n',     E'"'),
    (16, 'separators',              'a|b,c;d',                        '|',        ','),
    (17, 'the glyphs the grid uses', '∅ ⏎ ⇥ ⊘ …',                    '∅',        '⏎'),
    (18, 'escape and bell',         E'\x1b[31mred\x1b[0m\x07',        E'\x1b',    E'\x7f'),
    (19, 'bidi override',           E'abc‮def',                  E'⁦x⁩', 'z'),
    (20, 'CJK and fullwidth',       '日本語 ＡＢＣ',                  '한국어',   'ｱｲｳ'),
    (21, '100 kB',                  repeat('0123456789abcdef', 6400), NULL,       NULL),
    (22, 'long line, 5000 chars',   repeat('x', 5000),                NULL,       NULL);

-- Time: zones, precision, infinity dates, intervals.
CREATE TABLE gt_temporal (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    d     date,
    t     time,
    ttz   timetz,
    ts    timestamp,
    tstz  timestamptz,
    iv    interval
);
INSERT INTO gt_temporal VALUES
    (1,  'epoch',                 '1970-01-01', '00:00:00',           '00:00:00+00',          '1970-01-01 00:00:00',            '1970-01-01 00:00:00+00',         '0'),
    (2,  'microseconds',          '2024-03-05', '13:45:12.123456',    '13:45:12.123456+02',   '2024-03-05 13:45:12.123456',     '2024-03-05 13:45:12.123456+02',  '1 year 2 mons 3 days 04:05:06.789'),
    (3,  'far future',            '5874897-12-31', '23:59:59.999999', '23:59:59.999999-12',   '294276-12-31 23:59:59.999999',   '294276-12-31 23:59:59.999999+00', '178000000 years'),
    (4,  'year one',              '0001-01-01', '00:00:01',           '00:00:01+14',          '0001-01-01 00:00:00',            '0001-01-01 00:00:00+00',         '-178000000 years'),
    (5,  'before the common era', '4713-01-01 BC', '12:00:00',        '12:00:00-05:30',       '4713-01-01 00:00:00 BC',         '4713-11-24 00:00:00+00 BC',      '-1 year -2 mons -3 days -04:05:06'),
    (6,  'infinity',              'infinity',   NULL,                 NULL,                   'infinity',                       'infinity',                       NULL),
    (7,  '-infinity',             '-infinity',  NULL,                 NULL,                   '-infinity',                      '-infinity',                      NULL),
    (8,  'leap day',              '2024-02-29', '23:59:60',           '23:59:60+00',          '2024-02-29 23:59:59',            '2024-02-29 23:59:59+00',         '1 mon -1 day'),
    (9,  'offset -05:30',         '2024-06-01', '24:00:00',           '24:00:00-05:30',       '2024-06-01 00:00:00',            '2024-06-01 00:00:00-05:30',      '00:00:00.000001'),
    (10, 'offset +14',            '2024-06-01', '00:00:00.5',         '00:00:00.5+14',        '2024-06-01 00:00:00.5',          '2024-06-01 00:00:00+14',         '-00:00:00.000001'),
    (11, 'DST change in New York', '2024-03-10', '02:30:00',          '02:30:00-05',          '2024-03-10 02:30:00',            '2024-03-10 07:30:00+00',         '1 day 25 hours'),
    (12, 'whole-second values',   '2000-01-01', '12:34:56',           '12:34:56+00',          '2000-01-01 12:34:56',            '2000-01-01 12:34:56+00',         '3 days'),
    (13, 'all NULL',              NULL,         NULL,                 NULL,                   NULL,                             NULL,                             NULL);

-- bool, uuid and bytea.
CREATE TABLE gt_bool_uuid_bytea (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    flag  boolean,
    uid   uuid,
    bin   bytea
);
INSERT INTO gt_bool_uuid_bytea VALUES
    (1, 'true, a uuid, bytes',     true,  'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', '\xdeadbeef00ff'),
    (2, 'false, nil uuid, empty',  false, '00000000-0000-0000-0000-000000000000', '\x'),
    (3, 'NULL, max uuid, one NUL', NULL,  'ffffffff-ffff-ffff-ffff-ffffffffffff', '\x00'),
    (4, 'all 256 byte values',     NULL,  NULL, decode((SELECT string_agg(lpad(to_hex(i), 2, '0'), '' ORDER BY i) FROM generate_series(0, 255) i), 'hex')),
    (5, 'bytes that read as text', NULL,  NULL, 'hello'::bytea),
    (6, 'all NULL',                NULL,  NULL, NULL);

-- json and jsonb: key order, duplicate keys, number precision, scalars.
CREATE TABLE gt_json (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    j     json,
    jb    jsonb
);
INSERT INTO gt_json VALUES
    (1,  'object',                    '{"b": 1, "a": 2}',                      '{"b": 1, "a": 2}'),
    (2,  'big and exact numbers',     '{"big": 12345678901234567890, "dec": 1.10, "exp": 1E+2}', '{"big": 12345678901234567890, "dec": 1.10, "exp": 1E+2}'),
    (3,  'duplicate keys',            '{"k": 1, "k": 2}',                      '{"k": 1, "k": 2}'),
    (4,  'odd whitespace',            E'{ "a" :\n  [1 ,2] }',                  E'{ "a" :\n  [1 ,2] }'),
    (5,  'empty array and object',    '[]',                                    '{}'),
    (6,  'JSON null',                 'null',                                  'null'),
    (7,  'scalars',                   '"text"',                                '3.0'),
    (8,  'unicode escapes',           '"é 😀"',                 '"é 😀"'),
    (9,  'nested',                    '{"a": {"b": [1, {"c": null}]}}',        '{"a": {"b": [1, {"c": null}]}}'),
    (10, 'huge exponent',             '1E+400',                                '1E+400'),
    (11, 'true and false',            'true',                                  'false'),
    (12, 'SQL NULL',                  NULL,                                    NULL);

-- Arrays: nested, with NULLs, with elements that need quoting.
CREATE TABLE gt_arrays (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    ai    integer[],
    at    text[],
    an    numeric[],
    ab    boolean[],
    af    float8[],
    ats   timestamptz[],
    ay    bytea[],
    au    uuid[],
    aj    jsonb[],
    a4    real[],
    ajs   json[]
);
INSERT INTO gt_arrays VALUES
    (1, 'simple',                  '{1,2}',                    '{a,b}',                               '{1.10,2}',            '{t,f}',        '{1.5,2}',              '{"2024-03-05 13:45:12+02"}', '{"\\xdead"}',   '{a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11}', ARRAY['{"a": 1}'::jsonb]),
    (2, 'empty',                   '{}',                       '{}',                                  '{}',                  '{}',           '{}',                   '{}',                         '{}',            '{}',                                     '{}'),
    (3, 'with NULL elements',      '{1,NULL,3}',               '{a,NULL,"NULL"}',                     '{NULL,1}',            '{t,NULL}',     '{NULL,1}',             '{NULL}',                     '{NULL}',        '{NULL}',                                 '{NULL}'),
    (4, 'nested 2-D',              '{{1,2},{3,4}}',            '{{a,b},{c,d}}',                       '{{1,2},{3,4}}',       '{{t,f},{f,t}}', '{{1,2},{3,4}}',       NULL,                         NULL,            NULL,                                     NULL),
    (5, 'elements that need quotes', NULL,                     NULL,                                  NULL,        NULL,           NULL,                   NULL,                         NULL,            NULL,                                     NULL),
    (6, 'lower bound not 1',       '[0:2]={1,2,3}',            '[-1:0]={x,y}',                        NULL,                  NULL,           NULL,                   NULL,                         NULL,            NULL,                                     NULL),
    (7, 'NaN and Infinity',        NULL,                       NULL,                                  '{NaN}',               NULL,           '{NaN,Infinity,-Infinity}', NULL,                     NULL,            NULL,                                     NULL),
    (8, 'big integers',            '{2147483647,-2147483648}', NULL,                                  '{123456789012345678901234567890}', NULL, NULL,                  NULL,                         NULL,            NULL,                                     NULL),
    (9, 'all NULL',                NULL,                       NULL,                                  NULL,                  NULL,           NULL,                   NULL,                         NULL,            NULL,                                     NULL);
-- float4 arrays: dbx widens each element to f64 (exact) and returns NULL for NaN.
UPDATE gt_arrays SET a4 = '{0.1,1e-7,16777217}' WHERE id = 1;
UPDATE gt_arrays SET a4 = '{NULL}' WHERE id = 3;
UPDATE gt_arrays SET a4 = '{NaN,Infinity}' WHERE id = 7;
-- json[]: Postgres keeps each element's text as written (key order, spacing).
UPDATE gt_arrays SET ajs = ARRAY['{"b": 1,  "a": 2}'::json] WHERE id = 1;
UPDATE gt_arrays SET ajs = '{}' WHERE id = 2;
UPDATE gt_arrays SET ajs = ARRAY[NULL, 'null'::json, '[1 ,2]'::json] WHERE id = 3;
-- Row 5's text[] holds elements that need quoting, built with ARRAY[] so the
-- quoting is the server's own.
UPDATE gt_arrays SET at = ARRAY['a b', 'c,d', 'e"f', 'g\h', '', '{}', 'NULL', NULL] WHERE id = 5;

-- enum, domain, composite and range types.
CREATE TYPE gt_mood AS ENUM ('sad', 'ok', 'happy', 'a b', 'it''s');
CREATE DOMAIN gt_posint AS integer CHECK (VALUE > 0);
CREATE DOMAIN gt_word AS text CHECK (VALUE <> '');
CREATE TYPE gt_pair AS (a integer, b text);
CREATE TABLE gt_types (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    mood  gt_mood,
    posi  gt_posint,
    word  gt_word,
    pair  gt_pair,
    ir    int4range,
    nr    numrange,
    dr    daterange,
    tr    tstzrange,
    mr    int4multirange
);
INSERT INTO gt_types VALUES
    (1, 'plain values',           'ok',    7,    'word',  '(1,"a b")',  '[1,10)',       '[1.5,2.5]',                  '[2024-01-01,2024-02-01)', '[2024-03-05 13:45:12+02,2024-03-06 00:00:00+00)', '{[1,3),[5,7)}'),
    (2, 'quoted enum, unbounded ranges', 'a b',   2147483647, 'a b', '(,)',        '(,)',          '(,2.5)',                     '[2024-01-01,)',           '(,)',                                 '{}'),
    (3, 'apostrophe enum, empty ranges', 'it''s', 1,    'x',     '(3,"")',      'empty',        'empty',                      'empty',                   'empty',                               '{[1,2]}'),
    (4, 'extreme bounds',         'happy', 1,    'y',     '(-5,"x,y")', '[-2147483648,2147483646]', '[0.000000000000000000001,1e30]', '[-infinity,infinity]', '[-infinity,infinity]',     NULL),
    (5, 'all NULL',               NULL,    NULL, NULL,    NULL,         NULL,           NULL,                         NULL,                      NULL,                                  NULL);

-- Network address types.
CREATE TABLE gt_network (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    ip    inet,
    net   cidr,
    mac   macaddr,
    mac8  macaddr8
);
INSERT INTO gt_network VALUES
    (1, 'IPv4 with mask',     '192.168.1.5/24',     '192.168.1.0/24', '08:00:2b:01:02:03', '08:00:2b:01:02:03:04:05'),
    (2, 'IPv4 host',          '10.0.0.1',           '10.0.0.0/8',     'ff:ff:ff:ff:ff:ff', 'ff:ff:ff:ff:ff:ff:ff:ff'),
    (3, 'IPv6',               '2001:db8::1',        '2001:db8::/32',  '00:00:00:00:00:00', '00:00:00:00:00:00:00:00'),
    (4, 'IPv6 loopback',      '::1/128',            '::/0',           NULL,                NULL),
    (5, 'IPv4-mapped IPv6',   '::ffff:1.2.3.4',     '0.0.0.0/0',      NULL,                NULL),
    (6, 'all NULL',           NULL,                 NULL,             NULL,                NULL);

-- money, bit strings and the rest of the less common types.
CREATE TABLE gt_money_bits (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    m     money,
    b8    bit(8),
    vb    varbit,
    b1    bit(1)
);
INSERT INTO gt_money_bits VALUES
    (1, 'plain',            '12.50',                       B'10101010', B'101',  B'1'),
    (2, 'thousands',        '1234567.89',                  B'00000000', B'',     B'0'),
    (3, 'negative',         '-0.01',                       B'11111111', B'0',    NULL),
    (4, 'largest',          '92233720368547758.07',        NULL,        B'1111000011110000111100001111000011110000', NULL),
    (5, 'smallest',         '-92233720368547758.08',       NULL,        NULL,    NULL),
    (6, 'all NULL',         NULL,                          NULL,        NULL,    NULL);

CREATE TABLE gt_documents (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    x     xml,
    tsv   tsvector,
    tsq   tsquery
);
INSERT INTO gt_documents VALUES
    (1, 'simple',            '<a x="1">text</a>',                  to_tsvector('simple', 'The quick brown fox'),  to_tsquery('simple', 'quick & fox')),
    (2, 'declaration',       '<?xml version="1.0"?><r><c/></r>',   'a:1,3 b:2',                                  'a | b'),
    (3, 'entities',          '<a>&lt;&amp;&gt;</a>',               $$'it''s' 'a b':1$$,                          '!a'),
    (4, 'all NULL',          NULL,                                 NULL,                                         NULL);

CREATE TABLE gt_other_types (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    pt    point,
    bx    box,
    lsn   pg_lsn,
    cls   regclass,
    nm    name,
    ch    "char",
    tid_v tid
);
INSERT INTO gt_other_types VALUES
    (1, 'plain',     '(1.5,-2)', '((3,4),(1,2))', '16/B374D848', 'gt_other_types', 'a name',  'x', '(0,1)'),
    (2, 'extremes',  '(1e300,0)', '((0,0),(0,0))', '0/0',        'pg_class',        '',        '',  '(4294967295,65535)'),
    (3, 'all NULL',  NULL,       NULL,            NULL,          NULL,              NULL,      NULL, NULL);

-- ---- added after the review of the harness ------------------------------------

-- Arrays of date, timestamp, bigint, enum and composite.
CREATE TABLE gt_arrays_more (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    ad    date[],
    ats   timestamp[],
    ai8   bigint[],
    ae    gt_mood[],
    ac    gt_pair[]
);
INSERT INTO gt_arrays_more VALUES
    (1, 'simple',             '{2024-03-05,1970-01-01}', '{"2024-03-05 13:45:12.123456","1970-01-01 00:00:00"}', '{9007199254740993,-9223372036854775808}', ARRAY['ok','a b','it''s']::gt_mood[], ARRAY[ROW(1,'a b')::gt_pair, ROW(2,NULL)::gt_pair]),
    (2, 'empty',              '{}', '{}', '{}', '{}', '{}'),
    (3, 'with NULL elements', '{NULL,2024-02-29}', '{NULL}', '{NULL,1}', ARRAY[NULL,'sad']::gt_mood[], ARRAY[NULL, ROW(NULL,NULL)::gt_pair]),
    (4, 'infinity and BC',    '{infinity,-infinity,"4713-01-01 BC"}', '{infinity,-infinity,"4713-01-01 00:00:00 BC"}', '{9223372036854775807}', NULL, ARRAY[ROW(-5,'x,y')::gt_pair, ROW(0,'')::gt_pair]),
    (5, 'all NULL',           NULL, NULL, NULL, NULL, NULL);

-- Fractional-second precision.
CREATE TABLE gt_precision (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    ts3   timestamp(3),
    t3    time(3)
);
INSERT INTO gt_precision VALUES
    (1, 'whole second',          '2024-03-05 12:00:00',          '12:00:00'),
    (2, 'milliseconds',          '2024-03-05 13:45:12.123',      '13:45:12.123'),
    (3, 'rounds to a millisecond', '2024-03-05 13:45:12.1236',   '13:45:12.1236'),
    (4, 'rounds up a whole day', '2024-12-31 23:59:59.9996',     '23:59:59.9996'),
    (5, 'infinity',              'infinity',                     NULL),
    (6, 'all NULL',              NULL,                           NULL);

-- Geometric types other than point and box.
CREATE TABLE gt_geometric (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    ln    line,
    ls    lseg,
    pa    path,
    po    polygon,
    ci    circle
);
INSERT INTO gt_geometric VALUES
    (1, 'plain',              '{1,-1,0}',     '[(0,0),(1,1)]',     '[(0,0),(1,1),(2,0)]', '((0,0),(4,0),(4,3))',          '<(1,2),3>'),
    (2, 'closed path, fractions', '{0.5,2,-3.25}', '[(1.5,-2),(0,0.1)]', '((0,0),(1,1),(2,0))', '((0.1,0.2),(10000000000,0),(0,1))', '<(0,0),0.5>'),
    (3, 'degenerate and extreme', '{1e300,1,0}', '[(0,0),(0,0)]',   '[(1,1)]',             '((1,1))',                      '<(1e300,0),1e-300>'),
    (4, 'all NULL',           NULL,           NULL,                NULL,                  NULL,                           NULL);

-- jsonpath, xid8 and regtype.
CREATE TABLE gt_catalog_types (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    jp    jsonpath,
    x8    xid8,
    rt    regtype
);
INSERT INTO gt_catalog_types VALUES
    (1, 'plain',        '$.a[*] ? (@ > 1)',                  '0',                    'integer'),
    (2, 'extremes',     'strict $."a b".**{1 to last}',      '18446744073709551615', 'gt_mood'),
    (3, 'array type, beyond 2^32', '$',                      '4294967296',           'text[]'),
    (4, 'all NULL',     NULL,                                NULL,                   NULL);

-- Computed columns: what an aggregate or array_agg returns has no table column
-- to take its type from. (A view cannot hold a `row(...)`: the harness runs that
-- one, and other computed queries, itself; see `COMPUTED` in ground_truth.rs.)
CREATE VIEW gt_computed AS
SELECT v.grp                         AS id,
       'group ' || v.grp             AS label,
       sum(v.n)                      AS sum_numeric,
       sum(v.i)                      AS sum_int,
       sum(v.i::bigint)              AS sum_bigint,
       avg(v.i)                      AS avg_int,
       sum(v.f)                      AS sum_float,
       count(*)                      AS cnt,
       count(v.n)                    AS cnt_col,
       array_agg(v.i ORDER BY v.i, v.t) AS agg_int,
       array_agg(v.n ORDER BY v.i, v.t) AS agg_numeric,
       array_agg(v.t ORDER BY v.i, v.t) AS agg_text,
       json_agg(v.t ORDER BY v.i, v.t)  AS agg_json,
       jsonb_agg(v.t ORDER BY v.i, v.t) AS agg_jsonb
FROM (VALUES
    (1, 1.10::numeric,                       100,        'a'::text, 0.1::float8),
    (1, 2.20,                                200,        'b',       0.2),
    (2, 12345678901234567890.123456789,      2147483647, 'c',       1e-7),
    (2, NULL,                                2147483647, NULL,      NULL),
    (3, NULL,                                NULL,       NULL,      NULL)
) AS v(grp, n, i, t, f)
GROUP BY v.grp;
