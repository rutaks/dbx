#!/bin/sh
# Runs once, when the container initialises its (tmpfs) data directory.
set -eu
my() { mysql --user=root --password="$MYSQL_ROOT_PASSWORD" --default-character-set=utf8mb4 "$@"; }

# Sakila: the schema creates and selects the database `sakila`.
my </fixtures/sakila/mysql-sakila-schema.sql
my </fixtures/sakila/mysql-sakila-insert-data.sql

for f in /fixtures/edge/*.sql; do
  my <"$f"
done

# Marker for up-mysql.sh: everything above finished.
my -e 'CREATE DATABASE fixtures_ready'
