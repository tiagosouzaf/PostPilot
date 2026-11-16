//! Os modelos que vêm com o PostPilot, por servidor e por pasta.
//!
//! Pasta e nome são chaves de tradução (`tr_noop`); o SQL fica igual em
//! qualquer idioma. Os nomes a trocar vão em minúsculas com sublinhado
//! (`table_name`, `column_name`), fáceis de achar com duplo clique.

use pgstudio_core::connection::Engine;
use pgstudio_i18n::tr_noop;

/// Um modelo embutido.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub engine: Engine,
    /// Pasta, em inglês (a chave da tradução).
    pub folder: &'static str,
    /// Nome, em inglês (a chave da tradução).
    pub name: &'static str,
    pub sql: &'static str,
}

const fn pg(folder: &'static str, name: &'static str, sql: &'static str) -> Builtin {
    Builtin {
        engine: Engine::PostgreSql,
        folder,
        name,
        sql,
    }
}

const fn ms(folder: &'static str, name: &'static str, sql: &'static str) -> Builtin {
    Builtin {
        engine: Engine::SqlServer,
        folder,
        name,
        sql,
    }
}

const fn rd(folder: &'static str, name: &'static str, sql: &'static str) -> Builtin {
    Builtin {
        engine: Engine::Redis,
        folder,
        name,
        sql,
    }
}

/// Todos os modelos, na ordem em que aparecem.
pub const ALL: &[Builtin] = &[
    // ---------------------------------------------------------------
    // PostgreSQL
    // ---------------------------------------------------------------
    pg(
        tr_noop("Tables"),
        tr_noop("Create Table"),
        "create table schema_name.table_name (
    id          bigint generated always as identity primary key,
    name        text not null,
    created_at  timestamptz not null default now()
);

comment on table schema_name.table_name is 'description';
",
    ),
    pg(
        tr_noop("Tables"),
        tr_noop("Add Column"),
        "alter table schema_name.table_name
    add column column_name text;
",
    ),
    pg(
        tr_noop("Tables"),
        tr_noop("Add Foreign Key"),
        "alter table schema_name.table_name
    add constraint fk_table_name_other_table
    foreign key (other_id) references schema_name.other_table (id);
",
    ),
    pg(
        tr_noop("Tables"),
        tr_noop("Create Partitioned Table"),
        "create table schema_name.table_name (
    id          bigint generated always as identity,
    created_at  timestamptz not null,
    payload     jsonb,
    primary key (id, created_at)
) partition by range (created_at);

create table schema_name.table_name_2026_01
    partition of schema_name.table_name
    for values from ('2026-01-01') to ('2026-02-01');
",
    ),
    pg(
        tr_noop("Indexes"),
        tr_noop("Create Index"),
        "-- concurrently: does not block writes (cannot run inside a transaction)
create index concurrently if not exists ix_table_name_column_name
    on schema_name.table_name (column_name);
",
    ),
    pg(
        tr_noop("Indexes"),
        tr_noop("Unused Indexes"),
        "select s.schemaname, s.relname as table_name, s.indexrelname as index_name,
       pg_size_pretty(pg_relation_size(s.indexrelid)) as size, s.idx_scan
  from pg_stat_user_indexes s
  join pg_index i on i.indexrelid = s.indexrelid
 where s.idx_scan = 0
   and not i.indisunique
 order by pg_relation_size(s.indexrelid) desc;
",
    ),
    pg(
        tr_noop("Views"),
        tr_noop("Create View"),
        "create or replace view schema_name.view_name as
select t.id, t.name
  from schema_name.table_name t;
",
    ),
    pg(
        tr_noop("Views"),
        tr_noop("Create Materialized View"),
        "create materialized view schema_name.view_name as
select t.id, count(*) as total
  from schema_name.table_name t
 group by t.id
with no data;

create unique index on schema_name.view_name (id);

refresh materialized view concurrently schema_name.view_name;
",
    ),
    pg(
        tr_noop("Programmability"),
        tr_noop("Create Function"),
        "create or replace function schema_name.function_name(p_id bigint)
returns text
language plpgsql
stable
as $$
declare
    v_name text;
begin
    select t.name into v_name
      from schema_name.table_name t
     where t.id = p_id;

    return v_name;
end;
$$;
",
    ),
    pg(
        tr_noop("Programmability"),
        tr_noop("Create Procedure"),
        "create or replace procedure schema_name.procedure_name(p_id bigint)
language plpgsql
as $$
begin
    update schema_name.table_name
       set name = name
     where id = p_id;
end;
$$;

call schema_name.procedure_name(1);
",
    ),
    pg(
        tr_noop("Programmability"),
        tr_noop("Create Trigger"),
        "create or replace function schema_name.table_name_set_updated_at()
returns trigger
language plpgsql
as $$
begin
    new.updated_at := now();
    return new;
end;
$$;

create trigger trg_table_name_set_updated_at
    before update on schema_name.table_name
    for each row
    execute function schema_name.table_name_set_updated_at();
",
    ),
    pg(
        tr_noop("Security"),
        tr_noop("Create Login and User"),
        "create role user_name login password 'change_me';

grant connect on database database_name to user_name;
grant usage on schema schema_name to user_name;
",
    ),
    pg(
        tr_noop("Security"),
        tr_noop("Grant Read-Only Access"),
        "grant usage on schema schema_name to user_name;
grant select on all tables in schema schema_name to user_name;

-- tables created from now on
alter default privileges in schema schema_name
    grant select on tables to user_name;
",
    ),
    pg(
        tr_noop("Queries"),
        tr_noop("Common Table Expression (CTE)"),
        "with recent as (
    select t.*
      from schema_name.table_name t
     where t.created_at >= now() - interval '7 days'
)
select *
  from recent
 order by created_at desc;
",
    ),
    pg(
        tr_noop("Queries"),
        tr_noop("Insert or Update (Upsert)"),
        "insert into schema_name.table_name (id, name)
values (1, 'name')
on conflict (id) do update
   set name = excluded.name;
",
    ),
    pg(
        tr_noop("Queries"),
        tr_noop("Find Duplicate Rows"),
        "select column_name, count(*) as total
  from schema_name.table_name
 group by column_name
having count(*) > 1
 order by total desc;
",
    ),
    pg(
        tr_noop("Queries"),
        tr_noop("Paging"),
        "select *
  from schema_name.table_name
 order by id
 limit 50 offset 0;
",
    ),
    pg(
        tr_noop("Queries"),
        tr_noop("Update From Another Table"),
        "update schema_name.table_name t
   set name = o.name
  from schema_name.other_table o
 where o.id = t.other_id;
",
    ),
    pg(
        tr_noop("Queries"),
        tr_noop("Delete in Batches"),
        "-- run again until it deletes 0 rows
with batch as (
    select ctid
      from schema_name.table_name
     where created_at < now() - interval '90 days'
     limit 10000
)
delete from schema_name.table_name t
 using batch
 where t.ctid = batch.ctid;
",
    ),
    pg(
        tr_noop("Queries"),
        tr_noop("Change Inside a Transaction"),
        "begin;

update schema_name.table_name
   set name = 'new name'
 where id = 1
returning *;

-- check the rows above, then run one of the two
-- commit;
-- rollback;
",
    ),
    pg(
        tr_noop("Monitoring"),
        tr_noop("Running Queries"),
        "select pid, usename, datname, state, wait_event_type, wait_event,
       now() - query_start as running_for, query
  from pg_stat_activity
 where state <> 'idle'
   and pid <> pg_backend_pid()
 order by query_start;
",
    ),
    pg(
        tr_noop("Monitoring"),
        tr_noop("Blocked Sessions"),
        "select blocked.pid as blocked_pid, blocked.usename as blocked_user,
       blocking.pid as blocking_pid, blocking.usename as blocking_user,
       now() - blocked.query_start as waiting_for,
       blocked.query as blocked_query, blocking.query as blocking_query
  from pg_stat_activity blocked
  join pg_stat_activity blocking
    on blocking.pid = any (pg_blocking_pids(blocked.pid))
 order by waiting_for desc;
",
    ),
    pg(
        tr_noop("Monitoring"),
        tr_noop("End a Session"),
        "-- cancel only the running query
select pg_cancel_backend(0 /* pid */);

-- end the whole session
select pg_terminate_backend(0 /* pid */);
",
    ),
    pg(
        tr_noop("Monitoring"),
        tr_noop("Table Sizes"),
        "select n.nspname as schema_name, c.relname as table_name,
       c.reltuples::bigint as rows_estimate,
       pg_size_pretty(pg_table_size(c.oid)) as data,
       pg_size_pretty(pg_indexes_size(c.oid)) as indexes,
       pg_size_pretty(pg_total_relation_size(c.oid)) as total
  from pg_class c
  join pg_namespace n on n.oid = c.relnamespace
 where c.relkind in ('r', 'm', 'p')
   and n.nspname not in ('pg_catalog', 'information_schema')
 order by pg_total_relation_size(c.oid) desc
 limit 50;
",
    ),
    pg(
        tr_noop("Monitoring"),
        tr_noop("Database Sizes"),
        "select datname, pg_size_pretty(pg_database_size(datname)) as size
  from pg_database
 where not datistemplate
 order by pg_database_size(datname) desc;
",
    ),
    pg(
        tr_noop("Monitoring"),
        tr_noop("Vacuum and Dead Rows"),
        "select schemaname, relname, n_live_tup, n_dead_tup,
       last_vacuum, last_autovacuum, last_analyze, last_autoanalyze
  from pg_stat_user_tables
 order by n_dead_tup desc
 limit 50;
",
    ),
    // ---------------------------------------------------------------
    // SQL Server
    // ---------------------------------------------------------------
    ms(
        tr_noop("Tables"),
        tr_noop("Create Table"),
        "CREATE TABLE schema_name.table_name (
    id          BIGINT IDENTITY(1, 1) NOT NULL,
    name        NVARCHAR(200) NOT NULL,
    created_at  DATETIME2(3) NOT NULL
        CONSTRAINT df_table_name_created_at DEFAULT SYSUTCDATETIME(),
    CONSTRAINT pk_table_name PRIMARY KEY CLUSTERED (id)
);
GO
",
    ),
    ms(
        tr_noop("Tables"),
        tr_noop("Add Column"),
        "ALTER TABLE schema_name.table_name
    ADD column_name NVARCHAR(200) NULL;
GO
",
    ),
    ms(
        tr_noop("Tables"),
        tr_noop("Add Foreign Key"),
        "ALTER TABLE schema_name.table_name
    ADD CONSTRAINT fk_table_name_other_table
    FOREIGN KEY (other_id) REFERENCES schema_name.other_table (id);
GO
",
    ),
    ms(
        tr_noop("Indexes"),
        tr_noop("Create Index"),
        "CREATE NONCLUSTERED INDEX ix_table_name_column_name
    ON schema_name.table_name (column_name)
    INCLUDE (name);
GO
",
    ),
    ms(
        tr_noop("Indexes"),
        tr_noop("Missing Indexes"),
        "SELECT TOP (25)
       DB_NAME(d.database_id) AS database_name, d.statement AS table_name,
       d.equality_columns, d.inequality_columns, d.included_columns,
       s.user_seeks, s.avg_user_impact,
       s.user_seeks * s.avg_total_user_cost * s.avg_user_impact AS score
  FROM sys.dm_db_missing_index_details d
  JOIN sys.dm_db_missing_index_groups g ON g.index_handle = d.index_handle
  JOIN sys.dm_db_missing_index_group_stats s ON s.group_handle = g.index_group_handle
 WHERE d.database_id = DB_ID()
 ORDER BY score DESC;
",
    ),
    ms(
        tr_noop("Indexes"),
        tr_noop("Index Fragmentation"),
        "SELECT OBJECT_SCHEMA_NAME(s.object_id) AS schema_name,
       OBJECT_NAME(s.object_id) AS table_name, i.name AS index_name,
       s.avg_fragmentation_in_percent, s.page_count
  FROM sys.dm_db_index_physical_stats(DB_ID(), NULL, NULL, NULL, 'LIMITED') s
  JOIN sys.indexes i ON i.object_id = s.object_id AND i.index_id = s.index_id
 WHERE s.page_count > 1000
   AND s.avg_fragmentation_in_percent > 10
 ORDER BY s.avg_fragmentation_in_percent DESC;

-- ALTER INDEX index_name ON schema_name.table_name REORGANIZE;
-- ALTER INDEX index_name ON schema_name.table_name REBUILD;
",
    ),
    ms(
        tr_noop("Views"),
        tr_noop("Create View"),
        "CREATE OR ALTER VIEW schema_name.view_name
AS
SELECT t.id, t.name
  FROM schema_name.table_name AS t;
GO
",
    ),
    ms(
        tr_noop("Programmability"),
        tr_noop("Create Procedure"),
        "CREATE OR ALTER PROCEDURE schema_name.procedure_name
    @id BIGINT
AS
BEGIN
    SET NOCOUNT ON;

    SELECT t.id, t.name
      FROM schema_name.table_name AS t
     WHERE t.id = @id;
END;
GO

EXEC schema_name.procedure_name @id = 1;
",
    ),
    ms(
        tr_noop("Programmability"),
        tr_noop("Create Function"),
        "CREATE OR ALTER FUNCTION schema_name.function_name (@id BIGINT)
RETURNS TABLE
AS
RETURN
    SELECT t.id, t.name
      FROM schema_name.table_name AS t
     WHERE t.id = @id;
GO

SELECT * FROM schema_name.function_name(1);
",
    ),
    ms(
        tr_noop("Programmability"),
        tr_noop("Create Trigger"),
        "CREATE OR ALTER TRIGGER schema_name.trg_table_name_update
    ON schema_name.table_name
    AFTER UPDATE
AS
BEGIN
    SET NOCOUNT ON;

    UPDATE t
       SET updated_at = SYSUTCDATETIME()
      FROM schema_name.table_name AS t
      JOIN inserted AS i ON i.id = t.id;
END;
GO
",
    ),
    ms(
        tr_noop("Security"),
        tr_noop("Create Login and User"),
        "CREATE LOGIN login_name WITH PASSWORD = N'change_me';
GO

USE database_name;
GO

CREATE USER user_name FOR LOGIN login_name;
GO
",
    ),
    ms(
        tr_noop("Security"),
        tr_noop("Grant Read-Only Access"),
        "ALTER ROLE db_datareader ADD MEMBER user_name;
GO

-- or only one schema
-- GRANT SELECT ON SCHEMA::schema_name TO user_name;
",
    ),
    ms(
        tr_noop("Queries"),
        tr_noop("Common Table Expression (CTE)"),
        "WITH recent AS (
    SELECT t.*
      FROM schema_name.table_name AS t
     WHERE t.created_at >= DATEADD(DAY, -7, SYSUTCDATETIME())
)
SELECT *
  FROM recent
 ORDER BY created_at DESC;
",
    ),
    ms(
        tr_noop("Queries"),
        tr_noop("Insert or Update (Upsert)"),
        "MERGE schema_name.table_name AS target
USING (VALUES (1, N'name')) AS source (id, name)
   ON target.id = source.id
 WHEN MATCHED THEN
      UPDATE SET name = source.name
 WHEN NOT MATCHED THEN
      INSERT (id, name) VALUES (source.id, source.name);
",
    ),
    ms(
        tr_noop("Queries"),
        tr_noop("Find Duplicate Rows"),
        "SELECT column_name, COUNT(*) AS total
  FROM schema_name.table_name
 GROUP BY column_name
HAVING COUNT(*) > 1
 ORDER BY total DESC;
",
    ),
    ms(
        tr_noop("Queries"),
        tr_noop("Paging"),
        "SELECT *
  FROM schema_name.table_name
 ORDER BY id
OFFSET 0 ROWS FETCH NEXT 50 ROWS ONLY;
",
    ),
    ms(
        tr_noop("Queries"),
        tr_noop("Update From Another Table"),
        "UPDATE t
   SET t.name = o.name
  FROM schema_name.table_name AS t
  JOIN schema_name.other_table AS o ON o.id = t.other_id;
",
    ),
    ms(
        tr_noop("Queries"),
        tr_noop("Delete in Batches"),
        "DECLARE @rows INT = 1;

WHILE @rows > 0
BEGIN
    DELETE TOP (10000)
      FROM schema_name.table_name
     WHERE created_at < DATEADD(DAY, -90, SYSUTCDATETIME());

    SET @rows = @@ROWCOUNT;
END;
",
    ),
    ms(
        tr_noop("Queries"),
        tr_noop("Change Inside a Transaction"),
        "BEGIN TRY
    BEGIN TRANSACTION;

    UPDATE schema_name.table_name
       SET name = N'new name'
     WHERE id = 1;

    COMMIT TRANSACTION;
END TRY
BEGIN CATCH
    IF @@TRANCOUNT > 0
        ROLLBACK TRANSACTION;
    THROW;
END CATCH;
",
    ),
    ms(
        tr_noop("Monitoring"),
        tr_noop("Running Queries"),
        "SELECT r.session_id, s.login_name, DB_NAME(r.database_id) AS database_name,
       r.status, r.wait_type, r.blocking_session_id,
       r.total_elapsed_time / 1000 AS seconds, t.text AS query
  FROM sys.dm_exec_requests AS r
  JOIN sys.dm_exec_sessions AS s ON s.session_id = r.session_id
 CROSS APPLY sys.dm_exec_sql_text(r.sql_handle) AS t
 WHERE r.session_id <> @@SPID
 ORDER BY r.total_elapsed_time DESC;
",
    ),
    ms(
        tr_noop("Monitoring"),
        tr_noop("Blocked Sessions"),
        "SELECT r.session_id AS blocked_session, r.blocking_session_id AS blocking_session,
       r.wait_type, r.wait_time / 1000 AS waiting_seconds,
       blocked.text AS blocked_query, blocking.text AS blocking_query
  FROM sys.dm_exec_requests AS r
  LEFT JOIN sys.dm_exec_connections AS c ON c.session_id = r.blocking_session_id
 CROSS APPLY sys.dm_exec_sql_text(r.sql_handle) AS blocked
 OUTER APPLY sys.dm_exec_sql_text(c.most_recent_sql_handle) AS blocking
 WHERE r.blocking_session_id <> 0
 ORDER BY r.wait_time DESC;
",
    ),
    ms(
        tr_noop("Monitoring"),
        tr_noop("End a Session"),
        "KILL 0 /* session_id */;
",
    ),
    ms(
        tr_noop("Monitoring"),
        tr_noop("Table Sizes"),
        "SELECT TOP (50)
       s.name AS schema_name, t.name AS table_name,
       SUM(CASE WHEN p.index_id IN (0, 1) THEN p.row_count ELSE 0 END) AS rows,
       SUM(p.reserved_page_count) * 8 / 1024 AS reserved_mb,
       SUM(p.used_page_count) * 8 / 1024 AS used_mb
  FROM sys.dm_db_partition_stats AS p
  JOIN sys.tables AS t ON t.object_id = p.object_id
  JOIN sys.schemas AS s ON s.schema_id = t.schema_id
 GROUP BY s.name, t.name
 ORDER BY reserved_mb DESC;
",
    ),
    ms(
        tr_noop("Monitoring"),
        tr_noop("Database Sizes"),
        "SELECT DB_NAME(database_id) AS database_name,
       SUM(CASE WHEN type = 0 THEN size END) * 8 / 1024 AS data_mb,
       SUM(CASE WHEN type = 1 THEN size END) * 8 / 1024 AS log_mb
  FROM sys.master_files
 GROUP BY database_id
 ORDER BY data_mb DESC;
",
    ),
    ms(
        tr_noop("Backup and Restore"),
        tr_noop("Back Up Database"),
        "BACKUP DATABASE database_name
    TO DISK = N'/var/opt/mssql/backup/database_name.bak'
    WITH COMPRESSION, CHECKSUM, STATS = 10;
",
    ),
    ms(
        tr_noop("Backup and Restore"),
        tr_noop("Restore Database"),
        "RESTORE FILELISTONLY
    FROM DISK = N'/var/opt/mssql/backup/database_name.bak';

RESTORE DATABASE database_name
    FROM DISK = N'/var/opt/mssql/backup/database_name.bak'
    WITH REPLACE, STATS = 10;
",
    ),
    ms(
        tr_noop("Backup and Restore"),
        tr_noop("Last Backups"),
        "SELECT d.name AS database_name,
       MAX(CASE WHEN b.type = 'D' THEN b.backup_finish_date END) AS last_full,
       MAX(CASE WHEN b.type = 'I' THEN b.backup_finish_date END) AS last_differential,
       MAX(CASE WHEN b.type = 'L' THEN b.backup_finish_date END) AS last_log
  FROM sys.databases AS d
  LEFT JOIN msdb.dbo.backupset AS b ON b.database_name = d.name
 GROUP BY d.name
 ORDER BY d.name;
",
    ),
    // ---------------------------------------------------------------
    // Redis
    // ---------------------------------------------------------------
    rd(
        tr_noop("Keys"),
        tr_noop("Find Keys by Pattern"),
        "# Repita com o cursor devolvido até ele voltar a 0.
SCAN 0 MATCH prefix:* COUNT 1000
",
    ),
    rd(
        tr_noop("Keys"),
        tr_noop("Inspect Key"),
        "TYPE key_name
TTL key_name
OBJECT ENCODING key_name
MEMORY USAGE key_name
",
    ),
    rd(
        tr_noop("Keys"),
        tr_noop("Set Expiration"),
        "EXPIRE key_name 3600
TTL key_name
",
    ),
    rd(
        tr_noop("Keys"),
        tr_noop("Delete Keys"),
        "# UNLINK libera a memória em segundo plano.
UNLINK key_name other_key
",
    ),
    rd(
        tr_noop("Strings"),
        tr_noop("Set with Expiration"),
        "SET key_name \"value\" EX 3600
GET key_name
",
    ),
    rd(
        tr_noop("Strings"),
        tr_noop("Counter"),
        "INCR counter_name
INCRBY counter_name 10
GET counter_name
",
    ),
    rd(
        tr_noop("Hashes"),
        tr_noop("Create Hash"),
        "HSET key_name field_name \"value\" other_field \"other value\"
HGETALL key_name
",
    ),
    rd(
        tr_noop("Hashes"),
        tr_noop("Read Fields"),
        "HGET key_name field_name
HMGET key_name field_name other_field
HLEN key_name
",
    ),
    rd(
        tr_noop("Lists and Sets"),
        tr_noop("Queue"),
        "RPUSH queue_name \"item\"
LRANGE queue_name 0 99
LPOP queue_name
LLEN queue_name
",
    ),
    rd(
        tr_noop("Lists and Sets"),
        tr_noop("Set Members"),
        "SADD set_name member_one member_two
SMEMBERS set_name
SISMEMBER set_name member_one
SCARD set_name
",
    ),
    rd(
        tr_noop("Sorted Sets"),
        tr_noop("Ranking"),
        "ZADD ranking_name 100 member_one 80 member_two
ZRANGE ranking_name 0 9 REV WITHSCORES
ZRANK ranking_name member_one
",
    ),
    rd(
        tr_noop("Server"),
        tr_noop("Server Information"),
        "INFO server
INFO memory
INFO keyspace
DBSIZE
",
    ),
    rd(
        tr_noop("Server"),
        tr_noop("Clients and Slow Commands"),
        "CLIENT LIST
SLOWLOG GET 20
",
    ),
    rd(
        tr_noop("Server"),
        tr_noop("Listen to a Channel"),
        "# Cancele a consulta para parar de escutar.
SUBSCRIBE channel_name
",
    ),
];

/// As pastas de um servidor, na ordem em que aparecem.
pub fn folders(engine: Engine) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for t in ALL.iter().filter(|t| t.engine == engine) {
        if !out.contains(&t.folder) {
            out.push(t.folder);
        }
    }
    out
}

/// Os modelos de uma pasta de um servidor.
pub fn in_folder(engine: Engine, folder: &str) -> impl Iterator<Item = &'static Builtin> {
    ALL.iter()
        .filter(move |t| t.engine == engine && t.folder == folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_has_templates_with_unique_names_per_folder() {
        for engine in Engine::ALL {
            let folders = folders(engine);
            assert!(folders.len() >= 5, "{engine:?}");
            for folder in folders {
                let names: Vec<_> = in_folder(engine, folder).map(|t| t.name).collect();
                assert!(!names.is_empty());
                let mut unique = names.clone();
                unique.sort_unstable();
                unique.dedup();
                assert_eq!(unique.len(), names.len(), "{engine:?}/{folder}");
            }
        }
    }

    #[test]
    fn every_template_ends_with_a_line_break_and_has_no_tabs() {
        for t in ALL {
            assert!(t.sql.ends_with('\n'), "{}", t.name);
            assert!(!t.sql.contains('\t'), "{}", t.name);
            assert!(!t.sql.trim().is_empty(), "{}", t.name);
        }
    }
}
