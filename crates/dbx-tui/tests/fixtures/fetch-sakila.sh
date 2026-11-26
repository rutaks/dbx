#!/usr/bin/env bash
# Fetches Sakila for SQLite (jOOQ/sakila, BSD-2-Clause) at a pinned commit and
# checks each file by sha256, as fetch-pagila.sh does. build.sh turns the two
# files into sakila.db.
set -euo pipefail
cd "$(dirname "$0")"
COMMIT=e089a5b1ec9af0df7a9c6a5d47d49fa1736a4e84
BASE="https://raw.githubusercontent.com/jOOQ/sakila/$COMMIT/sqlite-sakila-db"
mkdir -p sakila
if command -v shasum >/dev/null; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
elif command -v sha256sum >/dev/null; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
else
  echo "fetch-sakila: need shasum or sha256sum" >&2; exit 1
fi
fetch() {
  local file="$1" want="$2"
  if [ -f "sakila/$file" ] && [ "$(sha256 "sakila/$file")" = "$want" ]; then
    return 0
  fi
  curl -fsSL -o "sakila/$file.part" "$BASE/$file"
  local got; got="$(sha256 "sakila/$file.part")"
  if [ "$got" != "$want" ]; then
    rm -f "sakila/$file.part"
    echo "fetch-sakila: checksum mismatch for $file (got $got, want $want)" >&2
    exit 1
  fi
  mv "sakila/$file.part" "sakila/$file"
}
fetch sqlite-sakila-schema.sql      b1db76b3a5192f98493901a9ac40fecb9dba918feb5604b7ef6b74230a385f12
fetch sqlite-sakila-insert-data.sql 4d69a56acbd3f60501e45737353707df0cf51fdb7fcab65cc9db3a72c8c85f34
