//! Edição na grade de resultados contra um SQL Server de verdade (com
//! `MSSQL_TEST_HOST`; sem ele, os testes só avisam; veja
//! `docs/sqlserver.md`): célula gravada, NULL, erro de conversão, Delete,
//! linha que sumiu, tabela com trigger, e os casos só leitura.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_result_grid::Cell;
use pgstudio_ui_shell::execution::Execution;
use pgstudio_ui_shell::result_edit::Status;

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

/// Primeira coluna da primeira linha do último resultado de `sql`.
fn value(session: &Arc<dyn QuerySession>, sql: &str) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut first = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Columns(_) => first = None,
            QueryEvent::Row(r) if first.is_none() => first = r.into_iter().next().flatten(),
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return first,
            _ => {}
        }
    }
}

fn execute(session: &Arc<dyn QuerySession>, sql: &str, ctx: &egui::Context) -> Execution {
    let mut exec = Execution::start(session.clone(), sql.into(), 0, ctx);
    let start = Instant::now();
    while !exec.poll() {
        assert!(start.elapsed() < Duration::from_secs(30), "never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(&mut exec);
    exec
}

fn settle(exec: &mut Execution) {
    let start = Instant::now();
    let set = &mut exec.results[0];
    let store = set.shared_store();
    loop {
        let editor = set.editor.as_mut().expect("editor");
        let mut store = store.lock().unwrap();
        editor.step(&mut store, &mut set.grid);
        if !editor.is_busy() {
            return;
        }
        drop(store);
        assert!(start.elapsed() < Duration::from_secs(30), "never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn status(exec: &Execution) -> Status {
    exec.results[0].editor.as_ref().unwrap().status().clone()
}

fn message(exec: &Execution) -> String {
    exec.results[0]
        .editor
        .as_ref()
        .unwrap()
        .message()
        .unwrap_or_default()
        .to_string()
}

fn update(exec: &mut Execution, row: u64, col: usize, value: Option<&str>) {
    let set = &mut exec.results[0];
    let store = set.shared_store();
    {
        let mut s = store.lock().unwrap();
        set.editor
            .as_mut()
            .unwrap()
            .update(&mut s, Cell { row, col }, value.map(str::to_string));
    }
    settle(exec);
}

#[test]
fn edits_and_deletes_from_a_select() {
    let Some(session) = session() else {
        return;
    };
    let ctx = egui::Context::default();
    value(
        &session,
        "DROP TABLE IF EXISTS dbo.grid_edit; \
         CREATE TABLE dbo.grid_edit (id int PRIMARY KEY, [nome do cliente] nvarchar(50), \
             dobro AS (id * 2), criado datetime2(0), foto varbinary(10)); \
         INSERT INTO dbo.grid_edit (id, [nome do cliente], criado) VALUES \
             (1, N'Ana', '2026-01-02 03:04:05'), (2, N'Bruno', NULL), \
             (3, N'Carla', NULL), (4, N'Davi', NULL);\nGO\n\
         CREATE TRIGGER dbo.grid_edit_t ON dbo.grid_edit AFTER UPDATE, DELETE AS SET NOCOUNT ON;",
    );
    let mut exec = execute(
        &session,
        "SELECT [nome do cliente], id, dobro, UPPER([nome do cliente]), criado, foto \
         FROM dbo.grid_edit ORDER BY id",
        &ctx,
    );
    assert_eq!(status(&exec), Status::Editable);
    assert_eq!(
        exec.results[0].grid.editable,
        vec![true, true, false, false, true, false],
        "computed, expression and binary columns stay read-only"
    );
    let store = exec.results[0].shared_store();

    // Texto com aspas e acento, numa tabela com trigger.
    update(&mut exec, 1, 0, Some("d'água"));
    assert_eq!(
        value(
            &session,
            "SELECT [nome do cliente] FROM dbo.grid_edit WHERE id = 2"
        )
        .as_deref(),
        Some("d'água")
    );
    assert_eq!(store.lock().unwrap().cell(1, 0), Some("d'água"));
    assert_eq!(message(&exec), "1 row updated.");

    // A data volta como o servidor gravou.
    update(&mut exec, 0, 4, Some("2026-11-15 10:20:30"));
    assert_eq!(
        store.lock().unwrap().cell(0, 4),
        Some("2026-11-15 10:20:30")
    );

    // NULL.
    update(&mut exec, 0, 0, None);
    assert_eq!(
        value(
            &session,
            "SELECT [nome do cliente] FROM dbo.grid_edit WHERE id = 1"
        ),
        None
    );

    // Mudar a chave: a linha é achada de novo pelo valor novo.
    update(&mut exec, 3, 1, Some("40"));
    assert_eq!(store.lock().unwrap().cell(3, 1), Some("40"));
    assert_eq!(message(&exec), "1 row updated.");

    // Erro de conversão: nada muda e a edição volta.
    update(&mut exec, 2, 1, Some("abc"));
    assert!(message(&exec).contains("not saved"), "{}", message(&exec));
    assert_eq!(
        exec.results[0]
            .grid
            .editing
            .as_ref()
            .map(|e| e.text.as_str()),
        Some("abc")
    );

    // Delete de duas linhas.
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        let editor = set.editor.as_mut().unwrap();
        editor.request_delete(&mut s, 1..3);
        assert_eq!(editor.confirm_preview().unwrap().1, ["id = 2", "id = 3"]);
        editor.answer_delete(true, &mut s);
    }
    settle(&mut exec);
    assert_eq!(
        value(
            &session,
            "SELECT STRING_AGG(CAST(id AS varchar), ',') WITHIN GROUP (ORDER BY id) \
             FROM dbo.grid_edit"
        )
        .as_deref(),
        Some("1,40")
    );
    assert_eq!(store.lock().unwrap().len(), 2);
    assert_eq!(message(&exec), "2 rows deleted.");

    // A linha sumiu por fora.
    value(&session, "DELETE FROM dbo.grid_edit WHERE id = 40");
    update(&mut exec, 1, 0, Some("x"));
    assert!(message(&exec).contains("not found"), "{}", message(&exec));
    value(&session, "DROP TABLE dbo.grid_edit");
}

#[test]
fn queries_that_cannot_be_edited_say_why() {
    let Some(session) = session() else {
        return;
    };
    let ctx = egui::Context::default();
    value(
        &session,
        "DROP VIEW IF EXISTS dbo.grid_ro_v; \
         DROP TABLE IF EXISTS dbo.grid_ro_a, dbo.grid_ro_b, dbo.grid_ro_nokey; \
         CREATE TABLE dbo.grid_ro_a (id int PRIMARY KEY, b int); \
         CREATE TABLE dbo.grid_ro_b (id int PRIMARY KEY, nome nvarchar(10)); \
         CREATE TABLE dbo.grid_ro_nokey (x int);",
    );
    value(
        &session,
        "CREATE VIEW dbo.grid_ro_v AS SELECT id, b FROM dbo.grid_ro_a",
    );
    let reason = |sql: &str| {
        let exec = execute(&session, sql, &ctx);
        match status(&exec) {
            Status::ReadOnly(r) => r,
            other => panic!("{sql}: expected read-only, got {other:?}"),
        }
    };
    assert!(
        reason("SELECT a.id, b.nome FROM dbo.grid_ro_a a JOIN dbo.grid_ro_b b ON b.id = a.b")
            .contains("more than one table")
    );
    assert!(reason("SELECT b FROM dbo.grid_ro_a").contains("primary key"));
    assert!(reason("SELECT x FROM dbo.grid_ro_nokey").contains("no primary key"));
    assert!(reason("SELECT COUNT(*) FROM dbo.grid_ro_a").contains("no column"));
    // A view diz de onde vem cada coluna; fica editável pela tabela, como
    // no SSMS.
    let exec = execute(&session, "SELECT id, b FROM dbo.grid_ro_v", &ctx);
    assert_eq!(status(&exec), Status::Editable);
    value(
        &session,
        "DROP VIEW dbo.grid_ro_v; DROP TABLE dbo.grid_ro_a, dbo.grid_ro_b, dbo.grid_ro_nokey",
    );
}

/// O catálogo diz o editor de cada coluna (calendário, lista de valores),
/// e o que o editor escreve o SQL Server aceita sem perder a precisão.
#[test]
fn typed_columns_get_their_editor_and_keep_their_precision() {
    use pgstudio_result_grid::typed::Stamp;
    use pgstudio_result_grid::{CellKind, ColumnKind};

    let Some(session) = session() else {
        return;
    };
    let ctx = egui::Context::default();
    value(
        &session,
        "DROP TABLE IF EXISTS dbo.grid_typed; \
         CREATE TABLE dbo.grid_typed (id int PRIMARY KEY, criado datetimeoffset(7) NOT NULL, \
             visto datetime2(3), antigo datetime, dia date, hora time(2), ativo bit, \
             certo bit NOT NULL, nome nvarchar(40)); \
         INSERT INTO dbo.grid_typed VALUES (1, '2026-11-12 10:46:35.1873861 -03:00', \
             '2026-11-12 10:46:35.187', '2026-11-12 10:46:35.187', '2026-11-12', \
             '10:46:35.5', 1, 0, N'Ana')",
    );
    let mut exec = execute(
        &session,
        "SELECT id, criado, visto, antigo, dia, hora, ativo, certo, nome FROM dbo.grid_typed",
        &ctx,
    );
    assert_eq!(status(&exec), Status::Editable);
    let kinds = exec.results[0].grid.kinds.clone();
    let ms = |name: &str, nullable| ColumnKind::from_type(true, name, nullable, &[]);
    assert_eq!(kinds[0], ColumnKind::default());
    assert_eq!(kinds[1], ms("datetimeoffset(7)", false));
    assert_eq!(kinds[2], ms("datetime2(3)", true));
    assert_eq!(kinds[3], ms("datetime", true));
    assert_eq!(kinds[4], ms("date", true));
    assert_eq!(kinds[5], ms("time(2)", true));
    assert_eq!(kinds[6], ms("bit", true));
    assert_eq!(kinds[7], ms("bit", false));
    assert_eq!(kinds[8].kind, CellKind::Text);

    // Outro dia pelo calendário: a fração e o fuso do valor continuam.
    let set = &mut exec.results[0];
    let store = set.shared_store();
    let old = store.lock().unwrap().cell(0, 1).unwrap().to_string();
    assert_eq!(old, "2026-11-12 10:46:35.1873861 -03:00");
    let mut stamp = Stamp::parse(&kinds[1].kind, &old).expect("datetimeoffset");
    stamp.day = 30;
    stamp.hour = 7;
    let CellKind::Choice(bit) = &kinds[6].kind else {
        panic!("bit");
    };
    for (col, value) in [
        (1, Some(stamp.format(&kinds[1].kind))),
        (2, Some("2027-01-31 23:59:59.123".to_string())),
        (4, None),
        (5, Some("23:59:59.12".to_string())),
        (6, Some(bit[1].value.clone())),
        (7, Some(bit[0].value.clone())),
    ] {
        let set = &mut exec.results[0];
        {
            let mut s = store.lock().unwrap();
            let editor = set.editor.as_mut().unwrap();
            editor.request_update(&mut s, Cell { row: 0, col }, value);
            assert!(editor.is_confirming_update(), "column {col}");
            editor.answer_update(true, &mut s);
        }
        settle(&mut exec);
        assert_eq!(
            exec.results[0].editor.as_ref().unwrap().message(),
            Some("1 row updated."),
            "column {col}"
        );
    }
    let mut s = store.lock().unwrap();
    assert_eq!(s.cell(0, 1), Some("2026-11-30 07:46:35.1873861 -03:00"));
    assert_eq!(s.cell(0, 2), Some("2027-01-31 23:59:59.123"));
    assert_eq!(s.cell(0, 4), None);
    assert_eq!(s.cell(0, 5), Some("23:59:59.12"));
    assert_eq!(s.cell(0, 6), Some("0"));
    assert_eq!(s.cell(0, 7), Some("1"));
    drop(s);
    value(&session, "DROP TABLE dbo.grid_typed");
}
