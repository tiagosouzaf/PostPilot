//! O que dá para deixar com cada servidor.
//!
//! Ler uma tabela inteira para juntar aqui não serve para tabelas de
//! milhões de linhas. Este módulo lê o SELECT do usuário (já com as fontes
//! trocadas por `main."pp_N"`) e descobre, para cada tabela do PostgreSQL
//! ou do SQL Server:
//!
//! - as colunas que a consulta usa (as outras não são lidas);
//! - os filtros do `WHERE` e do `ON` que só falam dela (rodam no servidor);
//! - as ligações `a.x = b.y` com outras tabelas (a segunda é lida só com as
//!   chaves que a primeira trouxe);
//! - se a consulta é um "os N primeiros por uma ordem" de uma tabela (a
//!   ordem e o limite rodam no servidor).
//!
//! Tudo aqui é só para ler menos: o SQL do usuário roda inteiro no SQLite
//! depois, com os mesmos filtros. Por isso só entra o que o servidor
//! responde igual ou com linhas a mais, nunca a menos. O que não se
//! encaixa fica sem otimização.

use std::collections::HashSet;
use std::ops::ControlFlow;

use pgstudio_core::connection::Engine;
use sqlparser::ast::{
    BinaryOperator, Distinct, Expr, GroupByExpr, InTarget, JoinConstraint, JoinOperator,
    LimitClause, ObjectName, OrderByKind, OrderBySort, Query, Select, SelectItem,
    SelectItemQualifiedWildcardKind, SetExpr, Statement, TableFactor, UnaryOperator, Value,
    visit_expressions,
};
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;

use crate::plan::{Plan, Server};

/// Uma coluna como o usuário escreveu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// Veio entre aspas: maiúsculas e minúsculas valem.
    pub quoted: bool,
}

impl Column {
    fn same(&self, other: &Column) -> bool {
        self.name.eq_ignore_ascii_case(&other.name)
    }

    /// A coluna no SQL do servidor.
    pub fn sql(&self, engine: Engine) -> String {
        match engine {
            Engine::SqlServer => format!("[{}]", self.name.replace(']', "]]")),
            _ if self.quoted || !simple(&self.name) => {
                format!("\"{}\"", self.name.replace('"', "\"\""))
            }
            _ => self.name.clone(),
        }
    }
}

fn simple(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// O que um filtro precisa que a coluna seja para valer no servidor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Um número (comparação de maior e menor com um número).
    Number,
    /// Uma data, ou data e hora sem fuso; `date_only` quando o texto
    /// comparado é só a data.
    Date { date_only: bool },
}

/// Um filtro que o servidor pode aplicar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    /// O filtro no SQL do servidor.
    pub sql: String,
    /// O que as colunas dele precisam ser.
    pub needs: Vec<(Column, Need)>,
}

/// `coluna = coluna de outra fonte`: dá para ler esta fonte só com as
/// chaves que a outra trouxe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub column: Column,
    /// A outra fonte (índice em `Plan::sources`).
    pub parent: usize,
    pub parent_column: String,
}

/// O que dá para deixar com o servidor de uma fonte.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourcePlan {
    /// A fonte é uma tabela citada uma vez só, num SELECT simples.
    pub optimizable: bool,
    /// As colunas usadas; `None` quando é para ler todas.
    pub columns: Option<Vec<Column>>,
    pub filters: Vec<Filter>,
    pub links: Vec<Link>,
}

/// "Os N primeiros por uma ordem" de uma fonte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Top {
    pub source: usize,
    /// As colunas da ordem; `true` é decrescente. Vazio: qualquer ordem.
    pub order: Vec<(Column, bool)>,
    /// Linhas que a consulta precisa (limite mais o deslocamento).
    pub need: u64,
    /// Conta, no SQLite, quantas linhas a consulta já tem.
    pub check: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    pub sources: Vec<SourcePlan>,
    pub top: Option<Top>,
}

impl Analysis {
    /// Há alguma coisa para deixar com os servidores.
    pub fn any(&self) -> bool {
        self.sources.iter().any(|s| s.optimizable)
    }
}

/// Uma tabela do `FROM`.
struct Table {
    source: usize,
    /// O nome pelo qual a consulta fala dela (minúsculo).
    alias: String,
    /// Está do lado que pode vir vazio de um `LEFT JOIN`.
    nullable: bool,
}

struct Scope<'a> {
    tables: Vec<Table>,
    plan: &'a Plan,
    servers: &'a [Server],
}

impl Scope<'_> {
    fn engine(&self, table: usize) -> Engine {
        self.servers[self.plan.sources[self.tables[table].source].server].engine
    }

    /// A tabela e a coluna de uma expressão que é só uma coluna.
    fn column(&self, e: &Expr) -> Option<(usize, Column)> {
        match e {
            Expr::Nested(inner) => self.column(inner),
            Expr::Identifier(id) if self.tables.len() == 1 => Some((
                0,
                Column {
                    name: id.value.clone(),
                    quoted: id.quote_style.is_some(),
                },
            )),
            Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
                let alias = parts[0].value.to_lowercase();
                let table = self.tables.iter().position(|t| t.alias == alias)?;
                Some((
                    table,
                    Column {
                        name: parts[1].value.clone(),
                        quoted: parts[1].quote_style.is_some(),
                    },
                ))
            }
            _ => None,
        }
    }

    /// As tabelas de que uma expressão fala; `None` quando ela tem alguma
    /// coisa que não dá para saber de quem é.
    fn tables_of(&self, e: &Expr) -> Option<HashSet<usize>> {
        let mut found = HashSet::new();
        let mut unknown = false;
        let _ = visit_expressions(e, |inner: &Expr| {
            match inner {
                Expr::Identifier(_) | Expr::CompoundIdentifier(_) => match self.column(inner) {
                    Some((t, _)) => {
                        found.insert(t);
                    }
                    None => unknown = true,
                },
                _ => {}
            }
            ControlFlow::<()>::Continue(())
        });
        (!unknown).then_some(found)
    }

    /// O filtro `e`, que só fala da tabela `table`, no SQL do servidor
    /// dela; `None` quando o servidor poderia responder diferente.
    fn filter(&self, e: &Expr, table: usize) -> Option<Filter> {
        let mut needs = Vec::new();
        let sql = self.translate(e, table, &mut needs)?;
        Some(Filter { sql, needs })
    }

    fn translate(&self, e: &Expr, table: usize, needs: &mut Vec<(Column, Need)>) -> Option<String> {
        let engine = self.engine(table);
        let column = |e: &Expr| self.column(e).filter(|(t, _)| *t == table).map(|(_, c)| c);
        match e {
            Expr::Nested(inner) => self.translate(inner, table, needs),
            Expr::BinaryOp { left, op, right }
                if matches!(op, BinaryOperator::And | BinaryOperator::Or) =>
            {
                let word = if *op == BinaryOperator::And {
                    "AND"
                } else {
                    "OR"
                };
                Some(format!(
                    "({} {word} {})",
                    self.translate(left, table, needs)?,
                    self.translate(right, table, needs)?
                ))
            }
            Expr::BinaryOp { left, op, right } => {
                // coluna e valor, em qualquer ordem
                let (c, value, op) = match (column(left), literal(right)) {
                    (Some(c), Some(v)) => (c, v, op.clone()),
                    _ => (column(right)?, literal(left)?, flip(op)?),
                };
                compare(engine, &c, &op, &value, needs)
            }
            Expr::IsNull(inner) => Some(format!("{} IS NULL", column(inner)?.sql(engine))),
            Expr::IsNotNull(inner) => Some(format!("{} IS NOT NULL", column(inner)?.sql(engine))),
            Expr::In {
                expr,
                target: InTarget::List(list),
                negated,
                global: false,
            } => {
                let c = column(expr)?;
                let values: Vec<Literal> = list.iter().map(literal).collect::<Option<_>>()?;
                if values.is_empty() || values.contains(&Literal::Null) {
                    return None;
                }
                if *negated {
                    // Negar só é igual nos dois lados com números.
                    if values.iter().any(|v| !matches!(v, Literal::Number(_))) {
                        return None;
                    }
                    needs.push((c.clone(), Need::Number));
                }
                let list: Vec<String> = values.iter().map(|v| v.sql(engine)).collect();
                Some(format!(
                    "{} {}IN ({})",
                    c.sql(engine),
                    if *negated { "NOT " } else { "" },
                    list.join(", ")
                ))
            }
            Expr::Between {
                expr,
                negated: false,
                low,
                high,
            } => {
                let c = column(expr)?;
                let low = compare(engine, &c, &BinaryOperator::GtEq, &literal(low)?, needs)?;
                let high = compare(engine, &c, &BinaryOperator::LtEq, &literal(high)?, needs)?;
                Some(format!("({low} AND {high})"))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Literal {
    Number(String),
    Text(String),
    Null,
}

impl Literal {
    fn sql(&self, engine: Engine) -> String {
        match self {
            Literal::Number(n) => n.clone(),
            Literal::Text(t) => text_literal(engine, t),
            Literal::Null => "NULL".to_string(),
        }
    }
}

/// Um texto como literal no SQL do servidor.
pub fn text_literal(engine: Engine, text: &str) -> String {
    let body = text.replace('\'', "''");
    match engine {
        Engine::SqlServer if !text.is_ascii() => format!("N'{body}'"),
        // No PostgreSQL a barra só é literal com E'' fora de questão: sem
        // barra o texto vale igual em qualquer configuração.
        _ => format!("'{body}'"),
    }
}

fn literal(e: &Expr) -> Option<Literal> {
    match e {
        Expr::Nested(inner) => literal(inner),
        Expr::Value(v) => match &v.value {
            Value::Number(n, _) => number(&n.to_string()).map(Literal::Number),
            // Com barra o PostgreSQL antigo pode ler diferente; e um texto
            // enorme não vale a viagem.
            Value::SingleQuotedString(s) if !s.contains('\\') && s.len() <= 200 => {
                Some(Literal::Text(s.clone()))
            }
            Value::Null => Some(Literal::Null),
            _ => None,
        },
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => match literal(expr)? {
            Literal::Number(n) if !n.starts_with('-') => Some(Literal::Number(format!("-{n}"))),
            _ => None,
        },
        _ => None,
    }
}

/// Um número escrito do jeito que todo servidor lê igual.
fn number(text: &str) -> Option<String> {
    let ok = !text.is_empty()
        && text.len() <= 30
        && text.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && text.bytes().filter(|b| *b == b'.').count() <= 1
        && text.bytes().any(|b| b.is_ascii_digit());
    ok.then(|| text.to_string())
}

fn flip(op: &BinaryOperator) -> Option<BinaryOperator> {
    Some(match op {
        BinaryOperator::Eq => BinaryOperator::Eq,
        BinaryOperator::NotEq => BinaryOperator::NotEq,
        BinaryOperator::Lt => BinaryOperator::Gt,
        BinaryOperator::LtEq => BinaryOperator::GtEq,
        BinaryOperator::Gt => BinaryOperator::Lt,
        BinaryOperator::GtEq => BinaryOperator::LtEq,
        _ => return None,
    })
}

/// `AAAA-MM-DD`, com ou sem ` HH:MM:SS`: `Some(true)` quando é só a data.
fn iso_date(text: &str) -> Option<bool> {
    let b = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| b[range].iter().all(u8::is_ascii_digit);
    let date = b.len() >= 10
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10);
    if !date {
        return None;
    }
    match b.len() {
        10 => Some(true),
        19 if b[10] == b' '
            && digits(11..13)
            && b[13] == b':'
            && digits(14..16)
            && b[16] == b':'
            && digits(17..19) =>
        {
            Some(false)
        }
        _ => None,
    }
}

/// `coluna op valor` no SQL do servidor.
fn compare(
    engine: Engine,
    c: &Column,
    op: &BinaryOperator,
    value: &Literal,
    needs: &mut Vec<(Column, Need)>,
) -> Option<String> {
    let name = c.sql(engine);
    let word = match (op, value) {
        (_, Literal::Null) => return None,
        // Igual: o servidor responde igual ou com linhas a mais (texto sem
        // diferenciar maiúsculas, por exemplo); o filtro local acerta.
        (BinaryOperator::Eq, _) => "=",
        // Diferente só com número: com texto o servidor poderia tirar
        // linhas que aqui ficariam.
        (BinaryOperator::NotEq, Literal::Number(_)) => {
            needs.push((c.clone(), Need::Number));
            "<>"
        }
        (
            BinaryOperator::Lt | BinaryOperator::LtEq | BinaryOperator::Gt | BinaryOperator::GtEq,
            Literal::Number(_),
        ) => {
            needs.push((c.clone(), Need::Number));
            match op {
                BinaryOperator::Lt => "<",
                BinaryOperator::LtEq => "<=",
                BinaryOperator::Gt => ">",
                _ => ">=",
            }
        }
        (
            BinaryOperator::Lt | BinaryOperator::LtEq | BinaryOperator::Gt | BinaryOperator::GtEq,
            Literal::Text(t),
        ) => {
            // Datas: aqui elas são texto, e `2024-01-01 00:00:00` é maior
            // que `2024-01-01`; no servidor são iguais. Mandando sempre
            // com o igual junto, o servidor nunca tira linha a mais.
            let date_only = iso_date(t)?;
            needs.push((c.clone(), Need::Date { date_only }));
            match op {
                BinaryOperator::Lt | BinaryOperator::LtEq => "<=",
                _ => ">=",
            }
        }
        _ => return None,
    };
    Some(format!("{name} {word} {}", value.sql(engine)))
}

fn conjuncts<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            conjuncts(left, out);
            conjuncts(right, out);
        }
        Expr::Nested(inner)
            if matches!(
                **inner,
                Expr::BinaryOp {
                    op: BinaryOperator::And,
                    ..
                }
            ) =>
        {
            conjuncts(inner, out)
        }
        other => out.push(other),
    }
}

/// `main."pp_N"`: o índice da fonte.
fn source_of(name: &ObjectName, plan: &Plan) -> Option<usize> {
    let parts: Vec<&str> = name
        .0
        .iter()
        .map(|p| p.as_ident().map(|i| i.value.as_str()))
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        [schema, table] if schema.eq_ignore_ascii_case("main") => {
            plan.sources.iter().position(|s| s.table == *table)
        }
        _ => None,
    }
}

const AGGREGATES: [&str; 12] = [
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "total",
    "group_concat",
    "string_agg",
    "json_group_array",
    "json_group_object",
    "jsonb_group_array",
    "jsonb_group_object",
];

/// Analisa o plano: o que cada servidor pode fazer.
pub fn analyze(plan: &Plan, servers: &[Server]) -> Analysis {
    let mut analysis = Analysis {
        sources: vec![SourcePlan::default(); plan.sources.len()],
        top: None,
    };
    for statement in &plan.statements {
        let top = statement_plan(&statement.sql, plan, servers, &mut analysis.sources);
        if plan.statements.len() == 1 {
            analysis.top = top;
        }
    }
    analysis
}

/// Analisa um comando; o que não se encaixa fica como está (sem
/// otimização).
fn statement_plan(
    sql: &str,
    plan: &Plan,
    servers: &[Server],
    out: &mut [SourcePlan],
) -> Option<Top> {
    let mut parsed = Parser::parse_sql(&SQLiteDialect {}, sql).ok()?;
    if parsed.len() != 1 {
        return None;
    }
    let Statement::Query(query) = parsed.remove(0) else {
        return None;
    };
    if query.with.is_some() || query.fetch.is_some() {
        return None;
    }
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    // Subconsultas têm nomes próprios: sem elas dá para saber de quem é
    // cada coluna.
    let mut nested = false;
    let _ = visit_expressions(query.as_ref(), |e: &Expr| {
        if matches!(e, Expr::Subquery(_) | Expr::Exists { .. })
            || matches!(e, Expr::In { target, .. } if !matches!(target, InTarget::List(_)))
        {
            nested = true;
        }
        ControlFlow::<()>::Continue(())
    });
    if nested {
        return None;
    }

    // As tabelas do FROM e as condições de cada JOIN.
    let mut tables: Vec<Table> = Vec::new();
    // Condições que valem para a linha toda (WHERE e JOIN comum).
    let mut inner: Vec<&Expr> = Vec::new();
    // Condições do ON de um LEFT JOIN, com a tabela da direita.
    let mut left: Vec<(usize, &Expr)> = Vec::new();
    // Tem JOIN de um tipo em que filtrar antes muda o resultado.
    let mut only_columns = false;
    let add = |factor: &TableFactor, nullable: bool, tables: &mut Vec<Table>| -> Option<usize> {
        let TableFactor::Table {
            name, alias, args, ..
        } = factor
        else {
            return None;
        };
        if args.is_some() {
            return None;
        }
        let source = source_of(name, plan)?;
        let alias = match alias {
            Some(a) => a.name.value.to_lowercase(),
            None => plan.sources[source].table.to_lowercase(),
        };
        if tables.iter().any(|t| t.alias == alias) {
            return None;
        }
        tables.push(Table {
            source,
            alias,
            nullable,
        });
        Some(tables.len() - 1)
    };
    for from in &select.from {
        add(&from.relation, false, &mut tables)?;
        for join in &from.joins {
            let (constraint, is_left) = match &join.join_operator {
                JoinOperator::Join(c) | JoinOperator::Inner(c) | JoinOperator::CrossJoin(c) => {
                    (c, false)
                }
                JoinOperator::Left(c) | JoinOperator::LeftOuter(c) => (c, true),
                JoinOperator::Right(c)
                | JoinOperator::RightOuter(c)
                | JoinOperator::FullOuter(c) => {
                    only_columns = true;
                    (c, false)
                }
                _ => return None,
            };
            let table = add(&join.relation, is_left, &mut tables)?;
            match constraint {
                JoinConstraint::On(e) if is_left => {
                    let mut list = Vec::new();
                    conjuncts(e, &mut list);
                    left.extend(list.into_iter().map(|e| (table, e)));
                }
                JoinConstraint::On(e) => conjuncts(e, &mut inner),
                JoinConstraint::None => {}
                // USING e NATURAL usam colunas que a consulta não escreve.
                _ => return None,
            }
        }
    }
    if tables.is_empty() {
        return None;
    }
    if let Some(e) = &select.selection {
        conjuncts(e, &mut inner);
    }
    let scope = Scope {
        tables,
        plan,
        servers,
    };
    let optimizable = |t: &Table| {
        let source = &plan.sources[t.source];
        source.relation.is_some() && source.uses == 1
    };

    // Colunas usadas por tabela.
    let mut columns: Vec<Option<Vec<Column>>> = vec![Some(Vec::new()); scope.tables.len()];
    let mut aliases: Vec<String> = Vec::new();
    for item in &select.projection {
        match item {
            SelectItem::Wildcard(_) => columns.iter_mut().for_each(|c| *c = None),
            SelectItem::QualifiedWildcard(SelectItemQualifiedWildcardKind::ObjectName(name), _) => {
                let alias = name
                    .0
                    .last()
                    .and_then(|p| p.as_ident())
                    .map(|i| i.value.to_lowercase());
                match scope
                    .tables
                    .iter()
                    .position(|t| Some(&t.alias) == alias.as_ref())
                {
                    Some(t) => columns[t] = None,
                    None => columns.iter_mut().for_each(|c| *c = None),
                }
            }
            SelectItem::QualifiedWildcard(..) => columns.iter_mut().for_each(|c| *c = None),
            SelectItem::ExprWithAlias { alias, .. } => aliases.push(alias.value.to_lowercase()),
            _ => {}
        }
    }
    // Um nome solto no ORDER BY ou no GROUP BY pode ser o apelido de uma
    // coluna do resultado.
    let mut loose_aliases = 0usize;
    let is_alias =
        |e: &Expr| matches!(e, Expr::Identifier(id) if aliases.contains(&id.value.to_lowercase()));
    let order: Vec<&sqlparser::ast::OrderByExpr> = match &query.order_by {
        Some(o) => match &o.kind {
            OrderByKind::Expressions(list) => list.iter().collect(),
            _ => return None,
        },
        None => Vec::new(),
    };
    loose_aliases += order.iter().filter(|o| is_alias(&o.expr)).count();
    let grouped = match &select.group_by {
        GroupByExpr::Expressions(list, _) => {
            loose_aliases += list.iter().filter(|e| is_alias(e)).count();
            !list.is_empty()
        }
        GroupByExpr::All(_) => true,
    };
    let mut loose = 0usize;
    let mut aggregated = false;
    let _ = visit_expressions(query.as_ref(), |e: &Expr| {
        match e {
            Expr::Identifier(_) | Expr::CompoundIdentifier(_) => match scope.column(e) {
                Some((t, c)) => {
                    if let Some(list) = &mut columns[t]
                        && !list.iter().any(|x| x.same(&c))
                    {
                        list.push(c);
                    }
                }
                None => loose += 1,
            },
            Expr::Function(f) => {
                let name = f
                    .name
                    .0
                    .last()
                    .and_then(|p| p.as_ident())
                    .map(|i| i.value.to_lowercase())
                    .unwrap_or_default();
                if f.over.is_some() || AGGREGATES.contains(&name.as_str()) {
                    aggregated = true;
                }
            }
            _ => {}
        }
        ControlFlow::<()>::Continue(())
    });
    if loose > loose_aliases {
        // Há nomes que não dá para saber de qual tabela são.
        columns.iter_mut().for_each(|c| *c = None);
    }

    for (t, table) in scope.tables.iter().enumerate() {
        if !optimizable(table) {
            continue;
        }
        let plan_of = &mut out[table.source];
        plan_of.optimizable = true;
        plan_of.columns = columns[t].clone();
    }
    if !only_columns {
        // Filtros e ligações.
        let filter = |t: usize, e: &Expr, out: &mut [SourcePlan]| {
            if optimizable(&scope.tables[t])
                && let Some(f) = scope.filter(e, t)
            {
                out[scope.tables[t].source].filters.push(f);
            }
        };
        let link = |child: usize, parent: usize, c: Column, p: Column, out: &mut [SourcePlan]| {
            let (child_table, parent_table) = (&scope.tables[child], &scope.tables[parent]);
            if optimizable(child_table) && child_table.source != parent_table.source {
                out[child_table.source].links.push(Link {
                    column: c,
                    parent: parent_table.source,
                    parent_column: p.name,
                });
            }
        };
        let equality = |e: &Expr| match e {
            Expr::BinaryOp {
                left,
                op: BinaryOperator::Eq,
                right,
            } => match (scope.column(left), scope.column(right)) {
                (Some(a), Some(b)) if a.0 != b.0 => Some((a, b)),
                _ => None,
            },
            _ => None,
        };
        for e in &inner {
            if let Some(((a, ac), (b, bc))) = equality(e) {
                if !scope.tables[a].nullable && !scope.tables[b].nullable {
                    link(a, b, ac.clone(), bc.clone(), out);
                    link(b, a, bc, ac, out);
                }
                continue;
            }
            let Some(found) = scope.tables_of(e) else {
                continue;
            };
            if found.len() == 1 {
                let t = *found.iter().next().unwrap_or(&0);
                if !scope.tables[t].nullable {
                    filter(t, e, out);
                }
            }
        }
        for (right, e) in &left {
            if let Some(((a, ac), (b, bc))) = equality(e) {
                // Só a tabela da direita pode ser lida pelas chaves da outra.
                if a == *right {
                    link(a, b, ac, bc, out);
                } else if b == *right {
                    link(b, a, bc, ac, out);
                }
                continue;
            }
            if scope
                .tables_of(e)
                .is_some_and(|f| f.len() == 1 && f.contains(right))
            {
                filter(*right, e, out);
            }
        }
    }

    // "Os N primeiros": a ordem e o limite são de uma tabela só.
    if only_columns
        || grouped
        || aggregated
        || select.having.is_some()
        || !matches!(select.distinct, None | Some(Distinct::All))
    {
        return None;
    }
    let number = |e: &Expr| match literal(e)? {
        Literal::Number(n) => n.parse::<u64>().ok(),
        _ => None,
    };
    let (limit, offset) = match query.limit_clause.as_ref()? {
        LimitClause::LimitOffset {
            limit: Some(limit),
            offset,
            limit_by,
        } if limit_by.is_empty() => (
            number(limit)?,
            match offset {
                Some(o) => number(&o.value)?,
                None => 0,
            },
        ),
        LimitClause::OffsetCommaLimit { offset, limit } => (number(limit)?, number(offset)?),
        _ => return None,
    };
    let need = limit.checked_add(offset)?;
    let mut driver: Option<usize> = order.is_empty().then_some(0);
    let mut keys = Vec::new();
    for o in &order {
        if o.options.nulls_first.is_some() || o.with_fill.is_some() {
            return None;
        }
        let (t, c) = scope.column(&o.expr)?;
        if driver.is_some_and(|d| d != t) {
            return None;
        }
        driver = Some(t);
        let desc = match &o.options.sort {
            None | Some(OrderBySort::Asc) => false,
            Some(OrderBySort::Desc) => true,
            Some(OrderBySort::Using(_)) => return None,
        };
        keys.push((c, desc));
    }
    let driver = &scope.tables[driver?];
    if driver.nullable || !optimizable(driver) {
        return None;
    }
    // A mesma consulta, sem o deslocamento, só para contar.
    let mut counted: Query = (*query).clone();
    counted.limit_clause = Some(LimitClause::LimitOffset {
        limit: Some(Expr::value(Value::Number(need.to_string(), false))),
        offset: None,
        limit_by: Vec::new(),
    });
    let _: &Select = select;
    Some(Top {
        source: driver.source,
        order: keys,
        need,
        check: format!("SELECT count(*) FROM ({counted})"),
    })
}

/// Como os vazios (NULL) entram na ordem mandada ao servidor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nulls {
    /// A ordem do servidor já é a daqui (ou a coluna não tem vazios).
    Same,
    /// Diz ao PostgreSQL onde os vazios ficam, como no SQLite: primeiro
    /// na ordem crescente, por último na decrescente.
    Explicit,
}

/// O SELECT que lê uma tabela no servidor.
pub struct Read<'a> {
    pub engine: Engine,
    /// O nome da tabela no SQL do servidor.
    pub relation: &'a str,
    pub columns: Option<&'a [Column]>,
    /// Condições já no SQL do servidor.
    pub filters: Vec<String>,
    pub order: &'a [(Column, bool)],
    pub nulls: Nulls,
    pub limit: Option<u64>,
}

impl Read<'_> {
    pub fn sql(&self) -> String {
        let columns = match self.columns {
            None => "*".to_string(),
            // A consulta não usa coluna nenhuma (um count(*)): uma só,
            // para ter as linhas.
            Some([]) => "1 AS pp_one".to_string(),
            Some(list) => list
                .iter()
                .map(|c| c.sql(self.engine))
                .collect::<Vec<_>>()
                .join(", "),
        };
        let mut sql = String::from("SELECT ");
        if let (Engine::SqlServer, Some(n)) = (self.engine, self.limit) {
            sql.push_str(&format!("TOP ({n}) "));
        }
        sql.push_str(&format!("{columns} FROM {}", self.relation));
        if !self.filters.is_empty() {
            sql.push_str(&format!(" WHERE {}", self.filters.join(" AND ")));
        }
        if !self.order.is_empty() {
            let list: Vec<String> = self
                .order
                .iter()
                .map(|(c, desc)| {
                    let mut item = c.sql(self.engine);
                    if *desc {
                        item.push_str(" DESC");
                    }
                    if self.nulls == Nulls::Explicit && self.engine == Engine::PostgreSql {
                        item.push_str(if *desc { " NULLS LAST" } else { " NULLS FIRST" });
                    }
                    item
                })
                .collect();
            sql.push_str(&format!(" ORDER BY {}", list.join(", ")));
        }
        if let (true, Some(n)) = (self.engine != Engine::SqlServer, self.limit) {
            sql.push_str(&format!(" LIMIT {n}"));
        }
        sql
    }
}

/// `coluna IN (chaves)` no SQL do servidor. As chaves vão como texto: o
/// servidor converte para o tipo da coluna.
pub fn key_filter(engine: Engine, column: &Column, keys: &[String]) -> String {
    let list: Vec<String> = keys.iter().map(|k| text_literal(engine, k)).collect();
    format!("{} IN ({})", column.sql(engine), list.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Home, plan_with};

    fn servers() -> Vec<Server> {
        vec![
            Server {
                name: "main".into(),
                engine: Engine::PostgreSql,
                database: "app".into(),
            },
            Server {
                name: "saas".into(),
                engine: Engine::PostgreSql,
                database: "saas".into(),
            },
            Server {
                name: "erp".into(),
                engine: Engine::SqlServer,
                database: "erp".into(),
            },
            Server {
                name: "docs".into(),
                engine: Engine::MongoDb,
                database: "docs".into(),
            },
        ]
    }

    fn analyzed(sql: &str) -> (Plan, Analysis) {
        let servers = servers();
        let home = Home {
            server: 0,
            database: "app".into(),
        };
        let plan = plan_with(sql, &servers, Some(&home)).expect("plan");
        let analysis = analyze(&plan, &servers);
        (plan, analysis)
    }

    fn names(columns: &Option<Vec<Column>>) -> Vec<String> {
        columns
            .as_ref()
            .expect("columns")
            .iter()
            .map(|c| c.name.clone())
            .collect()
    }

    #[test]
    fn the_latest_row_of_a_join_reads_the_order_and_the_limit_on_the_server() {
        let (plan, a) = analyzed(
            "SELECT d.caixa_id, l.id FROM documento d \
             inner join saas.lotes l on l.chave_de_processamento = d.chave_processamento_tiib \
             ORDER BY d.id DESC LIMIT 1",
        );
        assert_eq!(plan.sources.len(), 2);
        let top = a.top.as_ref().expect("top");
        assert_eq!(top.source, 0);
        assert_eq!(top.need, 1);
        assert_eq!(top.order.len(), 1);
        assert_eq!(top.order[0].0.name, "id");
        assert!(top.order[0].1);
        assert_eq!(
            names(&a.sources[0].columns),
            ["caixa_id", "chave_processamento_tiib", "id"]
        );
        assert_eq!(
            names(&a.sources[1].columns),
            ["id", "chave_de_processamento"]
        );
        // lotes é lida pelas chaves que documento trouxe
        assert_eq!(a.sources[1].links.len(), 1);
        assert_eq!(a.sources[1].links[0].parent, 0);
        assert_eq!(a.sources[1].links[0].column.name, "chave_de_processamento");
        assert_eq!(
            a.sources[1].links[0].parent_column,
            "chave_processamento_tiib"
        );
        assert!(top.check.starts_with("SELECT count(*) FROM (SELECT"));
        assert!(top.check.contains("LIMIT 1"));
    }

    #[test]
    fn filters_of_one_table_go_to_its_server() {
        let (_, a) = analyzed(
            "SELECT c.nome, p.total FROM saas.clientes c JOIN erp.dbo.pedidos p ON p.cliente = c.id \
             WHERE c.uf = 'SP' AND p.total > 100 AND p.data >= '2024-01-01' \
             AND c.nome LIKE 'A%' AND (p.status IN ('A', 'B') OR p.status IS NULL) AND c.id <> p.id",
        );
        let c: Vec<&str> = a.sources[0]
            .filters
            .iter()
            .map(|f| f.sql.as_str())
            .collect();
        assert_eq!(c, ["uf = 'SP'"]);
        let p: Vec<&str> = a.sources[1]
            .filters
            .iter()
            .map(|f| f.sql.as_str())
            .collect();
        assert_eq!(
            p,
            [
                "[total] > 100",
                "[data] >= '2024-01-01'",
                "([status] IN ('A', 'B') OR [status] IS NULL)"
            ]
        );
        assert_eq!(a.sources[1].filters[0].needs[0].1, Need::Number);
        assert_eq!(
            a.sources[1].filters[1].needs[0].1,
            Need::Date { date_only: true }
        );
        assert!(a.top.is_none());
    }

    #[test]
    fn a_strict_date_comparison_is_sent_with_the_equal_sign() {
        let (_, a) = analyzed(
            "SELECT * FROM saas.vendas v WHERE v.dia > '2024-01-01' AND '2024-02-01 00:00:00' > v.dia",
        );
        let f: Vec<&str> = a.sources[0]
            .filters
            .iter()
            .map(|f| f.sql.as_str())
            .collect();
        assert_eq!(f, ["dia >= '2024-01-01'", "dia <= '2024-02-01 00:00:00'"]);
        assert!(a.sources[0].columns.is_none());
    }

    #[test]
    fn what_the_server_could_answer_differently_stays_here() {
        let (_, a) = analyzed(
            "SELECT * FROM saas.t t WHERE t.a <> 'x' AND NOT t.b = 1 AND t.c > 'abc' \
             AND t.d = lower('X') AND t.e NOT IN ('a') AND t.f = NULL AND t.g = 'a\\b'",
        );
        assert!(
            a.sources[0].filters.is_empty(),
            "{:?}",
            a.sources[0].filters
        );
    }

    #[test]
    fn a_left_join_filters_only_the_right_table() {
        let (_, a) = analyzed(
            "SELECT a.x, b.y FROM saas.a a LEFT JOIN erp.b b ON b.id = a.id AND b.kind = 2 AND a.live = 1 \
             WHERE b.y IS NULL AND a.z = 3",
        );
        let fa: Vec<&str> = a.sources[0]
            .filters
            .iter()
            .map(|f| f.sql.as_str())
            .collect();
        let fb: Vec<&str> = a.sources[1]
            .filters
            .iter()
            .map(|f| f.sql.as_str())
            .collect();
        assert_eq!(fa, ["z = 3"]);
        assert_eq!(fb, ["[kind] = 2"]);
        assert!(a.sources[0].links.is_empty());
        assert_eq!(a.sources[1].links.len(), 1);
    }

    #[test]
    fn a_star_reads_every_column_and_loose_names_too() {
        let (_, a) = analyzed("SELECT a.*, b.y FROM saas.a a JOIN erp.b b ON b.id = a.id");
        assert!(a.sources[0].columns.is_none());
        assert_eq!(names(&a.sources[1].columns), ["y", "id"]);
        let (_, a) = analyzed("SELECT nome, b.y FROM saas.a a JOIN erp.b b ON b.id = a.id");
        assert!(a.sources[0].columns.is_none());
        assert!(a.sources[1].columns.is_none());
        // um apelido do resultado na ordem não atrapalha
        let (_, a) = analyzed(
            "SELECT a.nome AS n, b.y FROM saas.a a JOIN erp.b b ON b.id = a.id ORDER BY n",
        );
        assert_eq!(names(&a.sources[0].columns), ["nome", "id"]);
    }

    #[test]
    fn one_table_takes_the_loose_names() {
        let (_, a) = analyzed(
            "SELECT id, nome FROM saas.clientes WHERE uf = 'SP' ORDER BY id LIMIT 10 OFFSET 5",
        );
        assert_eq!(names(&a.sources[0].columns), ["id", "nome", "uf"]);
        assert_eq!(a.sources[0].filters.len(), 1);
        let top = a.top.expect("top");
        assert_eq!(top.need, 15);
        assert!(!top.order[0].1);
    }

    #[test]
    fn groups_subqueries_and_repeated_tables_are_left_alone() {
        let (_, a) = analyzed("SELECT a.uf, count(*) FROM saas.a a GROUP BY a.uf LIMIT 5");
        assert!(a.top.is_none());
        assert!(a.sources[0].optimizable);
        let (_, a) = analyzed("SELECT * FROM saas.a a WHERE a.id IN (SELECT b.id FROM erp.b b)");
        assert!(!a.any());
        let (_, a) = analyzed("SELECT x.id FROM saas.a x JOIN saas.a y ON y.pai = x.id");
        assert!(!a.any());
        let (_, a) = analyzed("WITH t AS (SELECT * FROM saas.a) SELECT * FROM t");
        assert!(!a.any());
        // fonte escrita na língua do servidor e NoSQL: só servem de chave
        let (_, a) =
            analyzed("SELECT p.id FROM docs.pedidos d JOIN erp.pedidos p ON p.id = d.pedido");
        assert!(!a.sources[0].optimizable);
        assert!(a.sources[1].optimizable);
        assert_eq!(a.sources[1].links[0].parent, 0);
    }

    #[test]
    fn the_server_select_is_written_for_each_engine() {
        let id = Column {
            name: "id".into(),
            quoted: false,
        };
        let odd = Column {
            name: "Nome Completo".into(),
            quoted: true,
        };
        let order = [(id.clone(), true)];
        let columns = [id.clone(), odd.clone()];
        let mut read = Read {
            engine: Engine::PostgreSql,
            relation: "\"public\".\"t\"",
            columns: Some(&columns),
            filters: vec![
                "id > 1".into(),
                key_filter(Engine::PostgreSql, &odd, &["a'b".into(), "c".into()]),
            ],
            order: &order,
            nulls: Nulls::Explicit,
            limit: Some(10),
        };
        assert_eq!(
            read.sql(),
            "SELECT id, \"Nome Completo\" FROM \"public\".\"t\" WHERE id > 1 AND \"Nome Completo\" IN ('a''b', 'c') ORDER BY id DESC NULLS LAST LIMIT 10"
        );
        read.engine = Engine::SqlServer;
        read.relation = "[dbo].[t]";
        read.filters = vec![key_filter(Engine::SqlServer, &id, &["ação".into()])];
        assert_eq!(
            read.sql(),
            "SELECT TOP (10) [id], [Nome Completo] FROM [dbo].[t] WHERE [id] IN (N'ação') ORDER BY [id] DESC"
        );
        read.columns = Some(&[]);
        read.order = &[];
        read.limit = None;
        read.filters.clear();
        assert_eq!(read.sql(), "SELECT 1 AS pp_one FROM [dbo].[t]");
    }
}
