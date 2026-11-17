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

## Tests

Run every command from the dbx workspace root.

| What | Command | Needs |
|---|---|---|
| Fast: app, view, keys, FakeCore (layer 2) | `cargo test -p dbx-tui --lib --test no_colour_literals` | nothing |
| Store: `DbxCore` on scratch stores (layer 1) | `cargo test -p dbx-tui --test store -- --test-threads=1` | nothing external; temp dirs and key files |
| Fixtures: the test databases | `crates/dbx-tui/tests/fixtures/up.sh && crates/dbx-tui/tests/fixtures/check.sh` | Docker |
| Journeys: the built binary in tmux (layer 5) | `for j in crates/dbx-tui/tests/journeys/*.sh; do [ "$(basename "$j")" = lib.sh ] \|\| bash "$j"; done` | tmux, openssl |
| Lint | `cargo clippy -p dbx-tui --all-targets` | |

`cargo test -p dbx-tui` runs the fast and store tests together. Test helpers on
the production adapter (`DbxCore::seed_connection`, `check_seed_dir`) sit
behind the `test-support` feature, which tests and examples get through a self
dev-dependency; the client binary is built without it. `examples/seed_store`
refuses any directory outside the system temp directory unless
`DBX_TUI_ALLOW_SEED=1`, and always refuses the desktop app's store.

The store tests set `DBX_SECRET_KEY_FILE` for the whole process, so they live
in their own test binary and hold one lock each; `--test-threads=1` makes that
explicit. Every one uses its own temporary data directory and a scratch `HOME`, and
never reaches the keychain or the real store. Journeys do the same through `lib.sh`
(`$CLIENT` runs the binary on a scratch store with a scratch key); captures go
to `$JOURNEY_OUT`, or a temp directory printed at the end.

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
