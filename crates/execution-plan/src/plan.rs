//! O plano lido do `EXPLAIN (FORMAT JSON)`.
//!
//! O PostgreSQL devolve um array com um objeto por comando, cada um com o
//! nó raiz em `"Plan"` e os filhos em `"Plans"`. Os nós ficam numa lista
//! plana (o índice 0 é a raiz), o que deixa o desenho e as buscas simples
//! mesmo com milhares de nós.

use serde_json::Value;

/// Um nó do plano (uma operação, como `Seq Scan` ou `Hash Join`).
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// `Node Type`: `Seq Scan`, `Hash Join`, `Aggregate`…
    pub kind: String,
    /// Tabela lida, com schema quando o plano diz (`VERBOSE`).
    pub relation: Option<String>,
    pub alias: Option<String>,
    pub index: Option<String>,
    /// `Inner`, `Left`… nos joins.
    pub join_type: Option<String>,
    /// `Plain`, `Sorted`, `Hashed`… nos agregados.
    pub strategy: Option<String>,
    /// `Outer`, `Inner`, `InitPlan`, `SubPlan`… em relação ao pai.
    pub parent_relationship: Option<String>,
    pub startup_cost: f64,
    pub total_cost: f64,
    /// Linhas estimadas por execução do nó.
    pub plan_rows: f64,
    pub plan_width: f64,
    /// Números reais, com `ANALYZE`.
    pub actual: Option<Actual>,
    /// Todas as propriedades do nó, em ordem alfabética (como a janela
    /// Properties do SSMS), já como texto.
    pub properties: Vec<(String, String)>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

/// O que o `EXPLAIN ANALYZE` mediu num nó.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Actual {
    /// Linhas por execução (média), como o PostgreSQL mostra.
    pub rows: f64,
    pub loops: f64,
    pub startup_ms: f64,
    /// Tempo por execução (média).
    pub total_ms: f64,
}

impl Node {
    /// Nome da operação como o SSMS mostra embaixo do ícone: o tipo, com o
    /// tipo de join ou a estratégia quando ajudam (`Hash Join (Left)`).
    pub fn title(&self) -> String {
        match (&self.join_type, &self.strategy) {
            (Some(j), _) if j != "Inner" => format!("{} ({j})", self.kind),
            (_, Some(s)) if self.kind == "Aggregate" && s != "Plain" => {
                format!("{s} Aggregate")
            }
            _ => self.kind.clone(),
        }
    }

    /// A segunda linha: tabela e apelido, ou o índice.
    pub fn subtitle(&self) -> Option<String> {
        let relation = self.relation.as_ref().map(|r| match &self.alias {
            Some(a) if a != r => format!("{r} {a}"),
            _ => r.clone(),
        });
        match (&self.index, relation) {
            (Some(i), Some(r)) => Some(format!("{i} on {r}")),
            (Some(i), None) => Some(i.clone()),
            (None, r) => r,
        }
    }

    /// Linhas reais no total (por execução vezes execuções).
    pub fn actual_total_rows(&self) -> Option<f64> {
        self.actual.map(|a| a.rows * a.loops)
    }
}

/// O plano de um comando.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Os nós; o 0 é a raiz.
    pub nodes: Vec<Node>,
    pub planning_ms: Option<f64>,
    pub execution_ms: Option<f64>,
    /// O comando que gerou o plano, para o cabeçalho.
    pub statement: String,
}

/// O texto não é um plano em JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "could not read the execution plan: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// Lê a saída do `EXPLAIN (FORMAT JSON)` de um comando (um array com um
/// objeto por comando; o PostgreSQL sempre devolve um).
pub fn parse(json: &str, statement: &str) -> Result<Vec<Plan>, ParseError> {
    let value: Value = serde_json::from_str(json).map_err(|e| ParseError(e.to_string()))?;
    let items = match value {
        Value::Array(items) => items,
        other => vec![other],
    };
    items
        .iter()
        .map(|item| {
            let root = item
                .get("Plan")
                .ok_or_else(|| ParseError("missing \"Plan\"".into()))?;
            let mut nodes = Vec::new();
            push_node(root, None, &mut nodes)?;
            Ok(Plan {
                nodes,
                planning_ms: item.get("Planning Time").and_then(Value::as_f64),
                execution_ms: item.get("Execution Time").and_then(Value::as_f64),
                statement: statement.trim().to_string(),
            })
        })
        .collect()
}

fn push_node(v: &Value, parent: Option<usize>, nodes: &mut Vec<Node>) -> Result<usize, ParseError> {
    let obj = v
        .as_object()
        .ok_or_else(|| ParseError("a plan node is not an object".into()))?;
    let text = |key: &str| obj.get(key).and_then(Value::as_str).map(str::to_string);
    let number = |key: &str| obj.get(key).and_then(Value::as_f64);
    let kind = text("Node Type").ok_or_else(|| ParseError("missing \"Node Type\"".into()))?;
    let relation = text("Relation Name").map(|r| match text("Schema") {
        Some(s) => format!("{s}.{r}"),
        None => r,
    });
    let actual = number("Actual Loops").map(|loops| Actual {
        rows: number("Actual Rows").unwrap_or(0.0),
        loops,
        startup_ms: number("Actual Startup Time").unwrap_or(0.0),
        total_ms: number("Actual Total Time").unwrap_or(0.0),
    });
    let properties = obj
        .iter()
        .filter(|(k, _)| k.as_str() != "Plans")
        .map(|(k, v)| (k.clone(), display(v)))
        .collect();
    let id = nodes.len();
    nodes.push(Node {
        kind,
        relation,
        alias: text("Alias"),
        index: text("Index Name"),
        join_type: text("Join Type"),
        strategy: text("Strategy"),
        parent_relationship: text("Parent Relationship"),
        startup_cost: number("Startup Cost").unwrap_or(0.0),
        total_cost: number("Total Cost").unwrap_or(0.0),
        plan_rows: number("Plan Rows").unwrap_or(0.0),
        plan_width: number("Plan Width").unwrap_or(0.0),
        actual,
        properties,
        parent,
        children: Vec::new(),
    });
    if let Some(Value::Array(children)) = obj.get("Plans") {
        for c in children {
            let child = push_node(c, Some(id), nodes)?;
            nodes[id].children.push(child);
        }
    }
    Ok(id)
}

/// Um valor do JSON como texto para a lista de propriedades.
fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => String::new(),
        Value::Array(items) => items.iter().map(display).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

impl Plan {
    pub fn root(&self) -> &Node {
        &self.nodes[0]
    }

    /// Custo total estimado do comando (o da raiz).
    pub fn total_cost(&self) -> f64 {
        self.root().total_cost
    }

    /// Custo do próprio nó, sem o dos filhos. Um `Limit` pode custar menos
    /// que o filho (para antes), então o mínimo é zero.
    pub fn own_cost(&self, id: usize) -> f64 {
        let n = &self.nodes[id];
        let children: f64 = n.children.iter().map(|&c| self.nodes[c].total_cost).sum();
        (n.total_cost - children).max(0.0)
    }

    /// Fração do custo do comando que cabe ao nó (o "Cost: N %" do SSMS).
    /// As frações de todos os nós somam 1.
    pub fn cost_share(&self, id: usize) -> f64 {
        let sum: f64 = (0..self.nodes.len()).map(|i| self.own_cost(i)).sum();
        if sum > 0.0 {
            self.own_cost(id) / sum
        } else if id == 0 {
            1.0
        } else {
            0.0
        }
    }

    /// Profundidade de cada nó (a raiz é 0).
    pub fn depths(&self) -> Vec<usize> {
        let mut depth = vec![0; self.nodes.len()];
        // Os pais vêm antes dos filhos na lista.
        for i in 1..self.nodes.len() {
            if let Some(p) = self.nodes[i].parent {
                depth[i] = depth[p] + 1;
            }
        }
        depth
    }
}

/// Custo relativo de cada plano no lote ("Query cost (relative to the
/// batch)" do SSMS), somando 1.
pub fn batch_shares(plans: &[Plan]) -> Vec<f64> {
    let total: f64 = plans.iter().map(Plan::total_cost).sum();
    plans
        .iter()
        .map(|p| {
            if total > 0.0 {
                p.total_cost() / total
            } else {
                1.0 / plans.len() as f64
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOIN: &str = r#"[{"Plan": {
        "Node Type": "Hash Join", "Join Type": "Left", "Startup Cost": 1.0, "Total Cost": 10.0,
        "Plan Rows": 100, "Plan Width": 8, "Hash Cond": "(a.id = b.a_id)",
        "Plans": [
          {"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Relation Name": "a",
           "Alias": "a", "Startup Cost": 0.0, "Total Cost": 4.0, "Plan Rows": 100, "Plan Width": 4},
          {"Node Type": "Hash", "Parent Relationship": "Inner", "Startup Cost": 2.0,
           "Total Cost": 2.0, "Plan Rows": 50, "Plan Width": 4,
           "Plans": [{"Node Type": "Index Scan", "Index Name": "b_pkey", "Relation Name": "b",
             "Alias": "x", "Startup Cost": 0.0, "Total Cost": 2.0, "Plan Rows": 50,
             "Plan Width": 4, "Sort Key": ["a", "b"], "Parallel Aware": false}]}
        ]}}]"#;

    #[test]
    fn nodes_are_flat_with_parents_and_children() {
        let plans = parse(JOIN, "SELECT 1").unwrap();
        assert_eq!(plans.len(), 1);
        let p = &plans[0];
        let kinds: Vec<&str> = p.nodes.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(kinds, ["Hash Join", "Seq Scan", "Hash", "Index Scan"]);
        assert_eq!(p.nodes[0].children, [1, 2]);
        assert_eq!(p.nodes[3].parent, Some(2));
        assert_eq!(p.depths(), [0, 1, 1, 2]);
        assert_eq!(p.statement, "SELECT 1");
    }

    #[test]
    fn titles_and_subtitles_read_like_ssms() {
        let p = &parse(JOIN, "").unwrap()[0];
        assert_eq!(p.nodes[0].title(), "Hash Join (Left)");
        assert_eq!(p.nodes[1].subtitle().as_deref(), Some("a"));
        assert_eq!(p.nodes[3].subtitle().as_deref(), Some("b_pkey on b x"));
        let index = &p.nodes[3].properties;
        assert!(index.contains(&("Sort Key".into(), "a, b".into())));
        assert!(index.contains(&("Parallel Aware".into(), "False".into())));
        assert!(!index.iter().any(|(k, _)| k == "Plans"));
    }

    #[test]
    fn own_costs_add_up_to_the_whole() {
        let p = &parse(JOIN, "").unwrap()[0];
        // Hash Join: 10 - (4 + 2) = 4; Seq Scan 4; Hash 0; Index Scan 2.
        assert_eq!(p.own_cost(0), 4.0);
        assert_eq!(p.own_cost(2), 0.0);
        let shares: Vec<f64> = (0..4).map(|i| p.cost_share(i)).collect();
        assert_eq!(shares, [0.4, 0.4, 0.0, 0.2]);
    }

    #[test]
    fn limit_cheaper_than_its_child_costs_nothing() {
        let json = r#"[{"Plan": {"Node Type": "Limit", "Total Cost": 1.0,
            "Plans": [{"Node Type": "Seq Scan", "Total Cost": 50.0}]}}]"#;
        let p = &parse(json, "").unwrap()[0];
        assert_eq!(p.own_cost(0), 0.0);
        assert_eq!(p.cost_share(1), 1.0);
    }

    #[test]
    fn analyze_numbers_are_read() {
        let json = r#"[{"Plan": {"Node Type": "Seq Scan", "Total Cost": 1.0, "Plan Rows": 10,
            "Actual Startup Time": 0.1, "Actual Total Time": 0.5, "Actual Rows": 7,
            "Actual Loops": 3}, "Planning Time": 0.2, "Execution Time": 1.5}]"#;
        let p = &parse(json, "").unwrap()[0];
        let a = p.root().actual.unwrap();
        assert_eq!((a.rows, a.loops, a.total_ms), (7.0, 3.0, 0.5));
        assert_eq!(p.root().actual_total_rows(), Some(21.0));
        assert_eq!((p.planning_ms, p.execution_ms), (Some(0.2), Some(1.5)));
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(parse("not json", "").is_err());
        assert!(parse(r#"[{"Plan": {"Total Cost": 1}}]"#, "").is_err());
        assert!(parse(r#"[{"Nope": 1}]"#, "").is_err());
    }

    #[test]
    fn batch_shares_follow_the_costs() {
        let a = parse(
            r#"[{"Plan": {"Node Type": "Result", "Total Cost": 1.0}}]"#,
            "",
        )
        .unwrap();
        let b = parse(
            r#"[{"Plan": {"Node Type": "Result", "Total Cost": 3.0}}]"#,
            "",
        )
        .unwrap();
        let plans = [a[0].clone(), b[0].clone()];
        assert_eq!(batch_shares(&plans), [0.25, 0.75]);
    }
}
