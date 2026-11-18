-- Tables for the result grid (slice 07), in database `edge`.
--
-- `wide_120`: 120 data columns of mixed types, 400 rows. Every value encodes
-- its own column and row, so a test can tell from a cell alone which column it
-- belongs to and whether the header above it is the right one:
--   c001..c120 by (n mod 4): 1 text   'cNNN-rRRR'
--                            2 numeric NNN * 1000 + id  (with .50)
--                            3 jsonb  {"c": NNN, "r": id}
--                            0 integer NNN * 1000 + id
-- (NNN is the column number, RRR and id the row number 1..400.)
DO $$
DECLARE
    defs  text := '';
    vals  text := '';
    n     int;
    name  text;
BEGIN
    FOR n IN 1..120 LOOP
        name := format('c%s', lpad(n::text, 3, '0'));
        CASE n % 4
            WHEN 1 THEN
                defs := defs || format(', %I text', name);
                vals := vals || format(', %L || ''-r'' || lpad(g::text, 3, ''0'')', name);
            WHEN 2 THEN
                defs := defs || format(', %I numeric(12,2)', name);
                vals := vals || format(', %s + g + 0.50', n * 1000);
            WHEN 3 THEN
                defs := defs || format(', %I jsonb', name);
                vals := vals || format(', jsonb_build_object(''c'', %s, ''r'', g)', n);
            ELSE
                defs := defs || format(', %I integer', name);
                vals := vals || format(', %s + g', n * 1000);
        END CASE;
    END LOOP;
    EXECUTE format('CREATE TABLE wide_120 (id integer PRIMARY KEY%s)', defs);
    EXECUTE format('INSERT INTO wide_120 SELECT g%s FROM generate_series(1, 400) g', vals);
END $$;

-- Wide characters, combining marks and joiners, for width tests: what a cell
-- shows must be as wide as its display width in a terminal.
CREATE TABLE width_text (
    id    integer PRIMARY KEY,
    label text NOT NULL,
    v     text NOT NULL
);
INSERT INTO width_text VALUES
    (1, 'ascii',               'plain'),
    (2, 'cjk',                 '日本語のテキスト'),
    (3, 'emoji',               '😀😀😀'),
    (4, 'mixed',               '日本語 😀 abc'),
    (5, 'combining',           E'ééé ä'),
    (6, 'zwj family',          E'\U0001F468‍\U0001F469‍\U0001F467'),
    (7, 'fullwidth latin',     'ＡＢＣ１２３'),
    (8, 'halfwidth katakana',  'ｱｲｳｴｵ'),
    (9, 'hangul',              '한국어'),
    (10, 'flag',               E'\U0001F1EF\U0001F1F5');
