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

## Capabilities (what a connection can do)

Each `Connection` carries a `Capabilities` value (`src/core/mod.rs`): whether
its tree has a database level and a schema level, whether dbx runs queries on
it, browses its metadata, edits its table data and explains statements, the
default port, the SQL dialect (`Dialect`: what the highlighter, formatter and
completion quoting choose between) and whether it uses a single server
connection. `app` reads these and never asks which database it is: a test
fails if `DbKind::` appears in `src/app` code.

`DbxCore` fills it in (`capabilities_of` in `src/core/dbx.rs`) from dbx's own
description of the connection type, `plugins/connection-types/*.yaml`:

- from dbx's runtime manifest, `dbx_core::database_manifest` (the generated
  registry in `dbx-types`): `queryExecution`, `metadataBrowse` and
  `tableDataEdit` (with the support-level defaults dbx applies), the default
  port, the dialect name, `localFile` and `singleConnectionPool`;
- from the manifest dbx bundles
  (`crates/dbx-core/assets/database-drivers.manifest.json`), because the Rust
  manifest does not deserialize them: the traits `schemaAware`, `treeSchema`,
  `singleDatabase` and `capabilities.sqlExplain`.

The tree has a database level unless the type is a local file or
`singleDatabase`, and a schema level when it is `schemaAware` or `treeSchema`.
A level that is not there is not a node: the level above lists what is below
it, and the paths carry an empty database or schema name (the Core uses the
connection's own database). `browse` and `edit_in_form` are also gated by
`BROWSED_HERE` in `dbx.rs`, the types whose adapter calls and form this client
is built and tested for (PostgreSQL only so far): adding a database is one
line there plus its tests, not edits across `app`. A type whose
`queryExecution` is false is listed, and running in it says it cannot be
queried. `Syntax::capabilities(kind)` answers the same for a kind before any
connection exists (the new-connection form's port).

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
A `▕` scrollbar appears when the list is longer than the pane. Only the
connection types the client was built for can be browsed so far
(`Capabilities::browse`, see above): any other shows "Browsing MySQL is not
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

The query tab owns its target (`<n> <connection> / <database>` in the tab
bar, the editor's title) and a database session: dbx's `client_session_id` is the tab
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
| `f5`, `cmd+enter`, `ctrl+enter`, `ctrl+g r` | run the selection, or else the statement under the cursor (dbx splits statements at `;`, and also before a line starting with a keyword such as `CREATE`, `INSERT`, `UPDATE`, `DELETE` or `SET`, but not `SELECT` or `WITH`, after two or more blank lines; a cursor on a blank line after a statement runs that statement) |
| `ctrl+c`, `ctrl+f2`, `cmd+f2` (editor or results focused), `ctrl+g c` (anywhere) | cancel the run; pressed again while the cancel has not landed, ask whether to stop waiting (below). With nothing running, `ctrl+c` does nothing, except in the editor with a selection, where it copies |
| `ctrl+space` (editor focused) | open the completion popup (below) |
| `alt+enter` (editor focused) | the intention actions, as in IntelliJ: on a select list's `*` it expands it; anywhere else it does nothing and shows no hint. It is not a run key |
| anything else, editor focused | text, the arrows (shift selects), page keys and the editing keys below. The widget's Emacs keys (`ctrl+k`, `ctrl+u`, `ctrl+r`...) are not bound; `ctrl+u` and `ctrl+r` are no longer undo and redo (`ctrl+z` and `ctrl+shift+z` are) |

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

## Query tabs (slice 15b)

Each query tab has its own target, session, editor, run and result. The tab
bar sits on the editor's top edge: each tab's number, its target
(`local pg / pagila`, or `no target`) and a state glyph, yellow `●` while
its statement runs or is being cancelled, red `●` after a failure or a
timeout, none when idle or done. The active tab is `accent` (bold while the
editor has focus) and carries a `×`; the others are `overlay0`. When the tabs
do not fit, the active one stays and `…` marks the ones left out; the bar is
always one row. A target longer than 24 cells ends in `…`.

| Key | Does |
|---|---|
| `ctrl+g t` | new tab: on the active tab's target, in a session of its own (`dbx-tui-tab-<id>-1`), with an empty editor; it becomes active and the editor takes focus |
| `ctrl+g T`, `ctrl+f4`, `cmd+w` | close the active tab: at once when it is empty and idle; with text, a confirmation names the tab and says its text will be lost; with a statement running, it says the statement will be cancelled. The run is cancelled and its session closed once it ends (or after 2 s), as on a retarget; a statement that may still be running is named at quit. The tab to its left becomes active (to its right, for the first). Closing the last tab leaves one new, empty tab on the same target |
| `ctrl+g ]`, `cmd+shift+]`, `ctrl+tab` | next tab, wrapping |
| `ctrl+g [`, `cmd+shift+[`, `ctrl+shift+tab` | previous tab, wrapping |
| `ctrl+g 1` ... `ctrl+g 9` | go to that tab, if there is one |
| click a tab | make it active (and focus the editor) |
| click the `×` | close the active tab, with the same confirmations |
| `u` in the sidebar | give the active tab a target |

The `cmd` keys, `ctrl+tab` and `ctrl+shift+tab` need the kitty keyboard
protocol, and kitty keeps all of them (and `cmd+w`) for its own tabs unless
`kitty.conf` releases them (`map cmd+w`, `map ctrl+tab` with no action); not
yet tried by hand. `ctrl+f4` and the prefix keys work everywhere.
`alt+left`/`alt+right` are word movement in the editor, not tab keys.

A tab not on screen keeps running. The mode bar shows the active tab's run
state, and only its elapsed time wakes the client; a background tab's reply
lands in that tab, without a notice (a failure is said when the tab is shown
again), and repaints only because its glyph in the bar changed. A closed
tab's late reply changes nothing and repaints nothing.

## Browsing a table (slice 16)

`o` or `f4` on a table or view row in the sidebar (the `o browse` hint is
there and nowhere else) opens it in a tab titled with the table's name, on its
database, or goes to the tab that already browses it. It needs a connection
that browses and runs statements; on one that cannot be queried the key says
so under the sidebar. The tab's editor holds the select of the first page and
it runs at once, in the tab's own session; the results pane has the focus.
Primary keys are read from the columns the tree already loaded; a table the
tree has not opened is asked for its columns first.

The text is built by dbx's `build_table_data_select_sql` (`SELECT * FROM
"public"."film" ORDER BY "film_id" LIMIT 1000 OFFSET 1000;`), with the table's
primary key as the order, and the page size is the row cap (1,000). dbx adds no
order of its own, whatever `fallback_order_columns` suggests (nothing reads it),
so a table without a primary key is read unordered, and the header says `no
primary key: rows may move between pages`. `large_value_preview_size` is never
set, and `QueryExecutionOptions.table_data_preview` is not set either: without
a preview size it extracts nothing on Postgres, and on drivers that hand
large values back deferred it would ask for the placeholder, not the value.
Every value is exact (a 100 kB text arrives whole, tested against the fixture).

| Key (results pane of a browse tab) | Does |
|---|---|
| `]` | the next page; nothing on a page of fewer rows than the page size |
| `[` | the previous page; nothing on the first |
| `#` | counts the rows once with dbx's count statement (`SELECT COUNT(*) AS cnt FROM ...`); never automatic, because it is real work for the server on a large table |

The header reads `film · rows 1,001–2,000`, then ` of ?` from the moment `#`
asks until the answer arrives (never a guess), then ` of 1,000,000`. A page and
the count are runs: refused with a notice while the tab runs, cancelled by
`ctrl+c` (a cancelled count is unasked again), stale by the usual rules, and the
earlier page stays under `earlier run:` while the next one loads. A page that
fails leaves the earlier one and says so. The editor follows the page that
landed, unless there is typing in it: then `]` and `[` are refused (`esc` and
the text are never lost). Load more (`m`) is not offered. Editing the text, or
running a selection of it, and running makes the tab an ordinary query tab: the
title returns to its target and the paging keys leave the mode bar; giving the
tab another target does the same.

## Editor keys (slice 13)

Every editing action is a named action in the live bindings (`Keybinds`), with
IntelliJ's macOS key (`cmd+...`), its Windows/Linux key (Ctrl) and, where one
exists, a key every terminal delivers; both the Cmd and the Ctrl forms are
bound on every platform. `ctrl+g ?` (the prefix, then `?`) lists every action
with all its keys, from the live bindings, and marks in `[brackets]` the keys
this terminal cannot deliver (no keyboard protocol). A few actions also have a
prefix key (`ctrl+g d` duplicate, `x` delete line, `/` line comment, `*` block
comment, `f` find, `g` go to line, `a` select all, `u`/`U` undo/redo, `w`/`W`
extend/shrink, `J` join, `~` toggle case, `=` reformat, `+` reformat all, `%` matching brace); they work in the
editor only and are in the help, not in the mode bar.

| Action | Keys |
|---|---|
| duplicate line or selection | `cmd+d` `ctrl+d` |
| delete line | `cmd+backspace` `ctrl+y` |
| move line up / down | `alt+shift+up` / `down` |
| move statement up / down | `cmd+shift+up/down` `ctrl+shift+up/down` |
| line comment (`-- `) | `cmd+/` `ctrl+/` |
| block comment | `cmd+alt+/` `ctrl+shift+/` |
| extend / shrink selection (word, statement, all) | `ctrl+w` `alt+up` / `ctrl+shift+w` `alt+down` |
| select all | `cmd+a` `ctrl+a` |
| undo / redo | `cmd+z` `ctrl+z` / `cmd+shift+z` `ctrl+shift+z` |
| cut / copy | `cmd+x` `ctrl+x` / `cmd+c` (and `ctrl+c` with a selection, nothing running) |
| paste | the terminal's paste (arrives as a bracketed paste); `cmd+v`/`ctrl+v` only say so |
| find, next, previous | `cmd+f` `ctrl+f`; `enter` `f3` `cmd+g`; `shift+f3` `cmd+shift+g`; `esc` closes |
| go to line | `cmd+l`, `ctrl+g g` |
| new line after / before | `shift+enter` / `cmd+alt+enter` `ctrl+alt+enter` |
| join lines | `ctrl+shift+j` |
| toggle case | `cmd+shift+u` `ctrl+shift+u` |
| reformat the selection, else the statement | `cmd+alt+l` `ctrl+alt+l`, `ctrl+g =` |
| reformat the whole editor | `ctrl+g +` |
| indent / unindent | `tab` / `shift+tab` |
| word left/right, delete word | `alt`/`ctrl` + `left` `right`, `backspace` `delete` (shift selects) |
| line start/end (`home` toggles first non-blank / column 0) | `home` `end` `cmd+left` `cmd+right` |
| text start/end | `cmd+up` `cmd+home` `ctrl+home` / `cmd+down` `cmd+end` `ctrl+end` |
| matching brace | `ctrl+m` `ctrl+shift+m` |

Every edit, however many changes it makes, is one undo step (the widget's own
history is off; `Editor` keeps snapshots). `tab` belongs to the completion popup
first, then to a live template being filled in, then to a selection over several
lines (indent), and last inserts indentation to the next stop (spaces, or a tab
with `Settings::hard_tabs`). Find is plain text, case-insensitive unless the
query has a capital, with the matches in the `find_match` palette role. Smart
keys (each a setting, on): `(` `[` `'` `"` insert the pair, the closer typed
next is typed over, `tab` moves past a closer, `backspace` between an empty
pair takes both, an opener over a selection surrounds it, `enter` keeps the
indent and opens a block between brackets, `home` toggles.

Reformat (`src/app/format.rs`) lays the SQL out on the tokenizer's tokens and
changes only whitespace and, by setting, keyword case: every token is copied
from the text by its span (strings, `$$` bodies, `E''`, quoted names, comments),
and the result is tokenized again and refused if its tokens are not the
source's. Layout: each clause (`select`, `from`, `where`, `group by`, `having`,
`order by`, `limit`, `offset`, joins, `union`...) on its own line; the items of
`select`, `set` and `values` and the columns of a `create table` one per line,
indented; the `and`/`or` of `where`, `having` and `on` one per line, indented
(not a `between`'s `and`, nor one inside `case`); a subquery or CTE indented one
level with its `)` on its own line; calls and other parentheses stay on one
line; one blank line between statements. A comment stays after the token it
followed or on its own line before the token it preceded. The caret stays on the
same token, the result is one undo step, and text that does not tokenize (an
unterminated string or comment) is left alone with a notice giving the reason.
A line break between two string literals is kept (Postgres joins them by it),
and two operators in a row keep their space (`- -1`). A region with a client
command line (`DELIMITER`, a lone `GO`, a `\` psql command) is refused with a
notice, and so is a selection that starts or ends inside a string, comment,
quoted name, `$$` body or word. On MySQL-family and unknown databases only
words MySQL reserves are re-cased, so a table named `language` keeps its case.
Settings (`Settings`): `format_keyword_case` (`Leave` default, `Upper`,
`Lower`; applies to the keyword list the highlighter uses, never to quoted
names, strings, comments or a word after a `.`), `format_indent` (4) and
`format_comma` (`Trailing` default, or `Leading`). Not done: tabs (indents are
spaces), per-dialect style, reformat on paste.

When the caret is inside a subquery (`( select ... )` or a CTE body), run opens
a chooser (the subquery, the statement) and runs exactly the listed text, as a
range sent with the run; `Settings::run_at_caret` is `Ask` (default),
`Smallest` or `Statement`. Running all statements is not offered: the adapter
runs one.

## Terminal setup

At start the client asks the terminal once whether it speaks the kitty keyboard
protocol; if so it turns on disambiguated keys (Cmd arrives as `cmd+`, and
Ctrl+Enter is told from Enter) and turns it off again on quit, panic and signals.
Without the protocol it asks for xterm's `modifyOtherKeys` (level 2), which tmux
(and xterm) honour.

- **No setup needed:** `f5` runs, `ctrl+g` is the prefix, and in a terminal with
  the protocol every Ctrl binding works.
- **kitty:** kitty keeps `cmd+enter`, `cmd+f`, `cmd+l`, `cmd+c` and others for
  itself. `dbx-tui kitty-setup` prints the path of `extras/kitty/dbx-tui.conf`
  and the one line to add to `kitty.conf` (`include <path>`; `--write` copies the
  file into kitty's configuration directory first). The file uses `map
  --when-focus-on var:dbx_tui <key>` for each key, and the client sets the user
  variable `dbx_tui` on start and clears it on exit (`OSC 1337 ; SetUserVar`), so
  the keys are released only while the client is focused. `cmd+v` is not released:
  kitty's paste is how text reaches the client.
- **tmux:** `set -s extended-keys on` for `ctrl+enter` and `ctrl+/`. Cmd never
  passes through tmux; use the Ctrl keys, `f5` or the prefix.
- **macOS Terminal.app:** no keyboard protocol: `f5` and the prefix (and the
  legacy Ctrl keys such as `ctrl+d`, `ctrl+z`).
- With kitty's `macos_option_as_alt no`, Option+letter types a character; the
  Option-arrow combinations are listed in the slice report.

## The editor's look (syntax colouring)

The widget (`ratatui-textarea`) still edits; `app/view/editor.rs` draws. It
reads the lines, caret, selection and scroll offset through `Editor` (the
widget draws once into a scratch buffer only so its view follows the caret and
reports `scroll_offset`, which the mouse mapping needs). Left of the text is a
gutter: a `▎` on the rows of the statement that would run (in the
range-to-execute colour, no fill, like IntelliJ's outline; none while text is
selected, since the selection is what runs), then right-aligned line numbers,
the caret row's brighter. The caret row has a background, the bracket beside
the caret and its partner have the matched-brace background (brackets inside
strings and comments are skipped), and the selection background is drawn over
the syntax colours, which stay the text colour.

Colours come from `app/highlight.rs` (sqlparser 0.62's tokenizer, the dialect
from the active connection's kind): keyword, type, string, number, comment,
function (a word directly followed by `(`), identifier, comma and semicolon,
bad token. "Keyword" is a curated list of core SQL words, not every word
sqlparser knows (it knows `count`, `name`, `date`). Unterminated strings and
comments keep the tokens before them and colour the rest as what opened; an
unterminated quoted name is a bad token (an unterminated `"` run is a string on
MySQL and its family); any other refusal by the tokenizer marks that word bad
and tokenizing restarts after it. A word right after a `.` is an identifier
(`t.key`). The whole buffer is tokenized once per text change, whatever its
size, so a comment or string opened far above the screen is seen; a repaint,
a scroll or a caret move tokenizes nothing. The result is kept until the text,
the kind or the type names (by generation) change (`Highlighter::stats` counts
the work). In the `terminal` theme the editor's selection is drawn reversed
(`Palette::modifier(Role::SelectionBg)`), since no colour shows on every
terminal's own background. Type names come from the new
sync seam `core::Syntax` (`type_names`: dbx's dialect names and their aliases,
because `list_dialect_type_names` has `character varying` but not `varchar`),
asked once per kind. `Syntax::statement_range` places the statement dbx picks
in the buffer (`statement_at`, the function `run` uses, then
`locate_statement`) and is asked once per text and caret; when the text occurs
more than once and the caret is in none of the copies there is no outline.

Themes: `catppuccin` (default), `terminal`, `intellij-dark` (IntelliJ's Dark
values), chosen with the environment variable `DBX_TUI_THEME` until there is a
config file.

## Completion (the popup in the editor)

A popup of suggestions opens below the word being typed (above it when there
is no room, clipped to the terminal; no dimming, no box: it is not an
overlay). It opens by itself once an identifier has a character in it, and
right after a name and a dot; `ctrl+space` opens it by hand, whatever the
word. It does not open inside a string or a comment (the highlighter's roles
say which characters are), on a number, or for a word longer than 128 bytes.
Not at all for a name typed in full when that is the only thing it would list
(so `enter` after `film` is a line break).

| Key (popup open) | Does |
|---|---|
| `enter` | insert the selected suggestion at the caret |
| `tab` | replace the rest of the word to the right of the caret with it |
| `down` `ctrl+n` / `up` `ctrl+p` | move the selection (it stops at the ends) |
| `pagedown` / `pageup` | a window (10 rows) |
| `esc` | close it, only it: the text stays and the next `esc` is the editor's |
| anything else | goes to the editor, and the list follows the text |

The keys are in the mode bar while it is open (live bindings, clickable).
Mouse: a click on an item inserts it, a click anywhere else closes the popup
and goes on, the wheel over it scrolls the list. The popup closes when the
caret leaves the word, the editor loses focus, the prefix key is pressed, a
paste lands, or the tab is given another database.

**What it lists** (`app/complete_context.rs`, from tokens, so it works on SQL
that does not parse; a statement is the tokens between `;`s, read within
64 KiB and 200 rows of the caret):

- after FROM, JOIN, INTO, UPDATE, TABLE and after a comma in a FROM list:
  tables and views of the current schema, schemas, and only the keywords valid
  there (`lateral`, `only`);
- after `name.`: an alias or a table the statement names (`from film f`,
  `join film_actor as fa`) gives its columns with their types; a schema gives
  its tables; any other name is tried as a table of the current schema;
  `schema.table.` gives that table's columns;
- after a table name (and alias): the words that can follow it; after `as`:
  nothing; after `order`, `group`, `insert`, `left`... only what the grammar
  allows;
- elsewhere: keywords, the columns of the tables in the statement, then
  functions (searched by what is typed, from two letters on, in the current
  schema and `pg_catalog`; the common aggregates and window functions
  (`count`, `sum`, `row_number`...) are the client's own list, because dbx's
  search asks for `pg_proc.prokind` `f` and `p` and never returns them).

**In context** (`app/complete_scope.rs`, slice 12). The statement is read into
select blocks, CTEs and subqueries, and the caret's place in that tree gives
what is in scope: the block's own tables and aliases, derived tables (their
alias and select-list names), CTEs defined before the caret, and, for a
correlated subquery, the outer blocks after them (a derived table without
`lateral` and a CTE body see nothing outside). dbx's `analyze_sql_references`
(`Core::analyze_references`, asked once per statement text, answered later like
any Core call) replaces the tables the tokens found when the statement parses;
when it does not, the tokens stand. In a column position every source's columns
are listed with its alias, a name two sources share is inserted qualified
(`f.film_id`), and a keyword that starts like the word ranks before a column
only where it is valid. After `join x on` the first items are whole conditions
from `Core::foreign_keys` of `x` and of each earlier table (`f.film_id =
fa.film_id`, cached like columns). `insert into t (` lists all columns as one
item first; `group by`/`order by` list the select list's expressions and
aliases first. Accepting a table after `from`/`join` adds an alias (initials
of the `_`-separated words, numbered when taken; `Settings::alias_on_table`,
on by default), unless one is typed after the word or the word continues; a
name that exists in more than one schema (found by a search across schemas,
`CompletionRequest::global`) is inserted as `schema.name`. A generated alias is
never a reserved word of the dialect (Postgres' list, sqlparser's reserved
lists and the highlighter's keywords) nor a table, CTE or alias the statement
has, none is added when the statement already writes the table's own name
before a dot (`film_actor.actor_id`), and the row shows the alias that will be
inserted (`film f`). A column of an outer query, and every column while any
source's columns are not loaded yet, is inserted qualified (a bare name would be
the nearer query's own). `alt+enter` on a `*` of a select list expands it
(`Action::ExpandStar`, the intention actions; in the mode bar there only):
qualified by alias when there is more than one table or the table has an alias.
It refuses, with a notice saying why, when a source is not a table the client can
list (a function, `values`, `lateral`, a parenthesised join), when a subquery or
CTE has a column it cannot name, or when the block joins with `using` or
`natural`.

**Templates, colours and finishing** (`app/template.rs`, `marks.rs`, `finish.rs`, slice 12). Live templates `sel`
`selw` `selc` `lim` `ins` `upd` `del` `tab` `view` are offered only at a true statement start (nothing but
whitespace or comments since the previous `;` or the start of the buffer: `drop table ins` is not rewritten). When
the word is exactly a template's abbreviation, the template is the first item, highlighted, with the kind
`template` and a preview of its expansion on the right, and both `enter` and `tab` expand it (what is highlighted is
what you get; `Action::ExpandTemplate`, shown in the mode bar from the live bindings while one is highlighted);
one more character and the list is keywords again, with the templates last. Template mode (`TEMPLATE` in the mode
bar; `tab`, `shift+tab`, `esc`, from the live bindings) follows the text: typing replaces a variable, a variable
named twice is typed in both places, a table accepted in a table variable fills `$alias$` and, for `ins`,
`$columns$` (a late reply is written only while its table is still in the variable; otherwise the table there is
asked for); moving the caret out, an undo, a redo or a paste leaves it. Postfix `.from` `.cfrom` `.join` (first
foreign key of the table) `.cast` expand with the expansion key. Semantic colours (`Role::SynColumn`,
`Role::SynUnresolved`) come only from a statement dbx parsed, Postgres only, and claim only what the client knows:
a table is unresolved only when written with a schema whose whole relation list (the search's, which has
partitions) is loaded and lacks it, never when unqualified; a column only when the statement has no source the
client cannot list (function, `values`, `lateral`, parenthesised join, a subquery or CTE with an unnamed item),
the sources' columns are loaded, and the name is not a system column, a whole-row reference or a select-list name;
a statement with a set operation is left unmarked. dbx's analysis is asked about a statement only once the text has
been still for 200 ms (a deadline, not a tick; completion reads the same analyses and never asks), and what is
worked out for a statement is kept by its text and the loaded schema, so working it out again is a lookup. `f2` /
`shift+f2` go to the next / previous unresolved name (in the mode bar only when there is one). `cmd+shift+enter`,
`ctrl+shift+enter` and prefix `;` close open brackets and add the semicolon (one undo step), reading the statement
with the highlighter's tokens; a statement with a string, comment or dollar-quoted body that is not closed is left
alone, with a notice.

**Order** (`app/complete_match.rs`): an exact match, then a prefix, then the
initials of the words of the name (split at `_`, `-`, spaces and camel-case
humps; `fa` is `film_actor`, `fiac` too), then anywhere in the name; shorter
names first within a tier. The first letter's case must match, the rest need
not (keywords ignore case). Nothing typed: the sources in order, a table's
columns in its own order. The matched characters are bold.

**Inserting** (`app/complete_insert.rs`): names exactly as stored, quoted when
the dialect needs it (anything but `[a-z_][a-z0-9_$]*`, and Postgres's
reserved words: `"Order Items"`, `"order"`; backticks for MySQL; a quote
inside is doubled). A keyword takes the case being typed (`SEL` gives
`SELECT`). dbx's own quoting functions are private, so the rule is ours.

**Where the names come from, in order.** The sidebar's tree when that node is
loaded (schemas of the database, tables of the schema, columns of the table);
else what the app asked the Core for and kept (`CompletionCache`, per
connection, database and schema); else a Core call, made now and filled in
when it answers. Typing never waits: the popup shows what it has, and a reply
refreshes it only while the caret is on the word it was asked about (otherwise
it lands in the cache and changes nothing on screen). The Core calls:

| `Core` method | dbx behind it | Asked for |
|---|---|---|
| `complete(CompletionRequest)` | `schema::completion_assistant_search_core` (prefix search, `ILIKE`, `incomplete` when it hit its limit) | a schema's tables and views (the whole schema once, 1,000 at most; by prefix only when that was cut), the database's schemas, functions by prefix (300) |
| `current_schema(at)` | `query::execute_sql_statement_with_options_typed` with `select current_schema()` | once per database, for unqualified names; a failure means `public` |
| `columns(table)` (existing) | `schema::get_columns_core` | a table's columns, through `CoreCall::TableColumns` and not the tree's `List`, so the sidebar is not touched |

Every one runs through `DbxCore::metadata`, raced against a cancel token like
the other metadata calls, and the request table keys each by what it is for
(`Target::Complete(Need)`), so a longer word that needs a new prefix search
makes the older reply stale. A refresh of a tree node (`r`), closing it on an
error, or removing its connection drops what was kept for that part of the
tree (and abandons what was in flight for it). A failed call is not asked
again until then, and the popup keeps what it has (keywords at least). Only
Postgres connections get names; other kinds get keywords.

Themes: the popup's fill is the role `popup_bg` (IntelliJ Dark `#2B2D30`), the
selected row is drawn in the selection style, kinds and types in `overlay0` /
`overlay1`.

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

Mouse: a click puts the cursor on a cell, a double-click opens it in the viewer, the wheel scrolls rows (three per
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
on screen are wrapped. `G` on a value over 64 KiB wraps only its tail, from
about a page of bytes before the end (from the start of the line there when one
begins within another page of bytes, else from a grapheme boundary, so the
wrapping of a single huge line may differ from the full wrap's); the line
numbers before that point are not known, so the position reads `end` (or
`end -N`). Scrolling up past the top of the wrapped tail wraps twice as much,
and `g` goes back to the top with its numbers. The scroll hints along the
bottom are clickable, the wheel scrolls three lines a notch, and a key that
moves nothing repaints nothing. `y` copies the
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

## Mouse and resizing

Mouse capture is on (the shell asks for SGR reports; hold shift to pass a drag
to the terminal for its own selection, which is the terminal's doing). What is
under the pointer is decided by the hit map `view` fills on every frame
(`app/layout.rs`); a double-click is two presses of the same thing within
350 ms of the `now` given to `update` (`app/pointer.rs`, no timers). An open
overlay takes every mouse event: nothing behind it is reached, and its buttons
(and the viewer's scroll hints) do what their keys do. No context menus.

| Where | Mouse |
|---|---|
| any pane | a press focuses it |
| sidebar | click selects a row, a click on the arrow or a double-click toggles it, the wheel scrolls, a press in the scrollbar column jumps there and dragging the thumb scrolls |
| editor | click places the caret (mapped through the widget's own scroll offset and `screen_to_data`, so wide characters, tabs and scrolling count), drag selects (past the edge the view follows), double-click selects the word, the wheel scrolls (shift or a horizontal wheel: sideways) |
| results | click moves the cell cursor, double-click opens the cell viewer, the wheel scrolls rows, shift+wheel or a horizontal wheel columns, the scrollbar like the sidebar's |
| mode bar | a click on a hint (each key, and the words for the first key) |
| dividers | drag the `│` (the sidebar) or the results' title row (the editor/results split); a double-click resets it |

The wheel scrolls what is under the pointer, whichever pane has focus.

**Resizing.** The sidebar (with its separator) keeps at least 14 columns and
leaves the editor and results at least 24 (on a narrow terminal, up to half the
width); the editor keeps at least 3 rows and the results 4. Without a saved
size the sidebar is about 30% of the width (26 to 34) and the editor three
fifths of the height. The keyboard does the same: prefix then `R` enters the
resize mode (`RESIZE` in the mode bar, with `h/l sidebar`, `j/k split` and
`esc done` as clickable hints); `h`/`l` or left/right move the sidebar's
divider a column, `j`/`k` or down/up the split a row; `esc` or `enter` leaves.
A drag is clamped to the limits wherever the pointer goes.

**Preferences.** What the user set is kept as thousandths of the width and
height (`sidebar_permille`, `editor_permille`) in `dbx-tui-prefs.json` in the
directory the store lives in (`DbxCore::store_dir`: `DBX_DATA_DIR`, or the
desktop app's directory). The shell reads it at the start and writes it
(`Command::SavePrefs`, atomically) when a drag ends, a double-click resets, the
resize mode is left, and on quit; `app` only holds the values
(`Ui.layout`). A missing, unreadable or corrupt file, or a value that is not
a usable proportion, means the default for that value, silently. A write that
fails is ignored. The collapsed state of the sidebar is not saved: there is no
collapse yet (zoom is not kept).

## Tests

Run every command from the dbx workspace root.

| What | Command | Needs |
|---|---|---|
| Fast: app, view, keys, FakeCore (layer 2) | `cargo test -p dbx-tui --lib --test no_colour_literals` | nothing |
| Store: `DbxCore` on scratch stores (layer 1) | `cargo test -p dbx-tui --test store -- --test-threads=1` | nothing external; temp dirs, key files, a local TCP port |
| Live: `DbxCore` against the fixture Postgres (layer 1) | `crates/dbx-tui/tests/fixtures/up.sh && cargo test -p dbx-tui --test live -- --test-threads=1` | Docker (the fixtures below) |
| Ground truth: every cell of the `edge` tables, `DbxCore` against `psql`, and through the drawn grid (layer 1; slice 08) | `crates/dbx-tui/tests/fixtures/up.sh && cargo test -p dbx-tui --test ground_truth -- --test-threads=1` | Docker (it runs `psql` inside the fixture container), about 5 s |
| Fixtures: the test databases | `crates/dbx-tui/tests/fixtures/up.sh && crates/dbx-tui/tests/fixtures/check.sh` | Docker |
| Journeys: the built binary in tmux (layer 5; `colours`, `add-connection`, `browse-pagila`, `browse-table`, `cancel`, `complete`, `first-query`, `mouse`, `quit-mid-run`, `tabs` and `wide-result` need the fixtures) | `for j in crates/dbx-tui/tests/journeys/*.sh; do [ "$(basename "$j")" = lib.sh ] \|\| bash "$j"; done` | tmux, openssl, python3 |
| Lint | `cargo clippy -p dbx-tui --all-targets` | |

`cargo test -p dbx-tui` runs the fast, store and live tests together (the live
ones fail, saying so, when the fixtures are not up). Test helpers on the
production adapter (`DbxCore::seed_config`, `runtime_config`,
`runtime_config_ids`, `probe_select_one`, `with_managed_keys`,
`check_seed_dir`) sit behind the `test-support` feature (as does `app::probe::GridProbe`, which
lets `ground_truth` read a result back from the drawn grid), which tests and
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
printed at the end. `browse-pagila` also starts a throwaway local socket that accepts and never answers (a listing that hangs, collapsed, and then in flight while the client quits). `mouse` writes SGR mouse reports to the pane with `tmux send-keys -l` (which reaches the client as the terminal's own reports would). `cancel` runs a small local proxy to the fixture that swallows Postgres cancel requests, for a cancel that never lands (the live tests have the same proxy in Rust). `examples/seed_store` saves a connection with a query timeout when `DBX_TUI_SEED_QUERY_TIMEOUT` is set. `examples/seed_store` takes the password from `DBX_TUI_SEED_PASSWORD` when set.

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

- `gt_*` tables (`edge/05-ground-truth.sql`): one per type family, each row
  labelled with what it probes: integers to 2^63, `numeric`, floats with NaN and
  the infinities, text (empty, `NULL`, emoji, RTL, ZWJ, 100 kB), date and time
  types, bool/uuid/bytea, json, arrays, enum/domain/range/composite, network
  types, money and bit strings, xml and tsvector. `ground_truth` compares them
  with `psql`; `tests/fixtures/KNOWN_DIFFERENCES.md` lists every cell where the
  client differs (and `GT_WRITE=1` regenerates it, marks new rows `UNREVIEWED` and fails that run; it is refused under `CI`), and
  `tests/fixtures/UPSTREAM-PROPOSAL.md` is the unsent write-up of dbx's losses.
  Apply a changed file to the running database with
  `docker exec -i dbxtui-fixtures-postgres-1 psql -U postgres -d edge -f - < tests/fixtures/edge/05-ground-truth.sql`
  (after dropping the tables it creates).

`up.sh` starts it and waits until it answers queries; `down.sh` removes it.
