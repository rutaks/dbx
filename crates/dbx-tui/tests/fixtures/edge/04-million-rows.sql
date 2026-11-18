-- `million_rows`: a million rows of an id, a text and a number, deterministic
-- (generate_series, no random()), for the result grid's row cap and "load
-- more". About two seconds to create.
CREATE TABLE million_rows (
    id  integer PRIMARY KEY,
    txt text NOT NULL,
    num numeric(10,2) NOT NULL
);
INSERT INTO million_rows
SELECT g, 'row ' || g, ((g::bigint * 7919) % 100003) / 100.0
FROM generate_series(1, 1000000) g;
ANALYZE million_rows;
