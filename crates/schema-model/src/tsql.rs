//! O DDL em T-SQL que leva um banco SQL Server de um [`Model`] a outro: o
//! mesmo trabalho do [`crate::diff`], com as regras do SQL Server.
//!
//! O que muda em relação ao PostgreSQL:
//!
//! - renomear é `sp_rename`; mudar de schema é `ALTER SCHEMA ... TRANSFER`;
//! - o default de uma coluna é uma constraint com nome dado pelo servidor:
//!   para mudar o default, o tipo ou apagar a coluna, ela sai antes (o nome
//!   é procurado no catálogo na hora);
//! - `ALTER COLUMN` recusa mudar o tipo de uma coluna usada por chave,
//!   índice, check ou FK, e `sp_rename` recusa renomear uma coluna usada
//!   num check, num índice filtrado ou numa coluna calculada: essas
//!   dependências saem antes e voltam depois;
//! - `IDENTITY` não entra nem sai de uma coluna que já existe
//!   ([`unsupported`] diz isso antes de salvar).
//!
//! A ordem dos comandos é a do [`crate::diff`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::diff::{Plan, renames, with_renames};
use crate::{Action, Column, ForeignKey, Id, Index, Key, Model, Table};

/// `[nome]`, sempre com colchetes (como os scripts do SSMS).
pub fn quote(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

/// `[schema].[nome]`.
pub fn qualified(schema: &str, name: &str) -> String {
    format!("{}.{}", quote(schema), quote(name))
}

fn table_name(t: &Table) -> String {
    qualified(&t.schema, &t.name)
}

fn idents(names: &[String]) -> String {
    names
        .iter()
        .map(|n| quote(n))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `N'texto'`.
fn literal(s: &str) -> String {
    format!("N'{}'", s.replace('\'', "''"))
}

/// Troca a coluna `old` por `new` numa expressão T-SQL (check, filtro de
/// índice, coluna calculada), sem diferenciar maiúsculas e sem mexer em
/// textos entre aspas simples nem em variáveis.
pub fn rename_in_expr(expr: &str, old: &str, new: &str) -> String {
    let word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '$');
    let same = |name: &str| name.to_lowercase() == old.to_lowercase();
    let mut out = String::with_capacity(expr.len());
    let mut rest = expr;
    while let Some(c) = rest.chars().next() {
        if c == '\'' {
            // Até a aspa que fecha (`''` é uma aspa dentro do texto).
            let mut end = 1;
            let b = rest.as_bytes();
            while end < b.len() {
                if b[end] == b'\'' {
                    if b.get(end + 1) == Some(&b'\'') {
                        end += 2;
                        continue;
                    }
                    break;
                }
                end += 1;
            }
            let end = (end + 1).min(rest.len());
            out.push_str(&rest[..end]);
            rest = &rest[end..];
        } else if c == '[' {
            let mut name = String::new();
            let mut chars = rest[1..].char_indices().peekable();
            let mut end = rest.len();
            while let Some((i, ch)) = chars.next() {
                if ch == ']' {
                    if chars.peek().is_some_and(|(_, n)| *n == ']') {
                        name.push(']');
                        chars.next();
                        continue;
                    }
                    end = i + 2;
                    break;
                }
                name.push(ch);
            }
            if same(&name) {
                out.push_str(&quote(new));
            } else {
                out.push_str(&rest[..end]);
            }
            rest = &rest[end..];
        } else if word(c) {
            let end = rest.find(|ch| !word(ch)).unwrap_or(rest.len());
            let name = &rest[..end];
            if same(name) && !name.starts_with('@') {
                out.push_str(&quote(new));
            } else {
                out.push_str(name);
            }
            rest = &rest[end..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Se a expressão usa a coluna `name`.
fn mentions(expr: &str, name: &str) -> bool {
    rename_in_expr(expr, name, "\u{1}") != expr
}

/// A definição da coluna no `CREATE TABLE` ou no `ADD`.
pub fn column_definition(c: &Column) -> String {
    if let Some(expr) = &c.generated {
        return format!("{} AS ({expr})", quote(&c.name));
    }
    let mut s = format!("{} {}", quote(&c.name), c.ty);
    if c.identity.is_some() {
        s.push_str(" IDENTITY(1,1)");
    }
    s.push_str(if c.nullable { " NULL" } else { " NOT NULL" });
    if let Some(default) = &c.default {
        s.push_str(&format!(" DEFAULT {default}"));
    }
    s
}

fn fk_definition(fk: &ForeignKey) -> String {
    let mut s = format!(
        "FOREIGN KEY ({}) REFERENCES {} ({})",
        idents(&fk.columns),
        qualified(&fk.ref_schema, &fk.ref_table),
        idents(&fk.ref_columns)
    );
    // O SQL Server não tem RESTRICT: o NO ACTION dele já recusa na hora.
    let action = |a: Action| !matches!(a, Action::NoAction | Action::Restrict);
    if action(fk.on_update) {
        s.push_str(&format!(" ON UPDATE {}", fk.on_update.sql()));
    }
    if action(fk.on_delete) {
        s.push_str(&format!(" ON DELETE {}", fk.on_delete.sql()));
    }
    s
}

/// O nome da coluna de uma entrada de índice (`nome`, `[nome]` ou
/// `nome DESC`) e se é decrescente.
pub(crate) fn index_column(entry: &str) -> (String, bool) {
    let entry = entry.trim();
    let upper = entry.to_uppercase();
    let (name, desc) = if upper.ends_with(" DESC") {
        (&entry[..entry.len() - 5], true)
    } else if upper.ends_with(" ASC") {
        (&entry[..entry.len() - 4], false)
    } else {
        (entry, false)
    };
    let name = name.trim();
    let name = match name.strip_prefix('[').and_then(|n| n.strip_suffix(']')) {
        Some(inner) => inner.replace("]]", "]"),
        None => name.to_string(),
    };
    (name, desc)
}

/// O `CREATE INDEX` do índice na tabela.
pub fn create_index(i: &Index, t: &Table) -> String {
    let mut s = String::from("CREATE ");
    if i.unique {
        s.push_str("UNIQUE ");
    }
    if !matches!(i.method.as_str(), "" | "btree" | "nonclustered") {
        s.push_str(&i.method.to_uppercase());
        s.push(' ');
    }
    s.push_str(&format!("INDEX {} ON {}", quote(&i.name), table_name(t)));
    if !i.columns.is_empty() {
        let columns: Vec<String> = i
            .columns
            .iter()
            .map(|c| {
                let (name, desc) = index_column(c);
                format!("{}{}", quote(&name), if desc { " DESC" } else { "" })
            })
            .collect();
        s.push_str(&format!(" ({})", columns.join(", ")));
    }
    if let Some(p) = &i.predicate {
        s.push_str(&format!(" WHERE {p}"));
    }
    s.push(';');
    s
}

/// O `CREATE TABLE` de uma tabela nova, com PK, uniques e checks; as FKs e
/// os índices vêm depois.
pub fn create_table(t: &Table) -> String {
    let mut lines: Vec<String> = t.columns.iter().map(column_definition).collect();
    if let Some(k) = &t.primary_key {
        lines.push(format!(
            "CONSTRAINT {} PRIMARY KEY ({})",
            quote(&k.name),
            idents(&k.columns)
        ));
    }
    for k in &t.uniques {
        lines.push(format!(
            "CONSTRAINT {} UNIQUE ({})",
            quote(&k.name),
            idents(&k.columns)
        ));
    }
    for c in &t.checks {
        lines.push(format!("CONSTRAINT {} CHECK ({})", quote(&c.name), c.expr));
    }
    format!(
        "CREATE TABLE {} (\n    {}\n);",
        table_name(t),
        lines.join(",\n    ")
    )
}

/// Apaga o default da coluna, qualquer que seja o nome que o servidor deu
/// à constraint (não faz nada se não houver).
fn drop_default(table: &str, column: &str) -> String {
    format!(
        "DECLARE @df nvarchar(max) = (SELECT N'ALTER TABLE {escaped} DROP CONSTRAINT ' + QUOTENAME(d.name) \
         FROM sys.default_constraints d \
         JOIN sys.columns c ON c.object_id = d.parent_object_id AND c.column_id = d.parent_column_id \
         WHERE d.parent_object_id = OBJECT_ID({object}) AND c.name = {column}); EXEC (@df);",
        escaped = table.replace('\'', "''"),
        object = literal(table),
        column = literal(column),
    )
}

fn same_type(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Mudanças que o SQL Server não faz numa coluna que já existe (hoje, pôr
/// ou tirar `IDENTITY`). O designer mostra isso em vez de salvar.
pub fn unsupported(old: &Model, new: &Model) -> Vec<String> {
    let mut out = Vec::new();
    for o in &old.tables {
        let Some(n) = new.table(o.id) else { continue };
        for oc in &o.columns {
            if let Some(nc) = n.column_by_id(oc.id)
                && oc.identity.is_some() != nc.identity.is_some()
            {
                out.push(pgstudio_i18n::trf!(
                    "SQL Server cannot add or remove IDENTITY on the existing column {column} of {table}. Create a new column or a new table instead.",
                    column = nc.name,
                    table = format!("{}.{}", n.schema, n.name)
                ));
            }
        }
    }
    out
}

/// Colunas (pelo nome novo) de cada tabela cujo tipo muda.
fn retyped(old: &Model, new: &Model) -> HashMap<Id, HashSet<String>> {
    let mut out = HashMap::new();
    for o in &old.tables {
        let Some(n) = new.table(o.id) else { continue };
        let set: HashSet<String> = o
            .columns
            .iter()
            .filter_map(|oc| {
                let nc = n.column_by_id(oc.id)?;
                (oc.generated.is_none() && nc.generated.is_none() && !same_type(&oc.ty, &nc.ty))
                    .then(|| nc.name.clone())
            })
            .collect();
        if !set.is_empty() {
            out.insert(o.id, set);
        }
    }
    out
}

/// Comandos T-SQL, cada um um lote terminado em `;`, que levam o banco de
/// `old` a `new`. Vazio se não há diferença.
pub fn diff(old: &Model, new: &Model) -> Vec<String> {
    let mut out = Plan::default();
    let new_arcs: HashMap<Id, &Arc<Table>> = new.tables.iter().map(|t| (t.id, t)).collect();
    let new_by_id: HashMap<Id, &Table> = new.tables.iter().map(|t| (t.id, &**t)).collect();
    let old_by_id: HashMap<Id, &Table> = old.tables.iter().map(|t| (t.id, &**t)).collect();
    let target = |m: &Model, fk: &ForeignKey| m.find(&fk.ref_schema, &fk.ref_table).map(|t| t.id);
    let retyped = retyped(old, new);
    let none = HashSet::new();
    // FK (já com os nomes novos) que aponta para uma coluna que muda de
    // tipo: sai antes e volta depois.
    let points_at_retyped = |m: &Model, fk: &ForeignKey| {
        target(m, fk)
            .and_then(|id| retyped.get(&id))
            .is_some_and(|set| fk.ref_columns.iter().any(|c| set.contains(c)))
    };

    for original in &old.tables {
        let untouched = new_arcs
            .get(&original.id)
            .is_some_and(|n| Arc::ptr_eq(n, original));
        if untouched
            && !original
                .foreign_keys
                .iter()
                .any(|fk| points_at_retyped(new, fk))
        {
            continue;
        }
        let Some(n) = new_by_id.get(&original.id) else {
            out.drop_tables
                .push(format!("DROP TABLE {};", table_name(original)));
            continue;
        };
        let q = table_name(original);
        let renamed = renames(original, n);
        let o = &with_renames(original, n, old, new);
        let changed = retyped.get(&o.id).unwrap_or(&none);
        // Expressão (como era no banco) que usa uma coluna que muda de nome
        // ou de tipo.
        let touched = |expr: &str| {
            renamed.keys().any(|c| mentions(expr, c))
                || original.columns.iter().any(|oc| {
                    n.column_by_id(oc.id)
                        .is_some_and(|nc| changed.contains(&nc.name))
                        && mentions(expr, &oc.name)
                })
        };
        let key_forced = |k: &Key| k.columns.iter().any(|c| changed.contains(c));
        let fk_forced = |fk: &ForeignKey| {
            fk.columns.iter().any(|c| changed.contains(c)) || points_at_retyped(new, fk)
        };
        let original_index = |name: &str| original.indexes.iter().find(|i| i.name == name);
        let index_forced = |i: &Index| {
            i.columns
                .iter()
                .any(|c| changed.contains(&index_column(c).0))
                || original_index(&i.name)
                    .and_then(|i| i.predicate.as_deref())
                    .is_some_and(touched)
        };
        let original_check = |name: &str| original.checks.iter().find(|c| c.name == name);
        let check_forced = |name: &str| original_check(name).is_some_and(|c| touched(&c.expr));
        // Coluna calculada que sai e volta: a expressão mudou ou usa uma
        // coluna que muda.
        let recomputed = |oc: &Column| {
            n.column_by_id(oc.id).is_some_and(|nc| {
                (oc.generated.is_some() || nc.generated.is_some())
                    && (o.column_by_id(oc.id).map(|c| &c.generated) != Some(&nc.generated)
                        || oc.generated.as_deref().is_some_and(touched))
            })
        };

        // 1. O que sai ou muda, na tabela com o nome antigo.
        let drop_constraint =
            |name: &str| format!("ALTER TABLE {q} DROP CONSTRAINT {};", quote(name));
        for fk in &o.foreign_keys {
            let same = n.foreign_keys.iter().find(|f| f.name == fk.name);
            let unchanged = same.is_some_and(|f| {
                f.columns == fk.columns
                    && f.ref_columns == fk.ref_columns
                    && f.on_update == fk.on_update
                    && f.on_delete == fk.on_delete
                    && target(new, f) == target(old, fk)
            });
            if !unchanged || fk_forced(fk) {
                out.drop_fks.push(drop_constraint(&fk.name));
            }
        }
        for k in o.primary_key.iter().chain(&o.uniques) {
            let kept = n
                .primary_key
                .iter()
                .chain(&n.uniques)
                .any(|nk: &Key| nk.name == k.name && nk.columns == k.columns);
            let was_pk = o.primary_key.as_ref() == Some(k);
            let is_pk = n.primary_key.as_ref().is_some_and(|nk| nk.name == k.name);
            if !kept || was_pk != is_pk || key_forced(k) {
                out.drop_constraints.push(drop_constraint(&k.name));
            }
        }
        for c in &o.checks {
            if !n.checks.contains(c) || check_forced(&c.name) {
                out.drop_constraints.push(drop_constraint(&c.name));
            }
        }
        for i in &o.indexes {
            if !n.indexes.contains(i) || index_forced(i) {
                out.drop_constraints
                    .push(format!("DROP INDEX {} ON {q};", quote(&i.name)));
            }
        }
        for oc in &original.columns {
            if recomputed(oc) && oc.generated.is_some() {
                out.drop_constraints
                    .push(format!("ALTER TABLE {q} DROP COLUMN {};", quote(&oc.name)));
            }
        }

        // 3. Nome e schema.
        let mut current = (o.schema.clone(), o.name.clone());
        if o.name != n.name {
            out.alter.push(format!(
                "EXEC sp_rename {}, {};",
                literal(&qualified(&current.0, &current.1)),
                literal(&n.name)
            ));
            current.1 = n.name.clone();
        }
        if o.schema != n.schema {
            out.alter.push(format!(
                "ALTER SCHEMA {} TRANSFER {};",
                quote(&n.schema),
                qualified(&current.0, &current.1)
            ));
        }

        // 4. Colunas.
        let q = table_name(n);
        let alter = |what: String| format!("ALTER TABLE {q} {what};");
        for oc in &original.columns {
            if let Some(nc) = n.column_by_id(oc.id)
                && nc.name != oc.name
                && !(recomputed(oc) && oc.generated.is_some())
            {
                out.alter.push(format!(
                    "EXEC sp_rename {}, {}, N'COLUMN';",
                    literal(&format!("{q}.{}", quote(&oc.name))),
                    literal(&nc.name)
                ));
            }
        }
        for oc in &original.columns {
            match n.column_by_id(oc.id) {
                None => {
                    if oc.default.is_some() {
                        out.alter.push(drop_default(&q, &oc.name));
                    }
                    out.alter
                        .push(alter(format!("DROP COLUMN {}", quote(&oc.name))));
                }
                Some(nc) if recomputed(oc) => {
                    if oc.generated.is_none() {
                        // Coluna comum que vira calculada.
                        if oc.default.is_some() {
                            out.alter.push(drop_default(&q, &nc.name));
                        }
                        out.alter
                            .push(alter(format!("DROP COLUMN {}", quote(&nc.name))));
                    }
                    out.alter
                        .push(alter(format!("ADD {}", column_definition(nc))));
                }
                Some(nc) if nc.generated.is_some() => {}
                Some(nc) => column_changes(oc, nc, &q, &mut out.alter),
            }
        }
        for nc in &n.columns {
            if o.column_by_id(nc.id).is_none() {
                out.alter
                    .push(alter(format!("ADD {}", column_definition(nc))));
            }
        }

        // 6 a 8. O que entra, muda ou saiu só para a coluna poder mudar.
        for k in n.primary_key.iter() {
            if o.primary_key.as_ref() != Some(k) || key_forced(k) {
                out.add_constraints.push(alter(format!(
                    "ADD CONSTRAINT {} PRIMARY KEY ({})",
                    quote(&k.name),
                    idents(&k.columns)
                )));
            }
        }
        for k in &n.uniques {
            if !o.uniques.contains(k) || key_forced(k) {
                out.add_constraints.push(alter(format!(
                    "ADD CONSTRAINT {} UNIQUE ({})",
                    quote(&k.name),
                    idents(&k.columns)
                )));
            }
        }
        for c in &n.checks {
            if !o.checks.contains(c) || check_forced(&c.name) {
                out.add_constraints.push(alter(format!(
                    "ADD CONSTRAINT {} CHECK ({})",
                    quote(&c.name),
                    c.expr
                )));
            }
        }
        for i in &n.indexes {
            if !o.indexes.contains(i) || index_forced(i) {
                out.indexes.push(create_index(i, n));
            }
        }
        for fk in &n.foreign_keys {
            let kept = o.foreign_keys.iter().any(|f| {
                f.name == fk.name
                    && f.columns == fk.columns
                    && f.ref_columns == fk.ref_columns
                    && f.on_update == fk.on_update
                    && f.on_delete == fk.on_delete
                    && target(old, f) == target(new, fk)
            });
            if !kept || fk_forced(fk) {
                out.add_fks.push(add_fk(n, fk));
            }
        }
    }

    // 5. Tabelas novas, na ordem do modelo.
    for n in &new.tables {
        if old_by_id.contains_key(&n.id) {
            continue;
        }
        out.create_tables.push(create_table(n));
        out.indexes
            .extend(n.indexes.iter().map(|i| create_index(i, n)));
        out.add_fks
            .extend(n.foreign_keys.iter().map(|fk| add_fk(n, fk)));
    }

    out.into_statements()
}

fn add_fk(t: &Table, fk: &ForeignKey) -> String {
    format!(
        "ALTER TABLE {} ADD CONSTRAINT {} {};",
        table_name(t),
        quote(&fk.name),
        fk_definition(fk)
    )
}

/// Tipo, nulo e default de uma coluna que fica. `IDENTITY` não muda
/// ([`unsupported`]).
fn column_changes(o: &Column, n: &Column, table: &str, out: &mut Vec<String>) {
    let col = quote(&n.name);
    let retype = !same_type(&o.ty, &n.ty);
    let default = o.default != n.default;
    // O default é uma constraint presa à coluna: sai para o tipo mudar.
    if o.default.is_some() && (default || retype) {
        out.push(drop_default(table, &n.name));
    }
    if retype || o.nullable != n.nullable {
        out.push(format!(
            "ALTER TABLE {table} ALTER COLUMN {col} {}{};",
            n.ty,
            if n.nullable { " NULL" } else { " NOT NULL" }
        ));
    }
    if let Some(d) = &n.default
        && (default || retype)
    {
        out.push(format!("ALTER TABLE {table} ADD DEFAULT {d} FOR {col};"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::Document;
    use crate::{Check, Dialect};

    fn column(name: &str, ty: &str) -> Column {
        Column {
            name: name.into(),
            ty: ty.into(),
            nullable: true,
            ..Default::default()
        }
    }

    /// cliente(id, nome) e pedido(id, cliente_id -> cliente, total), já
    /// "no banco".
    fn shop() -> (Document, Model) {
        let mut d = Document::new(Model {
            dialect: Dialect::Tsql,
            ..Default::default()
        });
        let cliente = d.add_table("dbo", "cliente").unwrap();
        d.add_column(cliente, column("nome", "nvarchar(60)"))
            .unwrap();
        let pedido = d.add_table("dbo", "pedido").unwrap();
        d.add_column(pedido, column("cliente_id", "int")).unwrap();
        d.add_column(pedido, column("total", "decimal(10,2)"))
            .unwrap();
        d.add_foreign_key(
            pedido,
            ForeignKey {
                columns: vec!["cliente_id".into()],
                ref_schema: "dbo".into(),
                ref_table: "cliente".into(),
                ref_columns: vec!["id".into()],
                ..Default::default()
            },
        )
        .unwrap();
        d.history.clear();
        let saved = d.model.clone();
        (d, saved)
    }

    fn id(d: &Document, name: &str) -> Id {
        d.model.tables.iter().find(|t| t.name == name).unwrap().id
    }

    fn col(d: &Document, table: &str, name: &str) -> Id {
        d.model.find("dbo", table).unwrap().column(name).unwrap().id
    }

    #[test]
    fn a_new_table_is_created_in_tsql() {
        let (d, _) = shop();
        let ddl = crate::diff::diff(&Model::default(), &d.model);
        assert_eq!(
            ddl[0],
            "CREATE TABLE [dbo].[cliente] (\n    [id] int IDENTITY(1,1) NOT NULL,\n    \
             [nome] nvarchar(60) NULL,\n    CONSTRAINT [cliente_pkey] PRIMARY KEY ([id])\n);"
        );
        assert_eq!(
            ddl.last().unwrap(),
            "ALTER TABLE [dbo].[pedido] ADD CONSTRAINT [pedido_cliente_id_fkey] \
             FOREIGN KEY ([cliente_id]) REFERENCES [dbo].[cliente] ([id]);"
        );
    }

    #[test]
    fn renames_use_sp_rename() {
        let (mut d, saved) = shop();
        let pedido = id(&d, "pedido");
        let total = col(&d, "pedido", "total");
        d.rename_column(pedido, total, "Valor Total").unwrap();
        d.rename_table(pedido, "vendas", "Pedidos").unwrap();
        assert_eq!(
            diff(&saved, &d.model),
            [
                "EXEC sp_rename N'[dbo].[pedido]', N'Pedidos';",
                "ALTER SCHEMA [vendas] TRANSFER [dbo].[Pedidos];",
                "EXEC sp_rename N'[vendas].[Pedidos].[total]', N'Valor Total', N'COLUMN';",
            ]
        );
    }

    #[test]
    fn a_check_on_a_renamed_column_leaves_and_comes_back() {
        let (mut d, _) = shop();
        let pedido = id(&d, "pedido");
        d.add_check(
            pedido,
            Check {
                name: "ck_total".into(),
                expr: "[total]>=(0)".into(),
            },
        )
        .unwrap();
        let saved = d.model.clone();
        let total = col(&d, "pedido", "total");
        d.rename_column(pedido, total, "valor").unwrap();
        assert_eq!(
            diff(&saved, &d.model),
            [
                "ALTER TABLE [dbo].[pedido] DROP CONSTRAINT [ck_total];",
                "EXEC sp_rename N'[dbo].[pedido].[total]', N'valor', N'COLUMN';",
                "ALTER TABLE [dbo].[pedido] ADD CONSTRAINT [ck_total] CHECK ([valor]>=(0));",
            ]
        );
    }

    #[test]
    fn retyping_a_key_column_rebuilds_what_depends_on_it() {
        let (mut d, saved) = shop();
        let cliente = id(&d, "cliente");
        let key = col(&d, "cliente", "id");
        d.update_column(cliente, key, |c| c.ty = "bigint".into())
            .unwrap();
        assert_eq!(
            diff(&saved, &d.model),
            [
                "ALTER TABLE [dbo].[pedido] DROP CONSTRAINT [pedido_cliente_id_fkey];",
                "ALTER TABLE [dbo].[cliente] DROP CONSTRAINT [cliente_pkey];",
                "ALTER TABLE [dbo].[cliente] ALTER COLUMN [id] bigint NOT NULL;",
                "ALTER TABLE [dbo].[cliente] ADD CONSTRAINT [cliente_pkey] PRIMARY KEY ([id]);",
                "ALTER TABLE [dbo].[pedido] ADD CONSTRAINT [pedido_cliente_id_fkey] \
                 FOREIGN KEY ([cliente_id]) REFERENCES [dbo].[cliente] ([id]);",
            ]
        );
    }

    #[test]
    fn defaults_are_constraints_that_leave_first() {
        let (mut d, _) = shop();
        let pedido = id(&d, "pedido");
        let total = col(&d, "pedido", "total");
        d.update_column(pedido, total, |c| c.default = Some("0".into()))
            .unwrap();
        let saved = d.model.clone();
        d.update_column(pedido, total, |c| {
            c.default = Some("1".into());
            c.nullable = false;
        })
        .unwrap();
        let ddl = diff(&saved, &d.model);
        assert_eq!(ddl.len(), 3, "{ddl:#?}");
        assert!(ddl[0].contains("sys.default_constraints") && ddl[0].contains("N'total'"));
        assert_eq!(
            ddl[1],
            "ALTER TABLE [dbo].[pedido] ALTER COLUMN [total] decimal(10,2) NOT NULL;"
        );
        assert_eq!(
            ddl[2],
            "ALTER TABLE [dbo].[pedido] ADD DEFAULT 1 FOR [total];"
        );
        d.drop_column(pedido, total).unwrap();
        let ddl = diff(&saved, &d.model);
        assert_eq!(ddl.len(), 2);
        assert_eq!(ddl[1], "ALTER TABLE [dbo].[pedido] DROP COLUMN [total];");
    }

    #[test]
    fn identity_on_an_existing_column_is_refused_before_saving() {
        let (mut d, saved) = shop();
        let pedido = id(&d, "pedido");
        let total = col(&d, "pedido", "total");
        assert!(unsupported(&saved, &d.model).is_empty());
        d.update_column(pedido, total, |c| {
            c.identity = Some(crate::Identity::Always);
        })
        .unwrap();
        assert_eq!(unsupported(&saved, &d.model).len(), 1);
    }

    #[test]
    fn indexes_keep_the_direction_and_the_filter() {
        let (d, _) = shop();
        let t = d.model.find("dbo", "pedido").unwrap();
        let i = Index {
            name: "ix".into(),
            unique: true,
            method: "nonclustered".into(),
            columns: vec!["total DESC".into(), "[cliente_id]".into()],
            predicate: Some("([total]>(0))".into()),
        };
        assert_eq!(
            create_index(&i, t),
            "CREATE UNIQUE INDEX [ix] ON [dbo].[pedido] ([total] DESC, [cliente_id]) \
             WHERE ([total]>(0));"
        );
    }

    #[test]
    fn only_narrowing_types_warn_about_data_loss() {
        let (mut d, saved) = shop();
        let (cliente, pedido) = (id(&d, "cliente"), id(&d, "pedido"));
        let (nome, total) = (col(&d, "cliente", "nome"), col(&d, "pedido", "total"));
        d.update_column(cliente, nome, |c| c.ty = "nvarchar(max)".into())
            .unwrap();
        d.update_column(pedido, total, |c| c.ty = "decimal(12,2)".into())
            .unwrap();
        assert!(crate::diff::data_loss_warnings(&saved, &d.model).is_empty());
        d.update_column(cliente, nome, |c| c.ty = "varchar(60)".into())
            .unwrap();
        d.update_column(pedido, total, |c| c.ty = "int".into())
            .unwrap();
        assert_eq!(crate::diff::data_loss_warnings(&saved, &d.model).len(), 2);
    }

    #[test]
    fn expressions_are_renamed_without_touching_text() {
        let r = |e: &str| rename_in_expr(e, "total", "Valor Total");
        assert_eq!(r("([TOTAL]>=(0))"), "([Valor Total]>=(0))");
        assert_eq!(r("total*2 + @total"), "[Valor Total]*2 + @total");
        assert_eq!(r("[x]='total' OR subtotal>1"), "[x]='total' OR subtotal>1");
    }
}
