-- leading comment, then a blank line

/* block before the statement */ select /* after select */ a /* after a */ , -- after the comma
  b -- trailing on b
  /* own line in the list */
, c from /* before the table */ t -- after t
where x=1 -- first condition
  and /* between */ y=2 or z between 1 and 3
  /* before group */ group by a,b having count(*)>1 order by a desc, b ; -- after the semicolon

select 'it''s a ; semicolon', 'select from where -- not a comment', '/* not a comment */', E'esc\'aped ; -- no', $$dollar ; body -- here
 select from$$, $tag$ $$ nested $$ $tag$, "select", "Quoted ""ident"" with ; and --", `back` from t;

select * from t where a::int=1 and b->>'k'='v' and c#>>'{x,y}'='z' and d @> '{"q":1}'::jsonb and e<>f and g!=h and i<=j and k>=l;

select -1, - 1, +2, a-1, a - -1, a*b, count(*), t.*, a.b.c, x[1], x [2], arr[1:2], 1.5e3, .5, 0x1F, $1, $2, ? from t;

with a as (select 1), b as (select * from a /* c */) select * from b -- end
;
with recursive r(n) as (select 1 union all select n+1 from r where n<10) select n from r;

select a from (select b from t where b in (select c from u where d=1)) s join v on s.b=v.b and s.c=v.c left outer join w using (id) cross join lateral (select 1) z where exists (select 1 from q);

select case when a=1 and b=2 then 'x' when a between 1 and 2 then 'y' else 'z' end, left(name, 2), right(name,2), replace(a,'b','c') from t;

select a from t union all select b from u intersect select c from v except select d from w;

insert into t(a,b) values(1,'x'),(2,'y') on conflict (a) do update set b=excluded.b returning a;
update t set a=1,b=2 where c=3 returning *;
delete from t where a in (1,2,3);

create table x (id serial primary key, name text not null default 'a;b', created timestamp with time zone default now(), check (id > 0), unique (name));
create index on x using btree (name) where id > 1;
create function f(a int) returns int language plpgsql as $body$
begin
  -- comment inside the body
  return a + 1;  /* and a block */
end;
$body$;
alter table only x add constraint c foreign key (id) references y(id) on delete cascade;
select distinct on (a) a, b from t order by a, b limit 10 offset 5;
select a as "select", b as "from" from "where";
select 'unicode: héllo wörld ☃' as s, "ünï" from t;
select count(*) filter (where a > 1) over (partition by b order by c rows between unbounded preceding and current row) from t;
select a from t where b = any(array[1,2,3]) and c is distinct from d and e not like 'x%' and f ilike 'y';
-- trailing comment with no newline
/* final block */

select 'foo'
'bar' as a, E'x'
  'y' from t;

select - -1, a - - b, 1 - -1;
select 1; -- semicolon comment
;
