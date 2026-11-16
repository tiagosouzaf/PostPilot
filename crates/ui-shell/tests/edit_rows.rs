//! Edit Top 200 Rows contra um PostgreSQL de verdade (com
//! `PGSTUDIO_TEST_HOST`; sem ele, os testes só avisam): editar, inserir e
//! excluir aparecem no banco, erro deixa a linha marcada sem mudar nada, e
//! tabela sem chave primária fica só leitura.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::edit_rows::{EditRows, RowState};

fn service() -> Option<pgstudio_pg_client::PgQueryService> {
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
    Some(pgstudio_pg_client::PgQueryService::new(
        settings,
        var("PGSTUDIO_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ))
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

/// Espera a janela terminar de ler ou gravar.
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

/// Cria a tabela `name` com `ddl` e abre a janela sobre ela.
fn open(
    service: &pgstudio_pg_client::PgQueryService,
    admin: &Arc<dyn QuerySession>,
    name: &str,
    ddl: &str,
    ctx: &egui::Context,
) -> EditRows {
    value(admin, &format!("DROP TABLE IF EXISTS {name}; {ddl}"));
    let oid: u32 = value(admin, &format!("SELECT '{name}'::regclass::oid"))
        .unwrap()
        .parse()
        .unwrap();
    let mut e = EditRows::new(service.open("postgres"), oid, 200, ctx);
    settle(&mut e, ctx);
    assert_eq!(e.load_error(), None);
    e
}

#[test]
fn edits_inserts_and_deletes_reach_the_database() {
    let Some(service) = service() else { return };
    let ctx = egui::Context::default();
    let admin = service.open("postgres");
    let t = format!("edit_rows_{}", std::process::id());
    let mut e = open(
        &service,
        &admin,
        &t,
        &format!(
            "CREATE TABLE {t} (id serial PRIMARY KEY, nome text NOT NULL, \
             total numeric DEFAULT 0, dobro int GENERATED ALWAYS AS (id * 2) STORED); \
             INSERT INTO {t} (nome, total) VALUES ('a', 1), ('b''s', 2), ('c', 3);"
        ),
        &ctx,
    );
    assert_eq!(e.row_count(), 3);
    assert_eq!(e.read_only_reason(), None);
    assert_eq!(e.value(1, 1), Some("b's"));
    let count = |a: &Arc<dyn QuerySession>| value(a, &format!("SELECT count(*) FROM {t}"));

    // Editar: a linha grava ao sair dela.
    e.select(0, 1, &ctx);
    e.set_cell(0, 1, Some("d'água".into()));
    e.set_cell(0, 2, None);
    // Coluna gerada não muda.
    e.set_cell(0, 3, Some("99".into()));
    assert_eq!(e.row_state(0), RowState::Modified);
    e.select(1, 1, &ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.row_state(0), RowState::Clean, "{:?}", e.row_error(0));
    assert_eq!(
        value(
            &admin,
            &format!("SELECT nome || '/' || coalesce(total::text, 'null') FROM {t} WHERE id = 1")
        ),
        Some("d'água/null".into())
    );
    assert_eq!(e.value(0, 3), Some("2"));

    // Inserir pela linha em branco do fim: id, total e dobro vêm do banco.
    e.select(3, 1, &ctx);
    e.set_cell(3, 1, Some("novo".into()));
    assert_eq!(e.row_state(3), RowState::New);
    e.commit(&ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.row_state(3), RowState::Clean);
    assert_eq!(e.row_count(), 4);
    assert_eq!(e.value(3, 0), Some("4"));
    assert_eq!(e.value(3, 2), Some("0"));
    assert_eq!(e.value(3, 3), Some("8"));
    assert_eq!(count(&admin), Some("4".into()));

    // Erro (NOT NULL): a linha fica marcada, o banco não muda; Esc desfaz.
    e.select(2, 1, &ctx);
    e.set_cell(2, 1, None);
    e.commit(&ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.row_state(2), RowState::Error);
    assert!(
        e.row_error(2).unwrap().contains("null value"),
        "{:?}",
        e.row_error(2)
    );
    assert_eq!(
        value(&admin, &format!("SELECT nome FROM {t} WHERE id = 3")),
        Some("c".into())
    );
    e.revert_row(2);
    assert_eq!(e.row_state(2), RowState::Clean);
    assert_eq!(e.value(2, 1), Some("c"));

    // Excluir pede confirmação; "No" não faz nada.
    e.request_delete(1);
    assert!(e.is_confirming());
    e.answer_delete(false, &ctx);
    settle(&mut e, &ctx);
    assert_eq!(count(&admin), Some("4".into()));
    e.request_delete(1);
    e.answer_delete(true, &ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.row_count(), 3);
    assert_eq!(count(&admin), Some("3".into()));
    assert_eq!(
        value(&admin, &format!("SELECT count(*) FROM {t} WHERE id = 2")),
        Some("0".into())
    );

    // Linha excluída por outra sessão: o UPDATE não acha e avisa.
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

#[test]
fn closing_with_a_changed_row_saves_it_first() {
    let Some(service) = service() else { return };
    let ctx = egui::Context::default();
    let admin = service.open("postgres");
    let t = format!("edit_rows_close_{}", std::process::id());
    let mut e = open(
        &service,
        &admin,
        &t,
        &format!("CREATE TABLE {t} (id int PRIMARY KEY, v text); INSERT INTO {t} VALUES (1, 'a');"),
        &ctx,
    );
    e.select(0, 1, &ctx);
    e.set_cell(0, 1, Some("b".into()));
    assert!(!e.request_close());
    settle(&mut e, &ctx);
    e.step(&ctx);
    assert!(e.take_closed());
    assert_eq!(
        value(&admin, &format!("SELECT v FROM {t}")),
        Some("b".into())
    );
    value(&admin, &format!("DROP TABLE {t}"));
}

#[test]
fn a_table_without_a_primary_key_is_read_only() {
    let Some(service) = service() else { return };
    let ctx = egui::Context::default();
    let admin = service.open("postgres");
    let t = format!("edit_rows_nokey_{}", std::process::id());
    let mut e = open(
        &service,
        &admin,
        &t,
        &format!("CREATE TABLE {t} (v text); INSERT INTO {t} VALUES ('a');"),
        &ctx,
    );
    assert!(e.read_only_reason().unwrap().contains("no primary key"));
    assert_eq!(e.row_count(), 1);
    e.set_cell(0, 0, Some("b".into()));
    assert_eq!(e.row_state(0), RowState::Clean);
    assert_eq!(e.value(0, 0), Some("a"));
    e.request_delete(0);
    assert!(!e.is_confirming());
    value(&admin, &format!("DROP TABLE {t}"));
}

#[test]
fn json_cells_are_edited_in_the_json_editor() {
    let Some(service) = service() else { return };
    let ctx = egui::Context::default();
    let admin = service.open("postgres");
    let t = format!("edit_rows_json_{}", std::process::id());
    let mut e = open(
        &service,
        &admin,
        &t,
        &format!(
            "CREATE TABLE {t} (id int PRIMARY KEY, data jsonb, raw json, note text); \
             INSERT INTO {t} VALUES (1, '{{\"a\": [1, 2]}}', '{{\"b\":  1, \"b\": 2}}', 'x');"
        ),
        &ctx,
    );
    // Coluna de texto não abre o editor de JSON.
    assert!(!e.open_json_editor(0, 3));
    e.select(0, 1, &ctx);
    assert!(e.open_json_editor(0, 1));
    let v = e.json_editor_mut().unwrap();
    assert_eq!(v.title(), "Edit JSON - data (row 1)");
    v.set_edit_text("{\"a\": [1, 2, 3], \"b\": }");
    assert!(v.error().is_some());
    assert!(!e.json_editor_ok(), "JSON inválido não grava");
    e.json_editor_mut()
        .unwrap()
        .set_edit_text("{\"a\": [1, 2, 3],\n \"ok\": true}");
    assert!(e.json_editor_ok());
    assert!(e.json_editor_mut().is_none());
    // json guarda o texto como veio; o editor mostra e grava compacto.
    assert!(e.open_json_editor(0, 2));
    e.json_editor_mut().unwrap().set_edit_text("");
    assert!(e.json_editor_ok(), "vazio é NULL");
    e.commit(&ctx);
    settle(&mut e, &ctx);
    assert_eq!(e.row_state(0), RowState::Clean, "{:?}", e.row_error(0));
    assert_eq!(
        value(
            &admin,
            &format!("SELECT data::text || ' / ' || coalesce(raw::text, 'NULL') FROM {t}")
        ),
        Some("{\"a\": [1, 2, 3], \"ok\": true} / NULL".into())
    );
    value(&admin, &format!("DROP TABLE {t}"));
}
