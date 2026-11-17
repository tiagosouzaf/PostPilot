//! O [`Model`] do schema-model lido do catálogo do SQL Server: as tabelas
//! de usuário de um banco com colunas, chaves, checks, chaves estrangeiras
//! e índices. É o que o Query Designer, os diagramas e o Schema Compare
//! leem.

use pgstudio_core::metadata::MetadataError;
use pgstudio_core::schema::{
    Action, Check, Column, ForeignKey, Identity, Index, Key, Model, Table,
};

use crate::explorer::{MssqlExplorer, bracket, plain_type};

/// Separador das listas de colunas (um nome pode ter vírgula).
const SEP: char = '\u{1f}';

fn text(r: &[Option<String>], i: usize) -> String {
    r.get(i).cloned().flatten().unwrap_or_default()
}

fn optional(r: &[Option<String>], i: usize) -> Option<String> {
    r.get(i).cloned().flatten().filter(|s| !s.is_empty())
}

fn number(r: &[Option<String>], i: usize) -> i64 {
    text(r, i).parse().unwrap_or(0)
}

fn list(r: &[Option<String>], i: usize) -> Vec<String> {
    text(r, i)
        .split(SEP)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// `update_referential_action` e `delete_referential_action`.
fn action(code: &str) -> Action {
    match code {
        "1" => Action::Cascade,
        "2" => Action::SetNull,
        "3" => Action::SetDefault,
        _ => Action::NoAction,
    }
}

/// A definição de um CHECK ou DEFAULT sem os parênteses de fora que o
/// servidor põe (`([valor]>=(0))` vira `[valor]>=(0)`).
pub fn unwrap_parens(def: &str) -> String {
    let d = def.trim();
    let inner = d.strip_prefix('(').and_then(|s| s.strip_suffix(')'));
    match inner {
        // Só quando o par de fora fecha um com o outro.
        Some(inner) if balanced(inner) => inner.to_string(),
        _ => d.to_string(),
    }
}

/// Se os parênteses de `s` fecham sem nunca ficar negativos.
fn balanced(s: &str) -> bool {
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

impl MssqlExplorer {
    /// O modelo das tabelas de usuário de `db`. Ids das tabelas são os
    /// `object_id`; das colunas, `object_id << 16 | column_id`.
    pub(crate) fn read_model(&self, db: &str) -> Result<Model, MetadataError> {
        let d = bracket(db);
        let sep = format!("NCHAR({})", SEP as u32);
        let mut tables: Vec<Table> = self
            .rows(&format!(
                "SELECT o.object_id, s.name, o.name FROM {d}.sys.tables o \
                 JOIN {d}.sys.schemas s ON s.schema_id = o.schema_id \
                 WHERE o.is_ms_shipped = 0 ORDER BY s.name, o.name"
            ))?
            .iter()
            .map(|r| Table {
                id: number(r, 0) as u64,
                schema: text(r, 1),
                name: text(r, 2),
                ..Default::default()
            })
            .collect();
        let slot = |tables: &[Table], r: &[Option<String>]| {
            let id = number(r, 0) as u64;
            tables.iter().position(|t| t.id == id)
        };

        for r in self.rows(&format!(
            "SELECT c.object_id, c.column_id, c.name, \
                    CASE WHEN t.is_user_defined = 1 \
                         THEN SCHEMA_NAME(t.schema_id) + '.' + t.name ELSE t.name END, \
                    c.max_length, c.precision, c.scale, c.is_nullable, c.is_identity, \
                    dc.definition, cc.definition, t.is_user_defined \
               FROM {d}.sys.columns c \
               JOIN {d}.sys.tables o ON o.object_id = c.object_id AND o.is_ms_shipped = 0 \
               JOIN {d}.sys.types t ON t.user_type_id = c.user_type_id \
               LEFT JOIN {d}.sys.default_constraints dc ON dc.object_id = c.default_object_id \
               LEFT JOIN {d}.sys.computed_columns cc ON cc.object_id = c.object_id \
                                                    AND cc.column_id = c.column_id \
              ORDER BY c.object_id, c.column_id"
        ))? {
            let Some(i) = slot(&tables, &r) else { continue };
            let ty = if text(&r, 11) == "1" {
                text(&r, 3)
            } else {
                plain_type(&text(&r, 3), number(&r, 4), number(&r, 5), number(&r, 6))
            };
            let id = (tables[i].id << 16) | number(&r, 1) as u64;
            tables[i].columns.push(Column {
                id,
                name: text(&r, 2),
                ty,
                nullable: text(&r, 7) == "1",
                default: optional(&r, 9).map(|d| unwrap_parens(&d)),
                identity: (text(&r, 8) == "1").then_some(Identity::Always),
                generated: optional(&r, 10).map(|d| unwrap_parens(&d)),
            });
        }

        // PK e UNIQUE, com as colunas na ordem da chave.
        for r in self.rows(&format!(
            "SELECT k.parent_object_id, k.name, RTRIM(k.type), \
                    (SELECT STRING_AGG(CAST(col.name AS nvarchar(max)), {sep}) \
                            WITHIN GROUP (ORDER BY ic.key_ordinal) \
                       FROM {d}.sys.index_columns ic JOIN {d}.sys.columns col \
                         ON col.object_id = ic.object_id AND col.column_id = ic.column_id \
                      WHERE ic.object_id = k.parent_object_id \
                        AND ic.index_id = k.unique_index_id AND ic.is_included_column = 0) \
               FROM {d}.sys.key_constraints k ORDER BY k.parent_object_id, k.name"
        ))? {
            let Some(i) = slot(&tables, &r) else { continue };
            let key = Key {
                name: text(&r, 1),
                columns: list(&r, 3),
            };
            if text(&r, 2) == "PK" {
                tables[i].primary_key = Some(key);
            } else {
                tables[i].uniques.push(key);
            }
        }

        for r in self.rows(&format!(
            "SELECT c.parent_object_id, c.name, c.definition \
               FROM {d}.sys.check_constraints c ORDER BY c.parent_object_id, c.name"
        ))? {
            let Some(i) = slot(&tables, &r) else { continue };
            tables[i].checks.push(Check {
                name: text(&r, 1),
                expr: unwrap_parens(&text(&r, 2)),
            });
        }

        let fk_columns = |side: &str| {
            format!(
                "(SELECT STRING_AGG(CAST(col.name AS nvarchar(max)), {sep}) \
                         WITHIN GROUP (ORDER BY fc.constraint_column_id) \
                    FROM {d}.sys.foreign_key_columns fc JOIN {d}.sys.columns col \
                      ON col.object_id = fc.{side}_object_id \
                     AND col.column_id = fc.{side}_column_id \
                   WHERE fc.constraint_object_id = f.object_id)"
            )
        };
        for r in self.rows(&format!(
            "SELECT f.parent_object_id, f.name, {}, rs.name, ro.name, {}, \
                    f.update_referential_action, f.delete_referential_action \
               FROM {d}.sys.foreign_keys f \
               JOIN {d}.sys.objects ro ON ro.object_id = f.referenced_object_id \
               JOIN {d}.sys.schemas rs ON rs.schema_id = ro.schema_id \
              ORDER BY f.parent_object_id, f.name",
            fk_columns("parent"),
            fk_columns("referenced")
        ))? {
            let Some(i) = slot(&tables, &r) else { continue };
            tables[i].foreign_keys.push(ForeignKey {
                name: text(&r, 1),
                columns: list(&r, 2),
                ref_schema: text(&r, 3),
                ref_table: text(&r, 4),
                ref_columns: list(&r, 5),
                on_update: action(&text(&r, 6)),
                on_delete: action(&text(&r, 7)),
            });
        }

        // Índices que não são de PK nem de UNIQUE; `DESC` fica na coluna.
        for r in self.rows(&format!(
            "SELECT i.object_id, i.name, i.is_unique, LOWER(i.type_desc), \
                    (SELECT STRING_AGG(CAST(col.name AS nvarchar(max)) + \
                                IIF(ic.is_descending_key = 1, ' DESC', ''), {sep}) \
                            WITHIN GROUP (ORDER BY ic.key_ordinal) \
                       FROM {d}.sys.index_columns ic JOIN {d}.sys.columns col \
                         ON col.object_id = ic.object_id AND col.column_id = ic.column_id \
                      WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id \
                        AND ic.is_included_column = 0), \
                    i.filter_definition \
               FROM {d}.sys.indexes i \
              WHERE i.index_id > 0 AND i.is_primary_key = 0 AND i.is_unique_constraint = 0 \
                AND i.is_hypothetical = 0 AND i.name IS NOT NULL \
              ORDER BY i.object_id, i.name"
        ))? {
            let Some(i) = slot(&tables, &r) else { continue };
            tables[i].indexes.push(Index {
                name: text(&r, 1),
                unique: text(&r, 2) == "1",
                method: text(&r, 3),
                columns: list(&r, 4),
                predicate: optional(&r, 5).map(|p| unwrap_parens(&p)),
            });
        }
        let mut model = Model::new(tables);
        model.dialect = pgstudio_core::schema::Dialect::Tsql;
        Ok(model)
    }
}

/// Tipos do SQL Server sugeridos no Table Designer, como o catálogo escreve.
const COMMON_TYPES: &[&str] = &[
    "bigint",
    "binary(50)",
    "bit",
    "char(10)",
    "date",
    "datetime",
    "datetime2(7)",
    "datetimeoffset(7)",
    "decimal(18,2)",
    "float",
    "int",
    "money",
    "nchar(10)",
    "nvarchar(50)",
    "nvarchar(max)",
    "real",
    "smallint",
    "time(7)",
    "tinyint",
    "uniqueidentifier",
    "varbinary(max)",
    "varchar(50)",
    "varchar(max)",
    "xml",
];

impl MssqlExplorer {
    /// Os tipos comuns mais os definidos no banco (`schema.nome`).
    pub(crate) fn read_type_names(&self, db: &str) -> Result<Vec<String>, MetadataError> {
        let d = bracket(db);
        let mut names: Vec<String> = COMMON_TYPES.iter().map(|t| (*t).to_string()).collect();
        for r in self.rows(&format!(
            "SELECT SCHEMA_NAME(t.schema_id) + '.' + t.name FROM {d}.sys.types t \
              WHERE t.is_user_defined = 1 AND t.is_table_type = 0"
        ))? {
            names.extend(r.into_iter().next().flatten());
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Roda os comandos, um lote cada, numa transação no banco: ou tudo
    /// entra, ou nada. O erro diz qual comando falhou. A sessão fica presa
    /// até o fim e volta para o banco onde estava.
    pub(crate) fn run_ddl(&self, db: &str, statements: &[String]) -> Result<(), MetadataError> {
        self.runtime.block_on(async {
            let mut session = self.healthy().await.map_err(MetadataError)?;
            let home = session
                .rows("SELECT DB_NAME()")
                .await
                .ok()
                .and_then(|r| r.into_iter().next()?.into_iter().next()?)
                .unwrap_or_else(|| "master".into());
            let back = format!(
                "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION; SET XACT_ABORT OFF; USE {};",
                bracket(&home)
            );
            let begin = format!("USE {}; SET XACT_ABORT ON; BEGIN TRANSACTION;", bracket(db));
            if let Err(e) = session.rows(&begin).await {
                let _ = session.rows(&back).await;
                return Err(MetadataError(e));
            }
            for (n, stmt) in statements.iter().enumerate() {
                if let Err(e) = session.rows(stmt).await {
                    let _ = session.rows(&back).await;
                    return Err(MetadataError(format!(
                        "{e} (statement {} of {}: {})",
                        n + 1,
                        statements.len(),
                        stmt.lines().next().unwrap_or_default()
                    )));
                }
            }
            let done = session.rows("COMMIT TRANSACTION;").await;
            let _ = session.rows(&back).await;
            done.map(|_| ()).map_err(MetadataError)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outer_parentheses_come_off_only_when_they_match() {
        assert_eq!(unwrap_parens("([valor]>=(0))"), "[valor]>=(0)");
        assert_eq!(unwrap_parens("((0))"), "(0)");
        assert_eq!(unwrap_parens("(getdate())"), "getdate()");
        assert_eq!(
            unwrap_parens("([a]>(0)) AND ([b]<(9))"),
            "([a]>(0)) AND ([b]<(9))"
        );
        assert_eq!(unwrap_parens("[a]*(2)"), "[a]*(2)");
    }
}
