-- A table whose name starts and ends with a double quote and carries a second
-- statement (slice 16 review). Written unquoted into SQL, the name would end
-- the identifier, run `select pg_sleep(0)` and comment out the rest. Loaded
-- into database `edge`:
--   docker exec -i dbxtui-fixtures-postgres-1 psql -U postgres -d edge -f - < tests/fixtures/edge/06-quoted-name.sql
CREATE TABLE """x""; select pg_sleep(0) --""" (
    id integer PRIMARY KEY,
    v  text NOT NULL
);
INSERT INTO """x""; select pg_sleep(0) --""" VALUES (1, 'one known row');
