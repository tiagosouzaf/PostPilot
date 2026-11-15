//! O plano lido do Showplan XML do SQL Server (`SET SHOWPLAN_XML ON` para o
//! estimado, `SET STATISTICS XML ON` para o real).
//!
//! Um documento traz os comandos de um lote; cada comando com plano
//! (`StmtSimple` com `QueryPlan`) vira um [`Plan`], com os `RelOp` como nós.

use std::collections::BTreeMap;

use roxmltree::{Document, Node as Xml};

use crate::plan::{Actual, Node, ParseError, Plan};

/// Nome da coluna em que o SQL Server devolve o plano em XML.
pub const SHOWPLAN_COLUMN: &str = "Microsoft SQL Server 2005 XML Showplan";

/// Lê um documento Showplan XML: um plano por comando que tem plano.
pub fn parse_showplan(xml: &str) -> Result<Vec<Plan>, ParseError> {
    let doc = Document::parse(xml).map_err(|e| ParseError(e.to_string()))?;
    if !doc.root_element().has_tag_name(name("ShowPlanXML")) {
        return Err(ParseError("missing <ShowPlanXML>".into()));
    }
    let mut plans = Vec::new();
    for stmt in doc.descendants().filter(|n| is(n, "StmtSimple")) {
        let Some(query) = child(&stmt, "QueryPlan") else {
            continue;
        };
        let Some(root) = child(&query, "RelOp") else {
            continue;
        };
        let mut nodes = Vec::new();
        push_node(&root, None, &mut nodes);
        for (k, v) in statement_properties(&stmt, &query) {
            nodes[0].properties.push((k, v));
        }
        nodes[0].properties.sort();
        let time = child(&query, "QueryTimeStats").and_then(|t| number(&t, "ElapsedTime"));
        plans.push(Plan {
            nodes,
            planning_ms: time.and(number(&query, "CompileTime")),
            execution_ms: time,
            statement: stmt
                .attribute("StatementText")
                .unwrap_or_default()
                // O servidor deixa o `;` do comando anterior no começo.
                .trim_matches(|c: char| c.is_whitespace() || c == ';')
                .to_string(),
        });
    }
    Ok(plans)
}

const NS: &str = "http://schemas.microsoft.com/sqlserver/2004/07/showplan";

fn name(local: &'static str) -> roxmltree::ExpandedName<'static, 'static> {
    (NS, local).into()
}

fn is(n: &Xml<'_, '_>, local: &'static str) -> bool {
    n.is_element() && n.tag_name().name() == local
}

fn child<'a, 'i>(n: &Xml<'a, 'i>, local: &'static str) -> Option<Xml<'a, 'i>> {
    n.children().find(|c| is(c, local))
}

fn number(n: &Xml<'_, '_>, attr: &str) -> Option<f64> {
    n.attribute(attr)?.parse().ok()
}

/// `[dbo]` sem os colchetes.
fn plain(s: &str) -> String {
    match s.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        Some(inner) => inner.replace("]]", "]"),
        None => s.to_string(),
    }
}

/// Descendentes de `n` com o nome `local`, sem entrar em outro `RelOp`.
fn within<'a, 'i>(n: &Xml<'a, 'i>, local: &'static str, out: &mut Vec<Xml<'a, 'i>>) {
    for c in n.children().filter(Xml::is_element) {
        if is(&c, local) {
            out.push(c);
            if local == "RelOp" {
                continue;
            }
        }
        if !is(&c, "RelOp") {
            within(&c, local, out);
        }
    }
}

fn push_node(op: &Xml<'_, '_>, parent: Option<usize>, nodes: &mut Vec<Node>) -> usize {
    let attr = |a: &str| op.attribute(a).map(str::to_string);
    let kind = attr("PhysicalOp").unwrap_or_else(|| "?".into());
    let logical = attr("LogicalOp").filter(|l| *l != kind);
    // O elemento do operador (`<IndexScan>`, `<NestedLoops>`...) é o filho
    // que não é um dos comuns a todos.
    const COMMON: &[&str] = &[
        "OutputList",
        "Warnings",
        "MemoryFractions",
        "RunTimeInformation",
        "RunTimePartitionSummary",
        "InternalInfo",
    ];
    let detail = op
        .children()
        .find(|c| c.is_element() && !COMMON.contains(&c.tag_name().name()));

    let mut props: BTreeMap<String, String> = BTreeMap::new();
    for a in op.attributes() {
        props.insert(label(a.name()), a.value().to_string());
    }
    let mut relation = None;
    let mut alias = None;
    let mut index = None;
    if let Some(detail) = &detail {
        for a in detail.attributes() {
            props.insert(label(a.name()), a.value().to_string());
        }
        let mut objects = Vec::new();
        within(detail, "Object", &mut objects);
        if let Some(o) = objects.first() {
            let table = o.attribute("Table").map(plain);
            relation = table.map(|t| match o.attribute("Schema") {
                Some(s) => format!("{}.{t}", plain(s)),
                None => t,
            });
            alias = o.attribute("Alias").map(plain);
            index = o.attribute("Index").map(plain);
            let full: Vec<&str> = ["Database", "Schema", "Table", "Index"]
                .iter()
                .filter_map(|a| o.attribute(*a))
                .collect();
            props.insert("Object".into(), full.join("."));
        }
        // Predicados: o texto que o SSMS mostra.
        for (tag, key) in [
            ("Predicate", "Predicate"),
            ("SeekPredicates", "Seek Predicates"),
            ("HashKeysProbe", "Hash Keys Probe"),
            ("OrderBy", "Order By"),
            ("GroupBy", "Group By"),
        ] {
            if let Some(p) = child(detail, tag) {
                let text = scalar_text(&p);
                if !text.is_empty() {
                    props.insert(key.into(), text);
                }
            }
        }
    }
    if let Some(out) = child(op, "OutputList") {
        let cols = columns(&out);
        if !cols.is_empty() {
            props.insert("Output List".into(), cols);
        }
    }
    if let Some(w) = child(op, "Warnings") {
        let text: Vec<String> = w
            .children()
            .filter(Xml::is_element)
            .map(|c| label(c.tag_name().name()))
            .collect();
        props.insert("Warnings".into(), text.join(", "));
    }

    // Números reais: somados entre as threads.
    let mut counters = Vec::new();
    if let Some(run) = child(op, "RunTimeInformation") {
        within(&run, "RunTimeCountersPerThread", &mut counters);
    }
    let actual = (!counters.is_empty()).then(|| {
        let sum = |a: &str| counters.iter().filter_map(|c| number(c, a)).sum::<f64>();
        let max = |a: &str| {
            counters
                .iter()
                .filter_map(|c| number(c, a))
                .fold(0.0, f64::max)
        };
        let rows = sum("ActualRows");
        let loops = sum("ActualExecutions");
        let elapsed = max("ActualElapsedms");
        props.insert("Actual Number of Rows".into(), rows.to_string());
        props.insert("Number of Executions".into(), loops.to_string());
        props.insert("Actual Elapsed (ms)".into(), elapsed.to_string());
        let reads = sum("ActualLogicalReads");
        if reads > 0.0 {
            props.insert("Actual Logical Reads".into(), reads.to_string());
        }
        let per = loops.max(1.0);
        Actual {
            rows: rows / per,
            loops,
            startup_ms: 0.0,
            total_ms: elapsed / per,
        }
    });

    let id = nodes.len();
    nodes.push(Node {
        kind,
        relation,
        alias,
        index,
        join_type: logical,
        strategy: None,
        parent_relationship: None,
        startup_cost: 0.0,
        total_cost: number(op, "EstimatedTotalSubtreeCost").unwrap_or(0.0),
        plan_rows: number(op, "EstimateRows").unwrap_or(0.0),
        plan_width: number(op, "AvgRowSize").unwrap_or(0.0),
        actual,
        properties: props.into_iter().collect(),
        parent,
        children: Vec::new(),
    });
    let mut children = Vec::new();
    within(op, "RelOp", &mut children);
    for c in children {
        let child = push_node(&c, Some(id), nodes);
        nodes[id].children.push(child);
    }
    id
}

/// `EstimateRows` como o SSMS escreve: `Estimate Rows`.
fn label(attr: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = attr.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let lower_before = i > 0 && chars[i - 1].is_lowercase();
        let acronym_end = i > 0
            && chars[i - 1].is_uppercase()
            && chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        if c.is_uppercase() && (lower_before || acronym_end) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Os `ScalarString` logo abaixo de `n` (o primeiro nível que tiver).
fn scalar_text(n: &Xml<'_, '_>) -> String {
    let mut found = Vec::new();
    fn walk(n: &Xml<'_, '_>, found: &mut Vec<String>) {
        for c in n.children().filter(Xml::is_element) {
            if let Some(s) = c.attribute("ScalarString") {
                found.push(s.to_string());
            } else {
                walk(&c, found);
            }
        }
    }
    walk(n, &mut found);
    if found.is_empty() {
        columns(n)
    } else {
        found.join(" AND ")
    }
}

/// As colunas citadas em `n`: `[c].nome, Expr1002`.
fn columns(n: &Xml<'_, '_>) -> String {
    n.descendants()
        .filter(|c| is(c, "ColumnReference"))
        .filter_map(|c| {
            let col = c.attribute("Column")?;
            Some(match c.attribute("Alias").or(c.attribute("Table")) {
                Some(t) => format!("{t}.{col}"),
                None => col.to_string(),
            })
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Propriedades do comando, mostradas no nó raiz: nível de otimização,
/// paralelismo, memória e índices que faltam.
fn statement_properties(stmt: &Xml<'_, '_>, query: &Xml<'_, '_>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for a in [
        "StatementOptmLevel",
        "StatementOptmEarlyAbortReason",
        "CardinalityEstimationModelVersion",
    ] {
        if let Some(v) = stmt.attribute(a) {
            out.push((label(a), v.to_string()));
        }
    }
    for a in [
        "DegreeOfParallelism",
        "MemoryGrant",
        "CachedPlanSize",
        "CompileTime",
    ] {
        if let Some(v) = query.attribute(a) {
            out.push((label(a), v.to_string()));
        }
    }
    let mut groups = Vec::new();
    within(query, "MissingIndexGroup", &mut groups);
    for (i, g) in groups.iter().enumerate() {
        let Some(ix) = g.descendants().find(|n| is(n, "MissingIndex")) else {
            continue;
        };
        let cols = |usage: &[&str]| {
            ix.children()
                .filter(|c| {
                    is(c, "ColumnGroup") && usage.contains(&c.attribute("Usage").unwrap_or(""))
                })
                .flat_map(|c| c.children().filter(|c| is(c, "Column")))
                .filter_map(|c| c.attribute("Name"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let keys = cols(&["EQUALITY", "INEQUALITY"]);
        let include = cols(&["INCLUDE"]);
        let mut sql = format!(
            "CREATE NONCLUSTERED INDEX [<name>] ON {}.{} ({keys})",
            ix.attribute("Schema").unwrap_or_default(),
            ix.attribute("Table").unwrap_or_default(),
        );
        if !include.is_empty() {
            sql.push_str(&format!(" INCLUDE ({include})"));
        }
        out.push((
            format!(
                "Missing Index {} (Impact {}%)",
                i + 1,
                g.attribute("Impact").unwrap_or("?")
            ),
            sql,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOIN: &str = r#"<ShowPlanXML xmlns="http://schemas.microsoft.com/sqlserver/2004/07/showplan" Version="1.564">
<BatchSequence><Batch><Statements>
<StmtSimple StatementText="declare @a int" StatementType="DECLARE"/>
<StmtSimple StatementText="select 1 from a join b on b.a_id = a.id" StatementSubTreeCost="0.01" StatementOptmLevel="FULL">
<QueryPlan DegreeOfParallelism="1" CompileTime="8">
<MissingIndexes><MissingIndexGroup Impact="91.5"><MissingIndex Database="[d]" Schema="[dbo]" Table="[b]">
  <ColumnGroup Usage="EQUALITY"><Column Name="[a_id]" ColumnId="2"/></ColumnGroup>
  <ColumnGroup Usage="INCLUDE"><Column Name="[x]" ColumnId="3"/></ColumnGroup>
</MissingIndex></MissingIndexGroup></MissingIndexes>
<QueryTimeStats ElapsedTime="12" CpuTime="3"/>
<RelOp NodeId="0" PhysicalOp="Nested Loops" LogicalOp="Inner Join" EstimateRows="100" AvgRowSize="11" EstimatedTotalSubtreeCost="0.01">
 <OutputList/>
 <RunTimeInformation>
  <RunTimeCountersPerThread Thread="1" ActualRows="60" ActualExecutions="1" ActualElapsedms="4"/>
  <RunTimeCountersPerThread Thread="2" ActualRows="40" ActualExecutions="1" ActualElapsedms="6"/>
 </RunTimeInformation>
 <NestedLoops Optimized="0">
  <RelOp NodeId="1" PhysicalOp="Clustered Index Scan" LogicalOp="Clustered Index Scan" EstimateRows="10" AvgRowSize="4" EstimatedTotalSubtreeCost="0.004">
   <OutputList><ColumnReference Table="[a]" Column="id"/></OutputList>
   <IndexScan Ordered="0"><Object Database="[d]" Schema="[dbo]" Table="[a]" Index="[PK_a]" Alias="[x]"/></IndexScan>
  </RelOp>
  <RelOp NodeId="2" PhysicalOp="Index Seek" LogicalOp="Index Seek" EstimateRows="10" AvgRowSize="4" EstimatedTotalSubtreeCost="0.003">
   <OutputList/>
   <Warnings><NoJoinPredicate/></Warnings>
   <IndexScan Ordered="1">
     <Object Schema="[dbo]" Table="[b]" Index="[ix_b]"/>
     <SeekPredicates><SeekPredicateNew><SeekKeys><Prefix ScanType="EQ">
       <RangeColumns><ColumnReference Table="[b]" Column="a_id"/></RangeColumns>
       <RangeExpressions><ScalarOperator ScalarString="[d].[dbo].[a].[id]"><Identifier/></ScalarOperator></RangeExpressions>
     </Prefix></SeekKeys></SeekPredicateNew></SeekPredicates>
     <Predicate><ScalarOperator ScalarString="[b].[x]&gt;(1)"><Compare/></ScalarOperator></Predicate>
   </IndexScan>
  </RelOp>
 </NestedLoops>
</RelOp>
</QueryPlan></StmtSimple>
</Statements></Batch></BatchSequence></ShowPlanXML>"#;

    fn prop<'a>(n: &'a Node, key: &str) -> Option<&'a str> {
        n.properties
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn statements_with_a_plan_become_plans() {
        let plans = parse_showplan(JOIN).unwrap();
        assert_eq!(plans.len(), 1);
        let p = &plans[0];
        assert_eq!(p.statement, "select 1 from a join b on b.a_id = a.id");
        let kinds: Vec<&str> = p.nodes.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["Nested Loops", "Clustered Index Scan", "Index Seek"]
        );
        assert_eq!(p.nodes[0].children, [1, 2]);
        assert_eq!(p.nodes[2].parent, Some(0));
        assert_eq!((p.planning_ms, p.execution_ms), (Some(8.0), Some(12.0)));
    }

    #[test]
    fn titles_objects_and_costs_read_like_ssms() {
        let p = &parse_showplan(JOIN).unwrap()[0];
        assert_eq!(p.nodes[0].title(), "Nested Loops (Inner Join)");
        assert_eq!(p.nodes[1].title(), "Clustered Index Scan");
        assert_eq!(p.nodes[1].subtitle().as_deref(), Some("PK_a on dbo.a x"));
        assert_eq!(p.nodes[2].subtitle().as_deref(), Some("ix_b on dbo.b"));
        assert_eq!(p.total_cost(), 0.01);
        let shares: Vec<i64> = (0..3)
            .map(|i| (p.cost_share(i) * 100.0).round() as i64)
            .collect();
        assert_eq!(shares, [30, 40, 30]);
    }

    #[test]
    fn properties_predicates_warnings_and_missing_indexes() {
        let p = &parse_showplan(JOIN).unwrap()[0];
        let seek = &p.nodes[2];
        assert_eq!(prop(seek, "Predicate"), Some("[b].[x]>(1)"));
        assert_eq!(prop(seek, "Seek Predicates"), Some("[d].[dbo].[a].[id]"));
        assert_eq!(prop(seek, "Warnings"), Some("No Join Predicate"));
        assert_eq!(prop(seek, "Estimate Rows"), Some("10"));
        assert_eq!(prop(seek, "Object"), Some("[dbo].[b].[ix_b]"));
        assert_eq!(prop(&p.nodes[1], "Output List"), Some("[a].id"));
        let root = &p.nodes[0];
        assert_eq!(prop(root, "Statement Optm Level"), Some("FULL"));
        assert_eq!(
            prop(root, "Missing Index 1 (Impact 91.5%)"),
            Some("CREATE NONCLUSTERED INDEX [<name>] ON [dbo].[b] ([a_id]) INCLUDE ([x])")
        );
    }

    #[test]
    fn actual_numbers_add_up_across_threads() {
        let p = &parse_showplan(JOIN).unwrap()[0];
        let a = p.nodes[0].actual.unwrap();
        assert_eq!((a.rows, a.loops, a.total_ms), (50.0, 2.0, 3.0));
        assert_eq!(p.nodes[0].actual_total_rows(), Some(100.0));
        assert!(p.nodes[1].actual.is_none());
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(parse_showplan("nada").is_err());
        assert!(parse_showplan("<x/>").is_err());
        assert_eq!(
            parse_showplan(
                r#"<ShowPlanXML xmlns="http://schemas.microsoft.com/sqlserver/2004/07/showplan"/>"#
            )
            .unwrap(),
            []
        );
    }
}
