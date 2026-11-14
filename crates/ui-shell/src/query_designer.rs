//! Query Designer (Ctrl+Shift+Q na janela de consulta), como o "Design
//! Query in Editor" do SSMS.
//!
//! Em cima, as tabelas da consulta no [`pgstudio_diagram_engine`], com uma
//! caixinha por coluna e uma linha por JOIN; ao adicionar uma tabela, as
//! FKs com as que já estão viram JOINs, e arrastar uma coluna até outra
//! cria um JOIN novo. Embaixo, a grade de colunas (apelido, saída,
//! ordenação, filtro e agrupamento) e o SQL gerado, que vai para o editor
//! no OK.
//!
//! É de mão dupla (marco 5.5): com um SELECT selecionado no editor, o
//! designer abre com ele montado ([`read`]) e o OK troca a seleção pelo SQL
//! novo. O que ele não sabe mostrar (CTE, subconsulta, UNION...) vira um
//! aviso, e aí o OK põe a consulta depois da seleção, sem apagá-la.
//!
//! Desde o marco 6.1 ele também desenha o que o SSMS desenha: JOINs
//! externos (as propriedades do JOIN, "all rows from"), DISTINCT, LIMIT (o
//! Top do SSMS), HAVING (filtro numa coluna agregada), OR (as colunas Or...
//! da grade), expressões no lugar de colunas e `count(*)`.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::metadata::{MetadataError, MetadataProvider, Model};
use pgstudio_core::schema::{Id, quote_ident};
use pgstudio_diagram_engine::{Diagram, DiagramEvent, Edge, Node, NodeColumn};

use crate::diagrams::{Picker, table_picker};

pub mod read;

/// Uma tabela na consulta (a mesma tabela pode entrar duas vezes, com
/// apelidos diferentes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryTable {
    pub table: Id,
    /// Nome usado nas colunas: o da tabela, ou `tabela_N` se repetir.
    pub alias: String,
}

/// Um JOIN entre as tabelas `left` e `right` (índices em
/// [`Query::tables`]), com os pares de colunas (esquerda, direita).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Join {
    pub left: usize,
    pub right: usize,
    pub on: Vec<(String, String)>,
    /// "Select All Rows from" a tabela `left` (como nas propriedades do
    /// JOIN no SSMS).
    pub all_left: bool,
    /// "Select All Rows from" a tabela `right`.
    pub all_right: bool,
}

impl Join {
    /// O tipo do JOIN no SQL, que entra na tabela que vem depois no FROM.
    pub fn kind(&self) -> JoinKind {
        let (all_first, all_second) = if self.left < self.right {
            (self.all_left, self.all_right)
        } else {
            (self.all_right, self.all_left)
        };
        match (all_first, all_second) {
            (false, false) => JoinKind::Inner,
            (true, false) => JoinKind::Left,
            (false, true) => JoinKind::Right,
            (true, true) => JoinKind::Full,
        }
    }

    /// Muda os "all rows from" para dar o tipo `kind`, em relação à ordem
    /// das tabelas no FROM.
    pub fn set_kind(&mut self, kind: JoinKind) {
        let (first, second) = match kind {
            JoinKind::Inner => (false, false),
            JoinKind::Left => (true, false),
            JoinKind::Right => (false, true),
            JoinKind::Full => (true, true),
        };
        if self.left < self.right {
            (self.all_left, self.all_right) = (first, second);
        } else {
            (self.all_right, self.all_left) = (first, second);
        }
    }
}

/// O tipo de um JOIN, em relação à ordem das tabelas no FROM.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JoinKind {
    #[default]
    Inner,
    /// Todas as linhas da tabela que vem antes.
    Left,
    /// Todas as linhas da tabela que vem depois.
    Right,
    Full,
}

impl JoinKind {
    pub const ALL: [JoinKind; 4] = [
        JoinKind::Inner,
        JoinKind::Left,
        JoinKind::Right,
        JoinKind::Full,
    ];

    pub fn sql(self) -> &'static str {
        match self {
            JoinKind::Inner => "INNER JOIN",
            JoinKind::Left => "LEFT JOIN",
            JoinKind::Right => "RIGHT JOIN",
            JoinKind::Full => "FULL JOIN",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortType {
    #[default]
    Unsorted,
    Ascending,
    Descending,
}

impl SortType {
    pub const ALL: [SortType; 3] = [
        SortType::Unsorted,
        SortType::Ascending,
        SortType::Descending,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortType::Unsorted => "Unsorted",
            SortType::Ascending => "Ascending",
            SortType::Descending => "Descending",
        }
    }
}

/// O que a coluna vira com o Group By ligado.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Aggregate {
    #[default]
    GroupBy,
    Count,
    Sum,
    Avg,
    Min,
    Max,
    /// Só filtra (vai para o WHERE), fora do GROUP BY e da saída.
    Where,
    /// Vai para a saída como está, fora do GROUP BY: uma expressão que já
    /// agrega (`sum(a) / count(*)`), como o "Expression" do SSMS.
    Expression,
}

impl Aggregate {
    pub const ALL: [Aggregate; 8] = [
        Aggregate::GroupBy,
        Aggregate::Count,
        Aggregate::Sum,
        Aggregate::Avg,
        Aggregate::Min,
        Aggregate::Max,
        Aggregate::Where,
        Aggregate::Expression,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Aggregate::GroupBy => "Group By",
            Aggregate::Count => "Count",
            Aggregate::Sum => "Sum",
            Aggregate::Avg => "Avg",
            Aggregate::Min => "Min",
            Aggregate::Max => "Max",
            Aggregate::Where => "Where",
            Aggregate::Expression => "Expression",
        }
    }

    fn function(self) -> Option<&'static str> {
        match self {
            Aggregate::GroupBy | Aggregate::Where | Aggregate::Expression => None,
            Aggregate::Count => Some("count"),
            Aggregate::Sum => Some("sum"),
            Aggregate::Avg => Some("avg"),
            Aggregate::Min => Some("min"),
            Aggregate::Max => Some("max"),
        }
    }
}

/// Uma linha da grade de colunas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// Índice em [`Query::tables`].
    pub table: usize,
    pub column: String,
    /// `AS` na lista do SELECT.
    pub alias: String,
    /// Vai para o SELECT (a caixinha da coluna).
    pub output: bool,
    pub sort: SortType,
    /// Ordem entre as colunas ordenadas (1 primeiro); 0 segue a da grade.
    pub sort_order: u32,
    /// Condição do WHERE: `> 10`, `LIKE 'a%'`; sem operador vira `= ...`.
    /// Numa coluna agregada (Count, Sum...), vai para o HAVING.
    pub filter: String,
    /// As colunas Or... da grade: cada coluna de critérios é um grupo
    /// ligado por AND, e os grupos se ligam por OR.
    pub or: Vec<String>,
    pub aggregate: Aggregate,
    /// `column` é uma expressão SQL (`lower(nome)`, `*`), não uma coluna
    /// de `table`, que fica sem uso.
    pub expression: bool,
}

impl Row {
    /// O critério da coluna `k` da grade (0 é Filter, 1 o primeiro Or...).
    pub fn criterion(&self, k: usize) -> &str {
        match k {
            0 => &self.filter,
            k => self.or.get(k - 1).map_or("", String::as_str),
        }
    }

    /// O critério da coluna `k`, para escrever.
    pub fn criterion_mut(&mut self, k: usize) -> &mut String {
        if k == 0 {
            return &mut self.filter;
        }
        if self.or.len() < k {
            self.or.resize(k, String::new());
        }
        &mut self.or[k - 1]
    }

    fn has_criteria(&self) -> bool {
        !self.filter.trim().is_empty() || self.or.iter().any(|c| !c.trim().is_empty())
    }

    /// Filtro que vai para o HAVING (coluna agregada), não para o WHERE.
    fn in_having(&self, group_by: bool) -> bool {
        group_by && !matches!(self.aggregate, Aggregate::GroupBy | Aggregate::Where)
    }
}

/// As consultas montadas pelo designer, sem a parte de tela (para testar).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub tables: Vec<QueryTable>,
    pub joins: Vec<Join>,
    pub rows: Vec<Row>,
    /// Group By ligado: cada linha ganha um [`Aggregate`].
    pub group_by: bool,
    /// `SELECT DISTINCT` ("Distinct Values" no SSMS).
    pub distinct: bool,
    /// `LIMIT n` (o "Top" do SSMS).
    pub limit: Option<u64>,
    /// Quantas colunas Or... a grade mostra.
    pub or_columns: usize,
}

impl Query {
    /// Põe a tabela `table` do modelo na consulta e liga pelas FKs às que
    /// já estão (nos dois sentidos). Devolve o índice dela.
    pub fn add_table(&mut self, model: &Model, table: Id) -> Option<usize> {
        let t = model.table(table)?;
        let alias = (0..)
            .map(|n| {
                if n == 0 {
                    t.name.clone()
                } else {
                    format!("{}_{n}", t.name)
                }
            })
            .find(|a| self.tables.iter().all(|q| &q.alias != a))
            .expect("algum apelido está livre");
        let new = self.tables.len();
        self.tables.push(QueryTable { table, alias });
        for (i, other) in self.tables[..new].iter().enumerate() {
            let Some(o) = model.table(other.table) else {
                continue;
            };
            for fk in &t.foreign_keys {
                if fk.ref_schema == o.schema && fk.ref_table == o.name {
                    let on = fk.ref_columns.iter().cloned().zip(fk.columns.clone());
                    self.joins.push(Join {
                        left: i,
                        right: new,
                        on: on.collect(),
                        ..Default::default()
                    });
                }
            }
            for fk in &o.foreign_keys {
                if fk.ref_schema == t.schema && fk.ref_table == t.name && other.table != table {
                    let on = fk.columns.iter().cloned().zip(fk.ref_columns.clone());
                    self.joins.push(Join {
                        left: i,
                        right: new,
                        on: on.collect(),
                        ..Default::default()
                    });
                }
            }
        }
        Some(new)
    }

    /// Tira a tabela `i`, os JOINs e as linhas dela.
    pub fn remove_table(&mut self, i: usize) {
        if i >= self.tables.len() {
            return;
        }
        self.tables.remove(i);
        let fix = |t: usize| if t > i { t - 1 } else { t };
        self.joins.retain(|j| j.left != i && j.right != i);
        for j in &mut self.joins {
            j.left = fix(j.left);
            j.right = fix(j.right);
        }
        self.rows.retain(|r| r.expression || r.table != i);
        for r in self.rows.iter_mut().filter(|r| !r.expression) {
            r.table = fix(r.table);
        }
    }

    /// JOIN de `left.left_column` com `right.right_column` (arrastado no
    /// diagrama). Junta ao JOIN que já liga as duas, se houver.
    pub fn add_join(&mut self, left: usize, left_column: &str, right: usize, right_column: &str) {
        if left == right {
            return;
        }
        let pair = (left_column.to_string(), right_column.to_string());
        if let Some(j) = self
            .joins
            .iter_mut()
            .find(|j| j.left == left && j.right == right)
        {
            if !j.on.contains(&pair) {
                j.on.push(pair);
            }
            return;
        }
        if let Some(j) = self
            .joins
            .iter_mut()
            .find(|j| j.left == right && j.right == left)
        {
            let swapped = (pair.1, pair.0);
            if !j.on.contains(&swapped) {
                j.on.push(swapped);
            }
            return;
        }
        self.joins.push(Join {
            left,
            right,
            on: vec![pair],
            ..Default::default()
        });
    }

    /// Muda o tipo do JOIN `j` e dos outros que entram na mesma tabela
    /// (eles saem juntos no mesmo `ON`).
    pub fn set_join_kind(&mut self, j: usize, kind: JoinKind) {
        let Some(at) = self.joins.get(j).map(|j| j.left.max(j.right)) else {
            return;
        };
        for join in &mut self.joins {
            if join.left.max(join.right) == at {
                join.set_kind(kind);
            }
        }
    }

    /// Acrescenta uma linha de expressão (a coluna digitada à mão).
    pub fn add_expression(&mut self, expression: &str) {
        self.rows.push(Row {
            column: expression.to_string(),
            output: true,
            expression: true,
            ..Default::default()
        });
    }

    /// Marca ou desmarca a coluna para a saída. Desmarcar tira a linha,
    /// a menos que ela filtre ou ordene.
    pub fn toggle_column(&mut self, table: usize, column: &str) {
        match self
            .rows
            .iter()
            .position(|r| !r.expression && r.table == table && r.column == column)
        {
            Some(i) if self.rows[i].output => {
                let r = &self.rows[i];
                if !r.has_criteria() && r.sort == SortType::Unsorted {
                    self.rows.remove(i);
                } else {
                    self.rows[i].output = false;
                }
            }
            Some(i) => self.rows[i].output = true,
            None => self.rows.push(Row {
                table,
                column: column.to_string(),
                output: true,
                ..Default::default()
            }),
        }
    }

    /// Coluna marcada para a saída.
    pub fn is_output(&self, table: usize, column: &str) -> bool {
        self.rows
            .iter()
            .any(|r| !r.expression && r.table == table && r.column == column && r.output)
    }

    fn column_ref(&self, r: &Row) -> String {
        if r.expression {
            return r.column.trim().to_string();
        }
        format!(
            "{}.{}",
            quote_ident(&self.tables[r.table].alias),
            quote_ident(&r.column)
        )
    }

    /// A coluna como vai no SELECT e no ORDER BY (com a agregação).
    fn expr(&self, r: &Row) -> String {
        let col = self.column_ref(r);
        match self.group_by.then(|| r.aggregate.function()).flatten() {
            Some(f) => format!("{f}({col})"),
            None => col,
        }
    }

    /// Linhas com coluna (as de expressão vazia ficam de fora).
    fn live_rows(&self) -> impl Iterator<Item = (usize, &Row)> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !(r.expression && r.column.trim().is_empty()))
    }

    /// Quantas colunas de critério a grade tem (Filter mais as Or...).
    pub fn criteria_columns(&self) -> usize {
        let used = self.rows.iter().map(|r| r.or.len()).max().unwrap_or(0);
        1 + self.or_columns.max(used)
    }

    /// As condições de um WHERE ou HAVING: cada coluna de critérios vira
    /// um grupo ligado por AND, e os grupos se ligam por OR.
    fn criteria(&self, having: bool, and: &str, or: &str) -> Option<String> {
        let groups: Vec<Vec<String>> = (0..self.criteria_columns())
            .map(|k| {
                self.live_rows()
                    .filter(|(_, r)| r.in_having(self.group_by) == having)
                    .filter(|(_, r)| !r.criterion(k).trim().is_empty())
                    .map(|(_, r)| {
                        let subject = if having {
                            self.expr(r)
                        } else {
                            self.column_ref(r)
                        };
                        format!("{subject} {}", condition(r.criterion(k)))
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|g| !g.is_empty())
            .collect();
        match groups.as_slice() {
            [] => None,
            [only] => Some(only.join(and)),
            _ => Some(
                groups
                    .iter()
                    .map(|g| format!("({})", g.join(" AND ")))
                    .collect::<Vec<_>>()
                    .join(or),
            ),
        }
    }

    /// O SQL da consulta.
    pub fn sql(&self, model: &Model) -> String {
        let mut out = String::from("SELECT ");
        if self.distinct {
            out.push_str("DISTINCT ");
        }
        let select: Vec<String> = self
            .live_rows()
            .filter(|(_, r)| r.output && !(self.group_by && r.aggregate == Aggregate::Where))
            .map(|(_, r)| {
                let e = self.expr(r);
                match r.alias.trim() {
                    "" => e,
                    a => format!("{e} AS {}", quote_ident(a)),
                }
            })
            .collect();
        if select.is_empty() {
            out.push('*');
        } else {
            out.push_str(&select.join(",\n       "));
        }
        for (i, t) in self.tables.iter().enumerate() {
            let Some(table) = model.table(t.table) else {
                continue;
            };
            let mut item = table.qualified();
            if t.alias != table.name {
                item.push_str(&format!(" AS {}", quote_ident(&t.alias)));
            }
            if i == 0 {
                out.push_str(&format!("\nFROM {item}"));
                continue;
            }
            // Condições com as tabelas que já vieram antes desta.
            let joins: Vec<&Join> = self
                .joins
                .iter()
                .filter(|j| j.left.max(j.right) == i)
                .collect();
            let on: Vec<String> = joins
                .iter()
                .flat_map(|j| {
                    j.on.iter().map(|(l, r)| {
                        format!(
                            "{}.{} = {}.{}",
                            quote_ident(&self.tables[j.left].alias),
                            quote_ident(l),
                            quote_ident(&self.tables[j.right].alias),
                            quote_ident(r)
                        )
                    })
                })
                .collect();
            if on.is_empty() {
                out.push_str(&format!("\n    CROSS JOIN {item}"));
            } else {
                let kind = joins.first().map(|j| j.kind()).unwrap_or_default();
                out.push_str(&format!(
                    "\n    {} {item} ON {}",
                    kind.sql(),
                    on.join(" AND ")
                ));
            }
        }
        if let Some(w) = self.criteria(false, "\n  AND ", "\n   OR ") {
            out.push_str(&format!("\nWHERE {w}"));
        }
        if self.group_by {
            let groups: Vec<String> = self
                .live_rows()
                .filter(|(_, r)| r.aggregate == Aggregate::GroupBy)
                .map(|(_, r)| self.column_ref(r))
                .collect();
            if !groups.is_empty() {
                out.push_str(&format!("\nGROUP BY {}", groups.join(", ")));
            }
            if let Some(h) = self.criteria(true, "\n   AND ", "\n    OR ") {
                out.push_str(&format!("\nHAVING {h}"));
            }
        }
        let mut sorted: Vec<(usize, &Row)> = self
            .live_rows()
            .filter(|(_, r)| r.sort != SortType::Unsorted)
            .collect();
        sorted.sort_by_key(|(i, r)| {
            (
                if r.sort_order == 0 {
                    u32::MAX
                } else {
                    r.sort_order
                },
                *i,
            )
        });
        if !sorted.is_empty() {
            let items: Vec<String> = sorted
                .iter()
                .map(|(_, r)| {
                    let dir = if r.sort == SortType::Descending {
                        " DESC"
                    } else {
                        ""
                    };
                    format!("{}{dir}", self.expr(r))
                })
                .collect();
            out.push_str(&format!("\nORDER BY {}", items.join(", ")));
        }
        if let Some(n) = self.limit {
            out.push_str(&format!("\nLIMIT {n}"));
        }
        out.push(';');
        out
    }
}

/// O filtro como condição: sem operador no começo, vira `= filtro`.
fn condition(filter: &str) -> String {
    let f = filter.trim();
    let lower = f.to_lowercase();
    let has_operator = ["=", "<", ">", "!", "~"].iter().any(|op| f.starts_with(op))
        || ["like ", "ilike ", "in ", "in(", "is ", "between ", "not "]
            .iter()
            .any(|op| lower.starts_with(op));
    if has_operator {
        f.to_string()
    } else {
        format!("= {f}")
    }
}

enum State {
    Loading(Receiver<Result<Model, MetadataError>>),
    Failed(String),
    Ready,
}

/// O Query Designer aberto (um por vez, sobre a janela de consulta).
pub struct QueryDesigner {
    /// Janela de consulta que recebe o SQL.
    pub doc: u32,
    database: String,
    state: State,
    model: Model,
    pub query: Query,
    diagram: Diagram,
    picker: Option<Picker>,
    /// Some(Some(sql)) no OK, Some(None) no Cancel.
    result: Option<Option<String>>,
    /// Ao carregar, põe todas as tabelas e marca a segunda coluna de cada
    /// (screenshots).
    all_tables: bool,
    /// SQL selecionado no editor, lido quando as tabelas chegarem.
    source: Option<String>,
    /// A seleção virou a consulta: o OK troca a seleção pelo SQL novo.
    replaces: bool,
    /// Por que a seleção não pôde ser lida.
    notice: Option<String>,
}

impl QueryDesigner {
    /// Abre para a janela `doc`, carregando as tabelas de `database`.
    pub fn open(
        provider: Arc<dyn MetadataProvider>,
        database: String,
        doc: u32,
        ctx: &egui::Context,
    ) -> Self {
        let (tx, rx) = channel();
        let db = database.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(provider.load_model(&db));
            repaint.request_repaint();
        });
        QueryDesigner {
            doc,
            database,
            state: State::Loading(rx),
            model: Model::default(),
            query: Query::default(),
            diagram: Diagram::default(),
            picker: None,
            result: None,
            all_tables: false,
            source: None,
            replaces: true,
            notice: None,
        }
    }

    /// Abre com o SQL selecionado no editor montado (se o designer souber
    /// mostrá-lo; senão, com um aviso).
    pub fn with_sql(mut self, sql: String) -> Self {
        if !sql.trim().is_empty() {
            self.source = Some(sql);
        }
        self
    }

    /// O OK troca a seleção do editor (em vez de pôr a consulta depois
    /// dela).
    pub fn replaces_selection(&self) -> bool {
        self.replaces
    }

    /// Aviso de que a seleção não pôde ser lida.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Ao carregar, põe todas as tabelas sem perguntar (screenshots).
    pub fn with_all_tables(mut self) -> Self {
        self.all_tables = true;
        self
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, State::Loading(_))
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn diagram(&self) -> &Diagram {
        &self.diagram
    }

    pub fn picker_mut(&mut self) -> Option<&mut Picker> {
        self.picker.as_mut()
    }

    /// O SQL como está agora.
    pub fn sql(&self) -> String {
        self.query.sql(&self.model)
    }

    /// OK ou Cancel, uma vez.
    pub fn take_result(&mut self) -> Option<Option<String>> {
        self.result.take()
    }

    fn step(&mut self) {
        let State::Loading(rx) = &self.state else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(model)) => {
                self.model = model;
                self.state = State::Ready;
                if let Some(sql) = self.source.take() {
                    match read::read(&sql, &self.model) {
                        Ok(query) => self.query = query,
                        Err(e) => {
                            self.replaces = false;
                            self.notice = Some(e.to_string());
                        }
                    }
                    self.rebuild();
                } else if self.all_tables {
                    let ids: Vec<Id> = self.model.tables.iter().map(|t| t.id).collect();
                    for id in ids {
                        if let Some(i) = self.query.add_table(&self.model, id)
                            && let Some(c) = self.model.table(id).and_then(|t| t.columns.get(1))
                        {
                            self.query.toggle_column(i, &c.name.clone());
                        }
                    }
                    self.rebuild();
                } else {
                    self.picker = Some(Picker::default());
                }
            }
            Ok(Err(e)) => self.state = State::Failed(e.0),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.state = State::Failed("The tables could not be loaded.".into());
            }
        }
    }

    /// Põe tabelas na consulta (com os JOINs pelas FKs).
    pub fn add_tables(&mut self, ids: &[Id]) {
        for id in ids {
            self.query.add_table(&self.model, *id);
        }
        self.rebuild();
    }

    /// Marca ou desmarca uma coluna (a caixinha no diagrama).
    pub fn toggle_column(&mut self, table: usize, column: &str) {
        self.query.toggle_column(table, column);
        self.rebuild();
    }

    pub fn add_join(&mut self, left: usize, left_column: &str, right: usize, right_column: &str) {
        self.query.add_join(left, left_column, right, right_column);
        self.rebuild();
    }

    /// Refaz as caixas a partir da consulta, mantendo onde estavam; as
    /// novas vão à direita das outras.
    fn rebuild(&mut self) {
        let old = std::mem::take(&mut self.diagram.nodes);
        let first = old.is_empty();
        let mut right = old
            .iter()
            .map(|n| n.rect().max.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let mut nodes = Vec::new();
        for (i, q) in self.query.tables.iter().enumerate() {
            let Some(t) = self.model.table(q.table) else {
                continue;
            };
            let pk: &[String] = t.primary_key.as_ref().map_or(&[], |k| &k.columns);
            let columns = t
                .columns
                .iter()
                .map(|c| NodeColumn {
                    name: c.name.clone(),
                    ty: c.ty.clone(),
                    primary_key: pk.contains(&c.name),
                    foreign_key: t.foreign_keys.iter().any(|fk| fk.columns.contains(&c.name)),
                    nullable: c.nullable,
                    checked: Some(self.query.is_output(i, &c.name)),
                })
                .collect();
            let mut node = Node {
                table: i as Id,
                schema: t.schema.clone(),
                name: t.name.clone(),
                columns,
                pos: egui::Pos2::ZERO,
                width: 240.0,
                alias: (q.alias != t.name).then(|| q.alias.clone()),
            };
            match old.iter().find(|n| n.table == i as Id && n.name == t.name) {
                Some(o) => {
                    node.pos = o.pos;
                    node.width = o.width;
                }
                None if !first => {
                    node.pos = egui::pos2(right + 60.0, 0.0);
                    right = node.rect().max.x;
                }
                None => {}
            }
            nodes.push(node);
        }
        self.diagram.nodes = nodes;
        self.diagram.edges = self
            .query
            .joins
            .iter()
            .filter(|j| j.left < self.diagram.nodes.len() && j.right < self.diagram.nodes.len())
            .map(|j| Edge {
                name: match j.kind() {
                    JoinKind::Inner => String::new(),
                    k => k.sql().to_string(),
                },
                from: j.right,
                to: j.left,
            })
            .collect();
        if first && !self.diagram.nodes.is_empty() {
            self.diagram.auto_layout();
        }
    }

    /// Desenha o designer (uma janela modal) e trata o que o usuário fez.
    pub fn show(&mut self, ctx: &egui::Context) {
        self.step();
        let mut result = None;
        let mut open_picker = false;
        let mut remove = None;
        let modal = crate::theme::modal(egui::Id::new("query_designer")).show(ctx, |ui| {
            let width = (ctx.content_rect().width() - 80.0).clamp(500.0, 1100.0);
            ui.set_width(width);
            ui.heading("Query Designer");
            match &self.state {
                State::Loading(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Loading tables of {}...", self.database));
                    });
                }
                State::Failed(e) => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("{} {e}", icon::WARNING),
                    );
                }
                State::Ready => {
                    if let Some(notice) = &self.notice {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!(
                                "{} {notice} The selection stays in the editor; OK adds the \
                                 designed query after it.",
                                icon::WARNING
                            ),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button(format!("{} Add Table...", icon::PLUS)).clicked() {
                            open_picker = true;
                        }
                        let selected = self.diagram.selected;
                        if ui
                            .add_enabled(
                                selected.is_some(),
                                egui::Button::new(format!("{} Remove Table", icon::X)),
                            )
                            .clicked()
                        {
                            remove = selected;
                        }
                        ui.checkbox(&mut self.query.group_by, "Group By");
                        ui.checkbox(&mut self.query.distinct, "Distinct");
                        let mut limited = self.query.limit.is_some();
                        if ui.checkbox(&mut limited, "Limit").changed() {
                            self.query.limit = limited.then_some(100);
                        }
                        if let Some(n) = &mut self.query.limit {
                            ui.add(egui::DragValue::new(n).range(0..=u64::MAX));
                        }
                        ui.separator();
                        if ui
                            .button(format!("{} Add Expression", icon::FUNCTION))
                            .on_hover_text(
                                "A column typed by hand: lower(name), price * 2, count(*)",
                            )
                            .clicked()
                        {
                            self.query.add_expression("");
                        }
                        if ui
                            .button(format!("{} Add Or Column", icon::PLUS))
                            .on_hover_text(
                                "Another column of criteria: each column is ANDed, \
                                 and the columns are ORed",
                            )
                            .clicked()
                        {
                            self.query.or_columns = self.query.criteria_columns();
                        }
                    });
                    ui.weak("Check columns to select them; drag a column onto another to join.");
                    let height = (ctx.content_rect().height() * 0.35).clamp(160.0, 320.0);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                            if self.diagram.nodes.is_empty() {
                                ui.set_min_size(egui::vec2(ui.available_width(), height));
                                ui.weak("Use Add Table to add tables to the query.");
                            } else {
                                self.diagram.show(ui);
                            }
                        });
                    });
                    self.joins(ui);
                    self.grid(ui);
                    ui.add_space(4.0);
                    let mut sql = self.sql();
                    egui::ScrollArea::vertical()
                        .id_salt("query_designer_sql")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut sql)
                                    .code_editor()
                                    .interactive(false)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                }
            }
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    result = Some(None);
                }
                let ready = matches!(self.state, State::Ready);
                if ui.add_enabled(ready, egui::Button::new("OK")).clicked() {
                    result = Some(Some(self.sql()));
                }
            });
        });
        if modal.should_close() && result.is_none() && self.picker.is_none() {
            result = Some(None);
        }
        for event in self.diagram.take_events() {
            match event {
                DiagramEvent::Toggle { table, column } => {
                    self.toggle_column(table as usize, &column);
                }
                DiagramEvent::Link {
                    from,
                    from_column,
                    to,
                    to_column,
                } => self.add_join(from as usize, &from_column, to as usize, &to_column),
                DiagramEvent::Moved(_) => {}
            }
        }
        if let Some(i) = remove {
            self.query.remove_table(i);
            self.diagram.selected = None;
            self.diagram.nodes.clear();
            self.rebuild();
        }
        if open_picker {
            self.picker = Some(Picker::default());
        }
        if let Some(picker) = &mut self.picker {
            let filter = picker.filter.to_lowercase();
            let mut candidates: Vec<(Id, String)> = self
                .model
                .tables
                .iter()
                .map(|t| (t.id, format!("{}.{}", t.schema, t.name)))
                .filter(|(_, n)| n.to_lowercase().contains(&filter))
                .collect();
            candidates.sort_by(|a, b| a.1.cmp(&b.1));
            match table_picker(
                ctx,
                egui::Id::new("query_designer_add"),
                picker,
                &candidates,
            ) {
                Some(true) => {
                    let ids: Vec<Id> = picker.checked.iter().copied().collect();
                    self.picker = None;
                    self.add_tables(&ids);
                }
                Some(false) => self.picker = None,
                None => {}
            }
        }
        if result.is_some() {
            self.result = result;
        }
    }

    /// Os JOINs, cada um com o tipo (as propriedades do JOIN no SSMS:
    /// "Select All Rows from" cada tabela).
    fn joins(&mut self, ui: &mut egui::Ui) {
        if self.query.joins.is_empty() {
            return;
        }
        let mut change = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Joins:");
            for (j, join) in self.query.joins.iter().enumerate() {
                let (first, second) = (join.left.min(join.right), join.left.max(join.right));
                let a = &self.query.tables[first].alias;
                let b = &self.query.tables[second].alias;
                let label = |k: JoinKind| match k {
                    JoinKind::Inner => format!("{a} \u{2194} {b}: matching rows"),
                    JoinKind::Left => format!("{a} \u{2194} {b}: all rows from {a}"),
                    JoinKind::Right => format!("{a} \u{2194} {b}: all rows from {b}"),
                    JoinKind::Full => format!("{a} \u{2194} {b}: all rows from both"),
                };
                let current = join.kind();
                egui::ComboBox::from_id_salt(("qd_join", j))
                    .selected_text(label(current))
                    .show_ui(ui, |ui| {
                        for k in JoinKind::ALL {
                            if ui
                                .selectable_label(k == current, label(k))
                                .on_hover_text(k.sql())
                                .clicked()
                            {
                                change = Some((j, k));
                            }
                        }
                    });
            }
        });
        if let Some((j, k)) = change {
            self.query.set_join_kind(j, k);
            self.rebuild();
        }
    }

    /// A grade de colunas.
    fn grid(&mut self, ui: &mut egui::Ui) {
        let tables: Vec<String> = self.query.tables.iter().map(|t| t.alias.clone()).collect();
        let group_by = self.query.group_by;
        let criteria = self.query.criteria_columns();
        let mut delete = None;
        let mut changed = false;
        egui::ScrollArea::vertical()
            .id_salt("query_designer_grid")
            .max_height(170.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("query_designer_columns")
                    .striped(true)
                    .num_columns(8 + criteria)
                    .show(ui, |ui| {
                        for h in [
                            "Column",
                            "Alias",
                            "Table",
                            "Output",
                            "Sort Type",
                            "Sort Order",
                            "Filter",
                        ] {
                            ui.strong(h);
                        }
                        for _ in 1..criteria {
                            ui.strong("Or...");
                        }
                        if group_by {
                            ui.strong("Group By");
                        }
                        ui.label("");
                        ui.end_row();
                        for (i, r) in self.query.rows.iter_mut().enumerate() {
                            if r.expression {
                                ui.add_sized(
                                    [140.0, 20.0],
                                    egui::TextEdit::singleline(&mut r.column)
                                        .id(egui::Id::new(("qd_expression", i)))
                                        .code_editor()
                                        .hint_text("expression"),
                                );
                            } else {
                                ui.label(&r.column);
                            }
                            ui.add_sized(
                                [90.0, 20.0],
                                egui::TextEdit::singleline(&mut r.alias)
                                    .id(egui::Id::new(("qd_alias", i))),
                            );
                            ui.label(if r.expression {
                                ""
                            } else {
                                tables.get(r.table).map_or("", String::as_str)
                            });
                            changed |= ui.checkbox(&mut r.output, "").changed();
                            egui::ComboBox::from_id_salt(("qd_sort", i))
                                .selected_text(r.sort.label())
                                .width(95.0)
                                .show_ui(ui, |ui| {
                                    for s in SortType::ALL {
                                        ui.selectable_value(&mut r.sort, s, s.label());
                                    }
                                });
                            let mut order = r.sort_order;
                            if ui
                                .add(egui::DragValue::new(&mut order).range(0..=99))
                                .changed()
                            {
                                r.sort_order = order;
                            }
                            ui.add_sized(
                                [140.0, 20.0],
                                egui::TextEdit::singleline(&mut r.filter)
                                    .id(egui::Id::new(("qd_filter", i)))
                                    .hint_text("e.g. > 10"),
                            );
                            for k in 1..criteria {
                                ui.add_sized(
                                    [110.0, 20.0],
                                    egui::TextEdit::singleline(r.criterion_mut(k))
                                        .id(egui::Id::new(("qd_or", i, k))),
                                );
                            }
                            if group_by {
                                egui::ComboBox::from_id_salt(("qd_group", i))
                                    .selected_text(r.aggregate.label())
                                    .width(85.0)
                                    .show_ui(ui, |ui| {
                                        for a in Aggregate::ALL {
                                            ui.selectable_value(&mut r.aggregate, a, a.label());
                                        }
                                    });
                            }
                            if ui
                                .small_button(icon::TRASH)
                                .on_hover_text("Remove from the grid")
                                .clicked()
                            {
                                delete = Some(i);
                            }
                            ui.end_row();
                        }
                    });
                if self.query.rows.is_empty() {
                    ui.weak("No columns yet: check them in the tables above.");
                }
            });
        if let Some(i) = delete {
            self.query.rows.remove(i);
            changed = true;
        }
        if changed {
            self.rebuild();
        }
    }
}

/// Coloca `sql` no lugar da seleção de `text` (em caracteres) e devolve a
/// seleção do texto inserido.
pub fn insert(text: &mut String, (a, b): (usize, usize), sql: &str) -> (usize, usize) {
    let (a, b) = (a.min(b), a.max(b));
    replace(text, (a, b), sql)
}

/// Põe `sql` numa linha nova logo depois da seleção, sem apagá-la, e
/// devolve a seleção do texto inserido.
pub fn insert_after(text: &mut String, (a, b): (usize, usize), sql: &str) -> (usize, usize) {
    let end = a.max(b);
    let (start, _) = replace(text, (end, end), &format!("\n{sql}"));
    (start + 1, start + 1 + sql.chars().count())
}

fn replace(text: &mut String, (a, b): (usize, usize), sql: &str) -> (usize, usize) {
    let start = text.char_indices().nth(a).map_or(text.len(), |(i, _)| i);
    let end = text.char_indices().nth(b).map_or(text.len(), |(i, _)| i);
    text.replace_range(start..end, sql);
    (a, a + sql.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgstudio_core::schema::edit::Document;
    use pgstudio_core::schema::{Column, ForeignKey};

    /// cliente <- pedido (cliente_id), produto solto.
    pub(super) fn shop() -> (Model, Id, Id, Id) {
        let mut d = Document::default();
        let cliente = d.add_table("public", "cliente").unwrap();
        let pedido = d.add_table("public", "pedido").unwrap();
        let produto = d.add_table("public", "produto").unwrap();
        for (t, name) in [(cliente, "nome"), (pedido, "total"), (produto, "nome")] {
            d.add_column(
                t,
                Column {
                    name: name.into(),
                    ty: "text".into(),
                    nullable: true,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        d.add_column(
            pedido,
            Column {
                name: "cliente_id".into(),
                ty: "integer".into(),
                ..Default::default()
            },
        )
        .unwrap();
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
        (d.model, cliente, pedido, produto)
    }

    #[test]
    fn tables_join_by_their_foreign_keys_either_way() {
        let (m, cliente, pedido, produto) = shop();
        let mut q = Query::default();
        q.add_table(&m, pedido);
        q.add_table(&m, cliente);
        q.add_table(&m, produto);
        assert_eq!(
            q.joins,
            [Join {
                left: 0,
                right: 1,
                on: vec![("cliente_id".into(), "id".into())],
                ..Default::default()
            }]
        );
        q.toggle_column(1, "nome");
        q.toggle_column(0, "total");
        assert_eq!(
            q.sql(&m),
            "SELECT cliente.nome,\n       pedido.total\n\
             FROM public.pedido\n    \
             INNER JOIN public.cliente ON pedido.cliente_id = cliente.id\n    \
             CROSS JOIN public.produto;"
        );
    }

    #[test]
    fn filters_sorting_aliases_and_grouping() {
        let (m, cliente, pedido, _) = shop();
        let mut q = Query::default();
        q.add_table(&m, cliente);
        q.add_table(&m, pedido);
        assert_eq!(q.sql(&m).lines().next(), Some("SELECT *"));
        q.toggle_column(0, "nome");
        q.toggle_column(1, "total");
        q.rows[1].alias = "Total Gasto".into();
        q.rows[1].sort = SortType::Descending;
        q.rows[0].sort = SortType::Ascending;
        q.rows[0].sort_order = 2;
        q.rows[1].sort_order = 1;
        q.rows[0].filter = "'Ana'".into();
        q.toggle_column(1, "id");
        q.rows[2].output = false;
        q.rows[2].filter = "> 10".into();
        assert_eq!(
            q.sql(&m),
            "SELECT cliente.nome,\n       pedido.total AS \"Total Gasto\"\n\
             FROM public.cliente\n    \
             INNER JOIN public.pedido ON cliente.id = pedido.cliente_id\n\
             WHERE cliente.nome = 'Ana'\n  AND pedido.id > 10\n\
             ORDER BY pedido.total DESC, cliente.nome;"
        );
        q.group_by = true;
        q.rows[1].aggregate = Aggregate::Sum;
        q.rows[2].aggregate = Aggregate::Count;
        let sql = q.sql(&m);
        assert!(
            sql.contains("sum(pedido.total) AS \"Total Gasto\""),
            "{sql}"
        );
        assert!(sql.contains("GROUP BY cliente.nome\n"), "{sql}");
        assert!(
            sql.ends_with("ORDER BY sum(pedido.total) DESC, cliente.nome;"),
            "{sql}"
        );
    }

    #[test]
    fn the_same_table_twice_gets_an_alias_and_manual_joins() {
        let (m, cliente, _, _) = shop();
        let mut q = Query::default();
        q.add_table(&m, cliente);
        q.add_table(&m, cliente);
        assert_eq!(q.tables[1].alias, "cliente_1");
        assert!(q.joins.is_empty());
        q.add_join(1, "nome", 0, "nome");
        q.add_join(0, "id", 1, "id");
        assert_eq!(q.joins.len(), 1);
        let sql = q.sql(&m);
        assert!(
            sql.contains(
                "INNER JOIN public.cliente AS cliente_1 ON cliente_1.nome = cliente.nome \
                 AND cliente_1.id = cliente.id"
            ),
            "{sql}"
        );
        q.toggle_column(1, "nome");
        q.remove_table(0);
        assert!(q.joins.is_empty());
        assert_eq!(q.rows[0].table, 0);
        // Desmarcar tira a linha.
        q.toggle_column(0, "nome");
        assert!(q.rows.is_empty());
    }

    #[test]
    fn filters_without_an_operator_compare_for_equality() {
        assert_eq!(condition("10"), "= 10");
        assert_eq!(condition(" >= 3 "), ">= 3");
        assert_eq!(condition("LIKE 'a%'"), "LIKE 'a%'");
        assert_eq!(condition("is null"), "is null");
        assert_eq!(condition("in (1, 2)"), "in (1, 2)");
        assert_eq!(condition("'Ana'"), "= 'Ana'");
    }

    #[test]
    fn insert_replaces_the_selection() {
        let mut t = "-- olá\nxx\n".to_string();
        let sel = insert(&mut t, (9, 7), "SELECT 1;");
        assert_eq!(t, "-- olá\nSELECT 1;\n");
        assert_eq!(sel, (7, 16));
        let mut t = String::new();
        assert_eq!(insert(&mut t, (0, 0), "x"), (0, 1));
    }

    #[test]
    fn insert_after_keeps_the_selection() {
        let mut t = "WITH x AS (SELECT 1) SELECT * FROM x;".to_string();
        let sel = insert_after(&mut t, (37, 0), "SELECT 1;");
        assert_eq!(t, "WITH x AS (SELECT 1) SELECT * FROM x;\nSELECT 1;");
        assert_eq!(sel, (38, 47));
    }
}
