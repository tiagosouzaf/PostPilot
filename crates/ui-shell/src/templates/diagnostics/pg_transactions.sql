-- Open transactions, the oldest first. A long or forgotten one ("idle in
-- transaction") holds locks and keeps vacuum from cleaning dead rows.
select a.pid,
       a.state,
       date_trunc('seconds', clock_timestamp() - a.xact_start)    as transaction_for,
       date_trunc('seconds', clock_timestamp() - a.state_change)  as in_this_state_for,
       (select count(*) from pg_locks l
         where l.pid = a.pid and l.granted)                       as locks_held,
       cardinality(array(select e.pid from pg_stat_activity e
                          where a.pid = any (pg_blocking_pids(e.pid)))) as sessions_blocked,
       age(a.backend_xmin)                                        as holds_back_vacuum_by_xids,
       a.datname                                                  as database,
       a.usename                                                  as "user",
       a.application_name                                         as application,
       a.client_addr                                              as client,
       a.query                                                    as last_query
  from pg_stat_activity a
 where a.xact_start is not null
   and a.pid <> pg_backend_pid()
 order by a.xact_start;
