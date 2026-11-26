# dbpane

dbpane is a database client for the terminal: a schema tree, tabbed SQL
editors with completion, a result grid, table browsing, history, export and
dump, all from the keyboard (the mouse works too). It is built on the Rust core
of [dbx](https://github.com/t8y2/dbx), so it reads and writes the same
connection store as the dbx desktop app.

```text
 connections                     │ 1 no target  2 rental ×
                                 │ ▎ 1 SELECT * FROM "public"."rental" ORDER BY "rental_id" LIMIT 1000;
 ▾ ● local pg           postgres │
   ▸ edge                        │
   ▾ pagila                      │
     ▾ public                    │
       ▸ actor                   │
       ▸ actor_info         view │
       ▸ customer                │
       ▸ film                    │
       ▸ language                ├─ results ───────────────────────────────────────────────────────────r 1/1,000  c 1/7─
       ▸ payment                 │ rental · rows 1–1,000
       ▸ rental                  │      rental_id  rental_date                inventory_id  customer_id
       ▸ staff                   │    1         1  2022-05-24T21:53:30+00:00           367          130                ▕
       ▸ store                   │    2         2  2022-05-24T21:54:33+00:00          1525          459                ▕
   ▸ postgres                    │    3         3  2022-05-24T22:03:39+00:00          1711          408                ▕
   ▸ template1                   │    4         4  2022-05-24T22:04:41+00:00          2452          333                ▕
                                 │    5         5  2022-05-24T22:05:21+00:00          2079          222                ▕
 RESULTS   enter view  y copy  Y copy row  h/j/k/l move  e export  C copy as  ] next page  # count     1,000 rows  44ms
```

*A capture of dbpane browsing the `rental` table of the Pagila sample database
(trimmed to fit).*

## Install

macOS (Apple Silicon or Intel) and Linux (x86_64, glibc 2.35 or newer: Ubuntu
22.04, Debian 12, Fedora 36 or newer; Alpine and other musl systems are not
supported):

```sh
curl -fsSL https://github.com/rutaks/dbpane/releases/latest/download/install.sh | sh
```

The script downloads the archive for your system from the latest release,
checks it against `sha256sums.txt`, and puts `dbpane` in `~/.local/bin`. It
never uses `sudo`, and tells you how to add that directory to your `PATH` if it
is not there. Useful variables: `DBPANE_VERSION=0.1.0` (a release other than
the latest; digits only, `X.Y.Z`), `DBPANE_INSTALL_DIR=<absolute dir>`. In a
shell running under Rosetta on an Apple Silicon Mac, it installs the native
arm64 build. `--dry-run` prints what it would do
and changes nothing: `curl -fsSL <url> | sh -s -- --dry-run`.

The binaries are **not signed**. On macOS the installer removes the quarantine
flag from the installed `dbpane` (and only from it), so Gatekeeper does not
block it. If you download the archive by hand and macOS blocks `dbpane`, either
open System Settings, Privacy & Security, scroll to the message about `dbpane`
and choose Open Anyway (on macOS 15 and later the right-click Open shortcut no
longer works for a bare program), or run
`xattr -d com.apple.quarantine <path to dbpane>` once. If you would rather read the script first, it is
[`install.sh`](https://github.com/rutaks/dbpane/releases/latest/download/install.sh).

Check it worked:

```sh
dbpane --version
```

## First steps

Start `dbpane`. With no connection saved it shows a welcome and the three keys
you need. The [getting-started guide](docs/getting-started.md) takes you from
here to a first query, a browsed table and an export in about ten minutes. In
short:

1. `ctrl+g n` opens the new-connection form; fill it in, `ctrl+t` tests it,
   `enter` saves.
2. In the tree, `enter` opens a node; on a database or schema, `u` gives the
   query tab that target.
3. `ctrl+g l` moves to the editor; type SQL and press `f5`.
4. `ctrl+g ?` lists every key. `ctrl+g q` quits.

## The essential keys

Keys that begin with `ctrl+g` use the prefix: press `ctrl+g`, release, then the
next key (the prefix is `ctrl+g` and not `ctrl+b`, so tmux and herdr keep
theirs). The mode bar at the bottom always shows the keys that work right now,
and each hint is clickable. `ctrl+g ?` opens the full list from the live
bindings: it is the reference for every key in every pane and overlay, each
with the name the action has, and a search field narrows it by action or key.

`ctrl+g p` opens the navigator: type part of a table or view name (its first
letters, its word initials such as `fa` for `film_actor`, or any part of it),
and `enter` shows it in the sidebar while `ctrl+o` opens its rows. It looks in
the database the tab uses, or else the one under the sidebar cursor.

<!-- keys: prefix -->
| Key | Does |
|---|---|
| `ctrl+g n` | new connection <!-- NewConnection --> |
| `ctrl+g ?` | help: every key <!-- Help --> |
| `ctrl+g q` | quit <!-- Quit --> |
| `ctrl+g h` | focus the pane on the left (the tree) <!-- FocusLeft --> |
| `ctrl+g l` | focus the pane on the right <!-- FocusRight --> |
| `ctrl+g j` | focus the pane below (results) <!-- FocusDown --> |
| `ctrl+g k` | focus the pane above (editor) <!-- FocusUp --> |
| `ctrl+g z` | zoom the focused pane <!-- Zoom --> |
| `ctrl+g t` | new query tab <!-- NewTab --> |
| `ctrl+g T` | close the tab <!-- CloseTab --> |
| `ctrl+g ]` | next tab <!-- NextTab --> |
| `ctrl+g [` | previous tab <!-- PrevTab --> |
| `ctrl+g e` | history <!-- History --> |
| `ctrl+g p` | go to a table or view by part of its name (the navigator) <!-- Navigator --> |
| `ctrl+g c` | cancel the running statement <!-- Cancel --> |
<!-- keys: direct -->
| Key | Does |
|---|---|
| `f5` `ctrl+enter` | run the selection, or the statement under the cursor <!-- RunDirect --> |
| `ctrl+c` | cancel a running statement <!-- CancelDirect --> |
| `ctrl+space` | completion in the editor <!-- CompleteOpen --> |
| `ctrl+alt+e` | history <!-- HistoryDirect --> |
| `cmd+o` | the navigator, in a terminal that delivers Cmd keys <!-- NavigatorDirect --> |
<!-- keys: sidebar -->
| Key | Does |
|---|---|
| `j` | move down <!-- MoveDown --> |
| `k` | move up <!-- MoveUp --> |
| `enter` `l` | open the node under the cursor <!-- Expand --> |
| `h` | close it, or go to the parent <!-- Collapse --> |
| `u` | use this database for the query tab <!-- UseTarget --> |
| `o` `f4` | browse the table <!-- OpenTable --> |
| `e` | edit the connection <!-- EditConnection --> |
| `d` | remove the connection (asks first) <!-- RemoveConnection --> |
| `D` | dump the database or schema to a file <!-- Dump --> |
| `r` | reload the node <!-- Refresh --> |
<!-- keys: results -->
| Key | Does |
|---|---|
| `h` | move left in the grid <!-- GridLeft --> |
| `l` | move right <!-- GridRight --> |
| `enter` | open the cell in a viewer <!-- OpenCell --> |
| `y` | copy the cell <!-- CopyCell --> |
| `Y` | copy the row <!-- CopyRow --> |
| `e` | export the rows to a file <!-- Export --> |
| `C` | copy the rows as CSV, JSON, Markdown or INSERTs <!-- CopyAs --> |
| `]` | next page of a browsed table <!-- NextPage --> |
| `[` | previous page <!-- PrevPage --> |
| `#` | count the rows of a browsed table <!-- CountRows --> |
| `m` | load more rows when a result was cut at 1,000 <!-- LoadMore --> |
<!-- keys: end -->

(The `ctrl+g` table is checked against dbpane's live key bindings by a test, so
a key listed here is a key that works in the pane named above its table.)

## Where your data is stored

dbpane keeps connections, their passwords and the statement history in a
**store**, the same SQLite file the dbx desktop app uses. At start it picks, in
order:

1. `--store dbx`, `--store own` or `--store <directory>`, if you gave one.
2. `DBX_DATA_DIR`, if set.
3. **dbpane's own store**, if it already exists: `~/Library/Application
   Support/dbpane` on macOS, `~/.local/share/dbpane` (or `$XDG_DATA_HOME/dbpane`)
   on Linux. Once it exists it stays the one in use, even if you install the
   desktop app later.
4. The **dbx desktop app's store**, if you have one: `~/Library/Application
   Support/com.dbx.app` or `~/.local/share/com.dbx.app`. The two apps then
   share connections and history, and the key that protects the saved
   passwords stays in your keychain (macOS) or Secret Service (Linux).
5. Otherwise a new dbpane store.

`dbpane --help` ends with the store it would use right now and why.

Saved passwords are encrypted. For dbx's store the key lives in the OS keychain
or Secret Service. For dbpane's own store the key is a file inside it,
`<store>/.dbx/secret.key` (readable by you only): back it up with the store, and
without it saved passwords cannot be read. Setting `DBX_SECRET_KEY_FILE` to a
file with a 64-hex-digit key, or `DBX_SECRET_KEY` to the 64 hex digits
themselves, overrides every other source (`DBX_SECRET_KEY_FILE` wins if both
are set).

A store you name with `--store <dir>` or `DBX_DATA_DIR` also uses the OS
keychain or Secret Service for its key, unless the directory already holds
`.dbx/secret.key` or `DBX_SECRET_KEY_FILE` (or `DBX_SECRET_KEY`) is set. To
keep a scratch store off your keychain, set `DBX_SECRET_KEY_FILE` as well as
`DBX_DATA_DIR`.

## Safety

- **Production confirmations.** Mark a connection as *production* in its form,
  or list databases as production in the dbx desktop app. A statement that is
  not read-only then asks first, in a red box that names the connection and
  database and shows the exact statement. Only `y` runs it; `enter` does not,
  so a fast `f5` `enter` cannot confirm by accident. A `PROD` marker is
  shown on the tab before anything runs. By default dangerous statements
  (such as `DROP`, `TRUNCATE`, `CREATE`, `SET` and transaction control) are
  confirmed on every connection, production or not. dbx decides what is
  dangerous; the box says only that dbx flags it.
- **Connections marked read-only** refuse writes with dbx's own error.
- **Nothing is deleted from the history**, and removing a connection or closing
  a tab with text asks first.
- **Export copies what the client holds.** A result is cut at 1,000 rows
  (`load more` gets the next 1,000); the export overlay says how many rows the
  file will hold, and lists what the format loses (CSV quotes everything and
  writes NULL as `\N`; Markdown shows NULL as the text `NULL`; Excel shows NULL
  and the empty string alike; SQL INSERT does not create the table).
- **A dump is not a backup.** dbpane asks dbx to write the dump, and dbx's dump
  loses some things while reporting success: Postgres *domains*, composite
  types used as arrays, partition definitions (six of Pagila's seven fail to
  parse), `NaN` and the infinities (written as `NULL`), geometric, `tsquery` and
  `tsvector` values, and tables with several parents. Restore into a scratch
  database and read the errors before relying on one. Use `pg_dump` for backups.

## Supported databases

**PostgreSQL** today: browsing, queries, completion, export, dump. dbx's
connection types for MySQL and SQLite are listed in the tree but cannot be
browsed yet; MySQL and SQLite support is coming.

## Uninstall

```sh
rm ~/.local/bin/dbpane                  # or the directory you installed to
rm -r ~/.local/share/dbpane             # dbpane's own store (Linux); this deletes your saved connections
rm -r ~/Library/Application\ Support/dbpane   # the same on macOS
```

Leave the last two alone if you want to keep your connections. The dbx desktop
app's store is never touched by dbpane's uninstall. If you ran `dbpane
kitty-setup --write`, remove the `include` line it told you to add to
`kitty.conf`.

## Licence and credits

dbpane is licensed under the Apache License 2.0. It is a modified work of
[dbx](https://github.com/t8y2/dbx) (Apache-2.0); the changes are one added
crate, `crates/dbx-tui`, in the fork
[rutaks/dbx](https://github.com/rutaks/dbx), branch `dbx-tui`, which is where
the source is. Its look and layout, and its colour roles, follow
[herdr](https://github.com/herdrdev/herdr) (Apache-2.0). Every release archive
contains `LICENSE`, `NOTICE` and `THIRD-PARTY.txt` (the licences of the crates
it is built from), and `dbpane --licenses` prints them.
