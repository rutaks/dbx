#!/bin/sh
# Copies the canonical docs (crates/dbx-tui/docs/) into the staged rutaks/dbpane
# repo (.dbpane-repo/, gitignored in the fork), and the legal files the repo
# root carries. The test tests/docs_keys.rs fails when the two differ.
#
#   crates/dbx-tui/scripts/stage-docs.sh
set -eu
crate="$(cd "$(dirname "$0")/.." && pwd)"
repo="${DBPANE_REPO_DIR:-$crate/../../.dbpane-repo}"
[ -d "$repo" ] || { echo "no staged repo at $repo" >&2; exit 1; }
mkdir -p "$repo/docs"
cp "$crate/docs/README.md" "$repo/README.md"
cp "$crate/docs/getting-started.md" "$repo/docs/getting-started.md"
cp "$crate/legal/LICENSE" "$repo/LICENSE"
cp "$crate/legal/NOTICE" "$repo/NOTICE"
echo "staged the docs and legal files into $repo"
