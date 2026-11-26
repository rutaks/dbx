#!/usr/bin/env bash
# Journey cli-flags (slice 24a): `dbpane --version`, `--help` and `--licenses`
# print and exit 0, without starting the client; a bad flag exits 2 saying so.
# Every run is under a scratch HOME with the store variables pointed at
# scratch locations, so --help's "store in use" line reaches nothing real.
. "$(dirname "$0")/lib.sh"

build_bin
scratch_store
run() { env -u DBX_SECRET_KEY HOME="$SCRATCH_HOME" DBX_DATA_DIR="$STORE" DBX_SECRET_KEY_FILE="$KEYFILE" "$BIN" "$@"; }

out="$(run --version)" || fail "--version exited non-zero"
echo "$out" | grep -Eq '^dbpane [0-9]+\.[0-9]+\.[0-9]+ \(dbx [0-9a-f]{7,40}\)$' || fail "--version: $out"
[ "$(echo "$out" | wc -l | tr -d ' ')" = 1 ] || fail "--version is one line"

out="$(run --help)" || fail "--help exited non-zero"
for needle in 'dbpane' '--store dbx|own|<dir>' '--version' '--licenses' 'kitty-setup' 'DBX_DATA_DIR' 'DBX_SECRET_KEY_FILE' 'DBX_TUI_THEME' 'Store in use now' "$STORE"; do
  echo "$out" | grep -Fq -- "$needle" || fail "--help lacks $needle"
done
run -h >/dev/null || fail "-h exited non-zero"

# The config file (slice 22): --help names it, `config --path` prints it,
# `config` prints a commented file of the defaults. Nothing is read or written.
noconfig() { env -u DBX_SECRET_KEY -u XDG_CONFIG_HOME -u DBPANE_CONFIG -u DBX_TUI_CONFIG HOME="$SCRATCH_HOME" DBX_DATA_DIR="$STORE" DBX_SECRET_KEY_FILE="$KEYFILE" "$BIN" "$@"; }
want="$SCRATCH_HOME/.config/dbpane/config.toml"
out="$(noconfig --help)" || fail "--help (config)"
for needle in 'dbpane config [--path]' 'DBPANE_CONFIG' 'DBX_TUI_CONFIG' 'DBPANE_THEME' 'XDG_CONFIG_HOME' 'Config file read at startup' "$want"; do
  echo "$out" | grep -Fq -- "$needle" || fail "--help lacks $needle"
done
[ "$(noconfig config --path)" = "$want" ] || fail "config --path is $(noconfig config --path), not $want"
[ ! -e "$want" ] || fail "config --path created the file"
[ "$(env -u DBX_SECRET_KEY -u XDG_CONFIG_HOME DBPANE_CONFIG="$OUT/mine.toml" HOME="$SCRATCH_HOME" "$BIN" config --path)" = "$OUT/mine.toml" ] || fail "config --path ignores DBPANE_CONFIG"
[ "$(env -u DBX_SECRET_KEY -u XDG_CONFIG_HOME -u DBPANE_CONFIG DBX_TUI_CONFIG="$OUT/old.toml" HOME="$SCRATCH_HOME" "$BIN" config --path)" = "$OUT/old.toml" ] || fail "config --path ignores DBX_TUI_CONFIG"
[ "$(env -u DBX_SECRET_KEY -u DBPANE_CONFIG -u DBX_TUI_CONFIG XDG_CONFIG_HOME="$OUT/xdg" HOME="$SCRATCH_HOME" "$BIN" config --path)" = "$OUT/xdg/dbpane/config.toml" ] || fail "config --path ignores XDG_CONFIG_HOME"
noconfig config >"$OUT/defaults.toml" || fail "config exited non-zero"
for needle in '[theme]' 'prefix = "ctrl+g"' '[keys.editor]' 'duplicate = ["cmd+d", "ctrl+d", "prefix+d"]' '[run]' 'row_cap = 1000' '# '; do
  grep -Fq -- "$needle" "$OUT/defaults.toml" || fail "config output lacks $needle"
done
noconfig config --wat >/dev/null 2>"$OUT/err.txt"; [ "$?" = 2 ] || fail "config --wat exits 2"

out="$(run --licenses)" || fail "--licenses exited non-zero"
for needle in 'modified work of dbx' 'https://github.com/rutaks/dbx' 'herdr' 'Apache License' 'ratatui' 'sqlparser'; do
  echo "$out" | grep -Fq -- "$needle" || fail "--licenses lacks $needle"
done

# The store flag is read by --help too: it shows the choice in force.
out="$(env -u DBX_SECRET_KEY -u DBX_DATA_DIR -u XDG_DATA_HOME HOME="$SCRATCH_HOME" "$BIN" --help)" || fail "--help with no store variable"
echo "$out" | grep -Fq "dbpane's own store" || fail "--help does not name the own store on a HOME without a dbx store: $out"

run --nope >/dev/null 2>"$OUT/err.txt"; rc=$?
[ "$rc" = 2 ] || fail "a bad flag exits 2, got $rc"
grep -Fq -- '--nope' "$OUT/err.txt" && grep -Fq -- '--help' "$OUT/err.txt" || fail "a bad flag is named and --help suggested"

echo "OK cli-flags; captures in $OUT"
