#!/usr/bin/env bash
# Checks the fixture databases hold exactly what the tests expect.
# Needs Docker. Usage: tests/fixtures/up.sh && tests/fixtures/check.sh
set -euo pipefail
cd "$(dirname "$0")"
q() { docker compose -f compose.yaml exec -T postgres psql -U postgres -d "$1" -tAX -F '|' -c "$2"; }
n=0
check() { # check DB SQL EXPECTED
  local got; got="$(q "$1" "$2")"
  if [ "$got" != "$3" ]; then
    echo "FAIL [$1] $2"; echo "  want: $3"; echo "  got:  $got"; exit 1
  fi
  n=$((n + 1))
}
check postgres "show server_version_num" "$(q postgres 'show server_version_num' | grep '^16')"
check pagila "select count(*) from information_schema.tables where table_schema = 'public' and table_type = 'BASE TABLE'" 22
check pagila "select count(*) from film" 1000
check pagila "select title from film where film_id = 1" "ACADEMY DINOSAUR"
check edge "select string_agg(table_schema || '.' || table_name, ',' order by table_schema || '.' || table_name collate \"C\") from information_schema.tables where table_schema in ('public', 'other')" \
  "other.things,public.Mixed Case,public.composite_key,public.empty_table,public.inherit_child,public.inherit_parent_a,public.inherit_parent_b,public.million_rows,public.no_pk,public.numbers,public.other_types,public.temporal,public.text_values,public.wide_120,public.width_text"
check edge "select string_agg(id || ':' || coalesce('[' || v || ']', 'NULL'), ',' order by id) from text_values where id <= 4" "1:NULL,2:[],3:[NULL],4:[ ]"
check edge "select big::text from numbers where id = 1" 9007199254740993
check edge "select exact::text from numbers where id = 1" 12345678901234567890.123456789
check edge "select approx::text from numbers where id in (3, 4) order by id" "NaN
Infinity"
check edge "select v from text_values where id = 7" "日本語 😀 👨‍👩‍👧 ａｂｃ"
check edge "select count(*) from empty_table" 0
check edge "select count(*) from pg_inherits where inhrelid = 'inherit_child'::regclass" 2
check edge "select \"select\" from \"Mixed Case\"" reserved
# The grid's tables (slice 07): 120 columns that name themselves, a million
# rows, and text of known display width.
check edge "select count(*) from information_schema.columns where table_schema = 'public' and table_name = 'wide_120'" 121
check edge "select count(*) from wide_120" 400
check edge "select c001 || '|' || c002 || '|' || c003 || '|' || c004 || '|' || c120 from wide_120 where id = 7" 'c001-r007|2007.50|{"c": 3, "r": 7}|4007|120007'
check edge "select count(*) || '|' || sum(num) from million_rows" "1000000|500009446.45"
check edge "select txt from million_rows where id = 999999" "row 999999"
check edge "select string_agg(id || ':' || octet_length(v), ',' order by id) from width_text where id in (2, 5, 6)" "2:24,5:13,6:18"
echo "OK fixtures: $n checks"
