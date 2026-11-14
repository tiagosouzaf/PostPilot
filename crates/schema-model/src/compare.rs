//! Schema Compare: o DDL que deixa as tabelas de um banco (o destino) como
//! as de outro (a origem).
//!
//! Os dois modelos vêm de bancos diferentes, então os [`Id`] (oids) não
//! batem. A origem é alinhada ao destino pelo nome: uma tabela com o mesmo
//! schema e nome, e nela uma coluna com o mesmo nome, ganham o id do
//! destino; o resto ganha um id novo. Assim o [`diff`] trata o que existe
//! nos dois como a mesma tabela, e uma coluna que só existe num lado vira
//! `ADD` ou `DROP` (renomear não se reconhece entre bancos).
//!
//! Cada tabela diferente pode entrar ou não: o script leva o destino a um
//! modelo misto, com as tabelas escolhidas como na origem e as outras como
//! estão.
//!
//! [`Outcome`] junta as tabelas com os outros objetos ([`crate::objects`]):
//! uma lista só de diferenças, e um script com tudo o que foi escolhido na
//! ordem das [`Phase`]s.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::objects::{self, Kind, Object, Phase};
use crate::snapshot::Schema;
use crate::{Id, Model, Table, diff::diff};

/// Schema e nome de uma tabela.
pub type Name = (String, String);

/// Como uma tabela difere entre os bancos.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Só na origem: o script cria.
    OnlyInSource,
    /// Só no destino: o script apaga.
    OnlyInTarget,
    /// Nos dois, diferente: o script altera.
    Different,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::OnlyInSource => pgstudio_i18n::tr("Only in source"),
            Status::OnlyInTarget => pgstudio_i18n::tr("Only in target"),
            Status::Different => pgstudio_i18n::tr("Different"),
        }
    }
}

/// Uma tabela que difere, com o DDL que só ela levaria.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableDiff {
    pub schema: String,
    pub name: String,
    pub status: Status,
    pub statements: Vec<String>,
}

impl TableDiff {
    pub fn key(&self) -> Name {
        (self.schema.clone(), self.name.clone())
    }
}

/// A comparação de dois modelos.
#[derive(Clone, Debug)]
pub struct Comparison {
    /// A origem, com os ids alinhados ao destino.
    source: Model,
    target: Model,
    /// As tabelas que diferem, por schema e nome.
    pub tables: Vec<TableDiff>,
    /// Quantas tabelas são iguais.
    pub equal: usize,
}

impl Comparison {
    pub fn new(source: &Model, target: &Model) -> Self {
        let source = align(source, target);
        let mut c = Comparison {
            source,
            target: target.clone(),
            tables: Vec::new(),
            equal: 0,
        };
        let mut names: BTreeSet<Name> = BTreeSet::new();
        for t in c.source.tables.iter().chain(&c.target.tables) {
            names.insert((t.schema.clone(), t.name.clone()));
        }
        for (schema, name) in names {
            let status = match (c.source.find(&schema, &name), c.target.find(&schema, &name)) {
                (Some(_), None) => Status::OnlyInSource,
                (None, Some(_)) => Status::OnlyInTarget,
                _ => Status::Different,
            };
            let key = (schema.clone(), name.clone());
            let statements = c.script(&BTreeSet::from([key]));
            if statements.is_empty() {
                c.equal += 1;
                continue;
            }
            c.tables.push(TableDiff {
                schema,
                name,
                status,
                statements,
            });
        }
        c
    }

    /// O DDL que leva o destino à origem só nas tabelas `chosen`.
    pub fn script(&self, chosen: &BTreeSet<Name>) -> Vec<String> {
        let mixed = self.mix(chosen);
        let statements = diff(&self.target, &mixed);
        with_sequences(statements)
    }

    /// O destino com as tabelas `chosen` como na origem (as que só existem
    /// no destino saem, junto com as FKs que apontam para elas).
    fn mix(&self, chosen: &BTreeSet<Name>) -> Model {
        let mut tables: Vec<Arc<Table>> = Vec::new();
        for t in &self.target.tables {
            let key = (t.schema.clone(), t.name.clone());
            if !chosen.contains(&key) {
                tables.push(t.clone());
            } else if let Some(s) = self.source.find(&t.schema, &t.name) {
                tables.push(Arc::new(s.clone()));
            }
        }
        for s in &self.source.tables {
            let key = (s.schema.clone(), s.name.clone());
            if chosen.contains(&key) && self.target.find(&s.schema, &s.name).is_none() {
                tables.push(s.clone());
            }
        }
        let present: BTreeSet<Name> = tables
            .iter()
            .map(|t| (t.schema.clone(), t.name.clone()))
            .collect();
        let exists = |schema: &str, name: &str| present.contains(&(schema.into(), name.into()));
        let dangling: Vec<usize> = tables
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.foreign_keys
                    .iter()
                    .any(|fk| !exists(&fk.ref_schema, &fk.ref_table))
            })
            .map(|(i, _)| i)
            .collect();
        for i in dangling {
            let mut t = (*tables[i]).clone();
            let keep: Vec<_> = t
                .foreign_keys
                .iter()
                .filter(|fk| exists(&fk.ref_schema, &fk.ref_table))
                .cloned()
                .collect();
            t.foreign_keys = keep;
            tables[i] = Arc::new(t);
        }
        Model {
            tables,
            next_id: self.source.next_id.max(self.target.next_id),
        }
    }
}

/// Uma diferença da lista do Schema Compare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub key: objects::Key,
    pub status: Status,
    /// O DDL que só este objeto levaria.
    pub statements: Vec<String>,
}

/// A comparação de dois schemas inteiros: tabelas e os outros objetos.
#[derive(Clone, Debug)]
pub struct Outcome {
    tables: Comparison,
    /// O DDL de cada objeto que difere, por fase.
    objects: BTreeMap<objects::Key, Vec<(Phase, String)>>,
    /// As diferenças, por tipo, schema e nome.
    pub items: Vec<Item>,
    /// Quantos objetos são iguais.
    pub equal: usize,
}

impl Outcome {
    pub fn new(source: &Schema, target: &Schema) -> Self {
        let tables = Comparison::new(&source.model, &target.model);
        let mut items: Vec<Item> = tables
            .tables
            .iter()
            .map(|t| Item {
                key: (Kind::Table, t.schema.clone(), t.name.clone()),
                status: t.status,
                statements: t.statements.clone(),
            })
            .collect();
        let mut equal = tables.equal;
        let by_key = |list: &[Object]| -> BTreeMap<objects::Key, Object> {
            list.iter().map(|o| (o.key(), o.clone())).collect()
        };
        let (s, t) = (by_key(&source.objects), by_key(&target.objects));
        let keys: BTreeSet<&objects::Key> = s.keys().chain(t.keys()).collect();
        let mut objects = BTreeMap::new();
        for key in keys {
            let (so, to) = (s.get(key), t.get(key));
            let statements = objects::statements(so, to);
            if statements.is_empty() {
                equal += 1;
                continue;
            }
            let status = match (so, to) {
                (Some(_), None) => Status::OnlyInSource,
                (None, Some(_)) => Status::OnlyInTarget,
                _ => Status::Different,
            };
            items.push(Item {
                key: key.clone(),
                status,
                statements: statements.iter().map(|(_, s)| s.clone()).collect(),
            });
            objects.insert(key.clone(), statements);
        }
        items.sort_by(|a, b| a.key.cmp(&b.key));
        Outcome {
            tables,
            objects,
            items,
            equal,
        }
    }

    /// O script para os objetos `chosen`, na ordem que o PostgreSQL aceita
    /// (veja [`Phase`]).
    pub fn script(&self, chosen: &BTreeSet<objects::Key>) -> Vec<String> {
        let names: BTreeSet<Name> = chosen
            .iter()
            .filter(|k| k.0 == Kind::Table)
            .map(|k| (k.1.clone(), k.2.clone()))
            .collect();
        let mut all: Vec<(Phase, String)> = Vec::new();
        if !names.is_empty() {
            all.extend(
                self.tables
                    .script(&names)
                    .into_iter()
                    .map(|s| (Phase::Tables, s)),
            );
        }
        for (key, statements) in &self.objects {
            if chosen.contains(key) {
                all.extend(statements.iter().cloned());
            }
        }
        // Estável: dentro de uma fase, a ordem da lista.
        all.sort_by_key(|(p, _)| *p);
        all.into_iter().map(|(_, s)| s).collect()
    }
}

/// A origem com os ids do destino onde schema e nome batem.
fn align(source: &Model, target: &Model) -> Model {
    let ids = |m: &Model| -> Id {
        m.tables
            .iter()
            .flat_map(|t| std::iter::once(t.id).chain(t.columns.iter().map(|c| c.id)))
            .max()
            .unwrap_or(0)
    };
    let mut next = ids(source).max(ids(target)).max(target.next_id) + 1;
    let mut fresh = || {
        next += 1;
        next
    };
    let tables = source
        .tables
        .iter()
        .map(|s| {
            let mut t = (**s).clone();
            let twin = target.find(&s.schema, &s.name);
            t.id = twin.map_or_else(&mut fresh, |o| o.id);
            for c in &mut t.columns {
                c.id = twin
                    .and_then(|o| o.column(&c.name))
                    .map_or_else(&mut fresh, |oc| oc.id);
            }
            t
        })
        .collect();
    let mut m = Model::new(tables);
    m.next_id = m.next_id.max(next + 1);
    m
}

/// Põe um `CREATE SEQUENCE IF NOT EXISTS` antes do script para cada
/// sequência que um `DEFAULT nextval('...')` novo usa (a coluna `serial`
/// de uma tabela que só existe na origem).
fn with_sequences(statements: Vec<String>) -> Vec<String> {
    let mut sequences: Vec<String> = Vec::new();
    for s in &statements {
        let mut rest = s.as_str();
        while let Some(i) = rest.find("nextval('") {
            rest = &rest[i + "nextval('".len()..];
            if let Some(end) = rest.find('\'') {
                let name = rest[..end].replace("''", "'");
                if !sequences.contains(&name) {
                    sequences.push(name);
                }
                rest = &rest[end..];
            }
        }
    }
    if sequences.is_empty() {
        return statements;
    }
    sequences
        .into_iter()
        .map(|n| format!("CREATE SEQUENCE IF NOT EXISTS {n};"))
        .chain(statements)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ForeignKey, Key};

    fn col(id: Id, name: &str, ty: &str) -> Column {
        Column {
            id,
            name: name.into(),
            ty: ty.into(),
            nullable: true,
            ..Default::default()
        }
    }

    fn cliente(base: Id, extra: bool) -> Table {
        let mut columns = vec![
            col(base + 1, "id", "integer"),
            col(base + 2, "nome", "text"),
        ];
        columns[0].nullable = false;
        if extra {
            columns.push(col(base + 3, "email", "text"));
        }
        Table {
            id: base,
            schema: "public".into(),
            name: "cliente".into(),
            columns,
            primary_key: Some(Key {
                name: "cliente_pkey".into(),
                columns: vec!["id".into()],
            }),
            ..Default::default()
        }
    }

    fn pedido(base: Id) -> Table {
        Table {
            id: base,
            schema: "public".into(),
            name: "pedido".into(),
            columns: vec![
                Column {
                    default: Some("nextval('pedido_id_seq'::regclass)".into()),
                    ..col(base + 1, "id", "integer")
                },
                col(base + 2, "cliente_id", "integer"),
            ],
            foreign_keys: vec![ForeignKey {
                name: "pedido_cliente_id_fkey".into(),
                columns: vec!["cliente_id".into()],
                ref_schema: "public".into(),
                ref_table: "cliente".into(),
                ref_columns: vec!["id".into()],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn velha(base: Id) -> Table {
        Table {
            id: base,
            schema: "public".into(),
            name: "velha".into(),
            columns: vec![col(base + 1, "x", "integer")],
            ..Default::default()
        }
    }

    #[test]
    fn tables_are_matched_by_name_across_databases() {
        // Os mesmos objetos com oids diferentes.
        let source = Model::new(vec![cliente(100, true), pedido(200)]);
        let target = Model::new(vec![cliente(5000, false), velha(6000)]);
        let c = Comparison::new(&source, &target);
        let summary: Vec<_> = c
            .tables
            .iter()
            .map(|t| (t.name.as_str(), t.status))
            .collect();
        assert_eq!(
            summary,
            [
                ("cliente", Status::Different),
                ("pedido", Status::OnlyInSource),
                ("velha", Status::OnlyInTarget),
            ]
        );
        assert_eq!(
            c.tables[0].statements,
            ["ALTER TABLE public.cliente ADD COLUMN email text;"]
        );
        assert_eq!(
            c.tables[1].statements[0],
            "CREATE SEQUENCE IF NOT EXISTS pedido_id_seq;"
        );
        assert!(c.tables[1].statements[1].starts_with("CREATE TABLE public.pedido"));
        assert_eq!(c.tables[2].statements, ["DROP TABLE public.velha;"]);

        // Tudo junto: depois o destino fica igual à origem.
        let all: BTreeSet<Name> = c.tables.iter().map(TableDiff::key).collect();
        let script = c.script(&all);
        assert!(
            script.last().unwrap().contains("FOREIGN KEY"),
            "{script:#?}"
        );
        let after = c.mix(&all);
        let again = Comparison::new(&source, &after);
        assert!(again.tables.is_empty(), "{:#?}", again.tables);
        assert_eq!(again.equal, 2);
    }

    #[test]
    fn an_unchosen_table_keeps_its_foreign_keys_unless_the_target_goes() {
        let source = Model::new(vec![cliente(100, false)]);
        let target = Model::new(vec![cliente(5000, false), pedido(6000)]);
        let c = Comparison::new(&source, &target);
        assert_eq!(c.tables.len(), 1);
        assert_eq!(c.tables[0].status, Status::OnlyInTarget);
        // Sem cliente no destino, pedido não muda; com ela saindo, a FK de
        // pedido sai junto.
        let source = Model::new(vec![pedido(100)]);
        let c = Comparison::new(&source, &target);
        let cliente = c.tables.iter().find(|t| t.name == "cliente").unwrap();
        assert_eq!(cliente.status, Status::OnlyInTarget);
        assert_eq!(
            cliente.statements,
            [
                "ALTER TABLE public.pedido DROP CONSTRAINT pedido_cliente_id_fkey;",
                "DROP TABLE public.cliente;"
            ]
        );
    }

    #[test]
    fn the_whole_schema_is_scripted_in_dependency_order() {
        use crate::objects::Object;
        let trg = |when: &str| {
            Object::trigger(
                "public",
                "pedido",
                "audita",
                &format!(
                    "CREATE TRIGGER audita {when} INSERT ON public.pedido FOR EACH ROW EXECUTE FUNCTION public.audita()"
                ),
            )
        };
        let source = Schema {
            model: Model::new(vec![cliente(100, false), pedido(200)]),
            objects: vec![
                Object::view("public", "v", "SELECT 1"),
                Object::enum_type("public", "status", vec!["a".into()]),
                trg("AFTER"),
            ],
        };
        let target = Schema {
            model: Model::new(vec![cliente(5000, false), pedido(6000)]),
            objects: vec![
                Object::view("public", "v", "SELECT 2"),
                Object::domain("public", "velho", "integer"),
                trg("BEFORE"),
            ],
        };
        let o = Outcome::new(&source, &target);
        let summary: Vec<_> = o
            .items
            .iter()
            .map(|i| (i.key.0, i.key.2.as_str(), i.status))
            .collect();
        assert_eq!(
            summary,
            [
                (Kind::View, "v", Status::Different),
                (Kind::Trigger, "pedido.audita", Status::Different),
                (Kind::Type, "status", Status::OnlyInSource),
                (Kind::Type, "velho", Status::OnlyInTarget),
            ]
        );
        assert_eq!(o.equal, 2);
        let all: BTreeSet<_> = o.items.iter().map(|i| i.key.clone()).collect();
        let script = o.script(&all);
        assert_eq!(
            script,
            [
                "DROP TRIGGER audita ON public.pedido;",
                "DROP VIEW public.v;",
                "CREATE TYPE public.status AS ENUM ('a');",
                "DROP DOMAIN public.velho;",
                "CREATE VIEW public.v AS\nSELECT 1;",
                "CREATE TRIGGER audita AFTER INSERT ON public.pedido FOR EACH ROW EXECUTE FUNCTION public.audita();",
            ]
        );
        // Só o tipo novo.
        let one = BTreeSet::from([(Kind::Type, "public".to_string(), "status".to_string())]);
        assert_eq!(o.script(&one), ["CREATE TYPE public.status AS ENUM ('a');"]);
    }
}
