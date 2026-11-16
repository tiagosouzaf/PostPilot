-- Overview: connections, what is running or blocked right now, memory and
-- how long data pages stay in memory (a low page life expectancy means the
-- server is short of memory).
select @@servername                                           as server,
       db_name()                                              as [database],
       (select count(*) from sys.dm_exec_sessions
         where is_user_process = 1)                           as connections,
       (select count(*) from sys.dm_exec_requests r
          join sys.dm_exec_sessions s on s.session_id = r.session_id
         where s.is_user_process = 1
           and r.session_id <> @@spid)                        as running,
       (select count(*) from sys.dm_exec_requests
         where blocking_session_id <> 0)                      as blocked,
       (select max(r.total_elapsed_time) / 1000
          from sys.dm_exec_requests r
          join sys.dm_exec_sessions s on s.session_id = r.session_id
         where s.is_user_process = 1
           and r.session_id <> @@spid)                        as longest_query_s,
       (select max(datediff(second, t.transaction_begin_time, getdate()))
          from sys.dm_tran_session_transactions st
          join sys.dm_tran_active_transactions t
            on t.transaction_id = st.transaction_id
         where st.is_user_transaction = 1)                    as longest_transaction_s,
       i.cpu_count                                            as cpus,
       i.physical_memory_kb / 1024                            as server_memory_mb,
       m.physical_memory_in_use_kb / 1024                     as sql_server_memory_mb,
       (select top (1) cntr_value
          from sys.dm_os_performance_counters
         where counter_name = 'Page life expectancy'
           and object_name like '%Buffer Manager%')           as page_life_expectancy_s,
       i.sqlserver_start_time                                 as server_started
  from sys.dm_os_sys_info i
 cross join sys.dm_os_process_memory m;
