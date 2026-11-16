-- Open transactions, the oldest first. A long or forgotten one (session
-- "sleeping" with a transaction open) holds locks and keeps the log from
-- being reused.
select st.session_id,
       s.status,
       datediff(second, t.transaction_begin_time, getdate())  as transaction_s,
       case when s.status = 'sleeping'
            then datediff(second, s.last_request_end_time, getdate())
       end                                                    as idle_s,
       (select count(*) from sys.dm_exec_requests w
         where w.blocking_session_id = st.session_id)         as sessions_blocked,
       d.databases,
       d.log_used_kb,
       s.login_name,
       s.host_name,
       s.program_name,
       q.text                                                 as last_batch
  from sys.dm_tran_session_transactions st
  join sys.dm_tran_active_transactions t on t.transaction_id = st.transaction_id
  join sys.dm_exec_sessions s on s.session_id = st.session_id
  left join sys.dm_exec_connections c
    on c.session_id = st.session_id and c.parent_connection_id is null
 outer apply sys.dm_exec_sql_text(c.most_recent_sql_handle) q
 outer apply (
        select count(*) as databases,
               sum(dt.database_transaction_log_bytes_used) / 1024 as log_used_kb
          from sys.dm_tran_database_transactions dt
         where dt.transaction_id = st.transaction_id
           and dt.database_id <> 32767
       ) d
 where st.session_id <> @@spid
 order by t.transaction_begin_time;
