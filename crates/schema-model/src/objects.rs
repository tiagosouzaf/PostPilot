//! O que o Schema Compare compara além das tabelas: views, funções,
//! procedures, triggers, sequências e tipos.
//!
//! Cada objeto guarda a definição como o servidor escreve
//! (`pg_get_viewdef`, `pg_get_functiondef`, `pg_get_triggerdef`) ou como o
//! PGStudio monta a partir do catálogo (sequências e tipos). Dois objetos
//! com a mesma chave e definições diferentes são "Different"; o DDL que leva
//! um ao outro sai de [`statements`], com a fase do script em que cada
//! comando entra.

use serde::{Deserialize, Serialize};

use crate::{qualified, quote_ident, tsql};

/// O tipo de objeto comparado, na ordem da lista do Schema Compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Kind {
    Table,
    View,
    Function,
    Procedure,
    Trigger,
    Sequence,
    Type,
}

impl Kind {
    pub fn label(self) -> &'static str {
        pgstudio_i18n::tr(match self {
            Kind::Table => "Table",
            Kind::View => "View",
            Kind::Function => "Function",
            Kind::Procedure => "Procedure",
            Kind::Trigger => "Trigger",
            Kind::Sequence => "Sequence",
            Kind::Type => "Type",
        })
    }
}

/// Um objeto: tipo, schema e nome (funções com os argumentos, triggers
/// como `tabela.trigger`).
pub type Key = (Kind, String, String);

/// Um objeto que não é tabela.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Object {
    pub kind: Kind,
    pub schema: String,
    /// O nome da chave: funções e procedures com os argumentos
    /// (`f(a integer)`), triggers com a tabela (`pedido.audita`).
    pub name: String,
    /// O texto comparado.
    pub definition: String,
    #[serde(default)]
    pub detail: Detail,
}

/// O que cada tipo de objeto precisa, além da definição, para o DDL.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Detail {
    #[default]
    None,
    /// Função ou procedure: o nome sem argumentos e os argumentos, para o
    /// `DROP`.
    Routine {
        name: String,
        args: String,
    },
    /// Trigger: a tabela e o nome, para o `DROP TRIGGER`.
    Trigger {
        table: String,
        trigger: String,
    },
    Sequence(Sequence),
    /// Enum: os valores, em ordem (valores novos entram com `ADD VALUE`).
    Enum(Vec<String>),
    /// Domínio (`DROP DOMAIN` em vez de `DROP TYPE`).
    Domain,
    /// SQL Server: view, função, procedure ou trigger, com o texto inteiro
    /// do `CREATE` (do `sys.sql_modules`) na definição. `name` é o nome
    /// para o `DROP` (num trigger, sem a tabela).
    Module {
        name: String,
    },
    /// SQL Server: sequência (sem `OWNED BY`, sem `IF NOT EXISTS`).
    TsqlSequence(Sequence),
    /// SQL Server: tipo de alias (`CREATE TYPE ... FROM ...`).
    AliasType,
}

/// As opções de uma sequência.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequence {
    /// `smallint`, `integer` ou `bigint`.
    pub ty: String,
    pub start: String,
    pub increment: String,
    pub min: String,
    pub max: String,
    pub cache: String,
    pub cycle: bool,
    /// A coluna dona (`schema.tabela.coluna`, com aspas), de um `serial`.
    pub owned_by: Option<String>,
}

impl Sequence {
    /// As opções como o `CREATE SEQUENCE` e o `ALTER SEQUENCE` escrevem.
    fn options(&self) -> String {
        format!(
            "AS {} INCREMENT BY {} MINVALUE {} MAXVALUE {} START WITH {} CACHE {} {}",
            self.ty,
            self.increment,
            self.min,
            self.max,
            self.start,
            self.cache,
            if self.cycle { "CYCLE" } else { "NO CYCLE" }
        )
    }
}

impl Sequence {
    /// As opções que o `ALTER SEQUENCE` do SQL Server aceita. `cache`
    /// vazio é o cache padrão e `0`, sem cache.
    fn tsql_alter_options(&self) -> String {
        let cache = match self.cache.as_str() {
            "" => "CACHE".to_string(),
            "0" => "NO CACHE".to_string(),
            n => format!("CACHE {n}"),
        };
        format!(
            "INCREMENT BY {} MINVALUE {} MAXVALUE {} {} {cache}",
            self.increment,
            self.min,
            self.max,
            if self.cycle { "CYCLE" } else { "NO CYCLE" },
        )
    }

    /// As opções do `CREATE SEQUENCE` do SQL Server.
    fn tsql_options(&self) -> String {
        format!(
            "AS {} START WITH {} {}",
            self.ty,
            self.start,
            self.tsql_alter_options()
        )
    }
}

/// Em que ponto do script um comando entra. O script segue esta ordem, que
/// é a que o PostgreSQL aceita: sai o que depende das tabelas, entram os
/// tipos e as sequências que elas usam, mudam as tabelas, e depois entra o
/// que depende delas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    DropTriggers,
    DropViews,
    DropRoutines,
    Types,
    Sequences,
    Tables,
    DropSequences,
    DropTypes,
    OwnedBy,
    Routines,
    Views,
    Triggers,
}

impl Object {
    pub fn key(&self) -> Key {
        (self.kind, self.schema.clone(), self.name.clone())
    }

    /// Uma view, do texto do `pg_get_viewdef`.
    pub fn view(schema: &str, name: &str, query: &str) -> Self {
        Object {
            kind: Kind::View,
            schema: schema.into(),
            name: name.into(),
            definition: query.trim().trim_end_matches(';').trim_end().to_string(),
            detail: Detail::None,
        }
    }

    /// Uma função ou procedure, do texto do `pg_get_functiondef`.
    pub fn routine(schema: &str, name: &str, args: &str, def: &str, procedure: bool) -> Self {
        Object {
            kind: if procedure {
                Kind::Procedure
            } else {
                Kind::Function
            },
            schema: schema.into(),
            name: format!("{name}({args})"),
            definition: def.trim().to_string(),
            detail: Detail::Routine {
                name: name.into(),
                args: args.into(),
            },
        }
    }

    /// Um trigger, do texto do `pg_get_triggerdef`.
    pub fn trigger(schema: &str, table: &str, trigger: &str, def: &str) -> Self {
        Object {
            kind: Kind::Trigger,
            schema: schema.into(),
            name: format!("{table}.{trigger}"),
            definition: def.trim().to_string(),
            detail: Detail::Trigger {
                table: table.into(),
                trigger: trigger.into(),
            },
        }
    }

    /// SQL Server: uma view, função, procedure ou trigger, do texto do
    /// `sys.sql_modules`. Num trigger, `table` é a tabela dele.
    pub fn tsql_module(
        kind: Kind,
        schema: &str,
        name: &str,
        table: Option<&str>,
        def: &str,
    ) -> Self {
        Object {
            kind,
            schema: schema.into(),
            name: match table {
                Some(table) => format!("{table}.{name}"),
                None => name.into(),
            },
            definition: tsql::plain_create(def.replace("\r\n", "\n").trim()),
            detail: Detail::Module { name: name.into() },
        }
    }

    /// SQL Server: uma sequência.
    pub fn tsql_sequence(schema: &str, name: &str, seq: Sequence) -> Self {
        Object {
            kind: Kind::Sequence,
            schema: schema.into(),
            name: name.into(),
            definition: seq.tsql_options(),
            detail: Detail::TsqlSequence(seq),
        }
    }

    /// SQL Server: um tipo de alias sobre `base` (`varchar(8)`).
    pub fn tsql_alias_type(schema: &str, name: &str, base: &str, nullable: bool) -> Self {
        Object::type_with(
            schema,
            name,
            format!(
                "CREATE TYPE {} FROM {base} {};",
                tsql::qualified(schema, name),
                if nullable { "NULL" } else { "NOT NULL" }
            ),
            Detail::AliasType,
        )
    }

    pub fn sequence(schema: &str, name: &str, seq: Sequence) -> Self {
        let mut definition = seq.options();
        if let Some(owner) = &seq.owned_by {
            definition.push_str(&format!(" OWNED BY {owner}"));
        }
        Object {
            kind: Kind::Sequence,
            schema: schema.into(),
            name: name.into(),
            definition,
            detail: Detail::Sequence(seq),
        }
    }

    /// Um enum com estes valores.
    pub fn enum_type(schema: &str, name: &str, labels: Vec<String>) -> Self {
        let values: Vec<String> = labels.iter().map(|l| literal(l)).collect();
        Object {
            kind: Kind::Type,
            schema: schema.into(),
            name: name.into(),
            definition: format!(
                "CREATE TYPE {} AS ENUM ({});",
                qualified(schema, name),
                values.join(", ")
            ),
            detail: Detail::Enum(labels),
        }
    }

    /// Um tipo composto: `body` são os atributos (`a integer, b text`).
    pub fn composite_type(schema: &str, name: &str, body: &str) -> Self {
        Self::type_with(
            schema,
            name,
            format!("CREATE TYPE {} AS ({body});", qualified(schema, name)),
            Detail::None,
        )
    }

    /// Um tipo range: `body` são as opções (`SUBTYPE = integer`).
    pub fn range_type(schema: &str, name: &str, body: &str) -> Self {
        Self::type_with(
            schema,
            name,
            format!("CREATE TYPE {} AS RANGE ({body});", qualified(schema, name)),
            Detail::None,
        )
    }

    /// Um domínio: `body` é o tipo base com default e constraints.
    pub fn domain(schema: &str, name: &str, body: &str) -> Self {
        Self::type_with(
            schema,
            name,
            format!("CREATE DOMAIN {} AS {body};", qualified(schema, name)),
            Detail::Domain,
        )
    }

    fn type_with(schema: &str, name: &str, definition: String, detail: Detail) -> Self {
        Object {
            kind: Kind::Type,
            schema: schema.into(),
            name: name.into(),
            definition,
            detail,
        }
    }

    /// `schema.nome` com as aspas necessárias (sem argumentos).
    fn qualified(&self) -> String {
        match &self.detail {
            Detail::Module { name } => tsql::qualified(&self.schema, name),
            Detail::TsqlSequence(_) | Detail::AliasType => {
                tsql::qualified(&self.schema, &self.name)
            }
            Detail::Routine { name, .. } => qualified(&self.schema, name),
            _ => qualified(&self.schema, &self.name),
        }
    }

    /// O DDL que cria o objeto.
    fn create(&self) -> Vec<(Phase, String)> {
        let q = self.qualified();
        match (&self.kind, &self.detail) {
            // O texto do módulo já é o `CREATE` inteiro, um lote só.
            (kind, Detail::Module { .. }) => vec![(
                match kind {
                    Kind::View => Phase::Views,
                    Kind::Trigger => Phase::Triggers,
                    _ => Phase::Routines,
                },
                self.definition.clone(),
            )],
            (_, Detail::TsqlSequence(s)) => vec![(
                Phase::Sequences,
                format!("CREATE SEQUENCE {q} {};", s.tsql_options()),
            )],
            (Kind::View, _) => vec![(
                Phase::Views,
                format!("CREATE VIEW {q} AS\n{};", self.definition),
            )],
            (Kind::Function | Kind::Procedure, _) => {
                vec![(Phase::Routines, format!("{};", self.definition))]
            }
            (Kind::Trigger, _) => vec![(Phase::Triggers, format!("{};", self.definition))],
            (Kind::Sequence, Detail::Sequence(s)) => {
                let mut out = vec![(
                    Phase::Sequences,
                    format!("CREATE SEQUENCE IF NOT EXISTS {q} {};", s.options()),
                )];
                if let Some(owner) = &s.owned_by {
                    out.push((
                        Phase::OwnedBy,
                        format!("ALTER SEQUENCE {q} OWNED BY {owner};"),
                    ));
                }
                out
            }
            _ => vec![(Phase::Types, self.definition.clone())],
        }
    }

    /// O DDL que apaga o objeto.
    fn drop(&self) -> Vec<(Phase, String)> {
        let q = self.qualified();
        let (phase, sql) = match (&self.kind, &self.detail) {
            (Kind::Function, Detail::Module { .. }) => {
                (Phase::DropRoutines, format!("DROP FUNCTION {q};"))
            }
            (Kind::Procedure, Detail::Module { .. }) => {
                (Phase::DropRoutines, format!("DROP PROCEDURE {q};"))
            }
            (Kind::Trigger, Detail::Module { .. }) => {
                (Phase::DropTriggers, format!("DROP TRIGGER {q};"))
            }
            (_, Detail::TsqlSequence(_)) => (Phase::DropSequences, format!("DROP SEQUENCE {q};")),
            (Kind::View, _) => (Phase::DropViews, format!("DROP VIEW {q};")),
            (Kind::Function, Detail::Routine { args, .. }) => {
                (Phase::DropRoutines, format!("DROP FUNCTION {q}({args});"))
            }
            (Kind::Procedure, Detail::Routine { args, .. }) => {
                (Phase::DropRoutines, format!("DROP PROCEDURE {q}({args});"))
            }
            (Kind::Trigger, Detail::Trigger { table, trigger }) => (
                Phase::DropTriggers,
                format!(
                    "DROP TRIGGER {} ON {};",
                    quote_ident(trigger),
                    qualified(&self.schema, table)
                ),
            ),
            // A sequência de um serial some junto com a tabela.
            (Kind::Sequence, _) => (
                Phase::DropSequences,
                format!("DROP SEQUENCE IF EXISTS {q};"),
            ),
            (_, Detail::Domain) => (Phase::DropTypes, format!("DROP DOMAIN {q};")),
            _ => (Phase::DropTypes, format!("DROP TYPE {q};")),
        };
        vec![(phase, sql)]
    }

    /// O DDL que muda `self` (no destino) para `source`, quando os dois
    /// existem e diferem.
    fn change_to(&self, source: &Object) -> Vec<(Phase, String)> {
        let q = self.qualified();
        match (&self.detail, &source.detail) {
            // `CREATE OR ALTER` troca o módulo no lugar, sem perder as
            // permissões nem o que depende dele.
            (Detail::Module { .. }, Detail::Module { .. }) => {
                match tsql::create_or_alter(&source.definition) {
                    Some(sql) => source
                        .create()
                        .into_iter()
                        .map(|(phase, _)| (phase, sql.clone()))
                        .collect(),
                    None => [self.drop(), source.create()].concat(),
                }
            }
            // O tipo e o início não mudam com `ALTER SEQUENCE`.
            (Detail::TsqlSequence(t), Detail::TsqlSequence(s))
                if t.ty.eq_ignore_ascii_case(&s.ty) && t.start == s.start =>
            {
                vec![(
                    Phase::Sequences,
                    format!("ALTER SEQUENCE {q} {};", s.tsql_alter_options()),
                )]
            }
            (Detail::TsqlSequence(_), Detail::TsqlSequence(_)) => {
                let mut out: Vec<(Phase, String)> = self
                    .drop()
                    .into_iter()
                    .map(|(_, s)| (Phase::Sequences, s))
                    .collect();
                out.extend(source.create());
                out
            }
            (Detail::Sequence(t), Detail::Sequence(s)) => {
                let mut out = Vec::new();
                if t.options() != s.options() {
                    out.push((
                        Phase::Sequences,
                        format!("ALTER SEQUENCE {q} {};", s.options()),
                    ));
                }
                if t.owned_by != s.owned_by {
                    let owner = s.owned_by.as_deref().unwrap_or("NONE");
                    out.push((
                        Phase::OwnedBy,
                        format!("ALTER SEQUENCE {q} OWNED BY {owner};"),
                    ));
                }
                out
            }
            (Detail::Enum(t), Detail::Enum(s)) if is_subsequence(t, s) => {
                let mut out = Vec::new();
                for (i, label) in s.iter().enumerate() {
                    if t.contains(label) {
                        continue;
                    }
                    let place = match (i.checked_sub(1).map(|p| &s[p]), t.first()) {
                        (Some(prev), _) => format!(" AFTER {}", literal(prev)),
                        (None, Some(first)) => format!(" BEFORE {}", literal(first)),
                        (None, None) => String::new(),
                    };
                    out.push((
                        Phase::Types,
                        format!("ALTER TYPE {q} ADD VALUE {}{place};", literal(label)),
                    ));
                }
                out
            }
            // `CREATE OR REPLACE` mantém o que depende da função (triggers,
            // views); só não troca o tipo de retorno.
            (Detail::Routine { .. }, _)
                if returns(&self.definition) == returns(&source.definition) =>
            {
                source.create()
            }
            (Detail::Domain | Detail::None | Detail::Enum(_) | Detail::AliasType, _)
                if self.kind == Kind::Type =>
            {
                // Recriar um tipo é "apagar e criar" no mesmo ponto, antes
                // das tabelas.
                let mut out: Vec<(Phase, String)> = self
                    .drop()
                    .into_iter()
                    .map(|(_, s)| (Phase::Types, s))
                    .collect();
                out.extend(source.create());
                out
            }
            _ => [self.drop(), source.create()].concat(),
        }
    }
}

/// O DDL que leva o destino (`target`) à origem (`source`) neste objeto,
/// com a fase de cada comando, na ordem do script. Vazio se são iguais.
pub fn statements(source: Option<&Object>, target: Option<&Object>) -> Vec<(Phase, String)> {
    let mut out = match (source, target) {
        (Some(s), None) => s.create(),
        (None, Some(t)) => t.drop(),
        (Some(s), Some(t)) if s.definition != t.definition => t.change_to(s),
        _ => Vec::new(),
    };
    out.sort_by_key(|(p, _)| *p);
    out
}

/// A linha `RETURNS ...` do `pg_get_functiondef` (vazia numa procedure).
fn returns(def: &str) -> &str {
    def.lines()
        .map(str::trim)
        .find(|l| l.starts_with("RETURNS "))
        .unwrap_or("")
}

/// `small` está em `big`, na mesma ordem?
fn is_subsequence(small: &[String], big: &[String]) -> bool {
    let mut it = big.iter();
    small.iter().all(|s| it.any(|b| b == s))
}

/// Uma string SQL entre aspas simples.
pub fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsql_modules_change_in_place_and_drop_by_name() {
        let old = Object::tsql_module(
            Kind::View,
            "dbo",
            "v",
            None,
            "-- vendas\r\ncreate view dbo.v as select 1 as a\r\n",
        );
        let new = Object::tsql_module(
            Kind::View,
            "dbo",
            "v",
            None,
            "-- vendas\ncreate view dbo.v as select 2 as a",
        );
        assert_eq!(
            statements(Some(&new), Some(&old)),
            [(
                Phase::Views,
                "-- vendas\ncreate OR ALTER view dbo.v as select 2 as a".to_string()
            )]
        );
        assert_eq!(
            statements(Some(&new), None),
            [(Phase::Views, new.definition.clone())]
        );
        assert!(statements(Some(&old), Some(&old)).is_empty());
        let trigger = Object::tsql_module(
            Kind::Trigger,
            "dbo",
            "audita",
            Some("pedido"),
            "CREATE TRIGGER dbo.audita ON dbo.pedido AFTER INSERT AS SET NOCOUNT ON",
        );
        assert_eq!(trigger.name, "pedido.audita");
        assert_eq!(
            statements(None, Some(&trigger)),
            [(
                Phase::DropTriggers,
                "DROP TRIGGER [dbo].[audita];".to_string()
            )]
        );
    }

    #[test]
    fn tsql_sequences_and_alias_types() {
        let seq = |increment: &str, ty: &str| {
            Object::tsql_sequence(
                "dbo",
                "numero",
                Sequence {
                    ty: ty.into(),
                    start: "1".into(),
                    increment: increment.into(),
                    min: "1".into(),
                    max: "999".into(),
                    cache: String::new(),
                    cycle: false,
                    owned_by: None,
                },
            )
        };
        assert_eq!(
            sql(statements(Some(&seq("1", "int")), None)),
            [
                "CREATE SEQUENCE [dbo].[numero] AS int START WITH 1 INCREMENT BY 1 \
              MINVALUE 1 MAXVALUE 999 NO CYCLE CACHE;"
            ]
        );
        assert_eq!(
            sql(statements(Some(&seq("5", "int")), Some(&seq("1", "int")))),
            [
                "ALTER SEQUENCE [dbo].[numero] INCREMENT BY 5 MINVALUE 1 MAXVALUE 999 NO CYCLE CACHE;"
            ]
        );
        assert_eq!(
            sql(statements(
                Some(&seq("1", "bigint")),
                Some(&seq("1", "int"))
            ))
            .len(),
            2
        );
        let cep = Object::tsql_alias_type("dbo", "cep", "char(8)", false);
        assert_eq!(
            cep.definition,
            "CREATE TYPE [dbo].[cep] FROM char(8) NOT NULL;"
        );
        assert_eq!(
            sql(statements(None, Some(&cep))),
            ["DROP TYPE [dbo].[cep];"]
        );
    }

    fn sql(v: Vec<(Phase, String)>) -> Vec<String> {
        v.into_iter().map(|(_, s)| s).collect()
    }

    fn seq(ty: &str, owned_by: Option<&str>) -> Sequence {
        Sequence {
            ty: ty.into(),
            start: "1".into(),
            increment: "1".into(),
            min: "1".into(),
            max: "2147483647".into(),
            cache: "1".into(),
            cycle: false,
            owned_by: owned_by.map(Into::into),
        }
    }

    #[test]
    fn sequences_are_altered_not_recreated() {
        let s = Object::sequence(
            "public",
            "pedido_seq",
            seq("integer", Some("public.pedido.id")),
        );
        assert_eq!(
            sql(statements(Some(&s), None)),
            [
                "CREATE SEQUENCE IF NOT EXISTS public.pedido_seq AS integer INCREMENT BY 1 MINVALUE 1 MAXVALUE 2147483647 START WITH 1 CACHE 1 NO CYCLE;",
                "ALTER SEQUENCE public.pedido_seq OWNED BY public.pedido.id;"
            ]
        );
        let mut other = seq("integer", None);
        other.increment = "10".into();
        let t = Object::sequence("public", "pedido_seq", other);
        let change = statements(Some(&s), Some(&t));
        assert_eq!(change[0].0, Phase::Sequences);
        assert!(
            change[0]
                .1
                .starts_with("ALTER SEQUENCE public.pedido_seq AS integer INCREMENT BY 1 ")
        );
        assert_eq!(
            change[1],
            (
                Phase::OwnedBy,
                "ALTER SEQUENCE public.pedido_seq OWNED BY public.pedido.id;".into()
            )
        );
        assert_eq!(
            sql(statements(None, Some(&t))),
            ["DROP SEQUENCE IF EXISTS public.pedido_seq;"]
        );
        assert!(statements(Some(&s), Some(&s.clone())).is_empty());
    }

    #[test]
    fn enums_grow_with_add_value() {
        let labels = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let s = Object::enum_type("public", "Status", labels(&["novo", "pago", "it's", "fim"]));
        assert_eq!(
            s.definition,
            "CREATE TYPE public.\"Status\" AS ENUM ('novo', 'pago', 'it''s', 'fim');"
        );
        let t = Object::enum_type("public", "Status", labels(&["pago", "fim"]));
        assert_eq!(
            sql(statements(Some(&s), Some(&t))),
            [
                "ALTER TYPE public.\"Status\" ADD VALUE 'novo' BEFORE 'pago';",
                "ALTER TYPE public.\"Status\" ADD VALUE 'it''s' AFTER 'pago';",
            ]
        );
        // Tirar ou reordenar valores não tem ALTER: apaga e cria.
        let t = Object::enum_type("public", "Status", labels(&["fim", "pago"]));
        let change = statements(Some(&s), Some(&t));
        assert!(change.iter().all(|(p, _)| *p == Phase::Types));
        assert_eq!(change[0].1, "DROP TYPE public.\"Status\";");
        assert_eq!(change[1].1, s.definition);
    }

    #[test]
    fn domains_composites_and_ranges() {
        let d = Object::domain("public", "cep", "text CHECK (VALUE ~ '^[0-9]{8}$')");
        assert_eq!(
            d.definition,
            "CREATE DOMAIN public.cep AS text CHECK (VALUE ~ '^[0-9]{8}$');"
        );
        assert_eq!(sql(statements(None, Some(&d))), ["DROP DOMAIN public.cep;"]);
        let c = Object::composite_type("public", "par", "a integer, b text");
        assert_eq!(
            c.definition,
            "CREATE TYPE public.par AS (a integer, b text);"
        );
        let r = Object::range_type("public", "faixa", "SUBTYPE = integer");
        assert_eq!(sql(statements(None, Some(&r))), ["DROP TYPE public.faixa;"]);
        assert_eq!(statements(Some(&r), None)[0].0, Phase::Types);
    }

    #[test]
    fn routines_are_replaced_unless_the_return_type_changes() {
        let def = |ret: &str, body: &str| {
            format!(
                "CREATE OR REPLACE FUNCTION public.f(a integer)\n RETURNS {ret}\n LANGUAGE sql\nAS $function$select {body}$function$\n"
            )
        };
        let s = Object::routine("public", "f", "a integer", &def("integer", "a"), false);
        assert_eq!(s.name, "f(a integer)");
        let t = Object::routine("public", "f", "a integer", &def("integer", "1"), false);
        let change = statements(Some(&s), Some(&t));
        assert_eq!(change.len(), 1);
        assert_eq!(change[0].0, Phase::Routines);
        let t = Object::routine("public", "f", "a integer", &def("bigint", "1"), false);
        assert_eq!(
            sql(statements(Some(&s), Some(&t)))[0],
            "DROP FUNCTION public.f(a integer);"
        );
        let p = Object::routine(
            "public",
            "p",
            "",
            "CREATE OR REPLACE PROCEDURE public.p()",
            true,
        );
        assert_eq!(p.kind, Kind::Procedure);
        assert_eq!(
            sql(statements(None, Some(&p))),
            ["DROP PROCEDURE public.p();"]
        );
    }

    #[test]
    fn triggers_are_dropped_and_created() {
        let def = |when: &str| {
            format!(
                "CREATE TRIGGER audita {when} INSERT ON public.pedido FOR EACH ROW EXECUTE FUNCTION audita()"
            )
        };
        let s = Object::trigger("public", "pedido", "audita", &def("AFTER"));
        let t = Object::trigger("public", "pedido", "audita", &def("BEFORE"));
        assert_eq!(
            s.key(),
            (Kind::Trigger, "public".into(), "pedido.audita".into())
        );
        let change = statements(Some(&s), Some(&t));
        assert_eq!(
            change,
            [
                (
                    Phase::DropTriggers,
                    "DROP TRIGGER audita ON public.pedido;".into()
                ),
                (Phase::Triggers, format!("{};", def("AFTER"))),
            ]
        );
    }
}
