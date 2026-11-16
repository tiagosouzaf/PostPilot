-- Overview: connections, what is running or blocked right now, and the
-- counters of this database since stats_reset.
select current_database()                                    as database,
       pg_size_pretty(pg_database_size(current_database())) as database_size,
       a.connections,
       current_setting('max_connections')::int               as max_connections,
       a.running,
       a.blocked,
       a.idle_in_transaction,
       a.longest_query,
       a.longest_transaction,
       round(100.0 * d.blks_hit / nullif(d.blks_hit + d.blks_read, 0), 2) as cache_hit_pct,
       d.xact_commit                                         as commits,
       d.xact_rollback                                       as rollbacks,
       d.deadlocks,
       d.temp_files,
       pg_size_pretty(d.temp_bytes)                          as temp_size,
       d.stats_reset,
       pg_postmaster_start_time()                            as server_started
  from pg_stat_database d
 cross join (
        select count(*)                                               as connections,
               count(*) filter (where state = 'active')               as running,
               count(*) filter (where wait_event_type = 'Lock')       as blocked,
               count(*) filter (where state like 'idle in transaction%') as idle_in_transaction,
               date_trunc('second', max(clock_timestamp() - query_start)
                                    filter (where state = 'active'))  as longest_query,
               date_trunc('second', max(clock_timestamp() - xact_start)) as longest_transaction
          from pg_stat_activity
         where backend_type = 'client backend'
           and pid <> pg_backend_pid()
       ) a
 where d.datname = current_database();
