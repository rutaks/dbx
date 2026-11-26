#!/usr/bin/env bash
# Journey mysql (slice 23b): on an empty scratch store, add the MySQL fixture
# through the form (type MySQL: host, port 3306 to start with, user, password,
# database, TLS), see it in the sidebar, open it (databases, then tables, with
# no schema level), give the tab `sakila` with `u`, run `select title from film
# order by film_id limit 3`, get completion on `fil`, cancel `select sleep(30)`
# (the tab says cancelled, the server's processlist shows nothing, the next
# statement runs in the same session), and open `film` with `o`.
# Needs the MySQL fixture: tests/fixtures/up-mysql.sh
. "$(dirname "$0")/lib.sh"

key() { tm send-keys -t "$SESSION" "$@"; }
typ() { tm send-keys -t "$SESSION" -l "$1"; }
semicolon() { tm send-keys -t "$SESSION" '\;'; }
nc -z 127.0.0.1 53307 2>/dev/null || fail "the fixture MySQL is not up: run tests/fixtures/up-mysql.sh"
TAG="dbxtui_mysql_journey_$$"

# running TAG: how many statements containing TAG the server is running.
running() {
  docker exec dbxtui-fixtures-mysql-mysql-1 mysql -uroot -pdbxtui -N -B -e \
    "select count(*) from information_schema.processlist where info like '%$1%' and id <> connection_id() and command = 'Query'" 2>/dev/null
}

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
scratch_store
start_shell

tm send-keys -t "$SESSION" "$CLIENT; echo EXIT=\$?" Enter
wait_for 00-empty 'No connections yet'

# The form: the type row first; MySQL has every server field, and its own port.
key C-g n
wait_for 01-form 'New connection'
expect form 'type +● PostgreSQL +○ MySQL +○ SQLite'
expect form '5432'
key Up
key Space
wait_for 02-mysql-form 'type +○ PostgreSQL +● MySQL +○ SQLite'
expect mysql-form '3306'
expect_not mysql-form '5432'
for field in host port user password database tls production; do expect mysql-form "$field"; done
expect_not mysql-form 'file'

# Fill it in for the fixture, test it, save it.
key Down
typ 'fixture my'
key Tab; typ '127.0.0.1'
key Tab C-u; typ '53307'
key Tab; typ 'root'
key Tab; typ 'dbxtui'
key Tab; typ 'sakila'
key C-t
wait_for 03-test-ok 'Connection successful'
expect test-ok '••••••'
expect_not test-ok 'dbxtui +sakila'
key Enter
wait_for 04-saved 'fixture my +mysql'
expect_not saved 'New connection'

# Opening it lists the databases; opening one lists its tables, with no schema level.
key Enter
wait_for 05-databases '▸ sakila'
expect databases '▸ edge'
expect databases '▸ edge_other'
goto 'sakila'
key Enter
wait_for 06-tables 'film_actor'
expect tables 'customer_list'
expect tables 'film_list'

# `u` on the database gives the tab `sakila`.
key u
wait_for 07-used '1 fixture my / sakila'
key C-g l
wait_for 08-editor 'EDITOR'
typ 'select title from film order by film_id limit 3'
key F5
wait_for 09-rows 'ACADEMY DINOSAUR'
expect rows 'ACE GOLDFINGER'
expect rows 'ADAPTATION HOLES'
expect rows '3 rows'

# Completion on `fil`: the tables and the view of the current database.
key Enter Enter
typ 'select * from fil'
wait_for 10-popup 'film_actor fa +table'
expect popup 'film_category fc +table'
expect popup 'film_text ft +table'
expect popup 'film_list fl +view'
expect popup 'esc close'
key Escape

# Cancel `select sleep(30)`: cancelled, nothing left on the server, the tab runs again.
semicolon
key Enter
typ 'set @earlier = 41'
key F5
# dbx calls every SET dangerous: it asks (a `y` is ignored for 300 ms after the box opens).
wait_for 10a-asked 'Run a dangerous statement\?' 5
sleep 0.5
key y
wait_for 10b-set '0 rows affected' 5
semicolon
key Enter
typ "select sleep(30) as ${TAG}"
key F5
wait_for 11-running 'running 1s' 5
expect running 'ctrl\+c cancel'
[ "$(running "$TAG")" = 1 ] || fail "the statement is not running on the server"
key C-c
wait_for 12-cancelled 'cancelled after' 5
expect cancelled '● cancelled after'
expect_not cancelled 'ctrl\+c cancel'
for _ in $(seq 30); do [ "$(running "$TAG")" = 0 ] && break; sleep 0.1; done
[ "$(running "$TAG")" = 0 ] || fail "$TAG is still running on the server"
semicolon
key Enter
typ 'select @earlier + 1 as after_cancel'
key F5
wait_for 13-after '1 row' 5
expect after '42'

# `o` on film in the sidebar opens it paged.
key C-g h
wait_for 14-sidebar 'SIDEBAR'
# The cursor is still on `sakila` (where `u` was pressed): film is nine rows
# below it (actor, actor_info, address, category, city, country, customer,
# customer_list, film).
for _ in $(seq 9); do key j; done
key o
wait_for 15-film 'ACADEMY DINOSAUR'
expect film 'SELECT \* FROM `film` ORDER BY `film_id` LIMIT 1000'
expect film 'film_id'
expect film 'title'

key C-g q
wait_for 16-quit 'EXIT=0' 3
expect_terminal_restored quit
echo "OK mysql; captures in $OUT"
