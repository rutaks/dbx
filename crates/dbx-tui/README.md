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
editor's title) and a database session: dbx's `client_session_id` is the tab
and a serial (`dbx-tui-tab-1-2`), so `SET`, temporary tables and transactions
last from run to run. The tab starts a new session, with the next serial,
whenever it is given another database or connection, so a late close of the
old one can never end the new one (a tab given A, then B, then A again). The
old session is closed once a run still in flight there has ended (it is
cancelled first, so the cancel can reach the server) or after 2 s, and on quit,
by key, overlay or signal (the shell waits up to 2.5 s for runs to end and
sessions to close); collapsing the database in the sidebar does not touch it,
because `Core::release` removes only the database's shared pool. Removing a
connection cancels a run on it first, and the removal waits for that run.

| Key | Does |
|---|---|
| `f5`, `alt+enter`, `ctrl+g r` | run the selection, or else the statement under the cursor (dbx splits statements at `;`, and also before a line starting with a keyword such as `CREATE`, `INSERT`, `UPDATE`, `DELETE` or `SET`, but not `SELECT` or `WITH`, after two or more blank lines; a cursor on a blank line after a statement runs that statement) |
| `ctrl+c` (editor or results focused), `ctrl+g c` (anywhere) | cancel the run; pressed again while the cancel has not landed, ask whether to stop waiting (below). With nothing running, `ctrl+c` does nothing at all |
| anything else, editor focused | the editor's own (`ratatui-textarea` defaults: shift+arrows select, `ctrl+u`/`ctrl+r` undo/redo) |

A run has no time limit unless the connection sets its own query timeout in
the dbx desktop app (dbx's default of 60 counts as unset); a pasted text over
256 KiB, or with a line over 16 KiB, is refused with a notice.

Only one run at a time per tab: a second is refused with a notice, while the
first runs and while it is being cancelled (the session is one connection, and
a new statement would only queue behind the old one). The run's state is on
the right of the mode bar and at the top of the results pane:

| State | Mode bar | Results pane |
|---|---|---|
| running | `running 3s` (woken each second through `next_deadline`) | `● running 3s`, then the last result, headed `earlier run:` |
| timed out, stopping | `timed out: stopping… 3s`, once the connection's query timeout has passed | the same |
| cancelling | `cancelling… 3s` | `● cancelling…`, the last result as above |
| not stopped yet | `not stopped yet 3s`, red, 2 s after a cancel (or a timeout) the statement has not stopped | says the client cannot tell whether the cancel reached the server, and that `ctrl+c` again offers to stop waiting |
| cancelled | `cancelled after 1.2s` | the statement cancelled, then the last result, headed `earlier run:` |
| finished before the cancel | `finished before the cancel` | any changes it made stand; its result is not shown |
| failed after the cancel | `failed after the cancel` | the error, named (the statement is not called stopped) |
| stopped waiting | `stopped waiting: not confirmed stopped` | the tab runs in a new session |
| done | `N rows 12ms` (`N+` when the row cap of 1,000 cut the result), `N rows affected` | the first line of the statement that ran, the header and the rows that fit, numbers right-aligned, NULL as a dimmed `∅`, a line break in a cell as `⏎` |
| failed | `failed 4ms` | dbx's message, `line L, column C` when Postgres gives a position, and the statement that ran with a `^` under the position |
| timed out | `timed out after 1.0s`, peach | `● Timed out`, naming the connection's query timeout as the limit, and saying it was stopped only when it was |

A cancel bumps the run's generation, so nothing the run replies is shown; its
reply only says it has ended, which moves the tab from cancelling to cancelled.
Postgres never confirms a cancel, so `DbxCore` ends a cancelled run only once
the tab's session answers a `select 1` again, that is once the statement has
stopped: dbx itself returns as soon as it has sent the cancel request, even one
that never arrived. That wait never opens a session (only one dbx still holds,
and not one the tab has closed). The connection's query timeout is kept by
`DbxCore`, not dbx: when it passes, the run is cancelled through its token like
a user's cancel (dbx's own timeout would discard the session's connection, and
with it the session's `SET`s, temporary tables and transaction), and the
outcome is a timeout. A cancel lands in under 10 ms on the fixture.

If a cancel has not stopped the statement after 2 s, a second cancel asks
whether to stop waiting. Yes: the tab runs again at once in a new session, the
old one is closed when dbx lets go of the statement, and the results pane says
"an abandoned statement may still be running on <connection / database>"
until that run's last reply arrives. The statement may keep running on the
server: a write outside a transaction is committed when it ends, and until then
it may hold locks that make the tab's statements wait. Closing a busy session
returns at once but does not stop its statement; dbx drops the connection when
the statement ends. Giving the tab another target, or removing its connection,
while a run is in flight leaves the same marker. Quitting while a statement may
still be running prints, once the terminal is restored,
`dbx-tui: A statement may still be running on <connection / database>.`

## The result grid (results pane)

A run that gives columns shows a grid: a fixed header row, a row-number
gutter, a cell cursor, a `▕` scrollbar when the rows overflow, and
`r 501/10,000  c 12/120` on the pane's title row (`+` after the row count when
the row cap cut the result). Drawing and scrolling are virtual on both axes:
only the rows and whole columns in the window are touched, and each cell is
read only as far as a column can show it, so a million-row result or a 10 MB
cell costs a frame what a small one does. Keys, with the results pane focused
(all from `Keybinds`, shown in the mode bar and clickable):

| Key | Does |
|---|---|
| `h` `j` `k` `l` and the arrows | move the cell cursor |
| `pagedown` `ctrl+d` `pageup` `ctrl+u` | a page of rows |
| `home` `0` / `end` `$` | first / last column |
| `g` / `G` | first / last row |
| `enter` | open the cell viewer |
| `y` / `Y` | copy the cell / the row |
| `m` | load more (only on a truncated result) |

Mouse: a click puts the cursor on a cell, the wheel scrolls rows (three per
notch), shift+wheel or a horizontal wheel scrolls columns, a click on a hint
does what its key does.

**Column widths** are the display width (`unicode-width`) of the header and the
cells in a sample window, clamped to 3..40. The window is the rows from the
viewport's chunk of 128 rows, two chunks or more, so a column keeps its width
while the viewport stays inside it, a result of up to two chunks is measured
whole, and a jump to the end of a million rows measures a few hundred rows,
not the million. Columns are measured when first needed and cached until the
result or the window changes. Cells cut with `…`; a line break is `⏎`, a tab
`⇥`, NULL a dimmed `∅`, a cell a short row lacks `⊘`; numbers are right-aligned.

**The cell viewer** (`enter`) is an overlay with the column, type and row in
its title (and the value's length in characters and bytes): the whole value wrapped by display width, scrolled by line
(`j`/`k`), page and `g`/`G`; NULL, `empty string` and the text `NULL` look
different (so does a cell the row lacks); every hard line break ends in a dimmed `↵` (LF), `←` (CR) or `←↵` (CRLF) and trailing spaces are a dimmed `·`, so values that differ look different; a JSON cell is pretty-printed (whitespace only: keys keep their
order and numbers their digits) and `r` shows the stored text. Only the lines
on screen are wrapped; `G` on a huge value wraps it all once. `y` copies the
stored value, never the pretty form. `esc` closes it.

**Copy** sends `Command::CopyToClipboard`, which the shell writes as an OSC 52
sequence (`ESC ] 52 ; c ; <base64> BEL`): it works over SSH and in kitty, and
in tmux with `set-clipboard on`; a terminal that ignores OSC 52 (tmux's
default `set-clipboard external`, macOS Terminal.app, iTerm2's default) drops it
silently, so the notice says "Sent cell to the terminal clipboard", not
"Copied". A cell a short row lacks has nothing to copy. The text is base64, so cell data cannot be read by the terminal as
anything but the clipboard's content. At most 75,000 bytes (100,000 of
base64, xterm's default cap) are sent; a larger copy is refused with a notice
saying so. A row is the cells' exact texts joined by tabs, NULL as an empty
field; a cell that contains a tab or a line break is copied as it is.

**Load more** (`m`) re-runs `ResultSet.ran` with twice the rows fetched, up to
`MAX_ROW_CAP` (100,000 rows) and `MAX_CELLS` (2,000,000 cells, so a 120-column
result stops near 16,000 rows). The result on screen stays navigable under a
`loading more…` line, and the cursor stays on its row and column when the rows
land. When they land the old rows are compared once with the start of the new
ones, and "The rows changed since the last run." says so if they differ (and
that the cursor moved, if its row had to be clamped). It does nothing on a
result that was not cut, and is refused with a notice while the tab runs or
cancels, once the tab's session is another one, at the ceiling, and for a
statement that changes data; the `m more` hint is not offered where it could
only be refused. `ResultSet.rerun`, set by `DbxCore`, has three values: a first
keyword of `SELECT`, `WITH`, `VALUES` or `TABLE` and dbx's
`classify_sql_risk_for_database` saying read-only (a denylist) are needed for
anything but `Write`; then dbx's `prove_read_only_for_database` (an allowlist
of pure functions) decides between `Proven` (loads as is) and
`ReadOnlyUnproven` (`select f()`, `select pg_notify(..)`: `m` opens a question
naming the statement's first line, `enter` re-runs, `esc` cancels).

## Tests

Run every command from the dbx workspace root.

| What | Command | Needs |
|---|---|---|
| Fast: app, view, keys, FakeCore (layer 2) | `cargo test -p dbx-tui --lib --test no_colour_literals` | nothing |
| Store: `DbxCore` on scratch stores (layer 1) | `cargo test -p dbx-tui --test store -- --test-threads=1` | nothing external; temp dirs, key files, a local TCP port |
| Live: `DbxCore` against the fixture Postgres (layer 1) | `crates/dbx-tui/tests/fixtures/up.sh && cargo test -p dbx-tui --test live -- --test-threads=1` | Docker (the fixtures below) |
| Fixtures: the test databases | `crates/dbx-tui/tests/fixtures/up.sh && crates/dbx-tui/tests/fixtures/check.sh` | Docker |
| Journeys: the built binary in tmux (layer 5; `add-connection`, `browse-pagila`, `cancel`, `first-query`, `quit-mid-run` and `wide-result` need the fixtures) | `for j in crates/dbx-tui/tests/journeys/*.sh; do [ "$(basename "$j")" = lib.sh ] \|\| bash "$j"; done` | tmux, openssl, python3 |
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
printed at the end. `browse-pagila` also starts a throwaway local socket that accepts and never answers (a listing that hangs, collapsed, and then in flight while the client quits). `cancel` runs a small local proxy to the fixture that swallows Postgres cancel requests, for a cancel that never lands (the live tests have the same proxy in Rust). `examples/seed_store` saves a connection with a query timeout when `DBX_TUI_SEED_QUERY_TIMEOUT` is set. `examples/seed_store` takes the password from `DBX_TUI_SEED_PASSWORD` when set.

### Fixture databases

`tests/fixtures/compose.yaml` runs Postgres 16, pinned by digest, on
`127.0.0.1:55433` (user `postgres`, password `dbxtui`), compose project
`dbxtui-fixtures`, data on tmpfs. It holds two databases:

- `pagila`: Pagila at commit `5ba5a57`, fetched by `fetch-pagila.sh` and
  checked by SHA-256 (current Pagila master needs Postgres 18).
- `edge`: our edge-case schema (`edge/*.sql`), values whose exact text is
  known: NULL vs `''` vs `'NULL'`, wide text, integers beyond 2^53, NaN and
  Infinity, time zones, odd identifiers, a second schema. For the result
  grid: `wide_120` (120 columns `c001`..`c120` of text, numeric, jsonb and
  integer, 400 rows; every value encodes its own column and row), `million_rows`
  (id, text, number; a million rows by `generate_series`, about a second to
  load) and `width_text` (wide characters, combining marks, joiners).

`up.sh` starts it and waits until it answers queries; `down.sh` removes it.
