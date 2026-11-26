-- Database `edge` (and `edge_other`) for slice 23b, on MySQL 8.4: every row is
-- labelled with what it probes and all data is deterministic (no rand(), no
-- now()). `tests/ground_truth_mysql.rs` compares every cell of every table
-- and view here with what `mysql --xml` prints, and
-- `KNOWN_DIFFERENCES-mysql.md` lists the cells where the client differs.
--
-- Loaded by init-mysql/10-load.sh when the container starts. Apply to the
-- running fixtures by hand with:
--   docker exec -i dbxtui-fixtures-mysql-mysql-1 mysql -uroot -pdbxtui \
--     --default-character-set=utf8mb4 < tests/fixtures/mysql/edge/01-edge.sql
SET NAMES utf8mb4;
SET SESSION sql_mode = 'STRICT_ALL_TABLES,NO_ENGINE_SUBSTITUTION';

DROP DATABASE IF EXISTS edge;
DROP DATABASE IF EXISTS edge_other;
CREATE DATABASE edge DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;
CREATE DATABASE edge_other DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;
USE edge;

-- NULL, '' and 'NULL' are three different values; so are text of known width.
CREATE TABLE text_values (
    id INT PRIMARY KEY,
    v  VARCHAR(100)
);
INSERT INTO text_values VALUES
    (1, NULL),
    (2, ''),
    (3, 'NULL'),
    (4, ' '),
    (5, 'null'),
    (6, 'it''s a "quote" and a \\ backslash'),
    (7, '日本語 😀 👨‍👩‍👧 ａｂｃ'),
    (8, 'line one
line two'),
    (9, CONCAT('tab', CHAR(9), 'inside')),
    (10, CONCAT('cr', CHAR(13), 'lf', CHAR(10), 'end'));

-- Integers at every width and at their limits, past 2^53 included.
CREATE TABLE gt_integers (
    id    INT PRIMARY KEY,
    label VARCHAR(40) NOT NULL,
    i1    TINYINT,
    u1    TINYINT UNSIGNED,
    i2    SMALLINT,
    i3    MEDIUMINT,
    i4    INT,
    u4    INT UNSIGNED,
    i8    BIGINT,
    u8    BIGINT UNSIGNED
);
INSERT INTO gt_integers VALUES
    (1,  'zeros',                0,    0,   0,      0,        0,           0,          0,                    0),
    (2,  'minus one, one',       -1,   1,   -1,     -1,       -1,          1,          -1,                   1),
    (3,  'minimums',             -128, 0,   -32768, -8388608, -2147483648, 0,          -9223372036854775808, 0),
    (4,  'maximums',             127,  255, 32767,  8388607,  2147483647,  4294967295, 9223372036854775807,  18446744073709551615),
    (5,  'unsigned 64 maximum - 1', NULL, NULL, NULL, NULL,   NULL,        NULL,       NULL,                 18446744073709551614),
    (6,  '2^53 - 1',             NULL, NULL, NULL,  NULL,     NULL,        NULL,       9007199254740991,     9007199254740991),
    (7,  '2^53',                 NULL, NULL, NULL,  NULL,     NULL,        NULL,       9007199254740992,     9007199254740992),
    (8,  '2^53 + 1',             NULL, NULL, NULL,  NULL,     NULL,        NULL,       9007199254740993,     9007199254740993),
    (9,  '-(2^53) - 1',          NULL, NULL, NULL,  NULL,     NULL,        NULL,       -9007199254740993,    NULL),
    (10, 'all NULL',             NULL, NULL, NULL,  NULL,     NULL,        NULL,       NULL,                 NULL);

-- DECIMAL: 65 digits at most, 30 of them after the point.
CREATE TABLE gt_decimals (
    id    INT PRIMARY KEY,
    label VARCHAR(40) NOT NULL,
    d6530 DECIMAL(65,30),
    d102  DECIMAL(10,2),
    d300  DECIMAL(30,0),
    d111  DECIMAL(1,1)
);
INSERT INTO gt_decimals VALUES
    (1, '35 digits and 30 decimals',  12345678901234567890123456789012345.123456789012345678901234567890, 12345678.90, 999999999999999999999999999999, 0.9),
    (2, 'negative',                   -12345678901234567890123456789012345.000000000000000000000000000001, -0.01, -1, -0.9),
    (3, 'smallest step',              0.000000000000000000000000000001, 0.01, 1, 0.1),
    (4, 'zero with scale',            0, 0, 0, 0),
    (5, '65 digits',                  99999999999999999999999999999999999.999999999999999999999999999999, NULL, NULL, NULL),
    (6, 'trailing zeros',             1.500000000000000000000000000000, 1.50, NULL, NULL),
    (7, 'all NULL',                   NULL, NULL, NULL, NULL);

-- FLOAT and DOUBLE: values a float cannot hold, subnormals, the extremes. (MySQL
-- has no NaN and no infinity.)
CREATE TABLE gt_floats (
    id    INT PRIMARY KEY,
    label VARCHAR(40) NOT NULL,
    f4    FLOAT,
    f8    DOUBLE
);
INSERT INTO gt_floats VALUES
    (1,  'zero',                 0,            0),
    (2,  'one tenth',            0.1,          0.1),
    (3,  'one third',            1.0/3,        1.0/3),
    (4,  'one in ten million',   1e-7,         1e-7),
    (5,  'one in a million',     1e-6,         1e-6),
    (6,  '2^24 + 1',             16777217,     16777217),
    (7,  'largest finite',       3.402823466e38, 1.7976931348623157e308),
    (8,  'smallest normal',      1.175494351e-38, 2.2250738585072014e-308),
    (9,  'negative',             -123456.789,  -123456789.123456789),
    (10, 'integral, 1e8',        100000000,    100000000),
    (11, '2^53 + 1 as a double', NULL,         9007199254740993),
    (12, 'all NULL',             NULL,         NULL);

-- Dates and times: fractional seconds, the extremes, a TIME below zero, a YEAR.
-- The TIMESTAMP rows are inserted with the session time zone at +05:30: the
-- server keeps UTC and shows the reading session's zone.
CREATE TABLE gt_temporal (
    id    INT PRIMARY KEY,
    label VARCHAR(40) NOT NULL,
    d     DATE,
    t     TIME,
    t6    TIME(6),
    dt    DATETIME,
    dt6   DATETIME(6),
    ts    TIMESTAMP NULL,
    ts6   TIMESTAMP(6) NULL,
    y     YEAR
);
SET time_zone = '+05:30';
INSERT INTO gt_temporal VALUES
    (1, 'ordinary',          '2024-03-05', '13:45:12', '13:45:12.123456', '2024-03-05 13:45:12', '2024-03-05 13:45:12.123456', '2024-03-05 13:45:12', '2024-03-05 13:45:12.123456', 2024),
    (2, 'extremes',          '1000-01-01', '838:59:59', '838:59:58.999999', '1000-01-01 00:00:00', '9999-12-31 23:59:59.999999', '1970-01-01 05:30:01', '2038-01-19 08:44:07.999999', 2155),
    (3, 'time below zero',   '9999-12-31', '-838:59:59', '-00:00:01.5', '9999-12-31 23:59:59', '1000-01-01 00:00:00.000001', '2000-01-01 05:30:00', '2000-01-01 05:30:00.000001', 1901),
    (4, 'midnight',          '2000-02-29', '00:00:00', '00:00:00.000000', '2000-02-29 00:00:00', '2000-02-29 00:00:00.000000', '2000-02-29 05:30:00', '2000-02-29 05:30:00.500000', 2000),
    (5, 'all NULL',          NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);
SET time_zone = SYSTEM;

-- BIT, BINARY, BLOB, JSON, ENUM and SET.
CREATE TABLE gt_misc (
    id    INT PRIMARY KEY,
    label VARCHAR(40) NOT NULL,
    b8    BIT(8),
    b64   BIT(64),
    bin   BINARY(4),
    vbin  VARBINARY(16),
    bl    BLOB,
    js    JSON,
    en    ENUM('small', 'medium', 'large'),
    st    SET('a', 'b', 'c')
);
INSERT INTO gt_misc VALUES
    (1, 'bytes that read as text', b'01000001', b'0100000101000010', 'hell', 'hello', 'hello', '{"b": 1, "a": [1, 2.50, null, true], "t": "x"}', 'medium', 'a,c'),
    (2, 'zero bits',               b'00000000', b'0',               0x00000000, X'', X'', 'null', 'small', ''),
    (3, 'bytes past 0x7f',         b'11111111', b'1111111111111111111111111111111111111111111111111111111111111111', 0xFF00FE01, 0xFF00FE, 0xC328FE, '[]', 'large', 'a,b,c'),
    (4, 'emoji in JSON',           NULL, NULL, NULL, NULL, NULL, JSON_OBJECT('emoji', '😀', 'big', 12345678901234567890, 'n', 1.0e0), NULL, NULL),
    (5, 'all NULL',                NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

-- Text of several kinds: the same three values at every width, a long one.
CREATE TABLE gt_text (
    id    INT PRIMARY KEY,
    label VARCHAR(40) NOT NULL,
    c     CHAR(8),
    vc    VARCHAR(200),
    tx    TEXT,
    lt    LONGTEXT
);
INSERT INTO gt_text VALUES
    (1, 'null',            NULL, NULL, NULL, NULL),
    (2, 'empty',           '', '', '', ''),
    (3, 'the text NULL',   'NULL', 'NULL', 'NULL', 'NULL'),
    (4, 'padded char',     'ab', 'ab  ', 'ab', 'ab'),
    (5, 'emoji and CJK',   '😀日本', '👨‍👩‍👧 ａｂｃ', 'Ünïcödé — ✓', 'עברית ‏and Latin'),
    (6, 'multi-line',      'a', 'one
two', 'x
y
z', 'p

q'),
    (7, '100 kB',          'x', REPEAT('é', 200), REPEAT('0123456789', 6000), REPEAT('0123456789', 10240)),
    (8, 'the 4-byte limit', '𝕏', '𐌀!','U+10300 𐌀', '');

-- A table with no primary key, one with a composite key (its rows inserted out
-- of order), reserved and mixed-case names.
CREATE TABLE no_pk (a INT, b VARCHAR(20));
INSERT INTO no_pk VALUES (2, 'two'), (1, 'one'), (2, 'two'), (NULL, NULL);

CREATE TABLE composite_key (
    a INT NOT NULL,
    b INT NOT NULL,
    v VARCHAR(20),
    PRIMARY KEY (a, b)
);
INSERT INTO composite_key VALUES (2, 1, 'two-one'), (1, 2, 'one-two'), (1, 1, 'one-one'), (2, 2, 'two-two');

CREATE TABLE `Mixed Case` (
    `Id`     INT PRIMARY KEY,
    `select` VARCHAR(20),
    `Order`  VARCHAR(20)
);
INSERT INTO `Mixed Case` VALUES (1, 'reserved', 'also reserved'), (2, NULL, NULL);

CREATE TABLE `order` (
    `group` INT PRIMARY KEY,
    `key`   VARCHAR(20)
);
INSERT INTO `order` VALUES (1, 'a'), (2, 'b');

-- A name with a backtick in it, and one that starts and ends with a backtick
-- and has a second statement inside: browsing it must quote it, never run it.
CREATE TABLE `a``b` (id INT PRIMARY KEY, v VARCHAR(20));
INSERT INTO `a``b` VALUES (1, 'backtick in the name');

CREATE TABLE ```x``; select sleep(0) #``` (id INT PRIMARY KEY, v VARCHAR(40));
INSERT INTO ```x``; select sleep(0) #``` VALUES (1, 'one known row');

CREATE TABLE empty_table (id INT PRIMARY KEY, v VARCHAR(10));

-- A view over a table with a key: its cells are compared too.
CREATE VIEW gt_view AS
    SELECT id, label, u8, i8 FROM gt_integers WHERE id <= 5;

-- Numbers 1..400 for paging, in a table of several column types.
CREATE TABLE numbers (
    id  INT PRIMARY KEY,
    sq  BIGINT,
    txt VARCHAR(20)
);
INSERT INTO numbers
    WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 400)
    SELECT i, i * i, CONCAT('row ', i) FROM n;

-- The second database: the cross-database cases of the confirmation tests.
USE edge_other;
CREATE TABLE t (id INT PRIMARY KEY, v VARCHAR(20));
INSERT INTO t VALUES (1, 'one'), (2, 'two'), (3, 'three');
CREATE TABLE `Other Case` (id INT PRIMARY KEY, v VARCHAR(20));
INSERT INTO `Other Case` VALUES (1, 'x');
