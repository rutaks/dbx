-- Inheritance: a table with two parents. dbx lists tables by joining
-- pg_inherits, so such a table comes back once per parent (two rows, one
-- name): the schema tree must show it once.
CREATE TABLE inherit_parent_a (a integer);
CREATE TABLE inherit_parent_b (b integer);
CREATE TABLE inherit_child () INHERITS (inherit_parent_a, inherit_parent_b);
