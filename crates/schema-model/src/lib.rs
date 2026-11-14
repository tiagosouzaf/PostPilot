//! Modelo do schema de um banco em memória, a base do Table Designer, dos
//! diagramas e do Query Designer (Fase 3).
//!
//! - [`Model`]: tabelas com colunas, chave primária, unique, check, chaves
//!   estrangeiras e índices, como o `pg-metadata` lê do `pg_catalog`.
//! - [`edit`]: edições com desfazer e refazer ([`edit::History`]). Uma edição
//!   que mexe em nomes mantém o resto coerente (renomear uma coluna renomeia
//!   ela nas chaves e nas FKs de outras tabelas que apontam para ela).
//! - [`diff`]: o DDL que leva o banco de um modelo ao outro, na ordem em que
//!   o PostgreSQL aceita.
//!
//! Tabelas e colunas têm um [`Id`] estável: o do catálogo (oid da tabela,
//! número da coluna) ou um novo, de [`Model::new_id`]. É por ele que o diff
//! reconhece um objeto renomeado. Constraints e índices são reconhecidos
//! pelo nome.

use std::sync::Arc;

pub mod diff;
pub mod edit;
mod ident;

pub use ident::quote_ident;

/// Identificador estável de uma tabela ou coluna.
pub type Id = u64;

/// Primeiro [`Id`] dado a objetos novos, acima de qualquer oid.
pub const FIRST_NEW_ID: Id = 1 << 40;

/// O schema de um banco (as tabelas dos schemas de usuário).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    /// As tabelas, compartilhadas: copiar o modelo (o undo faz isso a cada
    /// edição) só copia ponteiros, e mudar uma tabela copia só ela.
    pub tables: Vec<Arc<Table>>,
    /// Próximo [`Id`] livre para objetos novos.
    pub next_id: Id,
}

/// Uma tabela.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub id: Id,
    pub schema: String,
    pub name: String,
    pub columns: Vec<Column>,
    pub primary_key: Option<Key>,
    pub uniques: Vec<Key>,
    pub checks: Vec<Check>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<Index>,
}

/// Uma coluna.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Column {
    pub id: Id,
    pub name: String,
    /// Tipo como o `format_type` escreve (`integer`, `character varying(80)`).
    pub ty: String,
    pub nullable: bool,
    /// Expressão do `DEFAULT`, como o `pg_get_expr` escreve.
    pub default: Option<String>,
    pub identity: Option<Identity>,
    /// Expressão de uma coluna `GENERATED ALWAYS AS (...) STORED`.
    pub generated: Option<String>,
}

/// `GENERATED ... AS IDENTITY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    Always,
    ByDefault,
}

impl Identity {
    fn sql(self) -> &'static str {
        match self {
            Identity::Always => "ALWAYS",
            Identity::ByDefault => "BY DEFAULT",
        }
    }
}

/// Chave primária ou unique: o nome da constraint e as colunas, em ordem.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub name: String,
    pub columns: Vec<String>,
}

/// Constraint `CHECK`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    /// A expressão, sem o `CHECK (...)` em volta.
    pub expr: String,
}

/// Chave estrangeira.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    pub on_update: Action,
    pub on_delete: Action,
}

/// O que uma FK faz quando a linha referenciada muda ou some.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}

impl Action {
    /// Pela letra de `confupdtype`/`confdeltype` do `pg_constraint`.
    pub fn from_catalog(code: &str) -> Self {
        match code {
            "r" => Action::Restrict,
            "c" => Action::Cascade,
            "n" => Action::SetNull,
            "d" => Action::SetDefault,
            _ => Action::NoAction,
        }
    }

    /// Todas, na ordem do SSMS.
    pub const ALL: [Action; 5] = [
        Action::NoAction,
        Action::Cascade,
        Action::SetNull,
        Action::SetDefault,
        Action::Restrict,
    ];

    /// Como o SQL escreve (`NO ACTION`, `CASCADE`...).
    pub fn sql(self) -> &'static str {
        match self {
            Action::NoAction => "NO ACTION",
            Action::Restrict => "RESTRICT",
            Action::Cascade => "CASCADE",
            Action::SetNull => "SET NULL",
            Action::SetDefault => "SET DEFAULT",
        }
    }
}

/// Índice que não pertence a uma constraint (as de PK e unique já criam o
/// seu).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub unique: bool,
    /// Método de acesso (`btree`, `gin`...).
    pub method: String,
    /// Uma entrada por coluna do índice: um nome ou uma expressão, como o
    /// `pg_get_indexdef` escreve.
    pub columns: Vec<String>,
    /// Condição de um índice parcial (`WHERE`).
    pub predicate: Option<String>,
}

impl Model {
    /// Modelo com estas tabelas; os novos ids começam acima dos que elas
    /// usam.
    pub fn new(tables: Vec<Table>) -> Self {
        let used = tables
            .iter()
            .flat_map(|t| std::iter::once(t.id).chain(t.columns.iter().map(|c| c.id)))
            .max()
            .unwrap_or(0);
        Model {
            tables: tables.into_iter().map(Arc::new).collect(),
            next_id: (used + 1).max(FIRST_NEW_ID),
        }
    }

    /// Um [`Id`] que nenhum objeto usa.
    pub fn new_id(&mut self) -> Id {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn table(&self, id: Id) -> Option<&Table> {
        self.tables.iter().find(|t| t.id == id).map(|t| &**t)
    }

    /// A tabela para mudar (copiada se outro modelo também a usa).
    pub fn table_mut(&mut self, id: Id) -> Option<&mut Table> {
        self.tables
            .iter_mut()
            .find(|t| t.id == id)
            .map(Arc::make_mut)
    }

    /// Põe uma tabela no fim.
    pub fn push(&mut self, table: Table) {
        self.tables.push(Arc::new(table));
    }

    /// Tabela pelo schema e nome.
    pub fn find(&self, schema: &str, name: &str) -> Option<&Table> {
        self.tables
            .iter()
            .find(|t| t.schema == schema && t.name == name)
            .map(|t| &**t)
    }
}

impl Table {
    /// `schema.tabela` com as aspas necessárias.
    pub fn qualified(&self) -> String {
        qualified(&self.schema, &self.name)
    }

    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    pub fn column_by_id(&self, id: Id) -> Option<&Column> {
        self.columns.iter().find(|c| c.id == id)
    }
}

/// `schema.nome` com as aspas necessárias.
pub fn qualified(schema: &str, name: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(name))
}

/// Lista de nomes com aspas, separados por vírgula.
pub(crate) fn idents(names: &[String]) -> String {
    names
        .iter()
        .map(|n| quote_ident(n))
        .collect::<Vec<_>>()
        .join(", ")
}

impl Column {
    /// A definição da coluna no `CREATE TABLE` ou no `ADD COLUMN`.
    pub fn definition(&self) -> String {
        let mut s = format!("{} {}", quote_ident(&self.name), self.ty);
        if let Some(expr) = &self.generated {
            s.push_str(&format!(" GENERATED ALWAYS AS ({expr}) STORED"));
        }
        if let Some(identity) = self.identity {
            s.push_str(&format!(" GENERATED {} AS IDENTITY", identity.sql()));
        }
        if !self.nullable {
            s.push_str(" NOT NULL");
        }
        if let Some(default) = &self.default {
            s.push_str(&format!(" DEFAULT {default}"));
        }
        s
    }
}

impl ForeignKey {
    /// O `FOREIGN KEY (...) REFERENCES ...` do `ADD CONSTRAINT`.
    pub fn definition(&self) -> String {
        let mut s = format!(
            "FOREIGN KEY ({}) REFERENCES {} ({})",
            idents(&self.columns),
            qualified(&self.ref_schema, &self.ref_table),
            idents(&self.ref_columns)
        );
        if self.on_update != Action::NoAction {
            s.push_str(&format!(" ON UPDATE {}", self.on_update.sql()));
        }
        if self.on_delete != Action::NoAction {
            s.push_str(&format!(" ON DELETE {}", self.on_delete.sql()));
        }
        s
    }
}

impl Index {
    /// O `CREATE INDEX` do índice na tabela.
    pub fn create(&self, table: &Table) -> String {
        let mut s = format!(
            "CREATE {}INDEX {} ON {}",
            if self.unique { "UNIQUE " } else { "" },
            quote_ident(&self.name),
            table.qualified()
        );
        if !self.method.is_empty() && self.method != "btree" {
            s.push_str(&format!(" USING {}", self.method));
        }
        let columns: Vec<String> = self.columns.iter().map(|c| index_column(c)).collect();
        s.push_str(&format!(" ({})", columns.join(", ")));
        if let Some(p) = &self.predicate {
            s.push_str(&format!(" WHERE {p}"));
        }
        s.push(';');
        s
    }
}

/// Uma entrada de índice: o nome de uma coluna vai com aspas se precisar;
/// uma expressão (com parênteses ou espaço) vai como está.
fn index_column(c: &str) -> String {
    if c.contains(['(', ' ', '"']) {
        c.to_string()
    } else {
        quote_ident(c)
    }
}

/// Troca a coluna `old` por `new` numa expressão SQL (índice, check,
/// coluna gerada), como o PostgreSQL faz ao renomear: só identificadores
/// soltos, fora de strings, que não sejam nome de função, de tipo (`::x`)
/// nem qualificados (`t.x`).
pub fn rename_in_expr(expr: &str, old: &str, new: &str) -> String {
    let b = expr.as_bytes();
    let mut out = String::with_capacity(expr.len());
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\'' {
            i += 1;
            while i < b.len() {
                if b[i] == b'\'' {
                    if b.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        let start = i;
        let ident = if c == b'"' {
            i += 1;
            let mut name = String::new();
            while i < b.len() {
                if b[i] == b'"' {
                    if b.get(i + 1) == Some(&b'"') {
                        name.push('"');
                        i += 2;
                        continue;
                    }
                    break;
                }
                let ch = expr[i..].chars().next().unwrap();
                name.push(ch);
                i += ch.len_utf8();
            }
            i += 1;
            Some(name)
        } else if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            while i < b.len()
                && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] >= 0x80)
            {
                i += 1;
            }
            Some(expr[start..i].to_lowercase())
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                i += 1;
            }
            None
        } else {
            i += 1;
            None
        };
        let Some(ident) = ident else { continue };
        let before = expr[..start].trim_end();
        let after = expr[i.min(expr.len())..].trim_start();
        if ident == old
            && !before.ends_with('.')
            && !before.ends_with("::")
            && !after.starts_with('(')
            && !after.starts_with('.')
        {
            out.push_str(&expr[last..start]);
            out.push_str(&quote_ident(new));
            last = i.min(expr.len());
        }
    }
    out.push_str(&expr[last..]);
    out
}

/// Nome padrão que o PostgreSQL daria a uma constraint: `tabela_pkey`,
/// `tabela_coluna_key`, `tabela_coluna_fkey`, `tabela_coluna_check`.
pub fn default_constraint_name(table: &str, columns: &[String], suffix: &str) -> String {
    let mut parts = vec![table.to_string()];
    parts.extend(columns.iter().cloned());
    parts.push(suffix.to_string());
    let name = parts.join("_");
    // Nomes passam de 63 bytes são cortados pelo servidor; corta antes.
    let mut end = name.len().min(63);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renaming_a_column_inside_expressions() {
        let r = |e: &str| rename_in_expr(e, "email", "e-mail");
        assert_eq!(r("lower(email)"), "lower(\"e-mail\")");
        assert_eq!(
            r("(email <> 'email'::text)"),
            "(\"e-mail\" <> 'email'::text)"
        );
        assert_eq!(
            r("t.email || \"email\" || email(x)"),
            "t.email || \"e-mail\" || email(x)"
        );
        assert_eq!(
            r("emails > 0 AND x::email IS NULL"),
            "emails > 0 AND x::email IS NULL"
        );
        assert_eq!(
            rename_in_expr("qty > 0", "qty", "quantidade"),
            "quantidade > 0"
        );
        assert_eq!(rename_in_expr("\"Qty\" > 0", "Qty", "q"), "q > 0");
        assert_eq!(rename_in_expr("1.5e3 * a", "e3", "x"), "1.5e3 * a");
    }

    pub(crate) fn column(id: Id, name: &str, ty: &str, nullable: bool) -> Column {
        Column {
            id,
            name: name.into(),
            ty: ty.into(),
            nullable,
            ..Default::default()
        }
    }

    #[test]
    fn new_ids_start_above_the_catalog() {
        let mut m = Model::new(vec![Table {
            id: 16384,
            columns: vec![column(1, "id", "integer", false)],
            ..Default::default()
        }]);
        assert_eq!(m.new_id(), FIRST_NEW_ID);
        assert_eq!(m.new_id(), FIRST_NEW_ID + 1);
        let mut more = Model::new(vec![Table {
            id: FIRST_NEW_ID + 7,
            ..Default::default()
        }]);
        assert_eq!(more.new_id(), FIRST_NEW_ID + 8);
    }

    #[test]
    fn column_and_index_definitions() {
        let mut c = column(1, "user", "integer", false);
        c.identity = Some(Identity::Always);
        assert_eq!(
            c.definition(),
            "\"user\" integer GENERATED ALWAYS AS IDENTITY NOT NULL"
        );
        let mut c = column(2, "Total", "numeric(10,2)", true);
        c.default = Some("0".into());
        assert_eq!(c.definition(), "\"Total\" numeric(10,2) DEFAULT 0");

        let t = Table {
            schema: "public".into(),
            name: "pedido".into(),
            ..Default::default()
        };
        let i = Index {
            name: "pedido_x".into(),
            unique: true,
            method: "btree".into(),
            columns: vec!["cliente_id".into(), "lower(status)".into()],
            predicate: Some("(total > 0)".into()),
        };
        assert_eq!(
            i.create(&t),
            "CREATE UNIQUE INDEX pedido_x ON public.pedido (cliente_id, lower(status)) WHERE (total > 0);"
        );
    }

    #[test]
    fn foreign_key_definition() {
        let fk = ForeignKey {
            name: "pedido_cliente_id_fkey".into(),
            columns: vec!["cliente_id".into()],
            ref_schema: "public".into(),
            ref_table: "cliente".into(),
            ref_columns: vec!["id".into()],
            on_update: Action::NoAction,
            on_delete: Action::Cascade,
        };
        assert_eq!(
            fk.definition(),
            "FOREIGN KEY (cliente_id) REFERENCES public.cliente (id) ON DELETE CASCADE"
        );
    }

    #[test]
    fn default_names_are_cut_at_63_bytes() {
        assert_eq!(
            default_constraint_name("pedido", &["cliente_id".into()], "fkey"),
            "pedido_cliente_id_fkey"
        );
        let long = "x".repeat(70);
        assert_eq!(default_constraint_name(&long, &[], "pkey").len(), 63);
    }
}
