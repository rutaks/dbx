#!/usr/bin/env bash
# Starts the MySQL fixture (its own compose project, dbxtui-fixtures-mysql: the
# Postgres container is not touched) and waits until Sakila answers.
# Usage: tests/fixtures/up-mysql.sh   (idempotent; prints the connection URL)
set -euo pipefail
cd "$(dirname "$0")"
./fetch-sakila.sh
if ! log="$(docker compose -p dbxtui-fixtures-mysql -f compose-mysql.yaml up -d --wait 2>&1)"; then
  echo "$log" >&2
  exit 1
fi
# Over TCP inside the container: the temporary server the image's init script
# runs has networking off, so this only answers once the real one is up.
q() { docker exec dbxtui-fixtures-mysql-mysql-1 mysql -h127.0.0.1 -uroot -pdbxtui -N -B -e "$2" "$1" 2>/dev/null; }
for _ in $(seq 1 180); do
  # Ready once the init script has created its marker and Sakila answers.
  if [ "$(q fixtures_ready 'select 1' || true)" = 1 ] \
    && [ "$(q sakila 'select count(*) from film' || true)" = 1000 ]; then
    echo "fixtures ready: mysql://root:dbxtui@127.0.0.1:53307/{sakila,edge,edge_other}"
    exit 0
  fi
  sleep 1
done
echo "up-mysql.sh: the MySQL fixture did not become ready in 180 s" >&2
docker compose -p dbxtui-fixtures-mysql -f compose-mysql.yaml logs --tail 40 mysql >&2
exit 1
