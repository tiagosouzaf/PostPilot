-- What the server has waited on the most since it started (idle and
-- background waits left out): locks (LCK_), disk (PAGEIOLATCH_, WRITELOG),
-- CPU pressure (SOS_SCHEDULER_YIELD), parallelism (CXPACKET), memory
-- (RESOURCE_SEMAPHORE).
select top (25)
       w.wait_type,
       w.wait_time_ms / 1000                                  as wait_s,
       cast(100.0 * w.wait_time_ms
            / nullif(sum(w.wait_time_ms) over (), 0) as decimal(5, 1)) as pct_of_waits,
       w.waiting_tasks_count                                  as waits,
       cast(w.wait_time_ms * 1.0
            / nullif(w.waiting_tasks_count, 0) as decimal(18, 2)) as avg_wait_ms,
       w.max_wait_time_ms                                     as max_wait_ms,
       w.signal_wait_time_ms / 1000                           as waiting_for_cpu_s
  from sys.dm_os_wait_stats w
 where w.wait_time_ms > 0
   and w.wait_type not like 'SLEEP[_]%'
   and w.wait_type not like 'BROKER[_]%'
   and w.wait_type not like 'XE[_]%'
   and w.wait_type not like 'HADR[_]%'
   and w.wait_type not like 'QDS[_]%'
   and w.wait_type not like 'SQLTRACE[_]%'
   and w.wait_type not like 'FT[_]%'
   and w.wait_type not like 'PREEMPTIVE[_]%'
   and w.wait_type not like 'PWAIT[_]%'
   and w.wait_type not like '%[_]QUEUE'
   and w.wait_type not like 'DISPATCHER[_]%'
   and w.wait_type not in (
           'LAZYWRITER_SLEEP', 'CHECKPOINT_QUEUE', 'REQUEST_FOR_DEADLOCK_SEARCH',
           'LOGMGR_QUEUE', 'WAITFOR', 'CLR_AUTO_EVENT', 'CLR_MANUAL_EVENT',
           'CLR_SEMAPHORE', 'DIRTY_PAGE_POLL', 'ONDEMAND_TASK_QUEUE',
           'SP_SERVER_DIAGNOSTICS_SLEEP', 'SERVER_IDLE_CHECK', 'WAIT_XTP_HOST_WAIT',
           'WAIT_XTP_OFFLINE_CKPT_NEW_LOG', 'WAIT_XTP_CKPT_CLOSE', 'XTP_PREEMPTIVE_TASK',
           'RESOURCE_QUEUE', 'KSOURCE_WAKEUP', 'DBMIRROR_EVENTS_QUEUE',
           'DBMIRRORING_CMD', 'DBMIRROR_DBM_EVENT', 'DBMIRROR_WORKER_QUEUE',
           'SNI_HTTP_ACCEPT', 'CHKPT', 'EXECSYNC', 'FSAGENT', 'MEMORY_ALLOCATION_EXT',
           'PARALLEL_REDO_DRAIN_WORKER', 'PARALLEL_REDO_LOG_CACHE',
           'PARALLEL_REDO_TRAN_LIST', 'PARALLEL_REDO_WORKER_SYNC',
           'PARALLEL_REDO_WORKER_WAIT_WORK', 'SOS_WORK_DISPATCHER',
           'STARTUP_DEPENDENCY_MANAGER', 'VDI_CLIENT_OTHER', 'UCS_SESSION_REGISTRATION',
           'WAIT_FOR_RESULTS', 'WAITFOR_TASKSHUTDOWN', 'REDO_THREAD_PENDING_WORK')
 order by w.wait_time_ms desc;
