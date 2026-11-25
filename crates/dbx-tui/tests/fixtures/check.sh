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
# Every user schema is read by the ground-truth harness: tables and views.
check edge "select string_agg(table_schema || '.' || table_name, ',' order by table_schema || '.' || table_name collate \"C\") from information_schema.tables where table_schema not in ('pg_catalog', 'information_schema') and left(table_schema, 3) <> 'pg_' and table_type in ('BASE TABLE', 'VIEW')" \
  "other.things,public.\"x\"; select pg_sleep(0) --\",public.Mixed Case,public.composite_key,public.empty_table,public.gt_arrays,public.gt_arrays_more,public.gt_bool_uuid_bytea,public.gt_catalog_types,public.gt_computed,public.gt_documents,public.gt_floats,public.gt_geometric,public.gt_integers,public.gt_json,public.gt_money_bits,public.gt_network,public.gt_numerics,public.gt_other_types,public.gt_precision,public.gt_temporal,public.gt_text,public.gt_types,public.inherit_child,public.inherit_parent_a,public.inherit_parent_b,public.million_rows,public.no_pk,public.numbers,public.other_types,public.temporal,public.text_values,public.wide_120,public.width_text"
check edge "select count(*) from pg_class c join pg_namespace n on n.oid = c.relnamespace where c.relkind in ('m', 'f', 'p') and n.nspname not in ('pg_catalog', 'information_schema') and left(n.nspname, 3) <> 'pg_'" 0
check edge "select string_agg(id || ':' || coalesce('[' || v || ']', 'NULL'), ',' order by id) from text_values where id <= 4" "1:NULL,2:[],3:[NULL],4:[ ]"
check edge "select big::text from numbers where id = 1" 9007199254740993
check edge "select exact::text from numbers where id = 1" 12345678901234567890.123456789
check edge "select approx::text from numbers where id in (3, 4) order by id" "NaN
Infinity"
check edge "select v from text_values where id = 7" "日本語 😀 👨‍👩‍👧 ａｂｃ"
check edge "select count(*) from empty_table" 0
check edge "select count(*) from pg_inherits where inhrelid = 'inherit_child'::regclass" 2
check edge "select \"select\" from \"Mixed Case\"" reserved
# The table named with quotes and a second statement (edge/06-quoted-name.sql).
check edge "select v from \"\"\"x\"\"; select pg_sleep(0) --\"\"\" where id = 1" "one known row"
# The grid's tables (slice 07): 120 columns that name themselves, a million
# rows, and text of known display width.
check edge "select count(*) from information_schema.columns where table_schema = 'public' and table_name = 'wide_120'" 121
check edge "select count(*) from wide_120" 400
check edge "select c001 || '|' || c002 || '|' || c003 || '|' || c004 || '|' || c120 from wide_120 where id = 7" 'c001-r007|2007.50|{"c": 3, "r": 7}|4007|120007'
check edge "select count(*) || '|' || sum(num) from million_rows" "1000000|500009446.45"
check edge "select txt from million_rows where id = 999999" "row 999999"
check edge "select string_agg(id || ':' || octet_length(v), ',' order by id) from width_text where id in (2, 5, 6)" "2:24,5:13,6:18"
# The ground-truth tables (slice 08, edge/05-ground-truth.sql): row counts, and
# a few values whose exact text the harness depends on.
check edge "select string_agg(t || ':' || n, ',' order by t) from (
    select 'arrays' t, count(*) n from gt_arrays union all select 'bool_uuid_bytea', count(*) from gt_bool_uuid_bytea
    union all select 'documents', count(*) from gt_documents union all select 'floats', count(*) from gt_floats
    union all select 'integers', count(*) from gt_integers union all select 'json', count(*) from gt_json
    union all select 'money_bits', count(*) from gt_money_bits union all select 'network', count(*) from gt_network
    union all select 'numerics', count(*) from gt_numerics union all select 'other_types', count(*) from gt_other_types
    union all select 'temporal', count(*) from gt_temporal union all select 'text', count(*) from gt_text
    union all select 'types', count(*) from gt_types union all select 'arrays_more', count(*) from gt_arrays_more
    union all select 'precision', count(*) from gt_precision union all select 'geometric', count(*) from gt_geometric
    union all select 'catalog_types', count(*) from gt_catalog_types union all select 'computed (view)', count(*) from gt_computed) x" \
  "arrays:9,arrays_more:5,bool_uuid_bytea:6,catalog_types:4,computed (view):3,documents:4,floats:19,geometric:4,integers:12,json:12,money_bits:6,network:6,numerics:13,other_types:3,precision:6,temporal:13,text:22,types:5"
check edge "select i8::text from gt_integers where label = '2^53 + 1'" 9007199254740993
check edge "select n::text from gt_numerics where label = '30 digits, no scale'" 123456789012345678901234567890
check edge "select f4::text || '|' || f8::text from gt_floats where label = 'NaN'" "NaN|NaN"
check edge "select f4::text from gt_floats where label = 'one in ten million'" "1e-07"
check edge "select octet_length(t)::text from gt_text where label = '100 kB'" 102400
check edge "select string_agg(label || '=' || coalesce('[' || t || ']', 'NULL'), ',' order by id) from gt_text where id <= 5" "null=NULL,empty string=[],the text NULL=[NULL],the text null=[null],one space=[ ]"
check edge "select doc from (select jb::text doc from gt_json where label = 'object') x" '{"a": 2, "b": 1}'
check edge "select length(bin) || '|' || encode(bin, 'hex') from gt_bool_uuid_bytea where label = 'bytes that read as text'" "5|68656c6c6f"
# Added after the review: json[] keeps its elements' text, a view of aggregates,
# fractional-second precision, geometric types, jsonpath, xid8 and regtype.
check edge "select ajs::text from gt_arrays where id = 1" '{"{\"b\": 1,  \"a\": 2}"}'
check edge "select string_agg(id || ':' || coalesce(sum_numeric::text, 'NULL') || '/' || cnt || '/' || cnt_col, ',' order by id) from gt_computed" "1:3.30/2/2,2:12345678901234567890.123456789/2/1,3:NULL/1/0"
check edge "select ln::text || '|' || ls::text || '|' || pa::text || '|' || po::text || '|' || ci::text from gt_geometric where id = 1" "{1,-1,0}|[(0,0),(1,1)]|[(0,0),(1,1),(2,0)]|((0,0),(4,0),(4,3))|<(1,2),3>"
check edge "select jp::text || '|' || x8::text || '|' || rt::text from gt_catalog_types where id = 2" 'strict $."a b".**{1 to last}|18446744073709551615|gt_mood'
check edge "select string_agg(ts3::text || '|' || t3::text, ',' order by id) from gt_precision where id in (3, 4)" "2024-03-05 13:45:12.124|13:45:12.124,2025-01-01 00:00:00|24:00:00"
check edge "select string_agg(ad::text || '|' || ai8::text || '|' || ae::text || '|' || ac::text, ',' order by id) from gt_arrays_more where id = 1" '{2024-03-05,1970-01-01}|{9007199254740993,-9223372036854775808}|{ok,"a b",it'"'"'s}|{"(1,\"a b\")","(2,)"}'
echo "OK fixtures: $n checks"
