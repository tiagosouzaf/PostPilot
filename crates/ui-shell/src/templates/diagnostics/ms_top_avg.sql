-- Slowest queries on average in the plan cache (at least 5 executions,
-- so one odd run does not lead).
select top (50)
       qs.total_worker_time / 1000                            as total_cpu_ms,
       cast(100.0 * qs.total_worker_time
            / nullif(sum(qs.total_worker_time) over (), 0) as decimal(5, 1)) as pct_of_cpu,
       qs.execution_count                                     as executions,
       cast(qs.total_worker_time / 1000.0 / qs.execution_count as decimal(18, 2))  as avg_cpu_ms,
       qs.total_elapsed_time / 1000                           as total_elapsed_ms,
       cast(qs.total_elapsed_time / 1000.0 / qs.execution_count as decimal(18, 2)) as avg_elapsed_ms,
       qs.max_elapsed_time / 1000                             as max_elapsed_ms,
       qs.total_logical_reads                                 as logical_reads,
       qs.total_logical_reads / qs.execution_count            as avg_logical_reads,
       qs.total_physical_reads                                as physical_reads,
       qs.total_logical_writes                                as writes,
       qs.total_rows                                          as rows,
       qs.last_execution_time                                 as last_execution,
       qs.creation_time                                       as cached_since,
       db_name(t.dbid)                                        as [database],
       object_name(t.objectid, t.dbid)                        as [object],
       substring(t.text, qs.statement_start_offset / 2 + 1,
                 (case qs.statement_end_offset
                      when -1 then datalength(t.text)
                      else qs.statement_end_offset
                  end - qs.statement_start_offset) / 2 + 1)   as statement
  from sys.dm_exec_query_stats qs
 cross apply sys.dm_exec_sql_text(qs.sql_handle) t
 where qs.execution_count >= 5
 order by qs.total_elapsed_time / qs.execution_count desc;
