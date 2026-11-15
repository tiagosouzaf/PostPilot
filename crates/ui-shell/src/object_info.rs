//! Alt+F1 (Object Information): o `sp_help` do SSMS para o PostgreSQL.
//!
//! O nome sob o cursor (ou a seleção) vira um lote de consultas aos
//! catálogos, que roda na própria janela de consulta e mostra um resultado
//! por assunto: o objeto, as colunas, os índices, as constraints e as
//! chaves estrangeiras que apontam para ele.

/// O nome (talvez com schema e aspas) que contém a posição `pos` (em
/// caracteres) de `text`. Também vale com o cursor logo depois do nome.
pub fn name_at(text: &str, pos: usize) -> Option<String> {
    name_at_for(text, pos, false)
}

/// [`name_at`] para o motor: em T-SQL (`tsql`), o nome também pode ter
/// colchetes, `#` e `@` (`[dbo].[pedidos]`, `#temp`).
pub fn name_at_for(text: &str, pos: usize, tsql: bool) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let part = |c: char| {
        c.is_alphanumeric()
            || matches!(c, '_' | '$' | '.' | '"')
            || (tsql && matches!(c, '[' | ']' | '#' | '@'))
    };
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

/// O Alt+F1 do SQL Server, como o `sp_help`: o objeto, as colunas, os
/// parâmetros, os índices, as constraints e as chaves estrangeiras que
/// apontam para ele. `OBJECT_ID` resolve o nome com as regras do servidor
/// (colchetes, schema padrão); se não achar, sai o erro do `sp_help`.
pub fn info_tsql(name: &str) -> String {
    let lit = format!("N'{}'", name.trim().replace('\'', "''"));
    // O tipo como o SSMS escreve: nvarchar(60), decimal(10,2), datetime2(3).
    let ty = "CASE \
         WHEN t.name IN ('nvarchar', 'nchar') THEN t.name + '(' + \
              IIF(c.max_length = -1, 'max', CAST(c.max_length / 2 AS varchar(10))) + ')' \
         WHEN t.name IN ('varchar', 'char', 'varbinary', 'binary') THEN t.name + '(' + \
              IIF(c.max_length = -1, 'max', CAST(c.max_length AS varchar(10))) + ')' \
         WHEN t.name IN ('decimal', 'numeric') THEN t.name + '(' + \
              CAST(c.precision AS varchar(10)) + ',' + CAST(c.scale AS varchar(10)) + ')' \
         WHEN t.name IN ('datetime2', 'time', 'datetimeoffset') THEN t.name + '(' + \
              CAST(c.scale AS varchar(10)) + ')' \
         ELSE t.name END";
    let columns_of = |index: &str| {
        format!(
            "(SELECT STRING_AGG(CAST(QUOTENAME(col.name) AS nvarchar(max)), ', ') \
              WITHIN GROUP (ORDER BY ic.key_ordinal) \
              FROM sys.index_columns ic JOIN sys.columns col \
                ON col.object_id = ic.object_id AND col.column_id = ic.column_id \
              WHERE ic.object_id = {index}.object_id AND ic.index_id = {index}.index_id \
                AND ic.is_included_column = 0)"
        )
    };
    let fk_columns = |side: &str| {
        format!(
            "(SELECT STRING_AGG(CAST(QUOTENAME(col.name) AS nvarchar(max)), ', ') \
              WITHIN GROUP (ORDER BY fc.constraint_column_id) \
              FROM sys.foreign_key_columns fc JOIN sys.columns col \
                ON col.object_id = fc.{side}_object_id AND col.column_id = fc.{side}_column_id \
              WHERE fc.constraint_object_id = f.object_id)"
        )
    };
    format!(
        "DECLARE @name nvarchar(776) = {lit};\n\
         DECLARE @id int = OBJECT_ID(@name);\n\
         IF @id IS NULL\n\
         BEGIN\n\
           DECLARE @db sysname = DB_NAME();\n\
           RAISERROR(N'The object ''%s'' does not exist in database ''%s'' or is invalid for this operation.', 16, 1, @name, @db);\n\
           RETURN;\n\
         END;\n\
         SELECT o.name AS [Name], s.name AS [Schema], \
         USER_NAME(ISNULL(o.principal_id, s.principal_id)) AS [Owner], \
         LOWER(REPLACE(o.type_desc, '_', ' ')) AS [Type], \
         (SELECT SUM(p.rows) FROM sys.partitions p \
           WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)) AS [Rows (estimate)], \
         CONVERT(varchar(19), o.create_date, 120) AS [Created], \
         CONVERT(varchar(19), o.modify_date, 120) AS [Last modified] \
         FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id \
         WHERE o.object_id = @id;\n\
         IF EXISTS (SELECT 1 FROM sys.columns WHERE object_id = @id)\n\
         SELECT c.name AS [Column_name], {ty} AS [Type], \
         CASE c.is_nullable WHEN 1 THEN 'yes' ELSE 'no' END AS [Nullable], \
         CASE c.is_identity WHEN 1 THEN 'yes' ELSE 'no' END AS [Identity], \
         CASE c.is_computed WHEN 1 THEN 'yes' ELSE 'no' END AS [Computed], \
         OBJECT_DEFINITION(c.default_object_id) AS [Default], \
         c.collation_name AS [Collation] \
         FROM sys.columns c JOIN sys.types t ON t.user_type_id = c.user_type_id \
         WHERE c.object_id = @id ORDER BY c.column_id;\n\
         IF EXISTS (SELECT 1 FROM sys.parameters WHERE object_id = @id)\n\
         SELECT IIF(c.parameter_id = 0, '(return value)', c.name) AS [Parameter_name], \
         {ty} AS [Type], \
         CASE c.is_output WHEN 1 THEN 'yes' ELSE 'no' END AS [Output] \
         FROM sys.parameters c JOIN sys.types t ON t.user_type_id = c.user_type_id \
         WHERE c.object_id = @id ORDER BY c.parameter_id;\n\
         IF EXISTS (SELECT 1 FROM sys.indexes WHERE object_id = @id AND index_id > 0)\n\
         SELECT i.name AS [Index_name], \
         LOWER(i.type_desc) + IIF(i.is_unique = 1, ', unique', '') + \
         IIF(i.is_primary_key = 1, ', primary key', '') AS [Description], \
         {index_columns} AS [Index_keys] \
         FROM sys.indexes i WHERE i.object_id = @id AND i.index_id > 0 ORDER BY i.name;\n\
         IF EXISTS (SELECT 1 FROM sys.objects WHERE parent_object_id = @id \
                    AND type IN ('PK', 'UQ', 'C', 'D', 'F'))\n\
         SELECT x.[Constraint_name], x.[Type], x.[Definition] FROM (\n\
           SELECT k.name AS [Constraint_name], \
           IIF(k.type = 'PK', 'PRIMARY KEY', 'UNIQUE') AS [Type], \
           (SELECT {key_columns} FROM sys.indexes i \
             WHERE i.object_id = k.parent_object_id AND i.index_id = k.unique_index_id) \
             AS [Definition] \
           FROM sys.key_constraints k WHERE k.parent_object_id = @id\n\
           UNION ALL SELECT c.name, 'CHECK', c.definition \
           FROM sys.check_constraints c WHERE c.parent_object_id = @id\n\
           UNION ALL SELECT d.name, 'DEFAULT', \
           d.definition + ' FOR ' + QUOTENAME(COL_NAME(d.parent_object_id, d.parent_column_id)) \
           FROM sys.default_constraints d WHERE d.parent_object_id = @id\n\
           UNION ALL SELECT f.name, 'FOREIGN KEY', \
           '(' + {fk_parent} + ') REFERENCES ' + \
           QUOTENAME(OBJECT_SCHEMA_NAME(f.referenced_object_id)) + '.' + \
           QUOTENAME(OBJECT_NAME(f.referenced_object_id)) + ' (' + {fk_referenced} + ')' \
           FROM sys.foreign_keys f WHERE f.parent_object_id = @id\n\
         ) x ORDER BY x.[Type], x.[Constraint_name];\n\
         IF EXISTS (SELECT 1 FROM sys.foreign_keys WHERE referenced_object_id = @id)\n\
         SELECT QUOTENAME(OBJECT_SCHEMA_NAME(f.parent_object_id)) + '.' + \
         QUOTENAME(OBJECT_NAME(f.parent_object_id)) AS [Table is referenced by foreign key], \
         f.name AS [Constraint_name] \
         FROM sys.foreign_keys f WHERE f.referenced_object_id = @id ORDER BY 1, 2",
        index_columns = columns_of("i"),
        key_columns = columns_of("i"),
        fk_parent = fk_columns("parent"),
        fk_referenced = fk_columns("referenced"),
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

    #[test]
    fn tsql_names_keep_their_brackets() {
        let text = "select * from [dbo].[Meus Pedidos] p where #t.x = 1";
        assert_eq!(name_at_for(text, 16, true).as_deref(), Some("[dbo].[Meus"));
        assert_eq!(
            name_at_for("exec dbo.[listar]", 12, true).as_deref(),
            Some("dbo.[listar]")
        );
        assert_eq!(name_at_for(text, 43, true).as_deref(), Some("#t.x"));
        // No PostgreSQL os colchetes são de arrays.
        assert_eq!(name_at("select a[1]", 8).as_deref(), Some("a"));
        let sql = info_tsql("dbo.o'neil");
        assert!(sql.contains("= N'dbo.o''neil';"), "{sql}");
    }
}
