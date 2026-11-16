-- Who is connected: connections by database, user, application and client,
-- with how many are running, idle or holding a transaction open.
select a.datname                                              as database,
       a.usename                                              as "user",
       a.application_name                                     as application,
       a.client_addr                                          as client,
       count(*)                                               as connections,
       count(*) filter (where a.state = 'active')             as running,
       count(*) filter (where a.state = 'idle')               as idle,
       count(*) filter (where a.state like 'idle in transaction%') as idle_in_transaction,
       date_trunc('seconds', max(clock_timestamp() - a.backend_start)) as oldest_connection
  from pg_stat_activity a
 where a.backend_type = 'client backend'
 group by a.datname, a.usename, a.application_name, a.client_addr
 order by connections desc;
