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
