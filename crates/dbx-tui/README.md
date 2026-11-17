# dbx-tui

A terminal database client, built as another head on dbx's core. Plans and
decisions live in the dbx-tui docs repo; this crate is the code.

Run it against a scratch store (never your real one while developing), from
the dbx workspace root:

```sh
store="$(mktemp -d)"                                   # DBX_DATA_DIR must exist and be absolute
openssl rand -hex 32 >"$store.key" && chmod 600 "$store.key"
cargo build -p dbx-tui                                 # not under the scratch HOME: cargo needs yours
HOME="$store" DBX_DATA_DIR="$store" DBX_SECRET_KEY_FILE="$store.key" target/debug/dbx-tui
```

`DBX_SECRET_KEY_FILE` (or `DBX_SECRET_KEY`, 64 hex characters) wins over every
key policy, so the keychain is not touched; pointing `HOME` at the scratch
directory keeps dbx from even looking for its per-user key file. Without
`DBX_DATA_DIR` the client opens the desktop app's store
(`~/Library/Application Support/com.dbx.app` on macOS) and its key from the
login keychain.

## The schema tree (sidebar)

Connection, database, schema, table or view, column, each level loaded on
demand through `Core::databases`, `schemas`, `tables` and `columns` and cached
per node (`src/app/tree.rs`). Every database on the server is listed, not only
the one the connection was saved with. System schemas (`pg_catalog`,
`information_schema`, `pg_toast*`) and the partitions of a partitioned table
are left out.

| Key | Does |
|---|---|
| `enter` `l` `right` | open the node (loads it once); on an open node, go to its first child |
| `h` `left` | close the node; on a closed node or a leaf, go to the parent |
| `j` `k` `down` `up` | move |
| `pagedown` `ctrl+d` `pageup` `ctrl+u` | a page |
| `g` `home` / `G` `end` | first / last row |
| `r` | reload the node under the cursor, which keeps showing what it had until the new listing arrives (on an error line, the node that failed); on a closed node it only drops what was cached, so the next open asks again |
| `e` `d` `delete` | edit / remove the connection (on a connection row) |

The keys are the sidebar's own (no prefix) and come from `Keybinds`, like the
mode bar's hints, which are clickable and do what their key does. Mouse: click
selects, a click on the arrow or a double-click toggles, the wheel scrolls.

Rows: `▸`/`▾`, two columns of indent per level, a state glyph on connections
(`○` before any call, yellow `●` while the first call runs, green `●` once a
call worked, red `●` when the last one failed), `view`/`matview`/`foreign` after
a name that is not a plain table, and for columns `◆` before a primary key's
name, the type right-aligned in `overlay0`, and `?` after it when the column
may be NULL. Errors are drawn under the node that failed (up to eight lines),
followed by `press <key> to retry`, the key taken from the live bindings. A
node that is loading or reloading has a yellow arrow, and a connection's glyph
is yellow while a call on it runs, even right after a failure.
A `▕` scrollbar appears when the list is longer than the pane. Only PostgreSQL
connections can be browsed so far: any other kind shows "Browsing MySQL is not
supported yet." under it and asks dbx nothing. The sidebar is about 30% of the
width, between 26 and 34 columns (with its separator). Closing a database
node, or a connection, tells the Core to `release` the databases it opened
(`Core::release`: dbx's `remove_pool_by_key` for `<connection id>:<database>`),
except the connection's saved default database, so browsing many databases
does not hold a server connection slot for each. Moving the cursor never
changes where the editor runs: `u` (use) on a database, or anything under one,
gives that connection and database to the query tab, and on a connection row
gives it the database the connection was saved with.

## Running a statement (editor and results)

The query tab owns its target (`editor · <connection> / <database>` in the
editor's title) and a database session: dbx's `client_session_id` is the tab,
so `SET`, temporary tables and transactions last from run to run. The session
is closed when the tab is given another database or connection (a run still in
flight there is cancelled first, and the close waits for that run to end so the
cancel reaches the server) and on quit, by key, overlay or signal (the shell
waits up to 2.5 s for runs to end and sessions to close); collapsing the database in the sidebar does not touch it, because
`Core::release` removes only the database's shared pool.

| Key | Does |
|---|---|
| `f5`, `alt+enter`, `ctrl+g r` | run the selection, or else the statement under the cursor (dbx splits statements at `;`, and also before a line starting with a keyword such as `CREATE`, `INSERT`, `UPDATE`, `DELETE` or `SET`, but not `SELECT` or `WITH`, after two or more blank lines; a cursor on a blank line after a statement runs that statement) |
| anything else, editor focused | the editor's own (`ratatui-textarea` defaults: shift+arrows select, `ctrl+u`/`ctrl+r` undo/redo) |
| `ctrl+c` | taken before the editor; cancels a run from slice 06 |

A run has no time limit unless the connection sets its own query timeout in
the dbx desktop app (dbx's default of 60 counts as unset); a pasted text over
256 KiB, or with a line over 16 KiB, is refused with a notice.

Only one run at a time per tab: a second is refused with a notice. The mode bar
shows `running 3s` (woken each second through `next_deadline`), then `N rows
12ms` (`N+` when the row cap of 1,000 cut the result), `N rows affected` or
`failed`. The results pane starts with the first line of the statement that
ran, then shows the header and the rows that fit, numbers
right-aligned, NULL as a dimmed `∅`, a line break in a cell as `⏎`; a failed
run shows dbx's message, `line L, column C` when Postgres gives a position, and
the statement that ran with a `^` under the position.

## Tests

Run every command from the dbx workspace root.

| What | Command | Needs |
|---|---|---|
| Fast: app, view, keys, FakeCore (layer 2) | `cargo test -p dbx-tui --lib --test no_colour_literals` | nothing |
| Store: `DbxCore` on scratch stores (layer 1) | `cargo test -p dbx-tui --test store -- --test-threads=1` | nothing external; temp dirs, key files, a local TCP port |
| Live: `DbxCore` against the fixture Postgres (layer 1) | `crates/dbx-tui/tests/fixtures/up.sh && cargo test -p dbx-tui --test live -- --test-threads=1` | Docker (the fixtures below) |
| Fixtures: the test databases | `crates/dbx-tui/tests/fixtures/up.sh && crates/dbx-tui/tests/fixtures/check.sh` | Docker |
| Journeys: the built binary in tmux (layer 5; `add-connection`, `browse-pagila`, `first-query` and `quit-mid-run` need the fixtures) | `for j in crates/dbx-tui/tests/journeys/*.sh; do [ "$(basename "$j")" = lib.sh ] \|\| bash "$j"; done` | tmux, openssl, python3 |
| Lint | `cargo clippy -p dbx-tui --all-targets` | |

`cargo test -p dbx-tui` runs the fast, store and live tests together (the live
ones fail, saying so, when the fixtures are not up). Test helpers on the
production adapter (`DbxCore::seed_config`, `runtime_config`,
`runtime_config_ids`, `probe_select_one`, `with_managed_keys`,
`check_seed_dir`) sit behind the `test-support` feature, which tests and
examples get through a self dev-dependency; the client binary is built
without it. Saving a connection is the real `Core::save_connection`, used by
the tests and by `examples/seed_store`, which refuses any directory outside
the system temp directory unless `DBX_TUI_ALLOW_SEED=1`, and always refuses
the desktop app's store.

The store and live tests set `DBX_SECRET_KEY_FILE` (or `DBX_SECRET_KEY`) for
the whole process, so they live in their own test binaries and hold one lock
each (`tests/common/mod.rs`); `--test-threads=1` makes that explicit. Every one
uses its own temporary data directory and a scratch `HOME`, and never reaches
the keychain or the real store: `Scratch::core()` refuses to build a client
without a scratch key, and the one test that needs "no key at all" uses
`with_managed_keys`, whose key policy never looks at the keychain. Journeys
do the same through `lib.sh` (`$CLIENT` runs the binary on a scratch store
with a scratch key); captures go to `$JOURNEY_OUT`, or a temp directory
printed at the end. `browse-pagila` also starts a throwaway local socket that accepts and never answers (a listing that hangs, collapsed, and then in flight while the client quits). `examples/seed_store` takes the password from `DBX_TUI_SEED_PASSWORD` when set.

### Fixture databases

`tests/fixtures/compose.yaml` runs Postgres 16, pinned by digest, on
`127.0.0.1:55433` (user `postgres`, password `dbxtui`), compose project
`dbxtui-fixtures`, data on tmpfs. It holds two databases:

- `pagila`: Pagila at commit `5ba5a57`, fetched by `fetch-pagila.sh` and
  checked by SHA-256 (current Pagila master needs Postgres 18).
- `edge`: our edge-case schema (`edge/*.sql`), values whose exact text is
  known: NULL vs `''` vs `'NULL'`, wide text, integers beyond 2^53, NaN and
  Infinity, time zones, odd identifiers, a second schema.

`up.sh` starts it and waits until it answers queries; `down.sh` removes it.
