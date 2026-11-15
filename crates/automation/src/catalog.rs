//! Consultas ao catálogo do PostgreSQL das ferramentas de leitura
//! (`list_databases`, `list_objects`, `describe_table`,
//! `describe_function`). Rodam como qualquer leitura do Claude: protocolo
//! simples, dentro da transação READ ONLY, então os nomes entram como
//! literais escapados.

/// Literal SQL com aspas simples dobradas (`standard_conforming_strings`
/// ligado, o padrão desde o 9.1).
pub fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Bancos em que o usuário pode conectar.
pub fn list_databases() -> String {
    "select d.datname as database,
       pg_catalog.pg_get_userbyid(d.datdba) as owner,
       pg_catalog.pg_encoding_to_char(d.encoding) as encoding,
       pg_catalog.pg_size_pretty(pg_catalog.pg_database_size(d.datname)) as size,
       pg_catalog.shobj_description(d.oid, 'pg_database') as comment
from pg_catalog.pg_database d
where not d.datistemplate and pg_catalog.has_database_privilege(d.datname, 'CONNECT')
order by 1"
        .into()
}

/// Tabelas, views, sequências e funções dos schemas do usuário. Com
/// `schema`, só os desse schema.
pub fn list_objects(schema: Option<&str>) -> String {
    let filter = |col: &str| match schema {
        Some(s) => format!("and {col} = {}", literal(s)),
        None => String::new(),
    };
    format!(
        "select * from (
  select n.nspname as schema, c.relname as name,
         case c.relkind when 'r' then 'table' when 'p' then 'partitioned table'
           when 'v' then 'view' when 'm' then 'materialized view'
           when 'f' then 'foreign table' when 'S' then 'sequence' end as kind,
         case when c.relkind in ('r','p','m','f') then greatest(c.reltuples, 0)::bigint end as estimated_rows,
         pg_catalog.obj_description(c.oid, 'pg_class') as comment
  from pg_catalog.pg_class c join pg_catalog.pg_namespace n on n.oid = c.relnamespace
  where c.relkind in ('r','p','v','m','f','S') and not c.relispartition
    and n.nspname not in ('pg_catalog', 'information_schema')
    and n.nspname !~ '^pg_(toast|temp)' {rel}
  union all
  select n.nspname, p.proname || '(' || pg_catalog.pg_get_function_identity_arguments(p.oid) || ')',
         case p.prokind when 'p' then 'procedure' when 'a' then 'aggregate' else 'function' end,
         null, pg_catalog.obj_description(p.oid, 'pg_proc')
  from pg_catalog.pg_proc p join pg_catalog.pg_namespace n on n.oid = p.pronamespace
  where n.nspname not in ('pg_catalog', 'information_schema')
    and n.nspname !~ '^pg_(toast|temp)' {fun}
    and not exists (select 1 from pg_catalog.pg_depend d
                    where d.classid = 'pg_catalog.pg_proc'::regclass and d.objid = p.oid and d.deptype = 'e')
) o
order by schema, kind, name",
        rel = filter("n.nspname"),
        fun = filter("n.nspname"),
    )
}

/// Nomes dos resultados de [`describe_table`].
pub const DESCRIBE_TABLE_LABELS: &[&str] = &[
    "table",
    "columns",
    "constraints",
    "indexes",
    "referenced_by",
];

/// Tabela ou view: colunas, restrições, índices e quem a referencia.
/// `table` é como se escreve no SQL (`pedidos`, `vendas."Itens"`).
pub fn describe_table(table: &str) -> String {
    let t = format!("pg_catalog.to_regclass({})", literal(table));
    format!(
        "select c.oid::regclass::text as table,
       case c.relkind when 'r' then 'table' when 'p' then 'partitioned table'
         when 'v' then 'view' when 'm' then 'materialized view'
         when 'f' then 'foreign table' else c.relkind::text end as kind,
       greatest(c.reltuples, 0)::bigint as estimated_rows,
       pg_catalog.pg_size_pretty(pg_catalog.pg_total_relation_size(c.oid)) as total_size,
       pg_catalog.obj_description(c.oid, 'pg_class') as comment,
       case when c.relkind in ('v','m') then pg_catalog.pg_get_viewdef(c.oid, true) end as view_definition
from pg_catalog.pg_class c where c.oid = {t};
select a.attname as column, pg_catalog.format_type(a.atttypid, a.atttypmod) as type,
       not a.attnotnull as nullable, pg_catalog.pg_get_expr(d.adbin, d.adrelid) as default,
       pg_catalog.col_description(a.attrelid, a.attnum) as comment
from pg_catalog.pg_attribute a
left join pg_catalog.pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
where a.attrelid = {t} and a.attnum > 0 and not a.attisdropped
order by a.attnum;
select conname as constraint, pg_catalog.pg_get_constraintdef(oid, true) as definition
from pg_catalog.pg_constraint where conrelid = {t}
order by case contype when 'p' then 0 when 'u' then 1 when 'f' then 2 else 3 end, conname;
select i.indexrelid::regclass::text as index, pg_catalog.pg_get_indexdef(i.indexrelid) as definition
from pg_catalog.pg_index i where i.indrelid = {t} order by 1;
select conrelid::regclass::text as table, conname as constraint,
       pg_catalog.pg_get_constraintdef(oid, true) as definition
from pg_catalog.pg_constraint where confrelid = {t} and contype = 'f' order by 1, 2;"
    )
}

/// Código de funções e procedures com esse nome (`calcula_total` ou
/// `vendas.calcula_total`; todas as sobrecargas).
pub fn describe_function(name: &str) -> String {
    let (schema, func) = match name.rsplit_once('.') {
        Some((s, f)) => (Some(unquote(s)), unquote(f)),
        None => (None, unquote(name)),
    };
    let schema_filter = match schema {
        Some(s) => format!("and n.nspname = {}", literal(&s)),
        None => "and pg_catalog.pg_function_is_visible(p.oid)".into(),
    };
    format!(
        "select n.nspname || '.' || p.proname || '(' || pg_catalog.pg_get_function_identity_arguments(p.oid) || ')' as function,
       case when p.prokind = 'a' then null else pg_catalog.pg_get_functiondef(p.oid) end as definition,
       pg_catalog.obj_description(p.oid, 'pg_proc') as comment
from pg_catalog.pg_proc p join pg_catalog.pg_namespace n on n.oid = p.pronamespace
where p.proname = {} {schema_filter}
order by 1",
        literal(&func)
    )
}

/// Tira aspas de um identificador (`"Itens"` vira `Itens`); sem aspas,
/// minúsculas, como o PostgreSQL faz.
fn unquote(ident: &str) -> String {
    let ident = ident.trim();
    match ident.strip_prefix('"').and_then(|i| i.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => ident.to_lowercase(),
    }
}

pub mod tsql {
    //! O mesmo para o SQL Server, pelas views `sys.*`. Rodam dentro da
    //! transação que o [`crate::safety::read_only_batch_for`] desfaz.

    /// Literal Unicode com aspas simples dobradas.
    pub fn literal(s: &str) -> String {
        format!("N'{}'", s.replace('\'', "''"))
    }

    /// Bancos que o login abre.
    pub fn list_databases() -> String {
        "select d.name as [database], suser_sname(d.owner_sid) as owner,
       d.collation_name as collation, d.compatibility_level, d.state_desc as state,
       (select cast(cast(sum(cast(f.size as bigint)) * 8 / 1024.0 as decimal(18,1)) as varchar(30)) + ' MB'
          from sys.master_files f where f.database_id = d.database_id) as size
from sys.databases d
where has_dbaccess(d.name) = 1
order by 1"
            .into()
    }

    /// Tabelas, views, sequências, funções e procedures do usuário. Com
    /// `schema`, só os desse schema.
    pub fn list_objects(schema: Option<&str>) -> String {
        let filter = match schema {
            Some(s) => format!("and s.name = {}", literal(s)),
            None => String::new(),
        };
        format!(
            "select s.name as [schema], o.name as name,
       case o.type when 'U' then 'table' when 'V' then 'view' when 'P' then 'procedure'
         when 'SO' then 'sequence' else 'function' end as kind,
       case when o.type = 'U' then (select sum(p.rows) from sys.partitions p
                                     where p.object_id = o.object_id and p.index_id in (0, 1)) end as estimated_rows,
       cast(ep.value as nvarchar(4000)) as comment
from sys.objects o
join sys.schemas s on s.schema_id = o.schema_id
left join sys.extended_properties ep
  on ep.class = 1 and ep.major_id = o.object_id and ep.minor_id = 0 and ep.name = 'MS_Description'
where o.is_ms_shipped = 0 and o.type in ('U', 'V', 'P', 'SO', 'FN', 'IF', 'TF') {filter}
order by 1, 3, 2"
        )
    }

    /// O tipo de uma coluna como o `CREATE TABLE` escreve; `c` é a linha
    /// de `sys.columns` e `t` a de `sys.types`.
    const TYPE: &str = "t.name + case
         when t.name in ('varchar', 'char', 'varbinary', 'binary')
           then '(' + iif(c.max_length = -1, 'max', cast(c.max_length as varchar(10))) + ')'
         when t.name in ('nvarchar', 'nchar')
           then '(' + iif(c.max_length = -1, 'max', cast(c.max_length / 2 as varchar(10))) + ')'
         when t.name in ('decimal', 'numeric')
           then '(' + cast(c.precision as varchar(10)) + ',' + cast(c.scale as varchar(10)) + ')'
         when t.name in ('datetime2', 'time', 'datetimeoffset')
           then '(' + cast(c.scale as varchar(10)) + ')'
         else '' end";

    /// As colunas de uma chave ou índice, em ordem; `{0}` é o filtro em
    /// `sys.index_columns ic`.
    fn key_columns(filter: &str) -> String {
        format!(
            "(select string_agg(cast(col_name(ic.object_id, ic.column_id) as nvarchar(max))
                      + iif(ic.is_descending_key = 1, ' DESC', ''), ', ')
               within group (order by ic.key_ordinal)
          from sys.index_columns ic where {filter} and ic.is_included_column = 0)"
        )
    }

    /// A definição de uma FK (`fk` é a linha de `sys.foreign_keys`).
    const FOREIGN_KEY: &str = "'FOREIGN KEY ('
       + (select string_agg(cast(col_name(k.parent_object_id, k.parent_column_id) as nvarchar(max)), ', ')
                 within group (order by k.constraint_column_id)
            from sys.foreign_key_columns k where k.constraint_object_id = fk.object_id)
       + ') REFERENCES ' + object_schema_name(fk.referenced_object_id) + '.' + object_name(fk.referenced_object_id) + ' ('
       + (select string_agg(cast(col_name(k.referenced_object_id, k.referenced_column_id) as nvarchar(max)), ', ')
                 within group (order by k.constraint_column_id)
            from sys.foreign_key_columns k where k.constraint_object_id = fk.object_id)
       + ')'
       + iif(fk.update_referential_action > 0, ' ON UPDATE ' + replace(fk.update_referential_action_desc, '_', ' '), '')
       + iif(fk.delete_referential_action > 0, ' ON DELETE ' + replace(fk.delete_referential_action_desc, '_', ' '), '')";

    /// Tabela ou view: colunas, restrições, índices e quem a referencia.
    /// `table` é como se escreve no SQL (`pedidos`, `vendas.[Itens]`).
    pub fn describe_table(table: &str) -> String {
        let t = format!("object_id({})", literal(table));
        let key =
            key_columns("ic.object_id = kc.parent_object_id and ic.index_id = kc.unique_index_id");
        let index = key_columns("ic.object_id = i.object_id and ic.index_id = i.index_id");
        format!(
            "select schema_name(o.schema_id) + '.' + o.name as [table],
       case o.type when 'U' then 'table' else 'view' end as kind,
       (select sum(p.rows) from sys.partitions p
         where p.object_id = o.object_id and p.index_id in (0, 1)) as estimated_rows,
       (select cast(cast(sum(au.total_pages) * 8 / 1024.0 as decimal(18,1)) as varchar(30)) + ' MB'
          from sys.partitions p join sys.allocation_units au on au.container_id = p.partition_id
         where p.object_id = o.object_id) as total_size,
       (select cast(ep.value as nvarchar(4000)) from sys.extended_properties ep
         where ep.class = 1 and ep.major_id = o.object_id and ep.minor_id = 0
           and ep.name = 'MS_Description') as comment,
       case when o.type = 'V' then object_definition(o.object_id) end as view_definition
from sys.objects o where o.object_id = {t} and o.type in ('U', 'V');
select c.name as [column], {TYPE} as type, c.is_nullable as nullable,
       dc.definition as [default], c.is_identity as [identity], cc.definition as computed,
       (select cast(ep.value as nvarchar(4000)) from sys.extended_properties ep
         where ep.class = 1 and ep.major_id = c.object_id and ep.minor_id = c.column_id
           and ep.name = 'MS_Description') as comment
from sys.columns c
join sys.types t on t.user_type_id = c.user_type_id
left join sys.default_constraints dc on dc.object_id = c.default_object_id
left join sys.computed_columns cc on cc.object_id = c.object_id and cc.column_id = c.column_id
where c.object_id = {t}
order by c.column_id;
select x.[constraint], x.definition from (
  select kc.name collate database_default as [constraint], 0 + iif(kc.type = 'PK', 0, 1) as position,
         (iif(kc.type = 'PK', 'PRIMARY KEY', 'UNIQUE') + ' (' + {key} + ')') collate database_default as definition
    from sys.key_constraints kc where kc.parent_object_id = {t}
  union all
  select fk.name collate database_default, 2, ({FOREIGN_KEY}) collate database_default
    from sys.foreign_keys fk where fk.parent_object_id = {t}
  union all
  select ck.name collate database_default, 3, ('CHECK ' + ck.definition) collate database_default
    from sys.check_constraints ck where ck.parent_object_id = {t}
) x order by x.position, x.[constraint];
select i.name as [index],
       iif(i.is_unique = 1, 'UNIQUE ', '') + i.type_desc collate database_default + ' (' + {index} + ')'
       + isnull(' INCLUDE (' + (select string_agg(cast(col_name(ic.object_id, ic.column_id) as nvarchar(max)), ', ')
                                 from sys.index_columns ic
                                where ic.object_id = i.object_id and ic.index_id = i.index_id
                                  and ic.is_included_column = 1) + ')', '')
       + isnull(' WHERE ' + i.filter_definition, '') as definition
from sys.indexes i where i.object_id = {t} and i.index_id > 0 and i.is_hypothetical = 0
order by 1;
select object_schema_name(fk.parent_object_id) + '.' + object_name(fk.parent_object_id) as [table],
       fk.name as [constraint], {FOREIGN_KEY} as definition
from sys.foreign_keys fk where fk.referenced_object_id = {t} order by 1, 2;"
        )
    }

    /// Código das funções e procedures com esse nome (`calcula_total` ou
    /// `vendas.calcula_total`).
    pub fn describe_function(name: &str) -> String {
        let (schema, func) = match name.rsplit_once('.') {
            Some((s, f)) => (Some(unquote(s)), unquote(f)),
            None => (None, unquote(name)),
        };
        let schema_filter = match schema {
            Some(s) => format!("and schema_name(o.schema_id) = {}", literal(&s)),
            None => String::new(),
        };
        format!(
            "select schema_name(o.schema_id) + '.' + o.name as [function],
       iif(o.type = 'P', 'procedure', 'function') as kind,
       m.definition,
       (select cast(ep.value as nvarchar(4000)) from sys.extended_properties ep
         where ep.class = 1 and ep.major_id = o.object_id and ep.minor_id = 0
           and ep.name = 'MS_Description') as comment
from sys.objects o join sys.sql_modules m on m.object_id = o.object_id
where o.type in ('P', 'FN', 'IF', 'TF') and o.name = {} {schema_filter}
order by 1",
            literal(&func)
        )
    }

    /// Tira colchetes ou aspas de um identificador (`[Itens]` vira
    /// `Itens`); o nome fica como veio, que o SQL Server compara sem
    /// diferenciar maiúsculas.
    fn unquote(ident: &str) -> String {
        let ident = ident.trim();
        if let Some(inner) = ident.strip_prefix('[').and_then(|i| i.strip_suffix(']')) {
            return inner.replace("]]", "]");
        }
        match ident.strip_prefix('"').and_then(|i| i.strip_suffix('"')) {
            Some(inner) => inner.replace("\"\"", "\""),
            None => ident.to_string(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::safety::classify_for;
        use pgstudio_core::connection::Engine;

        #[test]
        fn names_are_escaped_and_unquoted() {
            assert!(
                list_objects(Some("x'; drop table t; --")).contains("N'x''; drop table t; --'")
            );
            let sql = describe_function("[Vendas].[Total Geral]");
            assert!(
                sql.contains("schema_name(o.schema_id) = N'Vendas'"),
                "{sql}"
            );
            assert!(sql.contains("o.name = N'Total Geral'"));
        }

        #[test]
        fn introspection_is_read_only() {
            for sql in [
                list_databases(),
                list_objects(Some("dbo")),
                describe_table("dbo.t"),
                describe_function("f"),
            ] {
                let safety = classify_for(Engine::SqlServer, &sql);
                assert!(safety.is_read_only(), "{safety:?}\n{sql}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_escaped() {
        assert_eq!(literal("o'brien"), "'o''brien'");
        assert!(list_objects(Some("x'; drop table t; --")).contains("'x''; drop table t; --'"));
    }

    #[test]
    fn function_names() {
        let sql = describe_function("Vendas.\"Total\"");
        assert!(sql.contains("n.nspname = 'vendas'"));
        assert!(sql.contains("p.proname = 'Total'"));
        assert!(describe_function("f").contains("pg_function_is_visible"));
    }

    #[test]
    fn introspection_is_read_only() {
        use crate::safety::classify;
        assert!(classify(&list_databases()).is_read_only());
        assert!(classify(&list_objects(Some("public"))).is_read_only());
        assert!(classify(&describe_table("public.t")).is_read_only());
        assert!(classify(&describe_function("f")).is_read_only());
    }
}
