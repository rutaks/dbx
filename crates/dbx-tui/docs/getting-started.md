# Getting started with dbpane

About ten minutes, from install to a first query, a browsed table and an
export. You need a PostgreSQL server you can reach (a local one, a cloud one,
or a throwaway you start yourself); dbpane itself needs nothing else, and in
particular no Docker.

Keys that begin with `ctrl+g` are the **prefix**: press `ctrl+g`, release it,
then press the next key. The bar at the bottom of the window always shows the
keys that work right now; click a hint and it does what its key does.

## 1. Install

```sh
curl -fsSL https://github.com/rutaks/dbpane/releases/latest/download/install.sh | sh
dbpane --version
```

If `dbpane` is not found afterwards, the installer printed the line to add
`~/.local/bin` to your `PATH`. See the [README](../README.md) for the options
and for how to uninstall.

## 2. First run and the welcome

Run `dbpane`. With no connection saved, the main area says "Welcome to
dbpane", names the store it is using and where it is, and lists three keys:

<!-- keys: prefix -->
| Key | Does |
|---|---|
| `ctrl+g n` | add your first connection <!-- NewConnection --> |
| `ctrl+g ?` | help and all keys <!-- Help --> |
| `ctrl+g q` | quit <!-- Quit --> |
<!-- keys: end -->

If you already use the dbx desktop app, dbpane finds its store and your
connections are there already; the welcome does not appear. Otherwise dbpane
makes its own store (see "Where your data is stored" in the README).

## 3. Add a connection

Press `ctrl+g n`. Fill in the form, moving between fields with `tab`: name,
host, port (5432), user, password, database, then two switches, TLS and
**production**. Press `ctrl+t` to test the connection (a failure shows the
server's message, never your password) and `enter` to save. `esc` cancels
without losing what you typed.

Mark a connection *production* if it is one. Statements that change anything
then ask for a confirmation before they run (see Safety in the README).

The connection appears in the tree on the left. `enter` opens it, and then its
databases, schemas and tables. Only the databases you open are connected to.

To change or remove a connection, put the cursor on it in the tree:

<!-- keys: sidebar -->
| Key | Does |
|---|---|
| `e` | edit the connection <!-- EditConnection --> |
| `d` | remove it, after a question that names it <!-- RemoveConnection --> |
<!-- keys: end -->

## 4. Run a query

A query tab runs on one connection and database. Choose it in the tree: move
to a database (or anything inside it) and press `u`.

<!-- keys: sidebar -->
| Key | Does |
|---|---|
| `j` | move down the tree <!-- MoveDown --> |
| `k` | move up <!-- MoveUp --> |
| `enter` `l` | open a node <!-- Expand --> |
| `h` | close a node, or go to its parent <!-- Collapse --> |
| `u` | use this database for the query tab <!-- UseTarget --> |
<!-- keys: end -->

Then move to the editor with `ctrl+g l`, type a statement, and run it:

<!-- keys: direct -->
| Key | Does |
|---|---|
| `f5` `ctrl+enter` | run the selection, or the statement under the cursor <!-- RunDirect --> |
| `ctrl+space` | completion for tables, columns and keywords <!-- CompleteOpen --> |
| `ctrl+c` | cancel the statement while it runs <!-- CancelDirect --> |
<!-- keys: end -->

The result appears below the editor with its row count and time. Statements
are separated by `;`. A result is cut at 1,000 rows; `m` in the results pane
loads the next 1,000. Move around the grid with `ctrl+g j` to focus it and
then `h` `j` `k` `l`; `enter` opens a cell in a viewer and `y` copies it.

<!-- keys: prefix -->
| Key | Does |
|---|---|
| `ctrl+g l` | focus the pane on the right (the editor) <!-- FocusRight --> |
| `ctrl+g j` | focus the pane below (the results) <!-- FocusDown --> |
| `ctrl+g k` | focus the pane above (the editor) <!-- FocusUp --> |
| `ctrl+g h` | focus the pane on the left (the tree) <!-- FocusLeft --> |
| `ctrl+g z` | zoom the focused pane to the whole window <!-- Zoom --> |
<!-- keys: results -->
| Key | Does |
|---|---|
| `l` | move right in the grid <!-- GridRight --> |
| `enter` | open the cell <!-- OpenCell --> |
| `y` | copy the cell <!-- CopyCell --> |
| `m` | load more rows <!-- LoadMore --> |
<!-- keys: end -->

## 5. Tabs

Every tab has its own target, editor, results and database session, so a `SET`
or a temporary table lasts from run to run in that tab. A tab keeps running
while you look at another.

<!-- keys: prefix -->
| Key | Does |
|---|---|
| `ctrl+g t` | new tab, on the same target <!-- NewTab --> |
| `ctrl+g T` | close the tab (asks if it holds text) <!-- CloseTab --> |
| `ctrl+g ]` | next tab <!-- NextTab --> |
| `ctrl+g [` | previous tab <!-- PrevTab --> |
<!-- keys: end -->

## 6. Browse a table

In the tree, put the cursor on a table and press `o` (or `f4`). dbpane opens
the table in a tab of its own, with the select that reads its first page in the
editor, ordered by the primary key, and runs it.

<!-- keys: sidebar -->
| Key | Does |
|---|---|
| `o` `f4` | browse the table <!-- OpenTable --> |
<!-- keys: results -->
| Key | Does |
|---|---|
| `]` | next page <!-- NextPage --> |
| `[` | previous page <!-- PrevPage --> |
| `#` | count the rows (a real query, so only when you ask) <!-- CountRows --> |
<!-- keys: end -->

## 7. History

Every statement you run is saved in the store, in the same history the dbx
desktop app shows. `ctrl+g e` (or `ctrl+alt+e`) opens it: type to search,
`enter` puts the statement in the editor, `f5` runs it again where it ran
before. The same key closes it.

<!-- keys: prefix -->
| Key | Does |
|---|---|
| `ctrl+g e` | open the history <!-- History --> |
<!-- keys: direct -->
| Key | Does |
|---|---|
| `ctrl+alt+e` | open the history (reaches the program in more terminals) <!-- HistoryDirect --> |
<!-- keys: end -->

## 8. Export

With a result on screen and the results pane focused, press `e`. The overlay
offers CSV, JSON, Excel, Markdown and SQL INSERT statements, a path (by default
in the directory you started dbpane in, named after the table and the time),
and lists what the chosen format loses. `enter` writes the file; if one is
there it asks first, naming it. The file holds the rows dbpane holds: when a
result was cut at 1,000, the overlay says so.

`C` copies the rows as text to the terminal's clipboard instead (CSV, JSON,
Markdown or INSERTs; up to about 75 KB).

<!-- keys: results -->
| Key | Does |
|---|---|
| `e` | export to a file <!-- Export --> |
| `C` | copy as text <!-- CopyAs --> |
<!-- keys: end -->

## 9. Dump a database

In the tree, put the cursor on a database or a schema and press `D`. The
overlay asks what goes in (structure and data, structure only, data only), the
scope, whether to add `DROP TABLE IF EXISTS`, and the path.

<!-- keys: sidebar -->
| Key | Does |
|---|---|
| `D` | dump the database or schema <!-- Dump --> |
<!-- keys: end -->

**Read this before trusting a dump.** dbpane asks dbx to write it, and dbx
reports success while leaving some things out or writing SQL that does not
restore:

- Postgres domains are not written, so a table that uses one cannot be
  created and its rows are lost.
- Composite types used as arrays are not written.
- A partition's definition is written in a form that does not parse; a
  partitioned table's rows are written before its partitions exist.
- `NaN`, `Infinity` and `-Infinity` become `NULL`.
- Geometric and `tsquery` values come out as binary text Postgres refuses, and
  `tsvector` columns are left out.
- A table with several parents is created twice.

A structure-only dump restores every table dbx can make, empty. For a backup,
use `pg_dump`.

## 10. Terminal setup

dbpane asks your terminal for the kitty keyboard protocol and uses it if there
is one (then every key above works). Where there is none, `f5` and the
`ctrl+g` prefix work everywhere.

- **kitty:** kitty keeps `cmd+enter`, `cmd+w` and other Cmd shortcuts for
  itself. Run `dbpane kitty-setup`: it prints one line, `include <path>`, to add
  to `kitty.conf` (`dbpane kitty-setup --write` first copies the file into
  kitty's configuration directory). The file releases those keys only while
  dbpane is the program in the focused window.
- **tmux:** add `set -s extended-keys on` to `tmux.conf` so `ctrl+enter` and
  `ctrl+/` get through. Cmd keys never pass through tmux; use the Ctrl keys,
  `f5` or the prefix.
- **macOS Terminal.app:** no keyboard protocol; use `f5` and the prefix.
- **Over SSH** nothing extra is needed beyond the above on the machine whose
  terminal you are typing in.

## Troubleshooting

**macOS asks to use a keychain item, again, after an upgrade.** This applies to
the dbx desktop app's store, whose key is in your login keychain. The keychain
lets only the program that stored an item read it silently, and an unsigned
program has no stable identity, so each new `dbpane` build is a stranger. Choose
**Always Allow** (it lasts until the next upgrade), or use dbpane's own store,
which keeps its key in a file and never asks (`dbpane --store own`; it starts
empty). Never enter your password into a dialog you did not expect: this one is
named after `dbpane` or `dbx`.

**macOS says dbpane cannot be opened because it is from an unidentified
developer.** The installer removes the quarantine flag. If you downloaded the
archive in a browser, run `xattr -d com.apple.quarantine ./dbpane` once, or
open System Settings, Privacy & Security, and choose Open Anyway next to the
message about `dbpane`. (On macOS 15 and later, right-click Open no longer
works for a bare program.)

**Linux without a secret service** (a server, a container, a plain window
manager). dbx's own store would keep its key in the Secret Service, and there is
none, so dbx falls back to a key file, `~/.config/dbx/secret.key` (or
`$XDG_CONFIG_HOME/dbx/secret.key`). dbpane's own store avoids the question: its
key is the file `<store>/.dbx/secret.key`, made on first start, readable by you
only. Keep it with the store. To choose where the key is, point
`DBX_SECRET_KEY_FILE` at a file holding 64 hex digits (`openssl rand -hex 32`
makes one). If a Secret Service is running but locked or unreadable, dbpane
stops with a message that names `DBX_SECRET_KEY_FILE`: set it, or unlock the
keyring.

**"the store directory must exist" or "must be absolute".** `--store <dir>` and
`DBX_DATA_DIR` take an absolute path of a directory that already exists.

**Trying dbpane without touching your real store or keychain.** A store named
with `--store <dir>` or `DBX_DATA_DIR` keeps its key in the OS keychain or
Secret Service, unless the directory already holds `.dbx/secret.key` or a key
comes from `DBX_SECRET_KEY_FILE` (a file of 64 hex digits) or `DBX_SECRET_KEY`.
So for a scratch store set both:
`DBX_DATA_DIR=<empty dir> DBX_SECRET_KEY_FILE=<key file> dbpane`.

**Which store am I in?** `dbpane --help` ends with the store it would use and
why. The welcome and the connection list say the same.

**Keys do nothing, or the wrong thing.** Press `ctrl+g ?` for what is bound.
Inside kitty without the include file, or inside tmux without `extended-keys`,
Cmd and some Ctrl combinations never reach dbpane: use `f5` and the prefix, or
see Terminal setup above.

**No database to try it on, and no Docker needed.** Any PostgreSQL will do. If
you have Docker and want a throwaway:
`docker run --rm -e POSTGRES_PASSWORD=pw -p 55432:5432 postgres:16`, then
connect to `127.0.0.1` port `55432`, user `postgres`. Stop it with `ctrl+c`;
dbpane never starts or stops containers.

**A window that is too small.** `ctrl+g z` zooms one pane to the whole
window. `ctrl+g R` starts resizing: `h` `j` `k` `l` move the divider, `esc`
ends it.

<!-- keys: prefix -->
| Key | Does |
|---|---|
| `ctrl+g R` | resize the panes <!-- ResizeMode --> |
<!-- keys: resize -->
| Key | Does |
|---|---|
| `h` | move the divider left <!-- ResizeLeft --> |
| `esc` | end resizing <!-- ResizeDone --> |
<!-- keys: end -->
