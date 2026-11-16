-- The history of executed queries (total time, average, calls, reads) comes
-- from the pg_stat_statements extension, which is not installed in this
-- database. To turn it on:
--   1. postgresql.conf: shared_preload_libraries = 'pg_stat_statements'
--      (needs a server restart; managed services have it in the parameters)
--   2. in this database: create extension pg_stat_statements;
select 'pg_stat_statements is not installed in this database' as query_history,
       'add it to shared_preload_libraries, restart, then: create extension pg_stat_statements;' as how_to_turn_on;
