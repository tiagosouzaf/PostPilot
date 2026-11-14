//! O [`Model`] do schema-model lido do `pg_catalog`, e o DDL aplicado de
//! volta numa transação.

use pgstudio_core::metadata::{COMMON_TYPES, MetadataError};
use pgstudio_pg_client::describe;
use pgstudio_schema_model::{
    Action, Check, Column, ForeignKey, Identity, Index, Key, Model, Table,
};

use crate::CatalogExplorer;

mod sql {
    /// Tabelas comuns e particionadas dos schemas de usuário (partições e
    /// tabelas estrangeiras ficam de fora).
    pub const TABLES: &str = "SELECT c.oid, n.nspname::text, c.relname::text \
        FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
        WHERE c.relkind IN ('r', 'p') AND NOT c.relispartition \
        AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' \
        ORDER BY n.nspname, c.relname";

    pub const COLUMNS: &str = "SELECT a.attrelid, a.attnum::int4, a.attname::text, \
        pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull, \
        pg_catalog.pg_get_expr(d.adbin, d.adrelid), a.attidentity::text, a.attgenerated::text \
        FROM pg_catalog.pg_attribute a \
        LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
        WHERE a.attrelid = ANY($1) AND a.attnum > 0 AND NOT a.attisdropped \
        ORDER BY a.attrelid, a.attnum";

    /// PK, unique, check e FK próprias (não herdadas), com as colunas em
    /// ordem.
    pub const CONSTRAINTS: &str = "SELECT k.conrelid, k.conname::text, k.contype::text, \
        ARRAY(SELECT a.attname::text FROM unnest(k.conkey) WITH ORDINALITY u(n, o) \
            JOIN pg_catalog.pg_attribute a ON a.attrelid = k.conrelid AND a.attnum = u.n \
            ORDER BY u.o), \
        rn.nspname::text, rc.relname::text, \
        ARRAY(SELECT a.attname::text FROM unnest(k.confkey) WITH ORDINALITY u(n, o) \
            JOIN pg_catalog.pg_attribute a ON a.attrelid = k.confrelid AND a.attnum = u.n \
            ORDER BY u.o), \
        k.confupdtype::text, k.confdeltype::text, pg_catalog.pg_get_constraintdef(k.oid, true) \
        FROM pg_catalog.pg_constraint k \
        LEFT JOIN pg_catalog.pg_class rc ON rc.oid = k.confrelid \
        LEFT JOIN pg_catalog.pg_namespace rn ON rn.oid = rc.relnamespace \
        WHERE k.conrelid = ANY($1) AND k.contype IN ('p', 'u', 'c', 'f') AND k.coninhcount = 0 \
        ORDER BY k.conrelid, k.conname";

    /// Índices que não são de constraints, uma expressão por coluna.
    pub const INDEXES: &str = "SELECT i.indrelid, c.relname::text, i.indisunique, am.amname::text, \
        ARRAY(SELECT pg_catalog.pg_get_indexdef(i.indexrelid, k, true) \
            FROM generate_series(1, i.indnkeyatts) k ORDER BY k), \
        pg_catalog.pg_get_expr(i.indpred, i.indrelid, true) \
        FROM pg_catalog.pg_index i \
        JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid \
        JOIN pg_catalog.pg_am am ON am.oid = c.relam \
        WHERE i.indrelid = ANY($1) AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k \
            WHERE k.conindid = i.indexrelid AND k.conrelid = i.indrelid) \
        ORDER BY i.indrelid, c.relname";

    /// Enums, domínios e tipos compostos dos schemas de usuário.
    pub const TYPES: &str = "SELECT pg_catalog.format_type(t.oid, NULL) \
        FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
        WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' \
        AND (t.typtype IN ('e', 'd') OR (t.typtype = 'c' AND EXISTS (SELECT 1 \
            FROM pg_catalog.pg_class c WHERE c.oid = t.typrelid AND c.relkind = 'c'))) \
        ORDER BY 1";
}

impl CatalogExplorer {
    /// O modelo das tabelas dos schemas de usuário de `db`. Ids das tabelas
    /// são os oids; das colunas, `oid << 16 | attnum`, únicos no modelo.
    pub(crate) fn read_model(&self, db: &str) -> Result<Model, MetadataError> {
        let mut tables: Vec<Table> = self
            .query(db, sql::TABLES, &[])?
            .iter()
            .map(|r| {
                let oid: u32 = r.get(0);
                Table {
                    id: u64::from(oid),
                    schema: r.get(1),
                    name: r.get(2),
                    ..Default::default()
                }
            })
            .collect();
        let oids: Vec<u32> = tables.iter().map(|t| t.id as u32).collect();
        let slot = |tables: &mut Vec<Table>, oid: u32| -> Option<usize> {
            tables.iter().position(|t| t.id == u64::from(oid))
        };

        for r in self.query(db, sql::COLUMNS, &[&oids])? {
            let oid: u32 = r.get(0);
            let Some(i) = slot(&mut tables, oid) else {
                continue;
            };
            let attnum: i32 = r.get(1);
            let identity: String = r.get(6);
            let generated: String = r.get(7);
            let expr: Option<String> = r.get(5);
            let (default, generated) = if generated == "s" {
                (None, expr)
            } else {
                (expr, None)
            };
            tables[i].columns.push(Column {
                id: u64::from(oid) << 16 | attnum as u64,
                name: r.get(2),
                ty: r.get(3),
                nullable: !r.get::<_, bool>(4),
                default,
                identity: match identity.as_str() {
                    "a" => Some(Identity::Always),
                    "d" => Some(Identity::ByDefault),
                    _ => None,
                },
                generated,
            });
        }

        for r in self.query(db, sql::CONSTRAINTS, &[&oids])? {
            let oid: u32 = r.get(0);
            let Some(i) = slot(&mut tables, oid) else {
                continue;
            };
            let name: String = r.get(1);
            let kind: String = r.get(2);
            let columns: Vec<String> = r.get(3);
            let t = &mut tables[i];
            match kind.as_str() {
                "p" => t.primary_key = Some(Key { name, columns }),
                "u" => t.uniques.push(Key { name, columns }),
                "c" => {
                    let def: String = r.get(9);
                    t.checks.push(Check {
                        name,
                        expr: check_expr(&def),
                    });
                }
                _ => {
                    let update: String = r.get(7);
                    let delete: String = r.get(8);
                    t.foreign_keys.push(ForeignKey {
                        name,
                        columns,
                        ref_schema: r.get::<_, Option<String>>(4).unwrap_or_default(),
                        ref_table: r.get::<_, Option<String>>(5).unwrap_or_default(),
                        ref_columns: r.get(6),
                        on_update: Action::from_catalog(&update),
                        on_delete: Action::from_catalog(&delete),
                    });
                }
            }
        }

        for r in self.query(db, sql::INDEXES, &[&oids])? {
            let oid: u32 = r.get(0);
            let Some(i) = slot(&mut tables, oid) else {
                continue;
            };
            tables[i].indexes.push(Index {
                name: r.get(1),
                unique: r.get(2),
                method: r.get(3),
                columns: r.get(4),
                predicate: r.get(5),
            });
        }
        Ok(Model::new(tables))
    }

    /// Tipos comuns mais os do banco, em ordem alfabética.
    pub(crate) fn read_type_names(&self, db: &str) -> Result<Vec<String>, MetadataError> {
        let mut names: Vec<String> = COMMON_TYPES.iter().map(|t| (*t).to_string()).collect();
        for r in self.query(db, sql::TYPES, &[])? {
            names.push(r.get(0));
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Roda os comandos numa transação: ou tudo entra, ou nada. O erro diz
    /// qual comando falhou.
    pub(crate) fn run_ddl(&self, db: &str, statements: &[String]) -> Result<(), MetadataError> {
        let session = self.session(db)?;
        let client = session.client();
        self.runtime.block_on(async {
            client
                .batch_execute("BEGIN")
                .await
                .map_err(|e| MetadataError(describe(&e)))?;
            for (n, stmt) in statements.iter().enumerate() {
                if let Err(e) = client.batch_execute(stmt).await {
                    let _ = client.batch_execute("ROLLBACK").await;
                    return Err(MetadataError(format!(
                        "{} (statement {} of {}: {})",
                        describe(&e),
                        n + 1,
                        statements.len(),
                        first_line(stmt)
                    )));
                }
            }
            client
                .batch_execute("COMMIT")
                .await
                .map_err(|e| MetadataError(describe(&e)))
        })
    }
}

/// A expressão de um `CHECK (...)` do `pg_get_constraintdef`, sem o que vem
/// em volta (`NOT VALID` incluído).
fn check_expr(def: &str) -> String {
    let def = def.trim();
    let def = def.strip_suffix(" NOT VALID").unwrap_or(def);
    def.strip_prefix("CHECK (")
        .and_then(|d| d.strip_suffix(')'))
        .unwrap_or(def)
        .to_string()
}

fn first_line(stmt: &str) -> &str {
    stmt.lines().next().unwrap_or(stmt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_expressions_lose_their_wrapper() {
        assert_eq!(check_expr("CHECK (qty > 0)"), "qty > 0");
        assert_eq!(
            check_expr("CHECK ((a > 0) AND (b > 0)) NOT VALID"),
            "(a > 0) AND (b > 0)"
        );
    }
}
