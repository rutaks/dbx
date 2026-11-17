#!/usr/bin/env bash
# Starts the fixture databases and waits until they accept queries.
# Usage: tests/fixtures/up.sh   (idempotent; prints the connection URL)
set -euo pipefail
cd "$(dirname "$0")"
./fetch-pagila.sh
if ! log="$(docker compose -f compose.yaml up -d --wait 2>&1)"; then
  echo "$log" >&2
  exit 1
fi
q() { docker compose -f compose.yaml exec -T postgres psql -U postgres -d "$1" -tAc "$2" 2>/dev/null; }
for _ in $(seq 1 120); do
  # Ready once the init script has created its marker and Pagila answers.
  if [ "$(q fixtures_ready 'select 1' || true)" = 1 ] \
    && [ "$(q pagila 'select count(*) from film' || true)" = 1000 ]; then
    echo "fixtures ready: postgres://postgres:dbxtui@127.0.0.1:55433/{pagila,edge}"
    exit 0
  fi
  sleep 1
done
echo "up.sh: fixtures did not become ready in 120 s" >&2
docker compose -f compose.yaml logs --tail 40 postgres >&2
exit 1
