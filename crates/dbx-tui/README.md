# dbpane

dbpane is a terminal database client, built as another head on dbx's core. The
cargo package is `dbx-tui` (`crates/dbx-tui`); the binary is `dbpane`. Plans and
decisions live in the dbx-tui docs repo; this crate is the code.

Run it against a scratch store (never your real one while developing), from
the dbx workspace root:

```sh
store="$(mktemp -d)"                                   # DBX_DATA_DIR must exist and be absolute
openssl rand -hex 32 >"$store.key" && chmod 600 "$store.key"
cargo build -p dbx-tui                                 # not under the scratch HOME: cargo needs yours
HOME="$store" DBX_DATA_DIR="$store" DBX_SECRET_KEY_FILE="$store.key" target/debug/dbpane
```

`DBX_SECRET_KEY_FILE` (or `DBX_SECRET_KEY`, 64 hex characters) wins over every
key policy, so the keychain is not touched; pointing `HOME` at the scratch
directory keeps dbx from even looking for its per-user key file.

## The command line, the name and the store (slice 24a)

`dbpane --help` lists the flags and environment variables and ends with the
store in force; `dbpane --version` prints the version and the dbx commit it was
built from (`build.rs`; `DBPANE_DBX_COMMIT` overrides it); `dbpane --licenses`
prints `legal/NOTICE` (dbpane is a modified work of dbx, the fork's URL, herdr's
theme attribution, dbx's own NOTICE), `legal/THIRD-PARTY.txt` (the direct
dependencies and their licences; a test fails when one is missing) and
`legal/LICENSE` (dbx's Apache-2.0). These three files go in the release archive.
(The directory is `legal`, not `dist`: the repository's `.gitignore` ignores `dist/`.)

**Which store.** In order (`StoreOptions::resolve`, `src/core/store.rs`):

1. `--store dbx|own|<dir>`: the dbx desktop app's store, dbpane's own, or an
   absolute directory that already exists.
2. `DBX_DATA_DIR`, an absolute directory that already exists.
3. dbpane's own store if it already holds a `dbx.db`
   (`~/Library/Application Support/dbpane` on macOS, `$XDG_DATA_HOME/dbpane`
   or `~/.local/share/dbpane` on Linux). Once made it stays the one in use,
   even if the dbx desktop app is installed later.
4. Else the dbx desktop app's store if its data directory holds a `dbx.db`
   (`~/Library/Application Support/com.dbx.app` on macOS,
   `$XDG_DATA_HOME/com.dbx.app` or `~/.local/share/com.dbx.app` on Linux): the
   two share it, with the key dbx keeps in the keychain or Secret Service.
5. Otherwise a new dbpane store in the directory of step 3.

A relative `HOME` or `XDG_DATA_HOME` counts as unset. A named store
(`--store <dir>`, `DBX_DATA_DIR`) that already has a `.dbx/secret.key` uses it
(dbx's managed key policy) instead of the keychain.

dbx fixes the keychain entry's service name (`com.dbx.app.secret-store.v1`) in
a private constant, so an entry named `dbpane` needs a change in dbx. Until
that is decided, dbpane's own store keeps its key in a file inside it,
`.dbx/secret.key` (mode 0600, dbx's `ManagedDataDir` policy, the one dbx-web
uses), and never touches the keychain; `DBX_SECRET_KEY_FILE` still wins. On
Linux, dbx's platform policy with no Secret Service running (a headless box)
falls back to a per-user key file instead of failing, and a Secret Service that
exists but cannot be read stops with a message naming `DBX_SECRET_KEY_FILE`.

With no connection saved, the results pane shows a welcome: what dbpane is, the
store in use and its path, and the keys for adding a connection, help and quit
(read from the live bindings; the mode bar offers the same as clickable hints).
It is not drawn while the store is still answering (the first frame comes
first) or when the store failed, and it goes once a connection exists.

The release binary (`cargo build -p dbx-tui --release`, `target/release/dbpane`)
runs every journey: `PROFILE=release bash crates/dbx-tui/tests/journeys/<name>.sh`.
Under the release profile's `panic = "abort"` a panic still runs our hook, which
restores the terminal and exits 101 before any abort.

## Config (slice 22)

One TOML file, read once at startup: `$DBPANE_CONFIG` (`$DBX_TUI_CONFIG` too),
else `$XDG_CONFIG_HOME/dbpane/config.toml`, else `~/.config/dbpane/config.toml`
on macOS and Linux alike (`dbpane config --path` prints it; `dbpane config`
prints every default as a file that reads back as the defaults). The user docs
(`docs/README.md`, "Config", and `docs/getting-started.md`, section 11) have the
sections and an example; `tests/docs_keys.rs` parses every ```toml block in them
and checks the problems they quote.

- `src/app/config.rs` parses the text (`toml_edit`, parser only: it is already in
  the lock) into a `Config` and a list of `Problem`s with line and column; an
  entry with a problem keeps its default, invalid TOML means all defaults.
  `App::apply_config` puts it to use, shows one notice and keeps the problems
  for the help overlay. `src/shell/config.rs` finds and reads the file.
- Keys: `Keybinds::set_keys` / `set_prefix` / `conflicts` (`src/app/keys.rs`).
  A key given to an action is taken from any other action in a scope read at the
  same time (`READ_TOGETHER`); the prefix key, a plain character where you type,
  and a key a config entry already gave away are refused. The config group for
  the keys after the prefix is `after_prefix` (a TOML key cannot be both the
  string `[keys] prefix` and a table `[keys.prefix]`).
- Tests are layer 2 (`src/app/config_tests.rs`, config as text), the shell's path
  and file tests use temp directories only, and the journey `config` runs the
  binary with `DBPANE_CONFIG` under its scratch directory.

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
is built and tested for (PostgreSQL, MySQL and SQLite so far): adding a database is one
line there plus its tests, not edits across `app`. A type whose
`queryExecution` is false is listed, and running in it says it cannot be
queried. `Syntax::capabilities(kind)` answers the same for a kind before any
connection exists (the new-connection form's port).

## SQLite (slice 23a)

A SQLite file is a connection of type SQLite (`BROWSED_HERE` and `COMPLETED_HERE`
in `src/core/dbx.rs`; `Capabilities { local_file, completes, fixed_schema, .. }`).
The form's first row is the type (`space`, the arrows or a click on the choices
switch it; what was typed for one type is kept, and back when it is chosen
again); a file connection has a name, the **file** (saved as dbx's `host`, `~/`
expanded as dbx does) and the production flag. `DbxCore` refuses a path that
is not an existing regular file starting with SQLite's 16-byte header (an empty
file is an empty database and is accepted), and `~user/` (dbx would look the
user up), on a test and on a save of a new path, and never creates or writes
one (`check_local_file`). A relative path is saved joined to the working
directory (`absolute_file_path`: `.` dropped, `..` and symbolic links kept as
given, not resolved), so the store names the same file from anywhere.

- **Tree:** no database and no schema level; `u` on the connection row targets
  it (database name empty); `D` on it dumps the file (titled by the connection).
- **Names:** unqualified names resolve in `main` (`FIXED_SCHEMA`), so
  `current_schema` is never asked of the Core (`DbxCore::current_schema` answers
  `main` too). dbx's search treats `""` and `main` alike; the app uses `main`.
- **Tabs do not queue.** `single_connection_pool` is about the connection, not
  the sessions: dbx opens one SQLite handle per tab session for a file (only
  `:memory:` shares one), so two tabs read side by side, and closing a tab's
  session closes only its own handle (`tests/sqlite.rs`). A write holds the
  file's write lock; another tab's write waits on SQLite's busy handler (about
  ten seconds) and the interrupt does not end the wait (dbx notes the same):
  the run returns `Cancelled` only after the statement has ended, so the app
  shows "has not stopped yet" until then and never claims a stop early. The
  interrupt is remembered, though: when the lock comes free first (the other
  tab commits), the waiting write stops there and writes nothing, so
  `Cancelled` is the truth in both cases (pinned both ways). Not covered: a
  cancel landing in the instant after a statement finished but before dbx
  hands its result back is reported by dbx as cancelled (its cancel wins that
  race), and the client cannot tell. A recursive CTE counting to 10^9 is
  interrupted at once and the next statement runs.
- **Confirmations:** dbx's `prove_read_only_for_database` proves nothing on
  SQLite (`Unproven` for every statement), which would ask about every read
  and browse page on a production file and make load more always ask. So
  SQLite itself is asked (`src/core/sqlite_proof.rs`, used by the gate, the
  fresh-flags check and `Rerun`): the file is opened read-only and immutable
  (`file:…?mode=ro&immutable=1`: no lock, no journal, no `-wal` or `-shm`
  opened or made, nothing created; unit tests compare the directory before
  and after, with a WAL writer open too), each statement is prepared and
  never stepped, and it is proven only if every statement starts with
  `SELECT`, `WITH` or `VALUES`, prepares there, is `sqlite3_stmt_readonly`,
  is not `EXPLAIN`, and names neither `load_extension` nor `fts3_tokenizer`.
  A statement that does not prepare (a missing table, dbx's own `unistr` and
  `if`, an extension's function, a table newer than the WAL checkpoint) stays
  unproven and asks. Only for a local file with no URL parameters (dbx's
  extensions) and no SSH layer. `rusqlite` is a direct dependency at dbx's
  locked 0.32.1 with no features of ours: the same crate and bundled SQLite,
  nothing new in the lock. `ATTACH`, `VACUUM` and `PRAGMA writable_schema`
  are dangerous whatever dbx or SQLite says (`sqlite_dangerous`), and ask
  on production even with the danger setting off.
- **Values:** `tests/ground_truth_sqlite.rs` compares about 358,000 cells of
  `edge.db` and Sakila with `sqlite3 -quote`; every difference is in
  `tests/fixtures/KNOWN_DIFFERENCES-sqlite.md` (infinity shows as NULL, a blob as
  `0x..`, reals in their shortest spelling; reals are compared bit for bit
  through `hex(ieee754_to_blob(x))`).
- **Export and dump:** dbx supports both. For a column with no type name (an
  expression, or one declared without a type) `Column::numbers` keeps which
  cells dbx gave as JSON numbers, and the export writes those as numbers; an
  integer past 2^53 there comes from dbx as text and stays text. A dump loads
  into a new file with the same tables, views, indexes and rows, losing the
  things in `KNOWN_LOSSES_SQLITE` (`src/app/dump.rs`; pinned in
  `tests/sqlite.rs`). A dump whose file is the connection's own file or its
  `-wal`, `-journal` or `-shm` is refused (`sqlite_dump_target_refused`: by
  device and inode, or by directory and name for one not there yet): dbx
  would write it over the file it reads.
- **Fixtures, no Docker:** `tests/fixtures/fetch-sakila.sh` (jOOQ/sakila at a
  pinned commit, sha256-checked), `tests/fixtures/sqlite/build.sh` (`sakila.db`
  and `edge.db` from `edge.sql` into `target/sqlite-fixtures`, with the sqlite3
  version), `tests/fixtures/sqlite/check.sh` (also run by `check.sh`). Tests copy
  a file to a temporary directory before opening it. Run
  `cargo test -p dbx-tui --test sqlite --test ground_truth_sqlite -- --test-threads=1`
  and `tests/journeys/sqlite.sh`.
- **Not supported:** encrypted files, attached databases, creating a file,
  SQLite over SSH.

## MySQL (slice 23b)

MySQL is on in `BROWSED_HERE` and `COMPLETED_HERE` (`src/core/dbx.rs`). The form
is the Postgres form with the type row at MySQL and port 3306 from
`Capabilities`; TLS is dbx's `require_ssl` with no certificate check (a live test
reads `Ssl_cipher`). MariaDB, OceanBase and the other MySQL-family types stay
unswitched (their `DbKind` is separate, design section 28).

- **What dbx's MySQL calls take.** Completion's `database` names the pool and
  `schema`, when it is not empty, names the database searched
  (`completion_assistant_search` in `dbx-driver-mysql`): either field works for
  the current database, and a search of another database sets `schema`
  (`tests/mysql.rs`, `a_search_for_fil_in_sakila...`). `get_columns_core` and
  `list_foreign_keys_core` do the same (`mysql_table_metadata_catalog`), so the
  tree's empty schema name works. A `candidate.schema` is the database.
- **Current database.** `Core::current_schema` runs `select database()` on the
  database's own pool (`current_schema_sql`; `current_schema()` is an error on
  MySQL). It does not see a `USE` typed in a tab, whose session is not that
  pool: completion stays on the tab's database. The tree's `release` closes a
  database's pool for MySQL too (`<connection id>:<database>`).
- **Cancel is `KILL QUERY`** from a dedicated connection (dbx's
  `register_interrupt` in `dbx-core/src/query/mod.rs`), sent only by a cancel
  through the running-query list. Left alone, dbx stops waiting at once and
  discards the session's connection (a later run is a new connection: the
  variable, temporary table and transaction are gone). So a MySQL run asks for
  `await_cancel_completion`: it waits for the server's answer to the kill, which
  is error 1317 for almost every statement (mapped to `Cancelled`) and the
  connection stays. `SLEEP()` is the exception: interrupted, it returns 1 and the
  statement ends normally. After a cancel or timeout, a statement dbx proves
  read-only (or would, but for its `SLEEP()` calls: dbx's proof does not list
  SLEEP, which only waits; `proven_but_for_sleep`) that ended with a result is `Cancelled` (or the timeout, worded "the
  server then answered it with a result", never "stopped"), and the result is
  not shown; anything else that ended with a result (a write, a read calling a
  function dbx cannot vouch for) is shown with a note (`tests/mysql.rs`,
  `a_write_the_server_answers_after_a_cancel...`: an INSERT with `sleep(30)` in
  its values writes the row; `a_read_dbx_cannot_prove...`), and a run of more
  than one statement adds that the result is the first one's and the later
  statements may have run. The connection's query timeout sends the same kill
  (`running_queries.cancel`; `token.cancel()` alone ends nothing on the server)
  and waits for the server's answer at most `KILL_GRACE` (10 s), then lets the
  run go and says whether the server stopped it is not known; the `select 1`
  probe after it is bounded the same way. The `select 1` probe after a cancel
  runs on the session that survived; the tests check
  `information_schema.processlist` after each cancel, and that the connection
  id, a `SET @x`, and a temporary table are the same.
- **A cancel that leaves the outcome unknown.** Each statement of a MySQL
  multi-statement text commits on its own, and a table that is not
  transactional keeps what a killed statement wrote. So a cancelled (or timed
  out) run of more than one statement, or of one not proven read-only, is
  `CoreError::outcome_unknown`: the tab says "cancelled after …; earlier
  statements may have run, or part of this one: what it changed is not known",
  and the history keeps it as a failed run whose error starts "The outcome is
  unknown" (a plain cancel saves nothing). Postgres runs a multi-statement text
  in one implicit transaction, so this is MySQL only (`tests/mysql.rs`,
  `a_cancelled_run_of_more_than_one_statement...`: `update; <long select>`
  cancelled in the select; `history_tests.rs` for the app).
- **Transactions.** dbx's default for an auto-commit tab is to `ROLLBACK` an
  explicit `BEGIN` when the run that opened it ends, which turns `begin;
  update; rollback;` (three runs) into a committed update. Runs ask for
  `preserve_explicit_transaction` on MySQL, so the transaction stays open
  between runs, as on Postgres, and closing the tab's session rolls it back
  (live test). dbx tracks a client session per tab (`mysql_client_sessions`): a
  lost connection is `MYSQL_CLIENT_SESSION_LOST`, shown once with "This statement
  was not executed.", and the next run starts a new session. The same code after
  the statement was sent (dbx could not settle its transaction:
  "The transaction outcome may be unknown") is not a failure to connect: it is
  `outcome_unknown`, says "The session was lost after the statement was sent, so
  what it did is not known", and is saved to the history (`run_error`, unit
  test; no live repro: it needs the server to drop the connection between the
  statement and dbx's `ROLLBACK`).
- **Known gap: an open transaction is not shown.** dbx reports
  `MysqlAutoCommitTransaction::Preserved` only through
  `execute_sql_statement_with_options_typed_with_outcome`, which is
  `pub(crate)` in dbx-core; the public call this client uses drops it. So a tab
  with a `BEGIN` still open has no marker, and closing the tab, retargeting it,
  forgetting the connection or quitting rolls the transaction back without
  asking (as on Postgres, which has no marker either). Showing one needs a dbx
  change (the outcome made public) or a second query per run
  (`information_schema.innodb_trx` for the session's connection id).
- **`LOAD DATA LOCAL INFILE`:** dbx sets no handler, so a file of the client's is
  never sent. With the server's `local_infile` off the server refuses; with it
  on, the connection is closed (an error, the session lost, no row from the
  file). Tested both ways.
- **Confirmations.** `assess` (`src/core/dbx.rs`) judges MySQL text in every
  reading the server may give it (`src/core/dbx/mysql_read.rs`, a model of
  MySQL 8.4's lexer): `#`, and `--` followed by ASCII whitespace or an ASCII
  control character, run to the end of the line (a `--` followed by anything
  else, a non-breaking space or U+3000 included, is two minus signs, written
  `- -`); `/* */` ends at the first `*/`; `/*!` is code, and `/*!NNNNN` (five
  digits, or six when a sixth follows) is code on a server of that version or
  later and a comment, with one nested `/* */`, on an older one; `/*M!` is a
  comment to MySQL and code to MariaDB. Each of `NO_BACKSLASH_ESCAPES` and
  `ANSI_QUOTES` on and off, each server version the text's version comments
  name (and one older than all), MySQL and MariaDB: each reading is judged
  twice by dbx, as written with its comments gone and rewritten so that any
  lexer reads it alike (strings `'...'`, names `` `...` ``, no backslash, quote,
  `#` or comment mark inside), and the text is judged as written too. The
  verdicts are merged (production if any is, every reason and named database).
  A text is `Reason::Ambiguous` ("the server may read its quotes or comments
  differently…") when its readings differ in what lies outside quotes and
  comments, or when dbx's own scanner (`strip_sql_comments_and_literals`) reads
  the text, or a rewritten reading, otherwise than the reading does (dbx takes
  any `--` for a comment, closes `/*! */`, knows no `ANSI_QUOTES`, and quotes
  `$$`): then it asks as for production on any connection that lists one, is
  never proven read-only, and is never run again by "load more". Unless every
  reading is one statement (no `;` outside quotes and comments in any, dbx's
  scanner agreeing with each): then a reading whose quote never closes is a
  syntax error to the server and is set aside, and if every other reading's
  rewritten text gets the same verdict, the text is not ambiguous and that
  verdict stands (`MysqlReadings::single`). So `select * from users where name
  = 'O\'Brien'` and `"a\"b"` run without asking; a version comment, `--x`, or
  a backslash before a quote with a `;` after it still ask on such a
  connection.
- **Client charsets that swallow a backslash.** In big5, cp932, gb18030, gbk
  and sjis a two-byte character may end in `\` (0x5C) or `` ` `` (0x60), none
  in `'` or `"` (found on the fixture with `char_length(convert(unhex(...)
  using cs))` over every lead byte; eucjpms, ujis, euckr and gb2312 have none,
  ucs2/utf16/utf32 cannot be a client charset). The bytes of a UTF-8
  character there can join the next byte: `select '丁\\', '; delete …; -- '`
  is one SELECT to a UTF-8 reading and a SELECT and a DELETE to gbk. The
  gate does not model those lexers: MySQL text that is not all ASCII is
  `Reason::Ambiguous` (`mysql_unread`) when the connection's URL parameters
  name such a charset or `character_set` (dbx sends `SET NAMES` with
  `charset=` at connect; `dataCharset=` aside, which needs utf8mb4), when the
  session has run anything that may change the client charset
  (`CharsetSessions`: `SET NAMES`, `SET CHARACTER SET`, `SET CHARSET`,
  anything with `character`, `PREPARE`, or a `SET` of a name that is not
  ASCII, since the server compares variable names as utf8mb3_general_ci and
  `@@chàracter_ſet_client` sets it; stored routines put it back), until the
  session closes, or when the text itself may change it and goes on to
  another statement. ASCII text reads alike in every client charset. A `USE`
  in such text marks the session as switched. Asking `@@character_set_client`
  before each run was not chosen: it costs a round trip on the tab's session
  per run, and a text can change the charset partway through itself.
- **The differential.** `tests/mysql.rs`, `no_text_of_the_corpus_runs...`,
  runs the 61 texts of `tests/fixtures/mysql/gate_corpus.rs` (`GATE_CORPUS`
  and `CHARSET_CORPUS`, each followed by `select 'MARK'`, then by a write to a
  scratch table) under the four combinations of the two modes and six client
  charsets (utf8mb4, gbk, sjis, cp932, gb18030, big5, set by a confirmed `SET
  NAMES`), reads what the server ran from
  `performance_schema.events_statements_history`, and checks that the gate
  never let through a statement it did not read, never let a write reach the
  table through a production connection, and that whatever that connection
  let through unasked ran as one SELECT or a syntax error.
  `a_connection_whose_charset_parameter...` does the same for `charset=` in
  the URL parameters. Found in the slice 23b review, round 3: under gbk,
  `` select 1 as `丁`, '`; delete …; -- ' `` and `select '丁\\', '; delete …; -- '`
  ran the delete unasked on a production connection. Found in the slice 23b review:
  `select 1 --<U+00A0> …; delete …` and `select '\'' as "\"; delete … #"` (under
  `ANSI_QUOTES`) ran the delete without asking, and `/*!99999 -- */ --1;
  select …` was read only with its comment opened, which hid the second
  statement. Found in slice 23b, against dbx's scanners: `select 5
  --1; delete from prod.t` (a comment to dbx, two statements to MySQL) ran
  without asking through a selection; `delete b from a, prod.b`, `load data ...
  into table prod.t`, `select prod.f()` and `... lock in share mode` were not
  seen as aimed at `prod`, since dbx finds one target after `FROM`, `JOIN`,
  `UPDATE` or `INTO`. Whatever is not proven read-only and names a database of
  `production_databases` anywhere in its text (backticks and case aside) is
  now aimed at it, and `INTO OUTFILE`/`DUMPFILE` asks wherever the connection
  lists any production database (`Reason::WritesFile`). `USE` taints the
  session through the same views (so a `USE` behind `--1` counts). Unit tests
  are `src/core/dbx/mysql_gate_tests.rs`; live ones in `tests/mysql.rs`. `PREPARE`
  cannot hide a `USE` (MySQL answers 1295). Not covered: a function in the
  tab's own database that writes elsewhere, a trigger, or an event; dbx cannot
  know either, on Postgres as here.
- **Dump and export.** dbx's dump checks `mysql_data_charset_policy` first: it
  refuses only a connection whose URL parameters set `dataCharset` (GBK, GB18030
  or BIG5, set in the desktop app), with `MYSQL_DATA_CHARSET_EXPORT_UNSUPPORTED`,
  shown as dbx says it. A dump of Sakila (3.5 s) is tables, then views, then
  procedures, functions and triggers; it names no database, and loads into a
  scratch database with every table's checksum, row count and `SHOW CREATE
  TABLE` the same. What it loses: FLOAT cells are the six digits the server
  prints (`KNOWN_LOSSES_MYSQL`), in the dump and in a SQL INSERT export alike.
- **Values:** `tests/ground_truth_mysql.rs` compares 316,008 cells of `edge` and
  Sakila with `mysql --xml` (bytes and BIT through `hex()`, a FLOAT's stored
  value through `cast(.. as double)`, a DOUBLE by its bits: MySQL prints the
  shortest spelling that reads back). Thirteen groups differ and are in
  `tests/fixtures/KNOWN_DIFFERENCES-mysql.md`: five FLOAT cells (LOSS: the
  server's six digits are not the stored number, and the `mysql` client shows
  them the same way), BIT as binary digits, bytes as `0x`+hex and zero fractions
  of seconds dropped (FORMAT).
- **Not looked at:** MariaDB, OceanBase, SSH tunnels, client certificates.
- **Fixtures:** `tests/fixtures/compose-mysql.yaml` (project
  `dbxtui-fixtures-mysql`, container `dbxtui-fixtures-mysql-mysql-1`, MySQL 8.4.13
  pinned by digest, `127.0.0.1:53307`, root / `dbxtui`, data on tmpfs),
  `up-mysql.sh` (also fetches Sakila, sha256-checked), `down-mysql.sh`,
  `init-mysql/10-load.sh`, `mysql/edge/01-edge.sql` (databases `edge` and
  `edge_other`), `check-mysql.sh` (run by `check.sh` when the container is up).
  Run `cargo test -p dbx-tui --test mysql --test ground_truth_mysql --
  --test-threads=1` and `tests/journeys/mysql.sh`.

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
(`Capabilities::browse`, see above): any other (MariaDB, say) shows "Browsing
MariaDB is not supported yet." under it and asks dbx nothing. The sidebar is about 30% of the
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
`dbpane: A statement may still be running on <connection / database>.`

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

## Confirmations (slice 17)

The gate is in the adapter, on the run path: `DbxCore::run` assesses the exact
text it is about to send, with the connection's config as dbx holds it at that
moment, and executes nothing without a matching yes. Every path that runs a
statement goes through it: the run keys, the statement chooser, load more,
browse pages and counts (and history re-runs, later). A statement waits for a
yes when:

1. dbx's `classify_sql_risk_for_database` does not say read-only (a parse
   failure counts as a write) and `targets_production_database` holds: the
   connection is marked production, the database is in its
   `production_databases`, or the SQL names a marked database (MySQL-style
   `db.table`; on Postgres `x.t` is a schema) or dbx cannot tell. No setting
   turns this off.
2. It is read-only but `prove_read_only_for_database` cannot prove it (it
   calls functions dbx cannot vouch for), on production.
3. `is_dangerous_sql_for_database` holds and the setting `confirm_dangerous`
   (on by default; the config file comes in slice 22) is on.

A write on a `read_only` connection is not asked about: dbx refuses it with its
own error. The answer is `CoreErrorKind::NeedsConfirmation` carrying an
`Assessment` (why, the connection's name, the marked databases the SQL names,
and the `Confirmation` a yes sends back). A yes is bound to the exact text, the
connection, the database and the tab's session; a yes for anything else (the
text, or the tab's session, changed) is asked again, even for a statement that
would need none.

The app shows a red box: `Run on production?` or `Run a dangerous statement?`,
`On <connection> / <database>.`, why in words, and the exact statement,
scrollable (`j`/`k`, pages, `g`/`G`, the wheel), drawn through the painter.
`y run` sends the same run again with the yes (the confirmed text, whatever is
under the caret by then; load more keeps its cap, a page stays a page); `esc
cancel` runs nothing and says `Not run.`. `enter` does nothing, so `f5` then
`enter` cannot confirm. The tab stays idle and its result as it was while it
asks. A tab not on screen is told when it is shown again. A red `PROD` follows
the tab's label in the tab bar, and the mode bar's pane label, whenever the
tab's connection or its database is marked production, before anything runs.

**How this differs from the desktop app** (`useSqlExecution.ts:422-452`,
`productionSafety.ts:128`):

- The desktop's danger check is its own regex: a statement starting with
  `DROP`, `DELETE`, `TRUNCATE`, `ALTER`, `UPDATE`, `MERGE` or `REPLACE`. This
  client uses dbx's Rust `is_dangerous_sql_for_database`. Outside production a
  scoped `update t set a = 1 where id = 1` is confirmed by the desktop and not
  here; `create table` is confirmed here and not by the desktop.
- dbx's Rust check also flags transaction control (`commit`), `SET`
  (`set search_path to public`) and `select pg_terminate_backend(1)` (which
  its classifier calls a write), so with the setting on they are confirmed
  outside production too. The box says only "dbx flags it dangerous", since
  dbx gives no finer reason.
- A read dbx cannot prove read-only is confirmed on production here; the
  desktop lets every read through.
- dbx's classifier does not fail on text it cannot parse (`delet from t`): it
  falls back to keywords and calls it a write, so the reason shown is "writes";
  the danger check fails closed. The "cannot parse" reason is kept for when
  dbx does return an error.
- Flags set in the desktop while this client runs: the gate first judges with
  dbx's runtime copy. When that would let a statement through that dbx does
  not prove read-only, it reads the saved connections again
  (`Storage::load_connections`, which decrypts every saved secret; they stay
  inside the adapter and out of errors and logs) and judges again with the
  saved flags. Proven reads and statements carrying a matching yes skip this.
  A store that cannot be read stops the statement with the store's error.

**Sessions that switched database.** A `USE` lasts in the tab's session, but a
run is judged against the tab's database. So the adapter notes, per session,
that a statement beginning with `USE` was sent (in any statement of the text).
In that session, on a connection that is marked production or lists any
production database, every statement not proven read-only is treated as aimed
at production (reason: an earlier USE in this tab switched its database),
until the session closes. A `USE` is confirmed, when it names a production
database, with its own reason: it switches the session's database, and later
statements in this tab will be confirmed. This is the adapter's own check (a
first-word test), since dbx's `USE_RE` is private.

**`EXPLAIN ANALYZE`** runs the statement it explains. When the text is an
EXPLAIN with ANALYZE in any option form (`analyze`, `(analyze)`, `(analyze
true, verbose)`, `analyze verbose`, MySQL's `analyze format=tree`), the inner
statement is judged too: its risk and danger count, and the reason says
"EXPLAIN ANALYZE runs the statement it explains". dbx's own parser does not
take the `ANALYSE` spelling, so it also calls that form dangerous.

**A new box ignores `y` for 300 ms** (and the `y run` button), so typing that
was already under way when the reply arrived cannot confirm by accident. Nothing
is shown; `esc` and everything else work at once.

## History (slice 18)

**Shared with the desktop app.** The history is one table in dbx's store. A
statement this client runs is written with dbx's own `save_history_entry` (the
call the desktop uses), in the desktop's shape: a UTC time with milliseconds
(`2026-11-25T10:00:00.000Z`), source `sql`, and the activity kind its history
panel files it under (`query`, `data_change`, `schema_change`, from the same
rule as `historyActivityKind.ts`). So it shows in the desktop's history, and the
desktop's show here. Retention is the desktop's setting: the store trims on each
save, and this client has no limit of its own. Entries with source `mcp` (the MCP
server's) are never listed. Nothing is deleted from here.

**Which runs are saved.** A statement of the user's that reached the database and
has an outcome: it returned rows or changed some (rows affected are kept), failed
in the database (`success = false`, with the error text, cleaned) or timed out
(saved as a failure, naming the limit). The statement saved is `ResultSet.ran`
or the error's `statement`. The desktop records every finished execution, with
its error when it failed (`useSqlExecution.ts:645`); this client matches that
except where it cannot claim the statement ran:

- **Not saved:** a run that stops at a confirmation (nothing ran; the run after
  the `y` is saved), "load more" (the statement is already there), browse pages
  and counts (the client's own statements), the cancel probe and the catalog
  queries (they never go through a run), a run the Core refused before sending
  anything or could not deliver (no statement in the error, or a connection
  error), and a run the user cancelled that then stopped (nothing finished).
- **Cancelled but finished first:** saved, since it ran and what it changed
  stands. How the desktop records a cancelled run is not verified (its result
  carries the cancellation as an error); this client does not claim a statement
  that never finished.

After each such run the app issues one `SaveHistory` command, fire and forget. A
failed save shows one notice ("The statement ran, but it was not saved to the
history: ...") and blocks nothing. Quitting waits a moment for a save in flight.

**The overlay** (`ctrl+alt+e` or `cmd+alt+e`, IntelliJ's console history; or
prefix `e`; it works from any pane, and the same key closes it): a search field,
the entries newest first (`when  connection / database  ✓|✗  first line`), and the
highlighted statement in full below, with its time, duration, rows affected and,
for a failure, its error. `↑`/`↓`/`ctrl+n`/`ctrl+p`/`pgup`/`pgdn` and the wheel
move the highlight (`shift+↑↓` scrolls a long statement); a click on a row
highlights it. Typing searches after a 200 ms pause (the app's `next_deadline`;
more typing moves it), matching the statement, the connection's name and the
database without regard to case; a reply to an older search is dropped. The
highlight reaching the last entry loaded fetches the next page of 50 with dbx's
cursor. `tab` flips between this tab's connection (the default) and every
connection (the default when the tab has no database yet).

- `enter` puts the statement in the editor at the caret, on lines of its own
  (a line break before it when text precedes the caret on the line, and after it
  when text follows), as one undo step, and closes the overlay.
- `f5` (or `ctrl+enter`, `cmd+enter`: the run keys) runs it in the active tab on
  that tab's target, through the normal run path: the editor is left alone and
  slice 17's gate in `DbxCore::run` applies, so a statement that needs
  confirming opens the confirmation. A row that ran somewhere other than this
  tab's target is drawn in `peach`, and the preview says where it ran and where
  `f5` will run it. The run is itself saved.

Everything read from the store was written by another program, so it is drawn
through the painter (control characters become `�`); an inserted statement goes
through the editor's paste cleaning. A failed search says why and that typing
searches again; an empty history says to run a statement with the run key.

## Export and copy (slice 19)

In the results pane `e` opens the export overlay and `C` the copy-as list. Both
work on **the rows the client holds**: when the row cap cut the result, the
overlay says the file will hold the first N rows and that load more (`m`) or a
narrower query gets the rest. Nothing re-runs the statement.

- **Export to a file.** The overlay offers CSV, JSON, Excel, Markdown and SQL
  INSERT statements, a path (default `<cwd>/<name>-<YYYYMMDD-HHMMSS>.<ext>`,
  where `<name>` is the table of a browse tab and `result` otherwise; the
  directory is the one the client was started in), for SQL the target table
  and schema (the same default name; the schema of a browse tab), and for CSV
  whether NULL is `\N` or an empty field. `tab`/`shift+tab` or the arrows move
  between rows; `left`/`right` or `1`-`5` pick a format; `enter` writes; `esc`
  closes. Each format's losses are listed in the overlay before writing.
- **Writing** is a shell command (`Command::Export`), off the event loop: the
  bytes go to a hidden temporary file in the target's directory, then a rename
  (a hard link when the name must be new) puts them in place. A file that is
  there is replaced only after a question that names it (`y`; `enter` is not a
  yes; `esc` returns to the overlay as it was). A directory that is not there is
  refused by name and not made. The notice (path, rows, bytes) appears only after
  the shell reports the write returned success; a failure names the path and the
  error and leaves the overlay as typed. Quitting waits a moment for a write in
  flight.
- **Copy as** (`C`, then `1` CSV, `2` JSON, `3` Markdown, `4` SQL INSERT, or the
  arrows and `enter`, or a click) sends the text to the clipboard through
  OSC 52, like the other copies, and says "Sent ... to the terminal clipboard".
  Over 75,000 bytes it is refused, and the notice suggests exporting to a file.
- **Row copy** (`Y`) says "Lossy: ..." when pasting may not give the row back:
  NULL and an empty string both occur (both paste as an empty field), or a cell
  holds a tab or a line break (a spreadsheet splits on it).

dbx's own formats do the work (`dbx_core::csv_export`, `text_export`,
`xlsx_export`, `database_export`; no new dependency). Our cells are exact text,
dbx's formats take JSON values, so the adapter converts: NULL is `null`; an
integer column's text is a JSON number only when it is exactly the text of an
`i64` or a `u64`; everything else is a string with the exact text. A row longer
than the header loses the cells past it; a shorter one stays short (a missing
cell is not NULL). The values are the ones the client holds, so the known
differences of `tests/fixtures/KNOWN_DIFFERENCES.md` come along (NaN and the
infinities are NULL, for one).

### What each format loses

| Format | Loses, or changes | Checked by |
|---|---|---|
| CSV | Everything is quoted (`"1"`), NULL is a bare `\N` (or a bare empty field when the literal is empty, which a reader may take for `''`; the empty string is `""` either way). **The formula guard:** a text value whose first character after spaces is `=`, `+`, `-` (unless the whole value is a decimal of up to 15 digits), `@` (when an ASCII character follows), a tab or a carriage return gets a `'` in front; the file then holds the value with that `'`. A value that starts with `'` and would be read as guarded gets a second one. Integers that fit 64 bits are written without the guard. | `csv_read_back_equals_psqls_values...`: 5 cells of the fixtures carry the guard |
| JSON | Nothing: NULL is `null`, an integer column's value that fits 64 bits is a number (`9007199254740993` exact), everything else is the exact text; keys are in column order. | `json_read_back_keeps_null...` |
| Excel | NULL and `''` both **show** as an empty cell; the file tells them apart (NULL is a cell with no value, `''` is a text cell holding nothing). Text in a date or timestamp column (no time zone) becomes an Excel date, a number of days with a display style, so the text held (`2024-03-05 13:45:12.123456`) is not in the file (zoned and time-only values stay text). A numeric column's text of up to 15 significant digits becomes a number (`1.50` is the number 1.5 shown with two decimals); longer ones stay text with every digit, and so does `9007199254740993`. **There is no formula guard in Excel files**: `=1+1` is stored as text, not as a formula. **A value over Excel's limit of 32,767 characters is written whole** by `build_xlsx_workbook` (dbx splits such values over several cells only in its own re-running export, which this client does not use); Excel may cut it, and the overlay counts these values. | `excel_holds_null_as_an_empty_cell...`, `what_dbx_does_to_other_values_in_an_excel_file` |
| Markdown | NULL looks like the text `NULL`; a line break (`\n` or `\r\n`) becomes `<br>`; `\|` for `|`; every cell is padded with trailing spaces to its column's width, so trailing spaces cannot be told from padding. | `markdown_read_back_differs_from_psql_only_by_the_listed_losses` |
| SQL INSERT | Only the statements: the table is not created, so it must exist with columns of the same names. Values are written as the text held and the server converts them. The round trip into a table created `like` the source returned every row of `text_values`, `numbers`, `gt_text`, `gt_json` and `gt_integers` cell for cell. A result with no rows has nothing to insert and is refused. **dbx leaves some columns out** (`is_export_insert_column`): a Postgres `tsvector` column, a column named `__DBX_ROWID` (the synthetic row id of Postgres, Oracle and Xugu), SQL Server `timestamp`/`rowversion` columns. The overlay lists them by name before writing (`1 column is left out of the INSERT: doc`), and a result of only such columns is refused. Column, table and schema names that start and end with a quote are quoted whole (`guarded()`, see below), so the file carries the exact names. | `sql_inserts_run_into_a_scratch_table...`, `a_quoted_column_and_table_name_reload...`, `columns_dbx_will_not_insert_are_found_by_name` | `sql_inserts_run_into_a_scratch_table...` |

**Names given to dbx's SQL builders** (design section 31): dbx's quoting passes a name that starts and ends with a quote through unescaped on Postgres, GaussDB and openGauss. Every name handed to a builder in `core/dbx.rs` goes through `guarded()` (`guarded_all`, `guarded_if` for the skip probe whose text is never run): the page select (schema, table, order columns, primary keys), the count select (schema, table) and the INSERT export (schema, table, columns). The unit test `every_name_given_to_a_dbx_sql_builder_goes_through_guarded` fails on a builder call or a name that skips it.

## Dump (slice 20)

`D` on a database or a schema in the sidebar (of a connection the client can
query and browse) opens the dump overlay: what goes in (structure and data,
structure only, data only), the scope (the whole database, or the schema the
cursor was on), drop tables if they exist (off), and the path (default
`<cwd>/<database>-<YYYYMMDD-HHMMSS>.sql`). `enter` dumps; `esc` closes.

- **dbx does the work:** `database_export::export_database_sql_core` on
  `export_runtime::spawn_export_task` (driven by the client's 16 MiB-stack
  runtime), then the export's own session is closed
  (`close_client_session_pool_with_completion`) and its cancel flag cleared,
  the desktop app's sequence. Structure includes dbx's objects (views,
  functions, sequences, triggers); `fail_on_error` is off, as in the desktop's
  dialog, so a failing object becomes a `-- ERROR` comment and is counted.
- **dbx records the destination directory in the store it runs with.**
  `ensure_export_destination_dir` saves the directory's filesystem identity
  under `database_export_destination:<dir>` (`storage.save_state`), on every
  dump, and would make a directory that is not there. So every test and
  journey runs on a scratch store, and the client refuses a missing directory
  by name before dbx is called (the shell's check, the export's rules).
- **dbx never gets the target path** (design section 34). The shell makes a
  hidden temporary file beside the target (`.<name>.<pid>.<n>.<nanos>.tmp`,
  `create_new`, mode 0600) and dbx writes into that, told it may write over it
  (dbx truncates and follows symlinks; the target must not suffer either).
  Only when dbx's final progress is `Done` is the file put in place: **linked
  without replacing** for a new name (a name taken meanwhile fails, and the
  dump is not kept), or **renamed over the target** after the user's `y` to a
  red question naming it (a symlink at the target is replaced itself, never
  written through; the replaced regular file's mode is kept, a new file is
  0600). A cancel, a failure, or a return with no final `Done` removes the
  temporary file, so the earlier file is byte for byte as it was and the
  client says so. The shell checks the path first, so dbx is not called for a
  file that is there and not confirmed.
- **Columns named like a quoted identifier refuse the dump.** dbx reads column
  names from its own catalog and quotes them with `quote_table_identifier`,
  which on Postgres, GaussDB and openGauss passes a name that starts and ends
  with `"`, a backtick, or `[` and `]` through unescaped, in its read query
  and its `INSERT` column list. The client cannot guard names it does not
  hand to dbx (`dump_request` is registered in the guard scan test as
  unguarded for that reason), so before such a dump it asks the catalog for
  those columns in the scope (a schema, or every non-system schema) and
  refuses, naming up to six as `schema.table.column`. Table and schema names
  are not checked.
- **Progress** comes as events for the dump's request (stale ones change
  nothing); the adapter passes running reports on at most every 100 ms. The
  mode bar shows `dumping pagila: public: film (7/22) 12,340 rows 14s` in
  every pane (dbx counts a whole database's objects per schema, and names them
  `schema: object`); a click on it, or `D`, shows the progress with `c cancel
  dump`. One dump at a time: `D` on another database says so.
- **Cancel** shows `cancelling…` until dbx's final `Cancelled` progress (dbx
  checks its flag between pages and objects, every 50 ms while it waits), then
  says what is at the path. **What a cancel leaves behind** (dbx's own
  behaviour, measured on `million_rows`, `a_dump_cancelled_midway...` in
  `tests/live.rs`): one schema is written straight to the file it is given and
  a partial file is left (141 kB in the test); a whole database goes schema by
  schema into dbx's own temporary directory, removed when it ends, and the file
  is opened only after the last schema. dbx is given our temporary file, which
  is removed, so **at the path there is nothing, or the earlier file untouched**
  (a partial file at the path cannot happen). A failure is the same. The client
  says which, from the file system (the shell compares the file with what was
  there before).
- **Ending:** the end says "Dumped" only when dbx's last progress is `Done` and
  a new file is at the path. Then it gives the path, the size on disk, the
  schemas (whole database) or objects (one schema), the rows and the time. On
  **Postgres, GaussDB and openGauss** it does not stop at that: it says `dbx
  reported no errors; its dump is known not to restore whole:` and lists the
  losses below, and the setup overlay shows the same line before the dump
  starts (other kinds say nothing they do not know). Done with errors says
  `finished with N errors`, the first, and that the file has `-- ERROR`
  comments. When dbx returns without a final `Done` the end says dbx ended
  without reporting that it was done, and what is at the path (nothing is put
  there); `Done` with no new file says so. A failure gives dbx's error,
  cleaned.
- **Quit while dumping** asks first, naming the file, and says what may be
  left: the shell's hidden temporary file beside the target and, for a whole
  database, dbx's own temporary copy (`schema-N.sql` files, in a folder dbx
  makes with `tempfile::tempdir()`) in the system temporary folder, which the
  question names (the folder's own name is dbx's). `y` cancels the dump and
  quits once its final progress has come, or after 2 s; quitting also waits
  (bounded) for the dump's task, which closes its session. A signal cancels it
  the same way without asking.
- **Read only:** without `snapshot_session_id` dbx opens no transaction (the
  scheduled backups' `BEGIN ... READ ONLY` snapshot is not used); the live
  tests run `tests/fixtures/check.sh` after every dump.

### What dbx's dump loses (checked 2026-11-25, Postgres 16 fixtures)

dbx reports these dumps as done with 0 errors, but the files do not restore
whole. The live tests pin each loss, and fail when dbx changes.

| Loss | Seen with |
|---|---|
| Domains are not written: a table using one (`film.release_year year`, `gt_types`) cannot be created, and its rows are lost. | pagila `film`; edge `gt_types` (`gt_posint`) |
| A composite type used as an array (`gt_pair[]`) is not written. | edge `gt_arrays_more` |
| A partition's DDL is written as `PARTITION OF "public"."payment" (` + newline + `) FOR VALUES ...`, which does not parse (six of pagila's seven partitions). | pagila `payment_p2022_01`..`06` |
| A partitioned table's rows are written through the parent right after it is created, before any partition exists (and again for each partition). | pagila `payment` |
| `NaN`, `Infinity` and `-Infinity` in a float column are written as `NULL` (dbx reads values on the binary path; see `KNOWN_DIFFERENCES.md`). | edge `numbers.approx` |
| Values dbx reads only as binary are written as their wire bytes (`'0x3ff0…'`, `E'\x00\x00…'`): geometric types, `tsquery`. Postgres refuses the NUL bytes, and the rows of the batch are lost; a `tsvector` column is left out of the `INSERT`. | edge `gt_geometric`, `gt_documents` |
| A table with several inheritance parents is created twice (`relation "inherit_child" already exists`). | edge `inherit_child` |

With `ON_ERROR_STOP=1` pagila's dump stops at the first of these. Loaded past
its errors, every other pagila table, and edge's `text_values`, `gt_text`,
`gt_json`, `"Mixed Case"` and `other.things`, come back with the same rows
(count and md5 of the ordered rows). A structure-only dump has no `INSERT`
and restores every table dbx can make, empty.

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
  itself. `dbpane kitty-setup` prints the path of `extras/kitty/dbx-tui.conf`
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
height (`sidebar_permille`, `editor_permille`) in `dbpane-prefs.json` in the
directory the store lives in (`DbxCore::store_dir`: whichever store is in use,
see above). The shell reads it at the start and writes it
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
| Docs are true (slice 24b): every key in `docs/README.md` and `docs/getting-started.md` is bound in the pane its table names | `cargo test -p dbx-tui --test docs_keys` | nothing; also checks the staged `.dbpane-repo` copies match (`scripts/stage-docs.sh` copies them) |
| Store: `DbxCore` on scratch stores (layer 1) | `cargo test -p dbx-tui --test store -- --test-threads=1` | nothing external; temp dirs, key files, a local TCP port |
| Live: `DbxCore` against the fixture Postgres (layer 1) | `crates/dbx-tui/tests/fixtures/up.sh && cargo test -p dbx-tui --test live -- --test-threads=1` | Docker (the fixtures below) |
| Ground truth: every cell of the `edge` tables, `DbxCore` against `psql`, and through the drawn grid (layer 1; slice 08) | `crates/dbx-tui/tests/fixtures/up.sh && cargo test -p dbx-tui --test ground_truth -- --test-threads=1` | Docker (it runs `psql` inside the fixture container), about 5 s |
| Fixtures: the test databases | `crates/dbx-tui/tests/fixtures/up.sh && crates/dbx-tui/tests/fixtures/check.sh` | Docker |
| Journeys: the built binary in tmux (layer 5; `colours`, `add-connection`, `browse-pagila`, `browse-table`, `cancel`, `complete`, `confirm-production` (a scratch schema it creates and drops), `export` (writes files in a scratch directory only), `dump` (dumps `pagila` into a scratch directory, cancels a dump of `edge`), `first-query`, `first-run` (a scratch HOME with no dbx store; needs the fixtures), `history`, `mouse`, `quit-mid-run`, `tabs` and `wide-result` need the fixtures) | `for j in crates/dbx-tui/tests/journeys/*.sh; do [ "$(basename "$j")" = lib.sh ] \|\| bash "$j"; done` | tmux, openssl, python3 |
| Journeys against the release binary | `cargo build -p dbx-tui --release`, then the same loop with `PROFILE=release` in front (`PROFILE=release bash <journey>`) | as above |
| Lint | `cargo clippy -p dbx-tui --all-targets` | |

A journey that passes removes its scratch directory and tmux socket from `/tmp`;
set `JOURNEY_OUT=<dir>` to keep the captures (a failing journey always keeps
them). `cli-flags` (`--version`, `--help`, `--licenses`) needs nothing external.

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
