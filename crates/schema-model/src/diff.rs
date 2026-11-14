//! O DDL que leva o banco de um [`Model`] a outro.
//!
//! Tabelas e colunas são comparadas pelo [`crate::Id`], então renomear vira
//! `RENAME`, não apagar e criar. Constraints e índices são comparados pelo
//! nome: mudou a definição, apaga e cria de novo.
//!
//! A ordem segue o que o PostgreSQL exige:
//!
//! 1. apaga as FKs, constraints e índices que saem ou mudam (as FKs antes,
//!    porque podem depender de uma PK ou unique que também sai);
//! 2. apaga as tabelas que saem;
//! 3. renomeia e muda o schema das tabelas que ficam;
//! 4. colunas: renomeia, apaga, muda e acrescenta;
//! 5. cria as tabelas novas (sem FKs);
//! 6. cria PKs, uniques e checks novos ou mudados nas tabelas que ficam;
//! 7. cria os índices;
//! 8. cria as FKs, por último, quando todas as tabelas e chaves já existem.

use std::collections::HashMap;

use crate::{Column, ForeignKey, Id, Key, Model, Table, idents, qualified, quote_ident};

/// Comandos SQL, cada um terminado em `;`, que levam o banco de `old` a
/// `new`. Vazio se não há diferença.
pub fn diff(old: &Model, new: &Model) -> Vec<String> {
    let mut out = Plan::default();
    let new_by_id: HashMap<Id, &Table> = new.tables.iter().map(|t| (t.id, t)).collect();
    let old_by_id: HashMap<Id, &Table> = old.tables.iter().map(|t| (t.id, t)).collect();
    // Destino de uma FK pelo id da tabela, para não recriar FKs só porque a
    // tabela referenciada mudou de nome.
    let target = |m: &Model, fk: &ForeignKey| m.find(&fk.ref_schema, &fk.ref_table).map(|t| t.id);

    for o in &old.tables {
        let Some(n) = new_by_id.get(&o.id) else {
            // A tabela sai: as FKs dela saem junto. As de outras tabelas
            // que apontam para ela já sumiram do modelo novo.
            out.drop_tables
                .push(format!("DROP TABLE {};", o.qualified()));
            continue;
        };
        let q = o.qualified();
        // 1. O que sai ou muda, na tabela com o nome antigo.
        for fk in &o.foreign_keys {
            let same = n.foreign_keys.iter().find(|f| f.name == fk.name);
            let unchanged = same.is_some_and(|f| {
                f.columns == fk.columns
                    && f.ref_columns == fk.ref_columns
                    && f.on_update == fk.on_update
                    && f.on_delete == fk.on_delete
                    && target(new, f) == target(old, fk)
            });
            if !unchanged {
                out.drop_fks.push(drop_constraint(&q, &fk.name));
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
            if !kept || was_pk != is_pk {
                out.drop_constraints.push(drop_constraint(&q, &k.name));
            }
        }
        for c in &o.checks {
            if !n.checks.contains(c) {
                out.drop_constraints.push(drop_constraint(&q, &c.name));
            }
        }
        for i in &o.indexes {
            if !n.indexes.contains(i) {
                out.drop_constraints
                    .push(format!("DROP INDEX {};", qualified(&o.schema, &i.name)));
            }
        }

        // 3. Nome e schema.
        let mut current = (o.schema.clone(), o.name.clone());
        if o.name != n.name {
            out.alter.push(format!(
                "ALTER TABLE {} RENAME TO {};",
                qualified(&current.0, &current.1),
                quote_ident(&n.name)
            ));
            current.1 = n.name.clone();
        }
        if o.schema != n.schema {
            out.alter.push(format!(
                "ALTER TABLE {} SET SCHEMA {};",
                qualified(&current.0, &current.1),
                quote_ident(&n.schema)
            ));
        }

        // 4. Colunas.
        let q = n.qualified();
        let alter = |what: String| format!("ALTER TABLE {q} {what};");
        for oc in &o.columns {
            if let Some(nc) = n.column_by_id(oc.id)
                && nc.name != oc.name
            {
                out.alter.push(alter(format!(
                    "RENAME COLUMN {} TO {}",
                    quote_ident(&oc.name),
                    quote_ident(&nc.name)
                )));
            }
        }
        for oc in &o.columns {
            match n.column_by_id(oc.id) {
                None => out
                    .alter
                    .push(alter(format!("DROP COLUMN {}", quote_ident(&oc.name)))),
                Some(nc) if oc.generated != nc.generated => {
                    // Coluna gerada não muda de expressão no lugar; como o
                    // valor é calculado, recriar não perde dados.
                    out.alter
                        .push(alter(format!("DROP COLUMN {}", quote_ident(&nc.name))));
                    out.alter
                        .push(alter(format!("ADD COLUMN {}", nc.definition())));
                }
                Some(nc) => column_changes(oc, nc, &mut out.alter, &alter),
            }
        }
        for nc in &n.columns {
            if o.column_by_id(nc.id).is_none() {
                out.alter
                    .push(alter(format!("ADD COLUMN {}", nc.definition())));
            }
        }

        // 6 a 8. O que entra ou muda.
        for k in n.primary_key.iter() {
            if !o.primary_key.as_ref().is_some_and(|ok| ok == k) {
                out.add_constraints.push(alter(format!(
                    "ADD CONSTRAINT {} PRIMARY KEY ({})",
                    quote_ident(&k.name),
                    idents(&k.columns)
                )));
            }
        }
        for k in &n.uniques {
            if !o.uniques.contains(k) {
                out.add_constraints.push(alter(format!(
                    "ADD CONSTRAINT {} UNIQUE ({})",
                    quote_ident(&k.name),
                    idents(&k.columns)
                )));
            }
        }
        for c in &n.checks {
            if !o.checks.contains(c) {
                out.add_constraints.push(alter(format!(
                    "ADD CONSTRAINT {} CHECK ({})",
                    quote_ident(&c.name),
                    c.expr
                )));
            }
        }
        for i in &n.indexes {
            if !o.indexes.contains(i) {
                out.indexes.push(i.create(n));
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
            if !kept {
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
        out.indexes.extend(n.indexes.iter().map(|i| i.create(n)));
        out.add_fks
            .extend(n.foreign_keys.iter().map(|fk| add_fk(n, fk)));
    }

    out.into_statements()
}

/// Os comandos agrupados na ordem em que rodam.
#[derive(Default)]
struct Plan {
    drop_fks: Vec<String>,
    drop_constraints: Vec<String>,
    drop_tables: Vec<String>,
    alter: Vec<String>,
    create_tables: Vec<String>,
    add_constraints: Vec<String>,
    indexes: Vec<String>,
    add_fks: Vec<String>,
}

impl Plan {
    fn into_statements(self) -> Vec<String> {
        [
            self.drop_fks,
            self.drop_constraints,
            self.drop_tables,
            self.alter,
            self.create_tables,
            self.add_constraints,
            self.indexes,
            self.add_fks,
        ]
        .concat()
    }
}

fn drop_constraint(table: &str, name: &str) -> String {
    format!("ALTER TABLE {table} DROP CONSTRAINT {};", quote_ident(name))
}

fn add_fk(t: &Table, fk: &ForeignKey) -> String {
    format!(
        "ALTER TABLE {} ADD CONSTRAINT {} {};",
        t.qualified(),
        quote_ident(&fk.name),
        fk.definition()
    )
}

/// O `CREATE TABLE` de uma tabela nova, com PK, uniques e checks; as FKs e
/// os índices vêm depois.
pub fn create_table(t: &Table) -> String {
    let mut lines: Vec<String> = t.columns.iter().map(Column::definition).collect();
    if let Some(k) = &t.primary_key {
        lines.push(format!(
            "CONSTRAINT {} PRIMARY KEY ({})",
            quote_ident(&k.name),
            idents(&k.columns)
        ));
    }
    for k in &t.uniques {
        lines.push(format!(
            "CONSTRAINT {} UNIQUE ({})",
            quote_ident(&k.name),
            idents(&k.columns)
        ));
    }
    for c in &t.checks {
        lines.push(format!(
            "CONSTRAINT {} CHECK ({})",
            quote_ident(&c.name),
            c.expr
        ));
    }
    format!(
        "CREATE TABLE {} (\n    {}\n);",
        t.qualified(),
        lines.join(",\n    ")
    )
}

/// Tipo, nulo, default e identity de uma coluna que fica.
fn column_changes(o: &Column, n: &Column, out: &mut Vec<String>, alter: &dyn Fn(String) -> String) {
    let col = quote_ident(&n.name);
    if o.ty != n.ty {
        // O USING converte o que o cast converte (texto para número, por
        // exemplo); sem ele o PostgreSQL recusa tipos sem conversão
        // automática.
        out.push(alter(format!(
            "ALTER COLUMN {col} TYPE {ty} USING {col}::{ty}",
            ty = n.ty
        )));
    }
    if o.identity != n.identity {
        match (o.identity, n.identity) {
            (Some(_), None) => out.push(alter(format!("ALTER COLUMN {col} DROP IDENTITY"))),
            (None, Some(i)) => out.push(alter(format!(
                "ALTER COLUMN {col} ADD GENERATED {} AS IDENTITY",
                i.sql()
            ))),
            (Some(_), Some(i)) => out.push(alter(format!(
                "ALTER COLUMN {col} SET GENERATED {}",
                i.sql()
            ))),
            (None, None) => {}
        }
    }
    if o.default != n.default {
        out.push(alter(match &n.default {
            Some(d) => format!("ALTER COLUMN {col} SET DEFAULT {d}"),
            None => format!("ALTER COLUMN {col} DROP DEFAULT"),
        }));
    }
    if o.nullable != n.nullable {
        out.push(alter(format!(
            "ALTER COLUMN {col} {} NOT NULL",
            if n.nullable { "DROP" } else { "SET" }
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::Document;
    use crate::{Action, Check, Index};

    fn text(c: &str, ty: &str) -> Column {
        Column {
            name: c.into(),
            ty: ty.into(),
            nullable: true,
            ..Default::default()
        }
    }

    /// cliente(id, nome) e pedido(id, cliente_id -> cliente), já "no banco".
    fn shop() -> (Document, Model) {
        let mut d = Document::default();
        let cliente = d.add_table("public", "cliente").unwrap();
        d.add_column(cliente, text("nome", "text")).unwrap();
        let pedido = d.add_table("public", "pedido").unwrap();
        d.add_column(pedido, text("cliente_id", "integer")).unwrap();
        d.add_foreign_key(
            pedido,
            ForeignKey {
                columns: vec!["cliente_id".into()],
                ref_schema: "public".into(),
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

    #[test]
    fn new_tables_are_created_before_their_foreign_keys() {
        let (d, _) = shop();
        let sql = diff(&Model::default(), &d.model);
        assert_eq!(
            sql,
            [
                "CREATE TABLE public.cliente (\n    id integer GENERATED ALWAYS AS IDENTITY NOT NULL,\n    nome text,\n    CONSTRAINT cliente_pkey PRIMARY KEY (id)\n);",
                "CREATE TABLE public.pedido (\n    id integer GENERATED ALWAYS AS IDENTITY NOT NULL,\n    cliente_id integer,\n    CONSTRAINT pedido_pkey PRIMARY KEY (id)\n);",
                "ALTER TABLE public.pedido ADD CONSTRAINT pedido_cliente_id_fkey FOREIGN KEY (cliente_id) REFERENCES public.cliente (id);",
            ]
        );
        assert!(diff(&d.model, &d.model).is_empty());
    }

    #[test]
    fn renaming_a_referenced_table_keeps_the_foreign_key() {
        let (mut d, saved) = shop();
        d.rename_table(id(&d, "cliente"), "public", "customer")
            .unwrap();
        assert_eq!(
            diff(&saved, &d.model),
            ["ALTER TABLE public.cliente RENAME TO customer;"]
        );
    }

    #[test]
    fn column_edits_become_alter_table() {
        let (mut d, saved) = shop();
        let cliente = id(&d, "cliente");
        let nome = d.model.table(cliente).unwrap().columns[1].id;
        d.rename_column(cliente, nome, "Nome Completo").unwrap();
        d.update_column(cliente, nome, |c| {
            c.ty = "character varying(120)".into();
            c.nullable = false;
            c.default = Some("''::character varying".into());
        })
        .unwrap();
        d.add_column(cliente, text("email", "text")).unwrap();
        assert_eq!(
            diff(&saved, &d.model),
            [
                "ALTER TABLE public.cliente RENAME COLUMN nome TO \"Nome Completo\";",
                "ALTER TABLE public.cliente ALTER COLUMN \"Nome Completo\" TYPE character varying(120) USING \"Nome Completo\"::character varying(120);",
                "ALTER TABLE public.cliente ALTER COLUMN \"Nome Completo\" SET DEFAULT ''::character varying;",
                "ALTER TABLE public.cliente ALTER COLUMN \"Nome Completo\" SET NOT NULL;",
                "ALTER TABLE public.cliente ADD COLUMN email text;",
            ]
        );
    }

    #[test]
    fn dropping_a_referenced_table_drops_the_foreign_key_first() {
        let (mut d, saved) = shop();
        d.drop_table(id(&d, "cliente")).unwrap();
        assert_eq!(
            diff(&saved, &d.model),
            [
                "ALTER TABLE public.pedido DROP CONSTRAINT pedido_cliente_id_fkey;",
                "DROP TABLE public.cliente;",
            ]
        );
    }

    #[test]
    fn changed_constraints_are_dropped_and_added_again() {
        let (mut d, saved) = shop();
        let pedido = id(&d, "pedido");
        d.drop_constraint(pedido, "pedido_cliente_id_fkey").unwrap();
        d.add_foreign_key(
            pedido,
            ForeignKey {
                columns: vec!["cliente_id".into()],
                ref_schema: "public".into(),
                ref_table: "cliente".into(),
                ref_columns: vec!["id".into()],
                on_delete: Action::Cascade,
                ..Default::default()
            },
        )
        .unwrap();
        d.add_check(
            pedido,
            Check {
                name: "pedido_cliente_check".into(),
                expr: "cliente_id > 0".into(),
            },
        )
        .unwrap();
        d.add_index(
            pedido,
            Index {
                name: "pedido_cliente".into(),
                method: "btree".into(),
                columns: vec!["cliente_id".into()],
                ..Default::default()
            },
        )
        .unwrap();
        d.set_primary_key(pedido, vec!["id".into(), "cliente_id".into()])
            .unwrap();
        assert_eq!(
            diff(&saved, &d.model),
            [
                "ALTER TABLE public.pedido DROP CONSTRAINT pedido_cliente_id_fkey;",
                "ALTER TABLE public.pedido DROP CONSTRAINT pedido_pkey;",
                "ALTER TABLE public.pedido ALTER COLUMN cliente_id SET NOT NULL;",
                "ALTER TABLE public.pedido ADD CONSTRAINT pedido_pkey PRIMARY KEY (id, cliente_id);",
                "ALTER TABLE public.pedido ADD CONSTRAINT pedido_cliente_check CHECK (cliente_id > 0);",
                "CREATE INDEX pedido_cliente ON public.pedido (cliente_id);",
                "ALTER TABLE public.pedido ADD CONSTRAINT pedido_cliente_id_fkey FOREIGN KEY (cliente_id) REFERENCES public.cliente (id) ON DELETE CASCADE;",
            ]
        );
    }

    #[test]
    fn moving_a_table_to_another_schema() {
        let (mut d, saved) = shop();
        d.rename_table(id(&d, "pedido"), "vendas", "venda").unwrap();
        assert_eq!(
            diff(&saved, &d.model),
            [
                "ALTER TABLE public.pedido RENAME TO venda;",
                "ALTER TABLE public.venda SET SCHEMA vendas;",
            ]
        );
    }
}
