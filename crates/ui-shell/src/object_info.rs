//! Alt+F1 (Object Information): o `sp_help` do SSMS para o PostgreSQL.
//!
//! O nome sob o cursor (ou a seleção) vira um lote de consultas aos
//! catálogos, que roda na própria janela de consulta e mostra um resultado
//! por assunto: o objeto, as colunas, os índices, as constraints e as
//! chaves estrangeiras que apontam para ele.

/// O nome (talvez com schema e aspas) que contém a posição `pos` (em
/// caracteres) de `text`. Também vale com o cursor logo depois do nome.
pub fn name_at(text: &str, pos: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let part = |c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '.' || c == '"';
    let pos = pos.min(chars.len());
    let mut start = pos;
    while start > 0 && part(chars[start - 1]) {
        start -= 1;
    }
    let mut end = pos;
    while end < chars.len() && part(chars[end]) {
        end += 1;
    }
    let name: String = chars[start..end].iter().collect();
    let name = name.trim_matches('.');
    let has_letter = name.chars().any(|c| c.is_alphabetic() || c == '_');
    (has_letter && !name.is_empty()).then(|| name.to_string())
}

/// As consultas do Alt+F1 para `name` (texto como no editor: `pedido`,
/// `public.pedido`, `"Meu Schema"."Tabela"`). O nome vai como literal para
/// `regclass`, que segue as regras do PostgreSQL para aspas, maiúsculas e
/// `search_path`, e dá o erro "relation does not exist" se não houver.
pub fn info_sql(name: &str) -> String {
    let lit = format!("'{}'", name.trim().replace('\'', "''"));
    format!(
        "SELECT c.relname AS \"Name\", n.nspname AS \"Schema\", \
         pg_get_userbyid(c.relowner) AS \"Owner\", \
         CASE c.relkind WHEN 'r' THEN 'table' WHEN 'p' THEN 'partitioned table' \
         WHEN 'v' THEN 'view' WHEN 'm' THEN 'materialized view' WHEN 'i' THEN 'index' \
         WHEN 'S' THEN 'sequence' WHEN 'f' THEN 'foreign table' \
         WHEN 'c' THEN 'composite type' ELSE c.relkind::text END AS \"Type\", \
         pg_size_pretty(pg_total_relation_size(c.oid)) AS \"Size\", \
         c.reltuples::bigint AS \"Rows (estimate)\", \
         obj_description(c.oid, 'pg_class') AS \"Description\" \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.oid = {lit}::regclass;\n\
         SELECT a.attname AS \"Column_name\", \
         format_type(a.atttypid, a.atttypmod) AS \"Type\", \
         CASE WHEN a.attnotnull THEN 'no' ELSE 'yes' END AS \"Nullable\", \
         pg_get_expr(d.adbin, d.adrelid) AS \"Default\", \
         co.collname AS \"Collation\", \
         col_description(a.attrelid, a.attnum) AS \"Description\" \
         FROM pg_attribute a \
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         LEFT JOIN pg_collation co ON co.oid = a.attcollation AND a.attcollation <> 100 \
         WHERE a.attrelid = {lit}::regclass AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY a.attnum;\n\
         SELECT i.relname AS \"Index_name\", pg_get_indexdef(x.indexrelid) AS \"Definition\", \
         pg_size_pretty(pg_relation_size(x.indexrelid)) AS \"Size\" \
         FROM pg_index x JOIN pg_class i ON i.oid = x.indexrelid \
         WHERE x.indrelid = {lit}::regclass ORDER BY 1;\n\
         SELECT conname AS \"Constraint_name\", \
         CASE contype WHEN 'p' THEN 'PRIMARY KEY' WHEN 'f' THEN 'FOREIGN KEY' \
         WHEN 'u' THEN 'UNIQUE' WHEN 'c' THEN 'CHECK' WHEN 'x' THEN 'EXCLUDE' \
         ELSE contype::text END AS \"Type\", \
         pg_get_constraintdef(oid) AS \"Definition\" \
         FROM pg_constraint WHERE conrelid = {lit}::regclass ORDER BY 2, 1;\n\
         SELECT conrelid::regclass::text AS \"Table is referenced by foreign key\", \
         conname AS \"Constraint_name\", pg_get_constraintdef(oid) AS \"Definition\" \
         FROM pg_constraint WHERE confrelid = {lit}::regclass AND contype = 'f' ORDER BY 1, 2"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_under_the_cursor_is_found() {
        let text = "select * from public.pedido p where x = 1";
        assert_eq!(name_at(text, 16).as_deref(), Some("public.pedido"));
        // Logo depois do nome também vale.
        assert_eq!(name_at(text, 27).as_deref(), Some("public.pedido"));
        assert_eq!(name_at(text, 0).as_deref(), Some("select"));
        assert_eq!(name_at("a = 1", 4), None);
        assert_eq!(name_at("  ", 1), None);
        assert_eq!(
            name_at("from \"Meu Schema\".x", 7).as_deref(),
            Some("\"Meu")
        );
        assert_eq!(
            name_at("from \"Vendas\".\"Itens\";", 10).as_deref(),
            Some("\"Vendas\".\"Itens\"")
        );
        assert_eq!(name_at("x.", 2).as_deref(), Some("x"));
    }

    #[test]
    fn names_are_quoted_as_literals() {
        let sql = info_sql("o'neil");
        assert!(sql.contains("'o''neil'::regclass"));
        assert_eq!(sql.matches("::regclass").count(), 6);
        assert_eq!(sql.split(";\n").count(), 5);
    }
}
