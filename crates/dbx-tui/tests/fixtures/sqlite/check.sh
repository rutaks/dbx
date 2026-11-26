#!/usr/bin/env bash
# Checks the built SQLite fixtures hold what the tests expect. No Docker. It is
# also run at the end of ../check.sh. Usage: tests/fixtures/sqlite/check.sh [dir]
set -euo pipefail
cd "$(dirname "$0")"
OUT="$(./build.sh ${1:+"$1"} | sed -e 's/^sqlite fixtures in //' -e 's/ (.*$//')"
SQLITE3="${SQLITE3:-/usr/bin/sqlite3}"
n=0
check() { # check DB SQL EXPECTED
  local got; got="$("$SQLITE3" -batch "$OUT/$1.db" "$2")"
  if [ "$got" != "$3" ]; then
    echo "FAIL [sqlite $1] $2"; echo "  want: $3"; echo "  got:  $got"; exit 1
  fi
  n=$((n + 1))
}
check sakila "select count(*) from film" 1000
check sakila "select title from film where film_id = 1" "ACADEMY DINOSAUR"
check sakila "select count(*) from sqlite_master where type = 'table'" 16
check sakila "select count(*) from sqlite_master where type = 'view'" 5
check sakila "select count(*) from actor" 200
check sakila "select count(*) from rental" 16044
check sakila "select count(*) from payment" 16049
check sakila "select group_concat(name) from (select name from sqlite_master where type = 'table' and name like 'fil%' order by name)" "film,film_actor,film_category,film_text"
check edge "select group_concat(name, ',') from (select name from sqlite_master where type in ('table', 'view') order by name)" \
  '"x",Mixed Case,[x],blobs,child,composite_key,dates,documents,edge_view,empty_table,integers,no_pk,numbers_10k,parent,reals,text_values,texts,typed,untyped,x,x"; select 1 --'
check edge "select group_concat(id || ':' || coalesce('[' || v || ']', 'NULL'), ',') from (select * from text_values where id <= 4 order by id)" "1:NULL,2:[],3:[NULL],4:[ ]"
check edge "select v from integers where id = 2" 9007199254740993
check edge "select v from integers where id in (3, 4) order by id" "9223372036854775807
-9223372036854775808"
check edge "select typeof(v) || '|' || (v > 1.7976931348623157e308) from reals where id = 9" "real|1"
check edge "select typeof(v) from reals where id = 11" "null"
check edge "select v from texts where id = 2" "👨‍👩‍👧"
check edge "select hex(b) from blobs where id = 4" "FF00FE"
check edge "select group_concat(typeof(v), ',') from (select v from untyped order by id)" "integer,real,text,blob,null,integer,text,real,integer"
check edge "select count(*) from numbers_10k" 10000
check edge "select count(*) from no_pk" 4
check edge "select count(*) from pragma_table_info('composite_key') where pk > 0" 2
check edge "select v from \"x\"\"; select 1 --\" where id = 1" "one known row"
check edge "select (select count(*) from x) || ',' || (select count(*) from \"\"\"x\"\"\") || ',' || (select count(*) from \"[x]\")" "1,2,3"
echo "OK sqlite fixtures: $n checks ($(cut -d' ' -f1 "$OUT/sqlite3-version.txt"))"
