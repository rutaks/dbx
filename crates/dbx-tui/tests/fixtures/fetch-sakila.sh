#!/usr/bin/env bash
# Fetches Sakila (jOOQ/sakila, BSD-2-Clause) for SQLite and for MySQL at a pinned
# commit and checks each file by sha256, as fetch-pagila.sh does. sqlite/build.sh
# turns the SQLite files into sakila.db; init-mysql/10-load.sh loads the MySQL
# files into the MySQL container.
set -euo pipefail
cd "$(dirname "$0")"
COMMIT=e089a5b1ec9af0df7a9c6a5d47d49fa1736a4e84
BASE="https://raw.githubusercontent.com/jOOQ/sakila/$COMMIT"
mkdir -p sakila
if command -v shasum >/dev/null; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
elif command -v sha256sum >/dev/null; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
else
  echo "fetch-sakila: need shasum or sha256sum" >&2; exit 1
fi
fetch() {
  local dir="$1" file="$2" want="$3"
  if [ -f "sakila/$file" ] && [ "$(sha256 "sakila/$file")" = "$want" ]; then
    return 0
  fi
  curl -fsSL -o "sakila/$file.part" "$BASE/$dir/$file"
  local got; got="$(sha256 "sakila/$file.part")"
  if [ "$got" != "$want" ]; then
    rm -f "sakila/$file.part"
    echo "fetch-sakila: checksum mismatch for $file (got $got, want $want)" >&2
    exit 1
  fi
  mv "sakila/$file.part" "sakila/$file"
}
fetch sqlite-sakila-db sqlite-sakila-schema.sql      b1db76b3a5192f98493901a9ac40fecb9dba918feb5604b7ef6b74230a385f12
fetch sqlite-sakila-db sqlite-sakila-insert-data.sql 4d69a56acbd3f60501e45737353707df0cf51fdb7fcab65cc9db3a72c8c85f34
fetch mysql-sakila-db  mysql-sakila-schema.sql       f2c41c3bf6d6c239941b4f98fb37afad21f6be12b82bf586202529a793ccc2ee
fetch mysql-sakila-db  mysql-sakila-insert-data.sql  353ef858e4d2d1a60549969283434da15b905aef3ca7099d82b649b75f8de99f
