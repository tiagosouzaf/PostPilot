-- Slowest queries on average since the statistics were last reset (at
-- least 5 calls, so one odd run does not lead). From pg_stat_statements.
select round(s.total_exec_time::numeric, 1)                   as total_ms,
       round((100 * s.total_exec_time
              / nullif(sum(s.total_exec_time) over (), 0))::numeric, 1) as pct_of_total,
       s.calls,
       round(s.mean_exec_time::numeric, 2)                    as avg_ms,
       round(s.max_exec_time::numeric, 2)                     as max_ms,
       s.rows,
       round(s.rows::numeric / nullif(s.calls, 0), 1)         as rows_per_call,
       round(100.0 * s.shared_blks_hit
             / nullif(s.shared_blks_hit + s.shared_blks_read, 0), 1) as cache_hit_pct,
       pg_size_pretty(s.shared_blks_read * current_setting('block_size')::bigint)  as read_from_disk,
       pg_size_pretty(s.temp_blks_written * current_setting('block_size')::bigint) as temp_written,
       pg_size_pretty(s.wal_bytes)                            as wal_written,
       d.datname                                              as database,
       r.rolname                                              as "user",
       s.queryid,
       s.query
  from pg_stat_statements s
  left join pg_database d on d.oid = s.dbid
  left join pg_roles r on r.oid = s.userid
 where s.calls >= 5
 order by s.mean_exec_time desc
 limit 50;
