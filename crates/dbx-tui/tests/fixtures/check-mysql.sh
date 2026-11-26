#!/usr/bin/env bash
# Checks the MySQL fixture holds exactly what the tests expect (slice 23b).
# Needs Docker and the container of up-mysql.sh. It is also run at the end of
# check.sh. Usage: tests/fixtures/up-mysql.sh && tests/fixtures/check-mysql.sh
set -euo pipefail
cd "$(dirname "$0")"
# `docker exec` on the compose project's container, not `docker compose exec`:
# the live tests run this with a scratch HOME, where the compose plugin is not
# found. -N -B: no header, tab-separated, NULL as the word.
q() { docker exec -i dbxtui-fixtures-mysql-mysql-1 mysql -uroot -pdbxtui --default-character-set=utf8mb4 -N -B -e "$2" "$1" 2>/dev/null; }
n=0
check() { # check DB SQL EXPECTED
  local got; got="$(q "$1" "$2")"
  if [ "$got" != "$3" ]; then
    echo "FAIL [mysql $1] $2"; echo "  want: $3"; echo "  got:  $got"; exit 1
  fi
  n=$((n + 1))
}
check mysql "select substring_index(@@version, '.', 2)" "8.4"
check mysql "select group_concat(schema_name order by schema_name) from information_schema.schemata where schema_name not in ('information_schema', 'mysql', 'performance_schema', 'sys')" "edge,edge_other,fixtures_ready,sakila"
# Sakila (jOOQ/sakila at e089a5b).
check sakila "select count(*) from film" 1000
check sakila "select title from film where film_id = 1" "ACADEMY DINOSAUR"
check sakila "select count(*) from information_schema.tables where table_schema = 'sakila' and table_type = 'BASE TABLE'" 16
check sakila "select count(*) from information_schema.tables where table_schema = 'sakila' and table_type = 'VIEW'" 7
check sakila "select count(*) from actor" 200
check sakila "select count(*) from payment" 16049
check sakila "select count(*) from rental" 16044
check sakila "select group_concat(routine_name order by routine_name) from information_schema.routines where routine_schema = 'sakila'" "film_in_stock,film_not_in_stock,get_customer_balance,inventory_held_by_customer,inventory_in_stock,rewards_report"
check sakila "select group_concat(trigger_name order by trigger_name) from information_schema.triggers where trigger_schema = 'sakila'" "del_film,ins_film,upd_film"
# Our edge cases (mysql/edge/01-edge.sql).
check edge "select group_concat(table_name order by table_name separator ' ; ') from information_schema.tables where table_schema = 'edge'" \
  "Mixed Case ; \`x\`; select sleep(0) #\` ; a\`b ; composite_key ; empty_table ; gt_decimals ; gt_floats ; gt_integers ; gt_misc ; gt_temporal ; gt_text ; gt_view ; no_pk ; numbers ; order ; text_values"
check edge "select group_concat(id, ':', ifnull(concat('[', v, ']'), 'NULL') order by id) from text_values where id <= 5" "1:NULL,2:[],3:[NULL],4:[ ],5:[null]"
check edge "select u8 from gt_integers where id = 4" 18446744073709551615
check edge "select i8 from gt_integers where id = 8" 9007199254740993
check edge "select d6530 from gt_decimals where id = 1" 12345678901234567890123456789012345.123456789012345678901234567890
check edge "select t from gt_temporal where id = 3" "-838:59:59"
check edge "select ts from gt_temporal where id = 1" "2024-03-05 08:15:12"
check edge "select y from gt_temporal where id = 2" 2155
check edge "select hex(b8), hex(b64), hex(bin), hex(vbin), hex(bl) from gt_misc where id = 3" "FF	FFFFFFFFFFFFFFFF	FF00FE01	FF00FE	C328FE"
check edge "select js from gt_misc where id = 1" '{"a": [1, 2.5, null, true], "b": 1, "t": "x"}'
check edge "select en, st from gt_misc where id = 1" "medium	a,c"
check edge "select v from text_values where id = 7" "日本語 😀 👨‍👩‍👧 ａｂｃ"
check edge "select octet_length(lt), char_length(tx) from gt_text where id = 7" "102400	60000"
check edge "select count(*) from empty_table" 0
check edge "select group_concat(a, b order by a, b) from composite_key" "11,12,21,22"
check edge "select \`select\` from \`Mixed Case\` where \`Id\` = 1" reserved
check edge "select v from \`\`\`x\`\`; select sleep(0) #\`\`\`" "one known row"
check edge "select count(*) from numbers" 400
check edge "select f4, f8 from gt_floats where id = 7" "3.40282e38	1.7976931348623157e308"
check edge_other "select group_concat(id, v order by id) from t" "1one,2two,3three"
echo "OK mysql fixtures: $n checks"
