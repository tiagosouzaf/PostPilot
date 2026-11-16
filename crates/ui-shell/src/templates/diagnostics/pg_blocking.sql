-- Who blocks whom, as a tree: each session at the left margin holds a lock
-- and is not waiting for anyone; the sessions under it wait for it.
-- No rows = nobody is blocked.
with recursive
sessions as (
    select a.*, array_remove(pg_blocking_pids(a.pid), 0) as blockers
      from pg_stat_activity a
),
edges as (
    select s.pid, b.pid as blocker
      from sessions s
     cross join lateral unnest(s.blockers) as b(pid)
),
tree as (
    select s.pid, 0 as level, array[s.pid] as path, null::int as blocked_by
      from sessions s
     where s.blockers = '{}'
       and exists (select 1 from edges e where e.blocker = s.pid)
    union all
    select e.pid, t.level + 1, t.path || e.pid, e.blocker
      from tree t
      join edges e on e.blocker = t.pid
     where e.pid <> all (t.path)
)
select repeat('      ', t.level) || case when t.level > 0 then '└─ ' else '' end || t.pid as session,
       case when t.level = 0 then 'BLOCKING' else 'blocked' end as role,
       t.blocked_by,
       (select count(distinct x.pid) from tree x
         where x.pid <> t.pid and t.pid = any (x.path))       as sessions_waiting_behind,
       s.state,
       case when t.level > 0
            then date_trunc('milliseconds', clock_timestamp() - s.query_start)
       end                                                    as waiting_for,
       date_trunc('milliseconds', clock_timestamp() - s.xact_start) as transaction_for,
       w.mode                                                 as lock_wanted,
       w.object                                               as locked_object,
       s.datname                                              as database,
       s.usename                                              as "user",
       s.application_name                                     as application,
       s.client_addr                                          as client,
       s.query                                                as last_query
  from tree t
  join sessions s on s.pid = t.pid
  left join lateral (
        select l.mode,
               case
                   when l.locktype in ('relation', 'tuple', 'page', 'extend')
                        and l.database = (select oid from pg_database
                                           where datname = current_database())
                       then case l.locktype when 'relation' then 'table '
                                            when 'tuple' then 'row of '
                                            else l.locktype || ' of ' end
                            || l.relation::regclass
                   when l.locktype = 'transactionid'
                       then 'row changed by transaction ' || l.transactionid
                   else l.locktype
               end as object
          from pg_locks l
         where l.pid = t.pid and not l.granted
         limit 1
       ) w on true
 order by t.path;
