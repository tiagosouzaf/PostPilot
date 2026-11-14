//! O caminho inverso do Query Designer (marco 5.5): lê o SELECT
//! selecionado no editor com o `sqlparser` e monta a [`Query`] a partir
//! dele (tabelas, JOINs, colunas, WHERE, GROUP BY e ORDER BY).
//!
//! O que o designer não sabe desenhar vira um [`ReadError`] em vez de ser
//! carregado pela metade: CTE, subconsulta e UNION têm aviso próprio; LEFT
//! JOIN, DISTINCT, LIMIT, HAVING, OR e expressões que não são colunas, o
//! genérico [`ReadError::Unsupported`]. Assim o OK nunca troca a seleção
//! por uma consulta que faz outra coisa.

use std::fmt;
use std::ops::ControlFlow;

use pgstudio_core::metadata::Model;
use pgstudio_core::schema::Id;
use sqlparser::ast::{
    BinaryOperator, Distinct, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, GroupByExpr,
    Ident, InTarget, JoinConstraint, JoinOperator, ObjectName, OrderByKind, OrderBySort, Select,
    SelectItem, SetExpr, Statement, TableFactor, TableWithJoins, Value, visit_expressions,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

use super::{Aggregate, Query, QueryTable, Row, SortType, condition};

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
    if query.limit_clause.is_some() || query.fetch.is_some() {
        return unsupported("LIMIT, OFFSET or FETCH");
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
        for c in conjuncts(selection) {
            r.filter(c)?;
        }
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
    if query.rows.iter().any(|r| r.aggregate != Aggregate::GroupBy) {
        query.group_by = true;
    }
    // Com Group By, as colunas que só filtram ficam fora do GROUP BY.
    if query.group_by {
        for row in &mut query.rows {
            if !row.output
                && row.sort == SortType::Unsorted
                && !row.filter.is_empty()
                && !grouped.contains(&(row.table, row.column.clone()))
            {
                row.aggregate = Aggregate::Where;
            }
        }
    }
    Ok(query)
}

/// Partes do SELECT que o designer não tem como mostrar.
fn check_select(s: &Select) -> Result<(), ReadError> {
    match &s.distinct {
        None => {}
        Some(Distinct::On(_)) => return unsupported("DISTINCT ON"),
        Some(_) => return unsupported("DISTINCT"),
    }
    if s.top.is_some() {
        return unsupported("TOP");
    }
    if s.into.is_some() {
        return unsupported("SELECT INTO");
    }
    if s.having.is_some() {
        return unsupported("HAVING");
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

/// Quantas colunas a expressão cita.
fn column_count(e: &Expr) -> usize {
    let mut n = 0;
    let _ = visit_expressions(e, |e| {
        if matches!(e, Expr::Identifier(_) | Expr::CompoundIdentifier(_)) {
            n += 1;
        }
        ControlFlow::<()>::Continue(())
    });
    n
}

struct Reader<'a> {
    model: &'a Model,
    query: Query,
    /// Como cada tabela é citada nas colunas: (esquema, nome) se veio sem
    /// apelido, para aceitar `esquema.tabela.coluna`.
    names: Vec<Option<(String, String)>>,
    /// Colunas citadas no GROUP BY.
    grouped: Vec<(usize, String)>,
}

impl Reader<'_> {
    fn from(&mut self, from: &TableWithJoins) -> Result<(), ReadError> {
        self.table(&from.relation)?;
        for join in &from.joins {
            let right = self.table(&join.relation)?;
            match &join.join_operator {
                JoinOperator::Join(c) | JoinOperator::Inner(c) => match c {
                    JoinConstraint::On(on) => {
                        for e in conjuncts(on) {
                            if !self.join_condition(e, Some(right))? {
                                return unsupported(format!("the join condition {e}"));
                            }
                        }
                    }
                    JoinConstraint::None => {}
                    JoinConstraint::Using(_) | JoinConstraint::UsingAlias(..) => {
                        return unsupported("JOIN ... USING");
                    }
                    JoinConstraint::Natural => return unsupported("NATURAL JOIN"),
                },
                JoinOperator::CrossJoin(JoinConstraint::None) => {}
                JoinOperator::Left(_) | JoinOperator::LeftOuter(_) => {
                    return unsupported("LEFT JOIN (only INNER JOIN)");
                }
                JoinOperator::Right(_) | JoinOperator::RightOuter(_) => {
                    return unsupported("RIGHT JOIN (only INNER JOIN)");
                }
                JoinOperator::FullOuter(_) => return unsupported("FULL JOIN (only INNER JOIN)"),
                _ => return unsupported("this kind of JOIN"),
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

    /// `count(coluna)` e companhia: (agregação, tabela, coluna).
    fn aggregate(&self, e: &Expr) -> Result<Option<(Aggregate, usize, String)>, ReadError> {
        let Expr::Function(f) = e else {
            return Ok(None);
        };
        let aggregate = match object_name(&f.name).as_deref() {
            Some([n]) => match n.as_str() {
                "count" => Aggregate::Count,
                "sum" => Aggregate::Sum,
                "avg" => Aggregate::Avg,
                "min" => Aggregate::Min,
                "max" => Aggregate::Max,
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        let FunctionArguments::List(list) = &f.args else {
            return Ok(None);
        };
        let [FunctionArg::Unnamed(FunctionArgExpr::Expr(arg))] = list.args.as_slice() else {
            if list
                .args
                .iter()
                .any(|a| matches!(a, FunctionArg::Unnamed(FunctionArgExpr::Wildcard)))
            {
                return unsupported("count(*) (pick a column to count)");
            }
            return Ok(None);
        };
        let Some((table, column)) = self.column(arg)? else {
            return Ok(None);
        };
        // DISTINCT, FILTER, OVER e afins mudam o texto: só o simples passa.
        let plain = format!("{}({arg})", object_name(&f.name).unwrap_or_default()[0]);
        if f.to_string().to_lowercase() != plain.to_lowercase() {
            return unsupported(format!("{e}"));
        }
        Ok(Some((aggregate, table, column)))
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
        let (table, column, aggregate) = if let Some((t, c)) = self.column(expr)? {
            (t, c, Aggregate::GroupBy)
        } else if let Some((a, t, c)) = self.aggregate(expr)? {
            (t, c, a)
        } else {
            return unsupported(format!(
                "the expression {expr} (only columns and aggregates)"
            ));
        };
        self.query.rows.push(Row {
            table,
            column,
            alias,
            output: true,
            aggregate,
            ..Default::default()
        });
        Ok(Some(self.query.rows.len() - 1))
    }

    /// `a.x = b.y` vira JOIN; `true` se era isso. Com `right`, uma das duas
    /// colunas tem de ser da tabela do JOIN.
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
        self.query.add_join(lt, &lc, rt, &rc);
        Ok(true)
    }

    /// Um termo do WHERE: JOIN entre colunas ou filtro de uma coluna.
    fn filter(&mut self, e: &Expr) -> Result<(), ReadError> {
        if self.join_condition(e, None)? {
            return Ok(());
        }
        let subject = match e {
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
            _ => return unsupported(format!("the condition {e} in WHERE")),
        };
        let Some((table, column)) = self.column(subject)? else {
            return unsupported(format!("the condition {e} in WHERE"));
        };
        // O resto do termo, depois da coluna, é o filtro da grade.
        let text = e.to_string();
        let filter = text
            .strip_prefix(&subject.to_string())
            .map(str::trim)
            .unwrap_or_default();
        if filter.is_empty() || column_count(e) != 1 || condition(filter) != filter {
            return unsupported(format!("the condition {e} in WHERE"));
        }
        let free = self.query.rows.iter().position(|r| {
            r.table == table
                && r.column == column
                && r.filter.is_empty()
                && r.aggregate == Aggregate::GroupBy
        });
        match free {
            Some(i) => self.query.rows[i].filter = filter.to_string(),
            None => self.query.rows.push(Row {
                table,
                column,
                filter: filter.to_string(),
                ..Default::default()
            }),
        }
        Ok(())
    }

    fn group(&mut self, e: &Expr) -> Result<(), ReadError> {
        let Some((table, column)) = self.column(e)? else {
            return unsupported(format!("{e} in GROUP BY (only columns)"));
        };
        let known =
            self.query.rows.iter().any(|r| {
                r.table == table && r.column == column && r.aggregate == Aggregate::GroupBy
            });
        self.grouped.push((table, column.clone()));
        if !known {
            self.query.rows.push(Row {
                table,
                column,
                ..Default::default()
            });
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
        let (table, column, aggregate) = if let Some((t, c)) = self.column(e)? {
            (t, c, Aggregate::GroupBy)
        } else if let Some((a, t, c)) = self.aggregate(e)? {
            (t, c, a)
        } else {
            return unsupported(format!("{e} in ORDER BY (only columns and aggregates)"));
        };
        let same = |r: &Row| r.table == table && r.column == column && r.aggregate == aggregate;
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
                table,
                column,
                aggregate,
                ..Default::default()
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
                on: vec![("cliente_id".into(), "id".into())]
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
            "SELECT * FROM cliente LEFT JOIN pedido ON pedido.cliente_id = cliente.id",
            "SELECT * FROM cliente JOIN pedido USING (id)",
            "SELECT * FROM cliente JOIN pedido ON pedido.cliente_id > cliente.id",
            "SELECT * FROM cliente LIMIT 10",
            "SELECT DISTINCT nome FROM cliente",
            "SELECT * FROM cliente WHERE id = 1 OR id = 2",
            "SELECT id + 1 FROM cliente",
            "SELECT lower(nome) FROM cliente",
            "SELECT count(*) FROM cliente",
            "SELECT count(DISTINCT nome) FROM cliente",
            "SELECT nome FROM cliente GROUP BY nome HAVING count(id) > 1",
            "SELECT * FROM cliente WHERE nome ~ 'a'",
            "SELECT * FROM cliente WHERE 1 = id",
            "SELECT * FROM cliente ORDER BY nome NULLS FIRST",
            "SELECT c.* FROM cliente c",
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
            err("SELECT nome FROM cliente, produto"),
            ReadError::AmbiguousColumn(_)
        ));
    }
}
