//! Edit Top 200 Rows contra um SQL Server de verdade (com
//! `MSSQL_TEST_HOST`; sem ele, os testes só avisam): editar, inserir com
//! identidade, excluir, erro de NOT NULL, linha excluída por fora, tudo numa
//! tabela com trigger (onde `OUTPUT` sem `INTO` falharia).

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::edit_rows::{EditRows, RowState};

fn service() -> Option<pgstudio_mssql::MssqlQueryService> {
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
    Some(pgstudio_mssql::MssqlQueryService::new(
        settings,
        var("MSSQL_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ))
}

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

fn settle(e: &mut EditRows, ctx: &egui::Context) {
    let start = Instant::now();
    loop {
        e.step(ctx);
        if !e.is_loading() && !e.is_saving() {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn edits_inserts_and_deletes_reach_the_database() {
    let Some(service) = service() else { return };
    let ctx = egui::Context::default();
    let admin = service.open("tempdb");
    let t = format!("dbo.edit_rows_{}", std::process::id());
    value(
        &admin,
        &format!(
            "DROP TABLE IF EXISTS {t};\n\
             CREATE TABLE {t} (id int IDENTITY PRIMARY KEY, [nome do item] nvarchar(40) NOT NULL, \
                 total decimal(10,2) DEFAULT 0, dobro AS (id * 2));\n\
             INSERT INTO {t} ([nome do item], total) VALUES (N'a', 1), (N'b''s', 2), (N'c', 3);\n\
             GO\n\
             CREATE TRIGGER {t}_t ON {t} AFTER INSERT, UPDATE, DELETE AS SET NOCOUNT ON;"
        ),
    );
    let oid: u32 = value(&admin, &format!("SELECT OBJECT_ID('{t}')"))
        .unwrap()
        .parse()
        .unwrap();
    let mut e = EditRows::new(service.open("tempdb"), oid, 200, &ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.load_error(), None);
    assert_eq!(e.row_count(), 3);
    assert_eq!(e.read_only_reason(), None);
    assert_eq!(e.value(1, 1), Some("b's"));
    let count = || value(&admin, &format!("SELECT COUNT(*) FROM {t}"));

    // Editar: grava ao sair da linha; a coluna calculada não muda.
    e.select(0, 1, &ctx);
    e.set_cell(0, 1, Some("d'água".into()));
    e.set_cell(0, 2, None);
    e.set_cell(0, 3, Some("99".into()));
    e.select(1, 1, &ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.row_state(0), RowState::Clean, "{:?}", e.row_error(0));
    assert_eq!(
        value(
            &admin,
            &format!(
                "SELECT [nome do item] + '/' + ISNULL(CAST(total AS varchar), 'null') \
                 FROM {t} WHERE id = 1"
            )
        ),
        Some("d'água/null".into())
    );
    assert_eq!(e.value(0, 3), Some("2"));

    // Inserir: id (identidade), total (default) e dobro vêm do banco.
    e.select(3, 1, &ctx);
    e.set_cell(3, 1, Some("novo".into()));
    assert_eq!(e.row_state(3), RowState::New);
    e.commit(&ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.row_state(3), RowState::Clean, "{:?}", e.row_error(3));
    assert_eq!(e.value(3, 0), Some("4"));
    assert_eq!(e.value(3, 2), Some("0.00"));
    assert_eq!(e.value(3, 3), Some("8"));
    assert_eq!(count(), Some("4".into()));

    // NOT NULL: a linha fica marcada e o banco não muda.
    e.select(2, 1, &ctx);
    e.set_cell(2, 1, None);
    e.commit(&ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.row_state(2), RowState::Error);
    assert!(
        e.row_error(2).unwrap().contains("NULL"),
        "{:?}",
        e.row_error(2)
    );
    e.revert_row(2);
    assert_eq!(e.value(2, 1), Some("c"));

    // Excluir.
    e.request_delete(1);
    e.answer_delete(true, &ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.row_count(), 3);
    assert_eq!(count(), Some("3".into()));

    // Linha excluída por fora: o UPDATE avisa.
    value(&admin, &format!("DELETE FROM {t} WHERE id = 3"));
    let row = (0..e.row_count())
        .find(|&r| e.value(r, 0) == Some("3"))
        .unwrap();
    e.select(row, 1, &ctx);
    e.set_cell(row, 1, Some("x".into()));
    e.commit(&ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.row_state(row), RowState::Error);
    assert!(e.row_error(row).unwrap().contains("not found"));

    value(&admin, &format!("DROP TABLE {t}"));
}
