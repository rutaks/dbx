#!/usr/bin/env bash
# Fetches Pagila at a pinned commit and checks it byte for byte. Current Pagila
# master needs Postgres 18; this commit loads cleanly on Postgres 16.
set -euo pipefail
cd "$(dirname "$0")"
COMMIT=5ba5a57aeb159f75f02aca2432d3c262186d13d3
BASE="https://raw.githubusercontent.com/devrimgunduz/pagila/$COMMIT"
mkdir -p pagila
if command -v shasum >/dev/null; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
elif command -v sha256sum >/dev/null; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
else
  echo "fetch-pagila: need shasum or sha256sum" >&2; exit 1
fi
fetch() {
  local file="$1" want="$2"
  if [ -f "pagila/$file" ] && [ "$(sha256 "pagila/$file")" = "$want" ]; then
    return 0
  fi
  curl -fsSL -o "pagila/$file.part" "$BASE/$file"
  local got; got="$(sha256 "pagila/$file.part")"
  if [ "$got" != "$want" ]; then
    rm -f "pagila/$file.part"
    echo "fetch-pagila: checksum mismatch for $file (got $got, want $want)" >&2
    exit 1
  fi
  mv "pagila/$file.part" "pagila/$file"
}
fetch pagila-schema.sql 8ce358e4c8014087b85296694a0893887bd7a4190e3ce407f2721b86b98e5707
fetch pagila-data.sql   880580fb2cd4daaa99f290ced264988cdd657b3158be63cd281466f796f6dbf2
