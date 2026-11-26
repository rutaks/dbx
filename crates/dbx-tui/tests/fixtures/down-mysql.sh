#!/usr/bin/env bash
# Stops and removes the MySQL fixture (data is on tmpfs, nothing remains). Its
# own compose project: the Postgres fixture is not touched.
set -euo pipefail
cd "$(dirname "$0")"
docker compose -p dbxtui-fixtures-mysql -f compose-mysql.yaml down --volumes --remove-orphans
