#!/usr/bin/env bash
# Regenerates legal/THIRD-PARTY.txt: every crate in the dbpane binary's
# dependency graph with its licence, and the licence texts and copyright
# notices. Needs cargo-about (`cargo install cargo-about --locked --features cli`)
# and the dbx workspace's Cargo.lock; takes a few minutes. Run from anywhere.
#
#   scripts/gen-third-party.sh            write legal/THIRD-PARTY.txt
#   scripts/gen-third-party.sh --check    fail if the file is out of date
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
crate="$(dirname "$here")"
out="$crate/legal/THIRD-PARTY.txt"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
command -v cargo-about >/dev/null || { echo "cargo-about is not installed" >&2; exit 1; }
cargo-about generate --locked --manifest-path "$crate/Cargo.toml" \
  -c "$here/licenses/about.toml" "$here/licenses/about.hbs" -o "$tmp"
meta="$(mktemp)"
trap 'rm -f "$tmp" "$meta"' EXIT
cargo metadata --format-version 1 --locked --manifest-path "$crate/Cargo.toml" >"$meta"
{ printf '\n'; python3 -I "$here/no-licence-file.py" "$meta" <"$tmp"; } >>"$tmp.add"
cat "$tmp.add" >>"$tmp"
rm -f "$tmp.add"
if [ "${1:-}" = "--check" ]; then
  cmp -s "$tmp" "$out" || { echo "legal/THIRD-PARTY.txt is out of date: run scripts/gen-third-party.sh" >&2; exit 1; }
else
  cp "$tmp" "$out"
  echo "wrote $out ($(wc -c <"$out") bytes)"
fi
