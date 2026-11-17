//! Edição na grade de resultados contra um PostgreSQL de verdade (com
//! `PGSTUDIO_TEST_HOST`; sem ele, os testes só avisam): o SELECT de uma
//! tabela com chave primária fica editável, a célula gravada aparece no
//! banco e na grade, Delete exclui as linhas confirmadas, e JOIN,
//! agregação, tabela sem chave e lote com vários comandos ficam só leitura
//! com o motivo.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_result_grid::Cell;
use pgstudio_ui_shell::execution::Execution;
use pgstudio_ui_shell::result_edit::Status;

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

/// Roda `sql` como F5 e espera a edição saber se dá para editar.
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

/// Espera a edição do primeiro resultado parar de checar ou gravar.
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

fn reason(exec: &Execution) -> String {
    match status(exec) {
        Status::ReadOnly(r) => r,
        other => panic!("expected read-only, got {other:?}"),
    }
}

#[test]
fn edits_and_deletes_from_a_select() {
    let Some(service) = service() else {
        return;
    };
    let session = service.open("postgres");
    let ctx = egui::Context::default();
    value(
        &session,
        "DROP TABLE IF EXISTS grid_edit; \
         CREATE TABLE grid_edit (id int PRIMARY KEY, nome text, \
             dobro int GENERATED ALWAYS AS (id * 2) STORED); \
         INSERT INTO grid_edit (id, nome) VALUES (1, 'Ana'), (2, 'Bruno'), (3, 'Carla'), (4, 'Davi')",
    );
    let mut exec = execute(
        &session,
        "SELECT nome, id, dobro, upper(nome) FROM grid_edit ORDER BY id;",
        &ctx,
    );
    assert_eq!(status(&exec), Status::Editable);
    assert_eq!(
        exec.results[0].grid.editable,
        vec![true, true, false, false],
        "generated columns and expressions stay read-only"
    );

    // Uma célula: d'água (com aspas) grava e a grade mostra o valor.
    let set = &mut exec.results[0];
    let store = set.shared_store();
    {
        let mut s = store.lock().unwrap();
        let editor = set.editor.as_mut().unwrap();
        editor.update(&mut s, Cell { row: 1, col: 0 }, Some("d'água".into()));
    }
    settle(&mut exec);
    assert_eq!(
        value(&session, "SELECT nome FROM grid_edit WHERE id = 2").as_deref(),
        Some("d'água")
    );
    assert_eq!(store.lock().unwrap().cell(1, 0), Some("d'água"));
    assert_eq!(
        exec.results[0].editor.as_ref().unwrap().message(),
        Some("1 row updated.")
    );

    // Pela grade (Enter ou clique fora), nada grava antes do "Yes": a
    // confirmação mostra a coluna, o valor de antes, o novo e o comando.
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        let editor = set.editor.as_mut().unwrap();
        editor.request_update(&mut s, Cell { row: 2, col: 0 }, Some("Clara".into()));
        assert!(editor.is_confirming_update());
        assert!(!editor.is_busy());
        assert_eq!(
            editor.confirm_update_preview(),
            Some((
                "nome",
                Some("Carla"),
                Some("Clara"),
                Some("UPDATE public.grid_edit SET nome = 'Clara' WHERE id = '3' RETURNING nome")
            ))
        );
        // Enquanto pergunta, ordenar ou filtrar não troca a linha de lugar.
        editor.step(&mut s, &mut set.grid);
        assert!(set.grid.view_locked);
        // "No": o banco e a grade ficam como estavam.
        editor.answer_update(false, &mut s);
        assert!(!editor.is_confirming_update() && !editor.is_busy());
        assert_eq!(editor.message(), Some("The value was not changed."));
        assert_eq!(s.cell(2, 0), Some("Carla"));
    }
    assert_eq!(
        value(&session, "SELECT nome FROM grid_edit WHERE id = 3").as_deref(),
        Some("Carla")
    );
    // "Yes": grava.
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        let editor = set.editor.as_mut().unwrap();
        editor.request_update(&mut s, Cell { row: 2, col: 0 }, Some("Clara".into()));
        editor.answer_update(true, &mut s);
    }
    settle(&mut exec);
    assert_eq!(
        value(&session, "SELECT nome FROM grid_edit WHERE id = 3").as_deref(),
        Some("Clara")
    );
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        let editor = set.editor.as_mut().unwrap();
        // O mesmo valor: não há o que confirmar. Volta ao original para o
        // resto do teste.
        editor.request_update(&mut s, Cell { row: 2, col: 0 }, Some("Clara".into()));
        assert!(!editor.is_confirming_update());
        editor.update(&mut s, Cell { row: 2, col: 0 }, Some("Carla".into()));
    }
    settle(&mut exec);

    // NULL (Ctrl+0) também.
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        set.editor
            .as_mut()
            .unwrap()
            .update(&mut s, Cell { row: 0, col: 0 }, None);
    }
    settle(&mut exec);
    assert_eq!(
        value(&session, "SELECT nome FROM grid_edit WHERE id = 1"),
        None
    );

    // Um erro (texto num int) não muda nada e volta para a edição.
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        set.editor
            .as_mut()
            .unwrap()
            .update(&mut s, Cell { row: 2, col: 1 }, Some("abc".into()));
    }
    settle(&mut exec);
    let set = &exec.results[0];
    assert!(
        set.editor
            .as_ref()
            .unwrap()
            .message()
            .unwrap()
            .contains("not saved"),
        "{:?}",
        set.editor.as_ref().unwrap().message()
    );
    assert_eq!(
        set.grid.editing.as_ref().map(|e| e.text.as_str()),
        Some("abc")
    );
    assert_eq!(store.lock().unwrap().cell(2, 1), Some("3"));

    // Delete de duas linhas: pede confirmação, "No" não faz nada.
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        let editor = set.editor.as_mut().unwrap();
        editor.request_delete(&mut s, 1..3);
        let (count, preview) = editor.confirm_preview().unwrap();
        assert_eq!(count, 2);
        assert_eq!(preview, ["id = 2", "id = 3"]);
        editor.answer_delete(false, &mut s);
        assert!(!editor.is_confirming());
    }
    assert_eq!(
        value(&session, "SELECT count(*) FROM grid_edit").as_deref(),
        Some("4")
    );
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        let editor = set.editor.as_mut().unwrap();
        editor.request_delete(&mut s, 1..3);
        editor.answer_delete(true, &mut s);
    }
    settle(&mut exec);
    assert_eq!(
        value(
            &session,
            "SELECT string_agg(id::text, ',' ORDER BY id) FROM grid_edit"
        )
        .as_deref(),
        Some("1,4")
    );
    {
        let mut s = store.lock().unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s.cell(1, 1), Some("4"));
    }
    assert_eq!(
        exec.results[0].editor.as_ref().unwrap().message(),
        Some("2 rows deleted.")
    );

    // A linha sumiu por fora: o UPDATE avisa em vez de fingir que gravou.
    value(&session, "DELETE FROM grid_edit WHERE id = 4");
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        set.editor
            .as_mut()
            .unwrap()
            .update(&mut s, Cell { row: 1, col: 0 }, Some("x".into()));
    }
    settle(&mut exec);
    assert!(
        exec.results[0]
            .editor
            .as_ref()
            .unwrap()
            .message()
            .unwrap()
            .contains("not found")
    );
    value(&session, "DROP TABLE grid_edit");
}

#[test]
fn queries_that_cannot_be_edited_say_why() {
    let Some(service) = service() else {
        return;
    };
    let session = service.open("postgres");
    let ctx = egui::Context::default();
    value(
        &session,
        "DROP TABLE IF EXISTS grid_ro_a, grid_ro_b, grid_ro_nokey; \
         CREATE TABLE grid_ro_a (id int PRIMARY KEY, b int); \
         CREATE TABLE grid_ro_b (id int PRIMARY KEY, nome text); \
         CREATE TABLE grid_ro_nokey (x int); \
         INSERT INTO grid_ro_a VALUES (1, 1); INSERT INTO grid_ro_b VALUES (1, 'x'); \
         INSERT INTO grid_ro_nokey VALUES (1)",
    );
    let cases = [
        (
            "SELECT a.id, b.nome FROM grid_ro_a a JOIN grid_ro_b b ON b.id = a.b",
            "more than one table",
        ),
        (
            "SELECT b, count(*) FROM grid_ro_a GROUP BY b",
            "primary key",
        ),
        ("SELECT * FROM grid_ro_nokey", "no primary key"),
        ("SELECT 1 + 1", "no column"),
        (
            "SELECT 1; SELECT * FROM grid_ro_a",
            "more than one statement",
        ),
        (
            "SELECT a.id, a2.id FROM grid_ro_a a, grid_ro_a a2",
            "more than once",
        ),
    ];
    for (sql, expected) in cases {
        let exec = execute(&session, sql, &ctx);
        let r = reason(&exec);
        assert!(r.contains(expected), "{sql}: {r}");
    }
    // Uma view simples: o servidor não diz a tabela de origem.
    value(
        &session,
        "CREATE OR REPLACE VIEW grid_ro_v AS SELECT * FROM grid_ro_a",
    );
    let exec = execute(&session, "SELECT * FROM grid_ro_v", &ctx);
    assert!(reason(&exec).contains("read-only"));
    // Agrupado pela chave continua editável (uma linha por chave).
    let exec = execute(
        &session,
        "SELECT id, count(*) FROM grid_ro_a GROUP BY id",
        &ctx,
    );
    assert_eq!(status(&exec), Status::Editable);
    value(
        &session,
        "DROP VIEW grid_ro_v; DROP TABLE grid_ro_a, grid_ro_b, grid_ro_nokey",
    );
}

/// Com a grade ordenada e filtrada, a edição e a exclusão valem para as
/// linhas que estão à mostra, e a ordem pedida no meio de uma gravação só
/// entra depois dela.
#[test]
fn edits_and_deletes_follow_the_sorted_and_filtered_rows() {
    use pgstudio_result_grid::{Sort, ViewSpec};
    let Some(service) = service() else {
        return;
    };
    let session = service.open("postgres");
    let ctx = egui::Context::default();
    value(
        &session,
        "DROP TABLE IF EXISTS grid_edit_view; \
         CREATE TABLE grid_edit_view (id int PRIMARY KEY, nome text); \
         INSERT INTO grid_edit_view VALUES (1, 'Ana'), (2, 'Bruno'), (3, 'Carla'), \
             (4, 'Davi'), (5, 'Bia')",
    );
    let mut exec = execute(
        &session,
        "SELECT id, nome FROM grid_edit_view ORDER BY id;",
        &ctx,
    );
    assert_eq!(status(&exec), Status::Editable);
    let store = exec.results[0].shared_store();
    // Só os nomes com "b", do maior id para o menor: Bia (5), Bruno (2).
    let spec = ViewSpec {
        sort: Some(Sort {
            col: 0,
            descending: true,
        }),
        filter: "nome:b".into(),
    };
    let apply = |spec: &ViewSpec| {
        let mut s = store.lock().unwrap();
        while s.sync_view(spec, Duration::from_secs(1)).working {}
    };
    apply(&spec);
    assert_eq!(store.lock().unwrap().len(), 2);
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        let editor = set.editor.as_mut().unwrap();
        editor.update(&mut s, Cell { row: 0, col: 1 }, Some("Bianca".into()));
        // Gravando: a grade não troca a ordem até a resposta chegar.
        editor.step(&mut s, &mut set.grid);
        assert!(set.grid.view_locked);
    }
    settle(&mut exec);
    assert!(!exec.results[0].grid.view_locked);
    assert_eq!(
        value(&session, "SELECT nome FROM grid_edit_view WHERE id = 5").as_deref(),
        Some("Bianca")
    );
    assert_eq!(store.lock().unwrap().cell(0, 1), Some("Bianca"));

    // Exclui a segunda linha à mostra (Bruno, id 2).
    {
        let set = &mut exec.results[0];
        let mut s = store.lock().unwrap();
        let editor = set.editor.as_mut().unwrap();
        editor.request_delete(&mut s, 1..2);
        assert_eq!(editor.confirm_preview().unwrap().1, ["id = 2"]);
        editor.step(&mut s, &mut set.grid);
        assert!(set.grid.view_locked, "esperando o Yes");
        editor.answer_delete(true, &mut s);
    }
    settle(&mut exec);
    assert_eq!(
        value(
            &session,
            "SELECT string_agg(id::text, ',' ORDER BY id) FROM grid_edit_view"
        )
        .as_deref(),
        Some("1,3,4,5")
    );
    assert_eq!(store.lock().unwrap().len(), 1);
    // Sem ordem e sem filtro: o resto do resultado, na ordem original.
    apply(&ViewSpec::default());
    let mut s = store.lock().unwrap();
    let ids: Vec<_> = (0..s.len())
        .map(|r| s.cell(r, 0).map(str::to_string))
        .collect();
    assert_eq!(s.len(), 4);
    assert_eq!(ids[3].as_deref(), Some("5"));
    assert_eq!(s.cell(3, 1), Some("Bianca"));
    drop(s);
    value(&session, "DROP TABLE grid_edit_view");
}
