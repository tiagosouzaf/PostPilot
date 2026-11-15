//! Planos de execução contra um SQL Server de verdade (com
//! `MSSQL_TEST_HOST`; sem ele, os testes só avisam; veja
//! `docs/sqlserver.md`): o estimado não roda o comando, o real traz os
//! resultados e os números medidos, e a sessão volta ao normal depois.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Engine, SslMode};
use pgstudio_core::query::{QueryService, QuerySession};
use pgstudio_ui_shell::execution::{Execution, MessageKind, Outcome};

fn session() -> Option<Arc<dyn QuerySession>> {
    let Ok(host) = std::env::var("MSSQL_TEST_HOST") else {
        eprintln!("MSSQL_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let mut settings = ConnectionSettings::for_engine(Engine::SqlServer);
    settings.host = host;
    settings.port = var("MSSQL_TEST_PORT", "1433").parse().unwrap();
    settings.user = var("MSSQL_TEST_USER", "sa");
    settings.ssl = SslMode::Require;
    let service = pgstudio_mssql::MssqlQueryService::new(
        settings,
        var("MSSQL_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    );
    Some(service.open("tempdb"))
}

fn wait(mut exec: Execution) -> Execution {
    let start = Instant::now();
    while !exec.poll() {
        assert!(start.elapsed() < Duration::from_secs(60), "never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    exec
}

fn run(session: &Arc<dyn QuerySession>, sql: &str, ctx: &egui::Context) -> Execution {
    wait(Execution::start(session.clone(), sql.into(), 0, ctx))
}

fn errors(exec: &Execution) -> Vec<String> {
    exec.messages
        .iter()
        .filter(|m| m.kind == MessageKind::Error)
        .map(|m| m.text.clone())
        .collect()
}

const SETUP: &str = "IF OBJECT_ID('dbo.pp_plan_a') IS NOT NULL DROP TABLE dbo.pp_plan_a;
IF OBJECT_ID('dbo.pp_plan_b') IS NOT NULL DROP TABLE dbo.pp_plan_b;
CREATE TABLE dbo.pp_plan_a (id int PRIMARY KEY, nome nvarchar(40));
CREATE TABLE dbo.pp_plan_b (id int IDENTITY PRIMARY KEY, a_id int NOT NULL, total decimal(10,2));
INSERT INTO dbo.pp_plan_a VALUES (1, N'Ana'), (2, N'Bia'), (3, N'Caio');
INSERT INTO dbo.pp_plan_b (a_id, total) VALUES (1, 10), (1, 20), (2, 30);";

const JOIN: &str = "SELECT a.nome, SUM(b.total) AS total
FROM dbo.pp_plan_a AS a
JOIN dbo.pp_plan_b AS b ON b.a_id = a.id
GROUP BY a.nome";

#[test]
fn estimated_plan_reads_the_showplan_without_running() {
    let Some(session) = session() else { return };
    let ctx = egui::Context::default();
    let setup = run(&session, SETUP, &ctx);
    assert_eq!(errors(&setup), Vec::<String>::new());

    let sql = format!("{JOIN};\nDELETE FROM dbo.pp_plan_b WHERE a_id = 2");
    let exec = wait(Execution::start_plan(session.clone(), sql, 0, &ctx).unwrap());
    assert_eq!(errors(&exec), Vec::<String>::new());
    assert_eq!(exec.outcome(), Outcome::Succeeded);
    assert!(exec.results.is_empty(), "plans are not results");
    assert_eq!(exec.plans.len(), 2);
    let (select, _) = &exec.plans[0];
    assert!(select.statement.starts_with("SELECT a.nome"));
    assert!(select.nodes.len() >= 3, "{:?}", select.nodes);
    assert!(select.total_cost() > 0.0);
    assert!(select.nodes.iter().all(|n| n.actual.is_none()));
    let tables: Vec<_> = select
        .nodes
        .iter()
        .filter_map(|n| n.relation.clone())
        .collect();
    assert!(tables.contains(&"dbo.pp_plan_a".to_string()), "{tables:?}");
    assert!(tables.contains(&"dbo.pp_plan_b".to_string()), "{tables:?}");
    let delete = &exec.plans[1].0.statement;
    assert!(delete.starts_with("DELETE"), "{delete:?}");

    // Nada rodou, e a sessão voltou a executar.
    let after = run(&session, "SELECT COUNT(*) FROM dbo.pp_plan_b", &ctx);
    assert_eq!(after.results.len(), 1);
    assert_eq!(after.results[0].store().cell(0, 0), Some("3"));
}

#[test]
fn actual_plan_keeps_the_results_and_measures_rows() {
    let Some(session) = session() else { return };
    let ctx = egui::Context::default();
    let setup = run(&session, &SETUP.replace("pp_plan_", "pp_plan2_"), &ctx);
    assert_eq!(errors(&setup), Vec::<String>::new());

    let sql = JOIN.replace("pp_plan_", "pp_plan2_");
    let exec = wait(Execution::start_actual_plan(session.clone(), sql, 0, false, &ctx).unwrap());
    assert_eq!(errors(&exec), Vec::<String>::new());
    assert_eq!(exec.results.len(), 1, "the query result stays");
    assert_eq!(exec.results[0].store().len(), 2);
    assert_eq!(exec.plans.len(), 1);
    let plan = &exec.plans[0].0;
    // O Compute Scalar da raiz não é medido; o primeiro nó medido devolve
    // as duas linhas do resultado.
    let measured = plan.nodes.iter().find_map(|n| n.actual_total_rows());
    assert_eq!(measured, Some(2.0), "{:?}", plan.nodes);
    assert!(plan.execution_ms.is_some());

    // Com rollback, o comando roda mas não deixa rastro.
    let delete = "DELETE FROM dbo.pp_plan2_b WHERE a_id = 1".to_string();
    let exec = wait(Execution::start_actual_plan(session.clone(), delete, 0, true, &ctx).unwrap());
    assert_eq!(errors(&exec), Vec::<String>::new());
    assert_eq!(exec.plans.len(), 1);
    let after = run(&session, "SELECT COUNT(*) FROM dbo.pp_plan2_b", &ctx);
    assert_eq!(after.results[0].store().cell(0, 0), Some("3"));
    // E o plano não vem mais junto.
    assert!(after.plans.is_empty());
    assert_eq!(after.results.len(), 1);
}

#[test]
fn errors_in_plan_mode_point_at_the_statement() {
    let Some(session) = session() else { return };
    let ctx = egui::Context::default();
    let sql = "SELECT 1;\nSELECT * FROM dbo.pp_nao_existe".to_string();
    let mut exec = wait(Execution::start_plan(session.clone(), sql.clone(), 0, &ctx).unwrap());
    assert_eq!(errors(&exec).len(), 1, "{:?}", errors(&exec));
    let (at, _) = exec.take_error_position().expect("position");
    assert_eq!(&sql[at..at + 6], "SELECT");
    assert_eq!(at, 10);
    // O `SET SHOWPLAN_XML OFF` roda mesmo depois do erro.
    let after = run(&session, "SELECT 42", &ctx);
    assert_eq!(after.results[0].store().cell(0, 0), Some("42"));
}
