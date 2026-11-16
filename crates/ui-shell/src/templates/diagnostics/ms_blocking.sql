-- Who blocks whom, as a tree: each session at the left margin holds a lock
-- and is not waiting for anyone; the sessions under it wait for it.
-- No rows = nobody is blocked.
with sessions as (
    select s.session_id,
           isnull(r.blocking_session_id, 0)                   as blocked_by,
           s.status,
           r.command,
           r.wait_type,
           r.wait_time,
           r.wait_resource,
           s.open_transaction_count,
           db_name(isnull(r.database_id, s.database_id))      as database_name,
           s.login_name,
           s.host_name,
           s.program_name,
           isnull(r.sql_handle, c.most_recent_sql_handle)     as sql_handle
      from sys.dm_exec_sessions s
      left join sys.dm_exec_requests r on r.session_id = s.session_id
      left join sys.dm_exec_connections c
        on c.session_id = s.session_id and c.parent_connection_id is null
),
tree as (
    select s.session_id, 0 as level,
           cast(right('00000' + cast(s.session_id as varchar(10)), 6) as varchar(900)) as path
      from sessions s
     where s.blocked_by = 0
       and exists (select 1 from sessions w where w.blocked_by = s.session_id)
    union all
    select s.session_id, t.level + 1,
           cast(t.path + '/' + right('00000' + cast(s.session_id as varchar(10)), 6) as varchar(900))
      from tree t
      join sessions s on s.blocked_by = t.session_id
     where s.session_id <> s.blocked_by
)
select replicate(N'      ', t.level)
       + case when t.level > 0 then N'└─ ' else N'' end
       + cast(t.session_id as nvarchar(10))                   as session,
       case when t.level = 0 then 'BLOCKING' else 'blocked' end as role,
       nullif(s.blocked_by, 0)                                as blocked_by,
       (select count(*) from tree x
         where x.path like t.path + '/%')                     as sessions_waiting_behind,
       s.status,
       s.command,
       case when t.level > 0 then s.wait_time end             as waiting_ms,
       s.wait_type,
       s.wait_resource,
       s.open_transaction_count                               as open_transactions,
       s.database_name                                        as [database],
       s.login_name,
       s.host_name,
       s.program_name,
       q.text                                                 as last_batch
  from tree t
  join sessions s on s.session_id = t.session_id
 outer apply sys.dm_exec_sql_text(s.sql_handle) q
 order by t.path;
