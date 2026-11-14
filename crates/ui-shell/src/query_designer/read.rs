//! O caminho inverso do Query Designer (marco 5.5): lê o SELECT
//! selecionado no editor com o `sqlparser` e monta a [`Query`] a partir
//! dele (tabelas, JOINs, colunas, WHERE, GROUP BY e ORDER BY).
//!
//! Desde o marco 6.1 lê também JOINs externos, DISTINCT, LIMIT, HAVING,
//! OR (nas colunas Or... da grade), expressões e `count(*)`.
//!
//! O que o designer não sabe desenhar vira um [`ReadError`] em vez de ser
//! carregado pela metade: CTE, subconsulta e UNION têm aviso próprio; o
//! resto (USING, OFFSET, DISTINCT ON, NULLS FIRST...), o genérico
//! [`ReadError::Unsupported`]. Assim o OK nunca troca a seleção por uma
//! consulta que faz outra coisa.

use std::fmt;
use std::ops::ControlFlow;

use pgstudio_core::metadata::Model;
use pgstudio_core::schema::Id;
use sqlparser::ast::{
    BinaryOperator, Distinct, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, GroupByExpr,
    Ident, InTarget, JoinConstraint, JoinOperator, LimitClause, ObjectName, OrderByKind,
    OrderBySort, Select, SelectItem, SetExpr, Statement, TableFactor, TableWithJoins, Value,
    visit_expressions,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

use super::{Aggregate, JoinKind, Query, QueryTable, Row, SortType, condition};

/// Por que o SQL não pôde ser carregado no designer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Não é SQL válido para o parser.
    Syntax(String),
    /// Mais de um comando, ou um que não é SELECT.
    NotSelect,
    /// `WITH ...`.
    Cte,
    /// `UNION`, `INTERSECT` ou `EXCEPT` (o operador vai junto).
    SetOperation(String),
    /// Subconsulta no FROM, no WHERE ou na lista do SELECT.
    Subquery,
    /// Tabela que não está no banco (ou é uma view).
    UnknownTable(String),
    UnknownColumn(String),
    AmbiguousColumn(String),
    /// Outra parte que o designer não representa (o texto diz qual).
    Unsupported(String),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Syntax(e) => write!(f, "The selected text could not be parsed: {e}"),
            ReadError::NotSelect => f.write_str("Select a single SELECT statement to design it."),
            ReadError::Cte => f.write_str(
                "The query uses a common table expression (WITH), which the Query Designer \
                 can't show.",
            ),
            ReadError::SetOperation(op) => write!(
                f,
                "The query combines results with {op}, which the Query Designer can't show."
            ),
            ReadError::Subquery => {
                f.write_str("The query has a subquery, which the Query Designer can't show.")
            }
            ReadError::UnknownTable(t) => {
                write!(f, "The table {t} was not found in the database.")
            }
            ReadError::UnknownColumn(c) => write!(f, "The column {c} was not found."),
            ReadError::AmbiguousColumn(c) => write!(
                f,
                "The column {c} exists in more than one table; qualify it with the table name."
            ),
            ReadError::Unsupported(what) => {
                write!(f, "The Query Designer can't show {what}.")
            }
        }
    }
}

impl std::error::Error for ReadError {}

fn unsupported<T>(what: impl Into<String>) -> Result<T, ReadError> {
    Err(ReadError::Unsupported(what.into()))
}

/// Lê `sql` (um SELECT) com as tabelas de `model`.
pub fn read(sql: &str, model: &Model) -> Result<Query, ReadError> {
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, sql)
        .map_err(|e| ReadError::Syntax(e.to_string()))?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return Err(ReadError::NotSelect);
    };
    if query.with.is_some() {
        return Err(ReadError::Cte);
    }
    let select = match query.body.as_ref() {
        SetExpr::Select(s) => s,
        SetExpr::SetOperation { op, .. } => {
            return Err(ReadError::SetOperation(op.to_string().to_uppercase()));
        }
        SetExpr::Query(_) => return Err(ReadError::Subquery),
        _ => return Err(ReadError::NotSelect),
    };
    // Subconsulta em qualquer expressão (IN (SELECT ...), EXISTS, escalar).
    let found = visit_expressions(&statements, |e| match e {
        Expr::Subquery(_)
        | Expr::Exists { .. }
        | Expr::In {
            target: InTarget::Subquery(_),
            ..
        } => ControlFlow::Break(()),
        Expr::Function(f) if matches!(f.args, FunctionArguments::Subquery(_)) => {
            ControlFlow::Break(())
        }
        _ => ControlFlow::Continue(()),
    });
    if found.is_break() {
        return Err(ReadError::Subquery);
    }
    let limit = match &query.limit_clause {
        None => None,
        Some(LimitClause::LimitOffset {
            limit: Some(Expr::Value(v)),
            offset: None,
            limit_by,
        }) if limit_by.is_empty() => match &v.value {
            Value::Number(n, _) => match n.parse::<u64>() {
                Ok(n) => Some(n),
                Err(_) => return unsupported(format!("LIMIT {n}")),
            },
            _ => return unsupported("this LIMIT"),
        },
        Some(LimitClause::LimitOffset {
            offset: Some(_), ..
        }) => {
            return unsupported("OFFSET");
        }
        Some(_) => return unsupported("this LIMIT"),
    };
    if query.fetch.is_some() {
        return unsupported("FETCH FIRST (use LIMIT)");
    }
    if !query.locks.is_empty() {
        return unsupported("FOR UPDATE or FOR SHARE");
    }
    if query.for_clause.is_some()
        || query.settings.is_some()
        || query.format_clause.is_some()
        || !query.pipe_operators.is_empty()
    {
        return unsupported("this kind of query");
    }
    check_select(select)?;

    let mut r = Reader {
        model,
        query: Query::default(),
        names: Vec::new(),
        grouped: Vec::new(),
    };
    r.query.limit = limit;
    r.query.distinct = matches!(select.distinct, Some(Distinct::Distinct));
    for from in &select.from {
        r.from(from)?;
    }
    // Colunas da lista do SELECT, na ordem (índice da linha de cada item,
    // para ORDER BY 2).
    let mut items = Vec::new();
    for item in &select.projection {
        items.push(r.item(item)?);
    }
    if let Some(selection) = &select.selection {
        r.criteria(selection, false)?;
    }
    match &select.group_by {
        GroupByExpr::Expressions(exprs, modifiers) => {
            if !modifiers.is_empty() {
                return unsupported("ROLLUP, CUBE or GROUPING SETS");
            }
            if !exprs.is_empty() {
                r.query.group_by = true;
            }
            for e in exprs {
                r.group(e)?;
            }
        }
        GroupByExpr::All(_) => return unsupported("GROUP BY ALL"),
    }
    if let Some(having) = &select.having {
        r.query.group_by = true;
        r.criteria(having, true)?;
    }
    if let Some(order_by) = &query.order_by {
        if order_by.interpolate.is_some() {
            return unsupported("INTERPOLATE");
        }
        let OrderByKind::Expressions(exprs) = &order_by.kind else {
            return unsupported("ORDER BY ALL");
        };
        for (n, o) in exprs.iter().enumerate() {
            if o.options.nulls_first.is_some() || o.with_fill.is_some() {
                return unsupported("NULLS FIRST or NULLS LAST in ORDER BY");
            }
            let sort = match o.options.sort {
                None | Some(OrderBySort::Asc) => SortType::Ascending,
                Some(OrderBySort::Desc) => SortType::Descending,
                Some(_) => return unsupported("ORDER BY ... USING"),
            };
            let row = r.order_row(&o.expr, &items)?;
            let row = &mut r.query.rows[row];
            if row.sort != SortType::Unsorted {
                return unsupported("the same column twice in ORDER BY");
            }
            row.sort = sort;
            row.sort_order = n as u32 + 1;
        }
    }
    let Reader { query, grouped, .. } = r;
    let mut query = query;
    query.or_columns = query.rows.iter().map(|r| r.or.len()).max().unwrap_or(0);
    if query
        .rows
        .iter()
        .any(|r| !matches!(r.aggregate, Aggregate::GroupBy) || (r.expression && r.column == "*"))
    {
        query.group_by = true;
    }
    if query.group_by {
        let mut split = Vec::new();
        for row in &mut query.rows {
            if row.aggregate != Aggregate::GroupBy || grouped.contains(&key(row)) {
                continue;
            }
            if !row.output && row.sort == SortType::Unsorted && row.has_criteria() {
                // Só filtra: fica fora do GROUP BY.
                row.aggregate = Aggregate::Where;
            } else if row.output {
                // Na saída sem estar no GROUP BY: uma expressão que já
                // agrega, ou uma coluna que depende da chave. O filtro dela,
                // se houver, vai para uma linha Where à parte.
                row.aggregate = Aggregate::Expression;
                if row.has_criteria() {
                    split.push(Row {
                        table: row.table,
                        column: row.column.clone(),
                        expression: row.expression,
                        filter: std::mem::take(&mut row.filter),
                        or: std::mem::take(&mut row.or),
                        aggregate: Aggregate::Where,
                        ..Default::default()
                    });
                }
            }
        }
        query.rows.extend(split);
    }
    Ok(query)
}

/// Identidade de uma linha: (é expressão, tabela, coluna ou texto).
fn key(r: &Row) -> (bool, usize, String) {
    if r.expression {
        (true, 0, r.column.clone())
    } else {
        (false, r.table, r.column.clone())
    }
}

/// O que uma expressão do SQL vira numa linha da grade.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Subject {
    Column(usize, String),
    Expression(String),
}

impl Subject {
    fn row(&self) -> Row {
        match self {
            Subject::Column(table, column) => Row {
                table: *table,
                column: column.clone(),
                ..Default::default()
            },
            Subject::Expression(text) => Row {
                column: text.clone(),
                expression: true,
                ..Default::default()
            },
        }
    }

    fn matches(&self, r: &Row) -> bool {
        match self {
            Subject::Column(t, c) => !r.expression && r.table == *t && &r.column == c,
            Subject::Expression(text) => r.expression && &r.column == text,
        }
    }
}

/// Partes do SELECT que o designer não tem como mostrar.
fn check_select(s: &Select) -> Result<(), ReadError> {
    if let Some(Distinct::On(_)) = &s.distinct {
        return unsupported("DISTINCT ON");
    }
    if s.top.is_some() {
        return unsupported("TOP (use LIMIT)");
    }
    if s.into.is_some() {
        return unsupported("SELECT INTO");
    }
    if !s.named_window.is_empty() {
        return unsupported("WINDOW");
    }
    if s.exclude.is_some()
        || s.select_modifiers.is_some()
        || !s.lateral_views.is_empty()
        || s.prewhere.is_some()
        || !s.connect_by.is_empty()
        || !s.cluster_by.is_empty()
        || !s.distribute_by.is_empty()
        || !s.sort_by.is_empty()
        || s.qualify.is_some()
        || s.value_table_mode.is_some()
    {
        return unsupported("this kind of SELECT");
    }
    Ok(())
}

/// Os termos de um `a AND b AND c` (os parênteses em volta saem).
fn conjuncts(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            let mut v = conjuncts(left);
            v.extend(conjuncts(right));
            v
        }
        Expr::Nested(inner) => conjuncts(inner),
        e => vec![e],
    }
}

/// Os termos de um `a OR b OR c` (os parênteses em volta saem).
fn disjuncts(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Or,
            right,
        } => {
            let mut v = disjuncts(left);
            v.extend(disjuncts(right));
            v
        }
        Expr::Nested(inner) => disjuncts(inner),
        e => vec![e],
    }
}

/// Nome como o PostgreSQL entende: sem aspas, em minúsculas.
fn name(ident: &Ident) -> String {
    match ident.quote_style {
        Some(_) => ident.value.clone(),
        None => ident.value.to_lowercase(),
    }
}

fn object_name(o: &ObjectName) -> Option<Vec<String>> {
    o.0.iter().map(|p| p.as_ident().map(name)).collect()
}

/// Uma coluna citada no SQL, como `tabela.coluna` ou só `coluna`.
fn column_parts(e: &Expr) -> Option<Vec<String>> {
    match e {
        Expr::Identifier(i) => Some(vec![name(i)]),
        Expr::CompoundIdentifier(parts) => Some(parts.iter().map(name).collect()),
        Expr::Nested(inner) => column_parts(inner),
        _ => None,
    }
}

struct Reader<'a> {
    model: &'a Model,
    query: Query,
    /// Como cada tabela é citada nas colunas: (esquema, nome) se veio sem
    /// apelido, para aceitar `esquema.tabela.coluna`.
    names: Vec<Option<(String, String)>>,
    /// Colunas e expressões citadas no GROUP BY (veja [`key`]).
    grouped: Vec<(bool, usize, String)>,
}

impl Reader<'_> {
    fn from(&mut self, from: &TableWithJoins) -> Result<(), ReadError> {
        self.table(&from.relation)?;
        for join in &from.joins {
            let right = self.table(&join.relation)?;
            let (kind, constraint) = match &join.join_operator {
                JoinOperator::Join(c) | JoinOperator::Inner(c) => (JoinKind::Inner, c),
                JoinOperator::Left(c) | JoinOperator::LeftOuter(c) => (JoinKind::Left, c),
                JoinOperator::Right(c) | JoinOperator::RightOuter(c) => (JoinKind::Right, c),
                JoinOperator::FullOuter(c) => (JoinKind::Full, c),
                JoinOperator::CrossJoin(JoinConstraint::None) => continue,
                _ => return unsupported("this kind of JOIN"),
            };
            match constraint {
                JoinConstraint::On(on) => {
                    for e in conjuncts(on) {
                        if !self.join_condition(e, Some(right))? {
                            return unsupported(format!("the join condition {e}"));
                        }
                    }
                }
                JoinConstraint::None if kind == JoinKind::Inner => continue,
                JoinConstraint::None => return unsupported("an outer JOIN without ON"),
                JoinConstraint::Using(_) | JoinConstraint::UsingAlias(..) => {
                    return unsupported("JOIN ... USING");
                }
                JoinConstraint::Natural => return unsupported("NATURAL JOIN"),
            }
            if let Some(j) = self
                .query
                .joins
                .iter()
                .position(|j| j.left.max(j.right) == right)
            {
                self.query.set_join_kind(j, kind);
            }
        }
        Ok(())
    }

    /// Põe a tabela na consulta e devolve o índice.
    fn table(&mut self, factor: &TableFactor) -> Result<usize, ReadError> {
        let (name_parts, alias) = match factor {
            TableFactor::Table {
                name,
                alias,
                args: None,
                with_hints,
                version: None,
                with_ordinality: false,
                partitions,
                json_path: None,
                sample: None,
                index_directives,
                inheritance: None,
            } if with_hints.is_empty() && partitions.is_empty() && index_directives.is_empty() => {
                (name, alias)
            }
            TableFactor::Table { args: Some(_), .. } | TableFactor::Function { .. } => {
                return unsupported("a function in FROM");
            }
            TableFactor::Derived { .. } => return Err(ReadError::Subquery),
            TableFactor::NestedJoin { .. } => {
                return unsupported("parentheses around JOINs");
            }
            _ => return unsupported(format!("{factor} in FROM")),
        };
        let parts = object_name(name_parts)
            .ok_or_else(|| ReadError::UnknownTable(name_parts.to_string()))?;
        let id = self.resolve_table(&parts, name_parts)?;
        let table = self.model.table(id).expect("resolve_table achou");
        let reference = match alias {
            Some(a) => {
                if !a.columns.is_empty() {
                    return unsupported("column aliases on a table");
                }
                name(&a.name)
            }
            None => table.name.clone(),
        };
        if self.query.tables.iter().any(|t| t.alias == reference) {
            return unsupported(format!(
                "the table name {reference} twice (give each one an alias)"
            ));
        }
        self.names.push(
            alias
                .is_none()
                .then(|| (table.schema.clone(), table.name.clone())),
        );
        self.query.tables.push(QueryTable {
            table: id,
            alias: reference,
        });
        Ok(self.query.tables.len() - 1)
    }

    /// `[tabela]` ou `[esquema, tabela]`; sem esquema, prefere `public`.
    fn resolve_table(&self, parts: &[String], raw: &ObjectName) -> Result<Id, ReadError> {
        let unknown = || ReadError::UnknownTable(raw.to_string());
        match parts {
            [schema, table] => self
                .model
                .tables
                .iter()
                .find(|t| &t.schema == schema && &t.name == table)
                .map(|t| t.id)
                .ok_or_else(unknown),
            [table] => {
                let matches: Vec<_> = self
                    .model
                    .tables
                    .iter()
                    .filter(|t| &t.name == table)
                    .collect();
                matches
                    .iter()
                    .find(|t| t.schema == "public")
                    .or(match matches.as_slice() {
                        [only] => Some(only),
                        _ => None,
                    })
                    .map(|t| t.id)
                    .ok_or_else(unknown)
            }
            _ => Err(unknown()),
        }
    }

    /// (tabela, coluna) de uma coluna citada; `None` se não é coluna.
    fn column(&self, e: &Expr) -> Result<Option<(usize, String)>, ReadError> {
        let Some(parts) = column_parts(e) else {
            return Ok(None);
        };
        let has = |i: usize, c: &str| {
            self.model
                .table(self.query.tables[i].table)
                .is_some_and(|t| t.column(c).is_some())
        };
        let found = match parts.as_slice() {
            [c] => {
                let tables: Vec<usize> = (0..self.query.tables.len())
                    .filter(|&i| has(i, c))
                    .collect();
                match tables.as_slice() {
                    [i] => Some((*i, c.clone())),
                    [] => None,
                    _ => return Err(ReadError::AmbiguousColumn(e.to_string())),
                }
            }
            [t, c] => self
                .query
                .tables
                .iter()
                .position(|q| &q.alias == t)
                .map(|i| (i, c.clone())),
            [s, t, c] => self
                .names
                .iter()
                .position(|n| n.as_ref().is_some_and(|(ns, nt)| ns == s && nt == t))
                .map(|i| (i, c.clone())),
            _ => None,
        };
        match found {
            Some((i, c)) if has(i, &c) => Ok(Some((i, c))),
            _ => Err(ReadError::UnknownColumn(e.to_string())),
        }
    }

    /// Confere que toda coluna citada em `e` existe; devolve quantas são.
    fn check_columns(&self, e: &Expr) -> Result<usize, ReadError> {
        let mut n = 0;
        let mut error = None;
        let _ = visit_expressions(e, |e| {
            if matches!(e, Expr::Identifier(_) | Expr::CompoundIdentifier(_)) {
                match self.column(e) {
                    Ok(_) => n += 1,
                    Err(err) => {
                        error = Some(err);
                        return ControlFlow::Break(());
                    }
                }
            }
            ControlFlow::Continue(())
        });
        match error {
            Some(e) => Err(e),
            None => Ok(n),
        }
    }

    /// A coluna, ou a expressão como texto (com as colunas conferidas).
    fn subject(&self, e: &Expr) -> Result<Subject, ReadError> {
        if let Some((t, c)) = self.column(e)? {
            return Ok(Subject::Column(t, c));
        }
        self.check_columns(e)?;
        Ok(Subject::Expression(e.to_string()))
    }

    /// `count(coluna)`, `sum(expressão)`, `count(*)`: (agregação, o que
    /// agrega).
    fn aggregate(&self, e: &Expr) -> Result<Option<(Aggregate, Subject)>, ReadError> {
        let Expr::Function(f) = e else {
            return Ok(None);
        };
        let parts = object_name(&f.name);
        let Some([function]) = parts.as_deref() else {
            return Ok(None);
        };
        let aggregate = match function.as_str() {
            "count" => Aggregate::Count,
            "sum" => Aggregate::Sum,
            "avg" => Aggregate::Avg,
            "min" => Aggregate::Min,
            "max" => Aggregate::Max,
            _ => return Ok(None),
        };
        let FunctionArguments::List(list) = &f.args else {
            return Ok(None);
        };
        let (subject, arg) = match list.args.as_slice() {
            [FunctionArg::Unnamed(FunctionArgExpr::Wildcard)] if aggregate == Aggregate::Count => {
                (Subject::Expression("*".into()), "*".to_string())
            }
            [FunctionArg::Unnamed(FunctionArgExpr::Expr(arg))] => {
                (self.subject(arg)?, arg.to_string())
            }
            _ => return Ok(None),
        };
        // DISTINCT, FILTER, OVER e afins mudam o texto: esses viram
        // expressão.
        if f.to_string().to_lowercase() != format!("{function}({arg})").to_lowercase() {
            return Ok(None);
        }
        Ok(Some((aggregate, subject)))
    }

    /// Um item da lista do SELECT; devolve o índice da linha (`None` no
    /// `*`).
    fn item(&mut self, item: &SelectItem) -> Result<Option<usize>, ReadError> {
        let (expr, alias) = match item {
            SelectItem::Wildcard(opts) => {
                if self.query.rows.iter().any(|r| r.output) || opts.to_string().trim() != "" {
                    return unsupported("* together with other columns");
                }
                return Ok(None);
            }
            SelectItem::QualifiedWildcard(..) | SelectItem::ExprWithAliases { .. } => {
                return unsupported(format!("{item}"));
            }
            SelectItem::UnnamedExpr(e) => (e, String::new()),
            SelectItem::ExprWithAlias { expr, alias } => (expr, name(alias)),
        };
        let (subject, aggregate) = match self.aggregate(expr)? {
            Some((a, s)) => (s, a),
            None => (self.subject(expr)?, Aggregate::GroupBy),
        };
        self.query.rows.push(Row {
            alias,
            output: true,
            aggregate,
            ..subject.row()
        });
        Ok(Some(self.query.rows.len() - 1))
    }

    /// `a.x = b.y` vira JOIN; `true` se era isso. Com `right`, uma das duas
    /// colunas tem de ser da tabela do JOIN. No WHERE (`right` vazio), não
    /// entra num JOIN externo, onde mudaria o resultado: fica como filtro.
    fn join_condition(&mut self, e: &Expr, right: Option<usize>) -> Result<bool, ReadError> {
        let Expr::BinaryOp {
            left: l,
            op: BinaryOperator::Eq,
            right: r,
        } = e
        else {
            return Ok(false);
        };
        let (Some((lt, lc)), Some((rt, rc))) = (self.column(l)?, self.column(r)?) else {
            return Ok(false);
        };
        if lt == rt || right.is_some_and(|t| lt != t && rt != t) {
            return Ok(false);
        }
        let at = lt.max(rt);
        if right.is_none()
            && self
                .query
                .joins
                .iter()
                .any(|j| j.left.max(j.right) == at && j.kind() != JoinKind::Inner)
        {
            return Ok(false);
        }
        self.query.add_join(lt, &lc, rt, &rc);
        Ok(true)
    }

    /// Um WHERE ou HAVING: os termos ligados por AND vão para a coluna
    /// Filter; um termo `p OR q` espalha os outros por uma coluna de
    /// critérios para cada lado (`(c AND p) OR (c AND q)`).
    fn criteria(&mut self, e: &Expr, having: bool) -> Result<(), ReadError> {
        let mut common = Vec::new();
        let mut or: Option<Vec<&Expr>> = None;
        for c in conjuncts(e) {
            if !having && self.join_condition(c, None)? {
                continue;
            }
            let d = disjuncts(c);
            if d.len() > 1 {
                if or.is_some() {
                    return unsupported(format!("more than one group of OR in {e}"));
                }
                or = Some(d);
            } else {
                common.push(c);
            }
        }
        let groups: Vec<Vec<&Expr>> = match or {
            None => vec![common],
            Some(d) => d
                .into_iter()
                .map(|d| {
                    let mut g = common.clone();
                    g.extend(conjuncts(d));
                    g
                })
                .collect(),
        };
        for (k, group) in groups.iter().enumerate() {
            for c in group {
                if disjuncts(c).len() > 1 {
                    return unsupported(format!("the condition {c} (OR inside OR)"));
                }
                self.criterion(c, k, having)?;
            }
        }
        Ok(())
    }

    /// Um termo do WHERE ou do HAVING, na coluna de critérios `k`.
    fn criterion(&mut self, e: &Expr, k: usize, having: bool) -> Result<(), ReadError> {
        let place = if having { "HAVING" } else { "WHERE" };
        let left = match e {
            Expr::BinaryOp { left, .. } => left,
            Expr::Like { expr, .. }
            | Expr::ILike { expr, .. }
            | Expr::In {
                expr,
                target: InTarget::List(_),
                global: false,
                ..
            }
            | Expr::Between { expr, .. }
            | Expr::IsNull(expr)
            | Expr::IsNotNull(expr)
            | Expr::IsTrue(expr)
            | Expr::IsFalse(expr)
            | Expr::IsNotTrue(expr)
            | Expr::IsNotFalse(expr)
            | Expr::IsDistinctFrom(expr, _)
            | Expr::IsNotDistinctFrom(expr, _) => expr,
            _ => return unsupported(format!("the condition {e} in {place}")),
        };
        // O resto do termo, depois da coluna, é o critério da grade.
        let text = e.to_string();
        let filter = text
            .strip_prefix(&left.to_string())
            .map(str::trim)
            .unwrap_or_default();
        if filter.is_empty() || condition(filter) != filter {
            return unsupported(format!("the condition {e} in {place}"));
        }
        self.check_columns(e)?;
        let (subject, aggregate) = if having {
            match self.aggregate(left)? {
                Some((a, s)) => (s, a),
                None => match self.subject(left)? {
                    s @ Subject::Expression(_) => (s, Aggregate::Expression),
                    Subject::Column(..) => {
                        return unsupported(format!(
                            "the condition {e} in HAVING (move it to WHERE)"
                        ));
                    }
                },
            }
        } else {
            if self.check_columns(left)? == 0 {
                return unsupported(format!("the condition {e} in WHERE"));
            }
            (self.subject(left)?, Aggregate::GroupBy)
        };
        let free = self.query.rows.iter().position(|r| {
            subject.matches(r) && r.criterion(k).is_empty() && r.aggregate == aggregate
        });
        let row = match free {
            Some(i) => i,
            None => {
                self.query.rows.push(Row {
                    aggregate,
                    ..subject.row()
                });
                self.query.rows.len() - 1
            }
        };
        *self.query.rows[row].criterion_mut(k) = filter.to_string();
        Ok(())
    }

    fn group(&mut self, e: &Expr) -> Result<(), ReadError> {
        if self.aggregate(e)?.is_some() {
            return unsupported(format!("{e} in GROUP BY"));
        }
        let subject = self.subject(e)?;
        let known = self
            .query
            .rows
            .iter()
            .any(|r| subject.matches(r) && r.aggregate == Aggregate::GroupBy);
        let row = subject.row();
        self.grouped.push(key(&row));
        if !known {
            self.query.rows.push(row);
        }
        Ok(())
    }

    /// A linha que um item do ORDER BY ordena (criada se preciso).
    fn order_row(&mut self, e: &Expr, items: &[Option<usize>]) -> Result<usize, ReadError> {
        // ORDER BY 2: o segundo item do SELECT.
        if let Expr::Value(v) = e
            && let Value::Number(n, _) = &v.value
        {
            return match n
                .parse::<usize>()
                .ok()
                .and_then(|n| items.get(n.checked_sub(1)?))
            {
                Some(Some(row)) => Ok(*row),
                _ => unsupported(format!("ORDER BY {n}")),
            };
        }
        // Um apelido da lista do SELECT vale antes de uma coluna.
        if let Expr::Identifier(i) = e {
            let alias = name(i);
            if let Some(row) = self
                .query
                .rows
                .iter()
                .position(|r| r.output && r.alias == alias)
            {
                return Ok(row);
            }
        }
        let (subject, aggregate) = match self.aggregate(e)? {
            Some((a, s)) => (s, a),
            None => {
                let s = self.subject(e)?;
                if matches!(&s, Subject::Expression(_)) && self.check_columns(e)? == 0 {
                    return unsupported(format!("{e} in ORDER BY"));
                }
                (s, Aggregate::GroupBy)
            }
        };
        let same = |r: &Row| subject.matches(r) && r.aggregate == aggregate;
        let row = self
            .query
            .rows
            .iter()
            .position(|r| same(r) && r.output && r.sort == SortType::Unsorted)
            .or_else(|| {
                self.query
                    .rows
                    .iter()
                    .position(|r| same(r) && r.sort == SortType::Unsorted)
            });
        Ok(row.unwrap_or_else(|| {
            self.query.rows.push(Row {
                aggregate,
                ..subject.row()
            });
            self.query.rows.len() - 1
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_designer::Join;
    use crate::query_designer::tests::shop;

    fn err(sql: &str) -> ReadError {
        let (m, ..) = shop();
        read(sql, &m).expect_err(sql)
    }

    /// O SQL que o designer gera volta igual depois de lido.
    #[test]
    fn designer_sql_round_trips() {
        let (m, cliente, pedido, produto) = shop();
        let mut q = Query::default();
        q.add_table(&m, pedido);
        q.add_table(&m, cliente);
        q.add_table(&m, produto);
        q.toggle_column(1, "nome");
        q.toggle_column(0, "total");
        let back = read(&q.sql(&m), &m).unwrap();
        assert_eq!(back, q);

        let mut q = Query::default();
        q.add_table(&m, cliente);
        q.add_table(&m, pedido);
        q.toggle_column(0, "nome");
        q.toggle_column(1, "total");
        q.rows[1].alias = "Total Gasto".into();
        q.rows[1].sort = SortType::Descending;
        q.rows[1].sort_order = 1;
        q.rows[0].sort = SortType::Ascending;
        q.rows[0].sort_order = 2;
        q.rows[0].filter = "'Ana'".into();
        q.toggle_column(1, "id");
        q.rows[2].output = false;
        q.rows[2].filter = "> 10".into();
        let sql = q.sql(&m);
        let back = read(&sql, &m).unwrap();
        assert_eq!(back.sql(&m), sql);
        assert_eq!(back.rows[0].filter, "= 'Ana'");
        assert_eq!(back.rows[1].alias, "Total Gasto");

        q.group_by = true;
        q.rows[1].aggregate = Aggregate::Sum;
        q.rows[2].aggregate = Aggregate::Where;
        let sql = q.sql(&m);
        let back = read(&sql, &m).unwrap();
        assert_eq!(back.sql(&m), sql);
        assert!(back.group_by);
        assert_eq!(back.rows[2].aggregate, Aggregate::Where);
    }

    #[test]
    fn hand_written_select_with_aliases() {
        let (m, cliente, pedido, _) = shop();
        let q = read(
            "select c.nome as \"Cliente\", P.Total
               from public.cliente c
               join pedido p on p.cliente_id = c.id
              where c.nome like 'A%' and (p.total is not null) and p.total between 1 and 9
              order by 2 desc, c.id;",
            &m,
        )
        .unwrap();
        assert_eq!(
            q.tables,
            [
                QueryTable {
                    table: cliente,
                    alias: "c".into()
                },
                QueryTable {
                    table: pedido,
                    alias: "p".into()
                }
            ]
        );
        assert_eq!(
            q.joins,
            [Join {
                left: 1,
                right: 0,
                on: vec![("cliente_id".into(), "id".into())],
                ..Default::default()
            }]
        );
        let rows: Vec<_> = q
            .rows
            .iter()
            .map(|r| {
                (
                    r.table,
                    r.column.as_str(),
                    r.output,
                    r.filter.as_str(),
                    r.sort,
                    r.sort_order,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (0, "nome", true, "LIKE 'A%'", SortType::Unsorted, 0),
                (1, "total", true, "IS NOT NULL", SortType::Descending, 1),
                (1, "total", false, "BETWEEN 1 AND 9", SortType::Unsorted, 0),
                (0, "id", false, "", SortType::Ascending, 2),
            ]
        );
        assert_eq!(
            q.sql(&m),
            "SELECT c.nome AS \"Cliente\",\n       p.total\n\
             FROM public.cliente AS c\n    \
             INNER JOIN public.pedido AS p ON p.cliente_id = c.id\n\
             WHERE c.nome LIKE 'A%'\n  AND p.total IS NOT NULL\n  AND p.total BETWEEN 1 AND 9\n\
             ORDER BY p.total DESC, c.id;"
        );
    }

    #[test]
    fn joins_in_where_unqualified_columns_and_group_by() {
        let (m, ..) = shop();
        let q = read(
            "SELECT nome, count(pedido.id) AS n FROM cliente, pedido \
             WHERE pedido.cliente_id = cliente.id AND total > 0 \
             GROUP BY nome ORDER BY n DESC",
            &m,
        )
        .unwrap();
        assert_eq!(q.joins.len(), 1);
        assert!(q.group_by);
        assert_eq!(
            q.sql(&m),
            "SELECT cliente.nome,\n       count(pedido.id) AS n\n\
             FROM public.cliente\n    \
             INNER JOIN public.pedido ON pedido.cliente_id = cliente.id\n\
             WHERE pedido.total > 0\n\
             GROUP BY cliente.nome\n\
             ORDER BY count(pedido.id) DESC;"
        );
        // Sem colunas, `*`.
        let q = read("SELECT * FROM cliente WHERE id IN (1, 2)", &m).unwrap();
        assert_eq!(
            q.sql(&m),
            "SELECT *\nFROM public.cliente\nWHERE cliente.id IN (1, 2);"
        );
    }

    #[test]
    fn cte_subquery_and_union_are_refused() {
        assert_eq!(
            err("WITH x AS (SELECT 1) SELECT * FROM cliente"),
            ReadError::Cte
        );
        assert_eq!(
            err("SELECT id FROM cliente UNION SELECT id FROM pedido"),
            ReadError::SetOperation("UNION".into())
        );
        assert_eq!(
            err("SELECT id FROM cliente EXCEPT ALL SELECT id FROM pedido"),
            ReadError::SetOperation("EXCEPT".into())
        );
        for sql in [
            "SELECT * FROM (SELECT * FROM cliente) c",
            "SELECT * FROM cliente WHERE id IN (SELECT cliente_id FROM pedido)",
            "SELECT * FROM cliente c WHERE EXISTS (SELECT 1 FROM pedido)",
            "SELECT (SELECT max(total) FROM pedido) FROM cliente",
        ] {
            assert_eq!(err(sql), ReadError::Subquery, "{sql}");
        }
        assert!(
            err("WITH x AS (SELECT 1) SELECT * FROM cliente")
                .to_string()
                .contains("WITH")
        );
    }

    #[test]
    fn what_the_designer_cannot_show_is_refused() {
        for sql in [
            "SELECT * FROM cliente JOIN pedido USING (id)",
            "SELECT * FROM cliente JOIN pedido ON pedido.cliente_id > cliente.id",
            "SELECT * FROM cliente LEFT JOIN pedido ON pedido.total > 0",
            "SELECT * FROM cliente NATURAL JOIN pedido",
            "SELECT * FROM cliente LIMIT 10 OFFSET 5",
            "SELECT * FROM cliente FETCH FIRST 3 ROWS ONLY",
            "SELECT DISTINCT ON (nome) nome FROM cliente",
            "SELECT * FROM cliente WHERE (id = 1 OR id = 2) AND (nome = 'a' OR nome = 'b')",
            "SELECT nome FROM cliente GROUP BY nome HAVING nome = 'a'",
            "SELECT * FROM cliente WHERE 1 = id",
            "SELECT * FROM cliente WHERE lower(nome)",
            "SELECT * FROM cliente ORDER BY nome NULLS FIRST",
            "SELECT * FROM cliente ORDER BY 1 + 1",
            "SELECT c.* FROM cliente c",
            "SELECT count(*) FROM cliente GROUP BY count(*)",
        ] {
            assert!(
                matches!(err(sql), ReadError::Unsupported(_)),
                "{sql}: {:?}",
                err(sql)
            );
        }
        assert_eq!(err("SELECT 1; SELECT 2"), ReadError::NotSelect);
        assert_eq!(err("DELETE FROM cliente"), ReadError::NotSelect);
        assert!(matches!(err("SELEC nada"), ReadError::Syntax(_)));
        assert!(matches!(
            err("SELECT * FROM nada"),
            ReadError::UnknownTable(_)
        ));
        assert!(matches!(
            err("SELECT x FROM cliente"),
            ReadError::UnknownColumn(_)
        ));
        assert!(matches!(
            err("SELECT lower(x) FROM cliente"),
            ReadError::UnknownColumn(_)
        ));
        assert!(matches!(
            err("SELECT * FROM cliente WHERE nome > x"),
            ReadError::UnknownColumn(_)
        ));
        assert!(matches!(
            err("SELECT nome FROM cliente, produto"),
            ReadError::AmbiguousColumn(_)
        ));
    }

    /// Lê `sql` e devolve o SQL que o designer gera dele; o SQL gerado,
    /// lido de novo, tem de voltar igual.
    fn designed(sql: &str) -> String {
        let (m, ..) = shop();
        let q = read(sql, &m).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let out = q.sql(&m);
        let back = read(&out, &m).unwrap_or_else(|e| panic!("{out}: {e}"));
        assert_eq!(back.sql(&m), out, "round trip of {sql}");
        out
    }

    #[test]
    fn outer_joins() {
        assert_eq!(
            designed(
                "select c.nome, p.total from cliente c \
                 left join pedido p on p.cliente_id = c.id"
            ),
            "SELECT c.nome,\n       p.total\n\
             FROM public.cliente AS c\n    \
             LEFT JOIN public.pedido AS p ON p.cliente_id = c.id;"
        );
        assert!(
            designed(
                "SELECT * FROM pedido RIGHT OUTER JOIN cliente ON cliente.id = pedido.cliente_id"
            )
            .contains("RIGHT JOIN public.cliente ON cliente.id = pedido.cliente_id")
        );
        assert!(
            designed("SELECT * FROM cliente FULL JOIN pedido ON pedido.cliente_id = cliente.id")
                .contains("FULL JOIN public.pedido")
        );
        // Igualdade no WHERE com a tabela de um LEFT JOIN não vira JOIN
        // (mudaria o resultado): fica como filtro.
        let (m, ..) = shop();
        let q = read(
            "SELECT * FROM cliente LEFT JOIN pedido ON pedido.cliente_id = cliente.id \
             WHERE pedido.id = cliente.id",
            &m,
        )
        .unwrap();
        assert_eq!(q.joins.len(), 1);
        assert_eq!(q.joins[0].on.len(), 1);
        assert_eq!(q.joins[0].kind(), crate::query_designer::JoinKind::Left);
        assert!(q.sql(&m).contains("WHERE pedido.id = cliente.id"));
    }

    #[test]
    fn distinct_limit_and_regex() {
        assert_eq!(
            designed("SELECT DISTINCT nome FROM cliente WHERE nome ~ 'a' ORDER BY nome LIMIT 5"),
            "SELECT DISTINCT cliente.nome\nFROM public.cliente\n\
             WHERE cliente.nome ~ 'a'\nORDER BY cliente.nome\nLIMIT 5;"
        );
    }

    #[test]
    fn having_and_count_star() {
        assert_eq!(
            designed(
                "SELECT c.nome, count(*) AS pedidos, sum(p.total) FROM cliente c \
                 JOIN pedido p ON p.cliente_id = c.id WHERE p.total > 0 \
                 GROUP BY c.nome HAVING count(*) > 1 AND sum(p.total) >= 100 \
                 ORDER BY count(*) DESC"
            ),
            "SELECT c.nome,\n       count(*) AS pedidos,\n       sum(p.total)\n\
             FROM public.cliente AS c\n    \
             INNER JOIN public.pedido AS p ON p.cliente_id = c.id\n\
             WHERE p.total > 0\n\
             GROUP BY c.nome\n\
             HAVING count(*) > 1\n   AND sum(p.total) >= 100\n\
             ORDER BY count(*) DESC;"
        );
        assert_eq!(
            designed("SELECT count(*) FROM pedido"),
            "SELECT count(*)\nFROM public.pedido;"
        );
    }

    #[test]
    fn or_spreads_over_the_or_columns() {
        let (m, ..) = shop();
        let q = read(
            "SELECT nome FROM cliente WHERE nome = 'Ana' OR nome = 'Bia' OR id > 10",
            &m,
        )
        .unwrap();
        assert_eq!(q.or_columns, 2);
        assert_eq!(q.rows.len(), 2, "{:?}", q.rows);
        assert_eq!(q.rows[0].filter, "= 'Ana'");
        assert_eq!(q.rows[0].or, ["= 'Bia'"]);
        assert_eq!(q.rows[1].or, ["", "> 10"]);
        assert_eq!(
            designed("SELECT nome FROM cliente WHERE nome = 'Ana' OR nome = 'Bia' OR id > 10"),
            "SELECT cliente.nome\nFROM public.cliente\n\
             WHERE (cliente.nome = 'Ana')\n   OR (cliente.nome = 'Bia')\n   OR (cliente.id > 10);"
        );
        // Um termo comum vai para todas as colunas.
        assert_eq!(
            designed("SELECT * FROM cliente WHERE id > 0 AND (nome LIKE 'A%' OR nome IS NULL)"),
            "SELECT *\nFROM public.cliente\n\
             WHERE (cliente.id > 0 AND cliente.nome LIKE 'A%')\n   \
             OR (cliente.id > 0 AND cliente.nome IS NULL);"
        );
    }

    #[test]
    fn expressions_take_the_place_of_columns() {
        assert_eq!(
            designed(
                "SELECT lower(c.nome) AS n, p.total * 2 FROM cliente c, pedido p \
                 WHERE p.cliente_id = c.id AND lower(c.nome) LIKE 'a%' ORDER BY lower(c.nome)"
            ),
            "SELECT lower(c.nome) AS n,\n       p.total * 2\n\
             FROM public.cliente AS c\n    \
             INNER JOIN public.pedido AS p ON p.cliente_id = c.id\n\
             WHERE lower(c.nome) LIKE 'a%'\n\
             ORDER BY lower(c.nome);"
        );
        // Uma expressão que agrega fica fora do GROUP BY (Expression), e
        // count(DISTINCT ...) também é expressão.
        let (m, ..) = shop();
        let q = read(
            "SELECT cliente.nome, sum(total) / count(*) AS media, count(DISTINCT pedido.id) \
             FROM cliente JOIN pedido ON pedido.cliente_id = cliente.id GROUP BY cliente.nome",
            &m,
        )
        .unwrap();
        assert_eq!(q.rows[1].aggregate, Aggregate::Expression);
        assert_eq!(q.rows[2].aggregate, Aggregate::Expression);
        let sql = q.sql(&m);
        assert!(sql.contains("GROUP BY cliente.nome;"), "{sql}");
        assert!(sql.contains("sum(total) / count(*) AS media"), "{sql}");
        // Agrupar por expressão.
        assert!(
            designed("SELECT lower(nome), count(id) FROM cliente GROUP BY lower(nome)")
                .contains("GROUP BY lower(nome)")
        );
    }

    #[test]
    fn everything_the_designer_draws_round_trips() {
        let (m, cliente, pedido, produto) = shop();
        let mut q = Query::default();
        q.add_table(&m, cliente);
        q.add_table(&m, pedido);
        q.add_table(&m, produto);
        q.set_join_kind(0, crate::query_designer::JoinKind::Left);
        q.distinct = true;
        q.limit = Some(20);
        q.group_by = true;
        q.toggle_column(0, "nome");
        q.toggle_column(1, "total");
        q.rows[1].aggregate = Aggregate::Sum;
        q.rows[1].filter = "> 100".into();
        q.rows[1].sort = SortType::Descending;
        q.add_expression("count(*)");
        q.rows[2].aggregate = Aggregate::Expression;
        q.add_expression("upper(cliente.nome)");
        q.rows[3].output = false;
        q.rows[3].aggregate = Aggregate::Where;
        q.rows[3].filter = "LIKE 'A%'".into();
        *q.rows[3].criterion_mut(1) = "LIKE 'B%'".into();
        q.or_columns = 1;
        let sql = q.sql(&m);
        assert_eq!(
            sql,
            "SELECT DISTINCT cliente.nome,\n       sum(pedido.total),\n       count(*)\n\
             FROM public.cliente\n    \
             LEFT JOIN public.pedido ON cliente.id = pedido.cliente_id\n    \
             CROSS JOIN public.produto\n\
             WHERE (upper(cliente.nome) LIKE 'A%')\n   OR (upper(cliente.nome) LIKE 'B%')\n\
             GROUP BY cliente.nome\n\
             HAVING sum(pedido.total) > 100\n\
             ORDER BY sum(pedido.total) DESC\n\
             LIMIT 20;"
        );
        let back = read(&sql, &m).unwrap();
        assert_eq!(back.sql(&m), sql);
        // Tirar a tabela não leva as expressões.
        q.remove_table(2);
        assert_eq!(q.rows.iter().filter(|r| r.expression).count(), 2);
        // Uma expressão vazia não entra no SQL.
        q.add_expression("  ");
        assert!(!q.sql(&m).contains("  ,"));
    }
}
