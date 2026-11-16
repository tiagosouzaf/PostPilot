-- Who is connected: connections by database, login, application and
-- client, with how many are running or holding a transaction open.
select db_name(s.database_id)                                 as [database],
       s.login_name,
       s.program_name,
       s.host_name,
       count(*)                                               as connections,
       sum(case when s.status = 'running' then 1 else 0 end)  as running,
       sum(case when s.status = 'sleeping' then 1 else 0 end) as sleeping,
       sum(case when s.open_transaction_count > 0 then 1 else 0 end) as with_open_transaction,
       sum(s.cpu_time)                                        as cpu_ms,
       sum(s.memory_usage) * 8                                as memory_kb,
       min(s.login_time)                                      as oldest_connection
  from sys.dm_exec_sessions s
 where s.is_user_process = 1
 group by s.database_id, s.login_name, s.program_name, s.host_name
 order by connections desc;
