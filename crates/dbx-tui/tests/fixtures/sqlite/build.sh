#!/usr/bin/env bash
# Builds the SQLite fixtures with /usr/bin/sqlite3 (no Docker):
#   <out>/sakila.db  Sakila (fetch-sakila.sh), the schema and then the data
#   <out>/edge.db    our edge cases (edge.sql)
#   <out>/sqlite3-version.txt  the sqlite3 that built them
# <out> is $1, else target/sqlite-fixtures at the workspace root. A file that is
# newer than its sources is kept. Tests copy a file to a temporary directory
# before opening it, so no test writes a built fixture.
set -euo pipefail
cd "$(dirname "$0")"
HERE="$(pwd)"
SQLITE3="${SQLITE3:-/usr/bin/sqlite3}"
OUT="${1:-$HERE/../../../../../target/sqlite-fixtures}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
"$HERE/../fetch-sakila.sh"
SAKILA="$HERE/../sakila"

fresh() { # fresh FILE SOURCE...
  local out="$1"; shift
  [ -f "$out" ] || return 1
  for s in "$@"; do [ "$out" -nt "$s" ] || return 1; done
}

if ! fresh "$OUT/sakila.db" "$SAKILA/sqlite-sakila-schema.sql" "$SAKILA/sqlite-sakila-insert-data.sql" "$0"; then
  rm -f "$OUT/sakila.db.part"
  # The schema has triggers that write last_update on insert; the data is
  # loaded as it comes and the tests only compare what the file holds.
  "$SQLITE3" "$OUT/sakila.db.part" < "$SAKILA/sqlite-sakila-schema.sql"
  "$SQLITE3" "$OUT/sakila.db.part" < "$SAKILA/sqlite-sakila-insert-data.sql"
  mv "$OUT/sakila.db.part" "$OUT/sakila.db"
fi
if ! fresh "$OUT/edge.db" "$HERE/edge.sql" "$0"; then
  rm -f "$OUT/edge.db.part"
  "$SQLITE3" "$OUT/edge.db.part" < "$HERE/edge.sql"
  mv "$OUT/edge.db.part" "$OUT/edge.db"
fi
"$SQLITE3" --version > "$OUT/sqlite3-version.txt"
echo "sqlite fixtures in $OUT ($(cut -d' ' -f1 "$OUT/sqlite3-version.txt"))"
