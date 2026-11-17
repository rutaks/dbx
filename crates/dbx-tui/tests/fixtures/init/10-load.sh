#!/bin/sh
# Runs once, when the container initialises its (tmpfs) data directory.
set -eu
psql() { command psql -v ON_ERROR_STOP=1 --username postgres --no-psqlrc -q "$@"; }

psql -d postgres -c 'CREATE DATABASE pagila' -c 'CREATE DATABASE edge'

psql -d pagila -f /fixtures/pagila/pagila-schema.sql >/dev/null
psql -d pagila -f /fixtures/pagila/pagila-data.sql >/dev/null

for f in /fixtures/edge/*.sql; do
  psql -d edge -f "$f" >/dev/null
done

# Marker for up.sh: everything above finished.
psql -d postgres -c 'CREATE DATABASE fixtures_ready'
