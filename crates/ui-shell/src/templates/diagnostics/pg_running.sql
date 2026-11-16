-- Running now, the longest first: how long each query and its transaction
-- have been going, what it waits on and who blocks it.
select a.pid,
       a.datname                                              as database,
       a.usename                                              as "user",
       a.application_name                                     as application,
       a.client_addr                                          as client,
       a.state,
       case when a.state = 'active'
            then date_trunc('milliseconds', clock_timestamp() - a.query_start)
       end                                                    as running_for,
       date_trunc('milliseconds', clock_timestamp() - a.xact_start) as transaction_for,
       a.wait_event_type || ': ' || a.wait_event              as waiting_on,
       nullif(pg_blocking_pids(a.pid), '{}')                  as blocked_by,
       (select count(*) from pg_locks l
         where l.pid = a.pid and l.granted)                   as locks_held,
       a.query
  from pg_stat_activity a
 where a.backend_type = 'client backend'
   and a.pid <> pg_backend_pid()
   and a.state <> 'idle'
 order by coalesce(a.xact_start, a.query_start);
