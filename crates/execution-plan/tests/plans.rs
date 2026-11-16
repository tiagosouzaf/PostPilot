//! Planos de verdade: saídas do `EXPLAIN` gravadas de um PostgreSQL 16 e,
//! com `PGSTUDIO_TEST_HOST`, planos pedidos na hora ao servidor de teste
//! (mesmas variáveis dos testes do `pg-client`; sem elas, essa parte avisa
//! e passa).

use std::sync::mpsc;
use std::time::Duration;

use pgstudio_core::connection::{ConnectionSettings, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService};
use pgstudio_execution_plan::{PlanView, parse};
use pgstudio_pg_client::{DbRuntime, PgQueryService};

#[test]
fn recorded_estimated_plan() {
    let json = include_str!("plans/pg16_estimated.json");
    let plans = parse(json, "SELECT ...").unwrap();
    let p = &plans[0];
    let kinds: Vec<&str> = p.nodes.iter().map(|n| n.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "Sort",
            "Aggregate",
            "Hash Join",
            "Seq Scan",
            "Hash",
            "Seq Scan"
        ]
    );
    assert_eq!(p.nodes[1].title(), "Hashed Aggregate");
    assert!(p.nodes[0].actual.is_none());
    let total: f64 = (0..p.nodes.len()).map(|i| p.cost_share(i)).sum();
    assert!((total - 1.0).abs() < 1e-9);
    let view = PlanView::new(p);
    assert_eq!(view.positions.len(), 6);
}

#[test]
fn recorded_actual_plan() {
    let json = include_str!("plans/pg16_actual.json");
    let p = &parse(json, "").unwrap()[0];
    assert!(p.execution_ms.is_some() && p.planning_ms.is_some());
    assert!(p.nodes.iter().all(|n| n.actual.is_some()));
    assert!(
        p.nodes[0]
            .properties
            .iter()
            .any(|(k, _)| k == "Shared Hit Blocks")
    );
}

fn service() -> Option<PgQueryService> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let settings = ConnectionSettings {
        engine: Default::default(),
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
        ssh: None,
    };
    Some(PgQueryService::new(
        settings,
        var("PGSTUDIO_TEST_PASSWORD", ""),
        DbRuntime::new().unwrap(),
    ))
}

/// Junta o texto das linhas de cada resultado.
fn explain(sql: &str) -> Vec<String> {
    let service = service().unwrap();
    let session = service.open("postgres");
    let (tx, rx) = mpsc::channel();
    session.execute(sql.to_string(), Box::new(move |e| drop(tx.send(e))));
    let mut outputs = Vec::new();
    let mut current = String::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(30)).unwrap() {
            QueryEvent::Row(r) => current.push_str(r[0].as_deref().unwrap_or("")),
            QueryEvent::CommandComplete { .. } => outputs.push(std::mem::take(&mut current)),
            QueryEvent::Error(e) => panic!("{}", e.message),
            QueryEvent::Finished { .. } => return outputs,
            _ => {}
        }
    }
}

#[test]
fn plans_from_the_server_are_read() {
    if service().is_none() {
        return;
    }
    let outputs = explain(
        "EXPLAIN (FORMAT JSON) SELECT c.relname, n.nspname FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace ORDER BY 1; \
         EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT count(*) FROM generate_series(1, 1000)",
    );
    assert_eq!(outputs.len(), 2);
    let estimated = &parse(&outputs[0], "").unwrap()[0];
    assert!(estimated.nodes.len() >= 3);
    assert!(
        estimated
            .nodes
            .iter()
            .any(|n| n.relation.as_deref() == Some("pg_class"))
    );
    let actual = &parse(&outputs[1], "").unwrap()[0];
    assert_eq!(actual.root().actual_total_rows(), Some(1.0));
    let scan = &actual.nodes[1];
    assert_eq!(scan.kind, "Function Scan");
    assert_eq!(scan.actual_total_rows(), Some(1000.0));
}

#[test]
fn a_bad_estimate_from_the_server_is_flagged() {
    if service().is_none() {
        return;
    }
    // Com o limite vindo de uma subconsulta o planejador chuta 1000 linhas.
    let outputs =
        explain("EXPLAIN (ANALYZE, FORMAT JSON) SELECT * FROM generate_series(1, (SELECT 20000))");
    let plan = &parse(&outputs[0], "").unwrap()[0];
    let scan = plan
        .nodes
        .iter()
        .position(|n| n.kind == "Function Scan")
        .unwrap();
    assert_eq!(plan.nodes[scan].plan_rows, 1000.0);
    assert_eq!(plan.nodes[scan].actual_total_rows(), Some(20000.0));
    assert_eq!(plan.misestimate(scan), Some(20.0));
}
