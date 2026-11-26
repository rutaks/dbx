#!/usr/bin/env bash
# Journey sqlite (slice 23a): no Docker. On an empty scratch store, add a COPY of
# Sakila for SQLite through the form (type SQLite: a name and a file, no host,
# port or password), see it in the sidebar, open it (its tables are right under
# it), give the tab the file with `u`, run `select title from film order by
# film_id limit 3`, get completion on `fil`, and open `actor` with `o`.
# A missing file is refused with a message and nothing is saved.
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
typ() { tm send-keys -t "$SESSION" -l "$1"; }

# goto REGEX: press j until the cursor row (the one drawn on a background) matches.
goto() {
  local n=0
  while [ "$n" -lt 60 ]; do
    if tm capture-pane -e -p -t "$SESSION" | grep -E -- "$1" | grep -q '48;2;49;50;68'; then return 0; fi
    key j; sleep 0.12; n=$((n + 1))
  done
  capture "goto-failed"; fail "the cursor never reached /$1/"
}

build_bin
"$JOURNEY_DIR/../fixtures/sqlite/build.sh" "$OUT/fixtures" >/dev/null || fail "build the SQLite fixtures"
cp "$OUT/fixtures/sakila.db" "$OUT/sakila.db"
scratch_store
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 00-empty 'No connections yet'

# The form: the type row first; SQLite has only a name and a file.
key C-g n
wait_for 01-form 'New connection'
expect form 'type +● PostgreSQL +○ MySQL +○ SQLite'
expect form 'host'
key Up
key Space
key Space
wait_for 02-sqlite-form 'type +○ PostgreSQL +○ MySQL +● SQLite'
expect sqlite-form 'file'
expect sqlite-form 'production'
expect_not sqlite-form 'host'
expect_not sqlite-form 'port'
expect_not sqlite-form 'password'
expect_not sqlite-form 'tls'

# A file that is not there is refused, with the path in the message.
key Down
typ 'ghost'
key Tab
typ "$OUT/not-there.db"
key Enter
wait_for 03-refused 'not an existing regular file'
expect refused 'New connection'
[ ! -e "$OUT/not-there.db" ] || fail "the refused path was created"

# The real file.
key C-u
typ "$OUT/sakila.db"
key Enter
wait_for 04-saved 'ghost +sqlite'
expect_not saved 'New connection'
expect saved 'ghost +sqlite'

# Opening it lists its tables right away: no databases, no schemas.
key Enter
wait_for 05-tables 'film_actor'
expect tables '▸ film( |$)'
expect tables 'customer_list'
expect_not tables 'public'

# `u` on the connection gives the tab the file.
key g
key u
wait_for 06-used '1 ghost'
key C-g l
wait_for 07-editor 'EDITOR'
typ 'select title from film order by film_id limit 3'
key F5
wait_for 08-rows 'ACADEMY DINOSAUR'
expect rows 'ACE GOLDFINGER'
expect rows 'ADAPTATION HOLES'
expect rows '3 rows'

# Completion on `fil`: the four tables and the view.
key Enter Enter
typ 'select * from fil'
wait_for 09-popup 'film_actor fa +table'
expect popup 'film_category fc +table'
expect popup 'film_text ft +table'
expect popup 'film_list fl +view'
expect popup 'esc close'
key Escape

# `o` on actor in the sidebar opens it paged.
key C-g h
wait_for 10-sidebar 'SIDEBAR'
goto 'actor'
key o
wait_for 11-actor 'PENELOPE'
expect actor 'actor_id'
expect actor 'GUINESS'

key C-g q
wait_for 12-quit 'EXIT=0' 3
expect_terminal_restored quit
echo "OK sqlite; captures in $OUT"
