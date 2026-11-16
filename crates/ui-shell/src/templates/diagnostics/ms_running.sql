-- Running now, the heaviest first: time, CPU, reads and memory of each
-- request, what it waits on and who blocks it.
select r.session_id,
       s.login_name,
       s.host_name,
       s.program_name,
       db_name(r.database_id)                                 as [database],
       r.status,
       r.command,
       r.total_elapsed_time                                   as elapsed_ms,
       r.cpu_time                                             as cpu_ms,
       r.logical_reads,
       r.reads                                                as physical_reads,
       r.writes,
       r.granted_query_memory * 8                             as memory_kb,
       r.wait_type,
       r.wait_time                                            as wait_ms,
       r.wait_resource,
       nullif(r.blocking_session_id, 0)                       as blocked_by,
       r.open_transaction_count                               as open_transactions,
       nullif(r.percent_complete, 0)                          as percent_complete,
       substring(t.text, r.statement_start_offset / 2 + 1,
                 (case r.statement_end_offset
                      when -1 then datalength(t.text)
                      else r.statement_end_offset
                  end - r.statement_start_offset) / 2 + 1)    as statement,
       t.text                                                 as batch
  from sys.dm_exec_requests r
  join sys.dm_exec_sessions s on s.session_id = r.session_id
 outer apply sys.dm_exec_sql_text(r.sql_handle) t
 where s.is_user_process = 1
   and r.session_id <> @@spid
 order by r.cpu_time desc, r.total_elapsed_time desc;
