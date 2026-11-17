#!/usr/bin/env bash
# Stops and removes the fixture databases (data is on tmpfs, nothing remains).
set -euo pipefail
cd "$(dirname "$0")"
docker compose -f compose.yaml down --volumes --remove-orphans
