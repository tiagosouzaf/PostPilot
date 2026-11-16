//! Edição de documentos na grade de resultados contra um MongoDB de
//! verdade (com `MONGODB_TEST_HOST`; sem ele, os testes só avisam; veja
//! `docs/mongodb.md`): o resultado de um `find` fica editável, a célula
//! gravada mantém o tipo do campo, Delete exclui os documentos
//! confirmados e o resto fica só leitura com o motivo.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Connector, Engine};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_result_grid::Cell;
use pgstudio_ui_shell::execution::Execution;
use pgstudio_ui_shell::result_edit::Status;

fn session(database: &str) -> Option<Arc<dyn QuerySession>> {
    let Ok(host) = std::env::var("MONGODB_TEST_HOST") else {
        eprintln!("MONGODB_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let mut settings = ConnectionSettings::for_engine(Engine::MongoDb);
    settings.host = host;
    settings.port = var("MONGODB_TEST_PORT", "27017").parse().unwrap();
    settings.user = var("MONGODB_TEST_USER", "");
    let connector =
        pgstudio_mongodb::MongoConnector::new(pgstudio_pg_client::DbRuntime::new().unwrap());
    let connected = connector
        .connect(&settings, &var("MONGODB_TEST_PASSWORD", ""))
        .expect("connect");
    Some(connected.queries.open(database))
}

/// As linhas do último resultado de `text`, com NULL para o que falta.
fn rows(session: &Arc<dyn QuerySession>, text: &str) -> Vec<Vec<String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(text.into(), Box::new(move |e| drop(tx.send(e))));
    let mut rows = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Columns(_) => rows.clear(),
            QueryEvent::Row(r) => rows.push(
                r.into_iter()
                    .map(|c| c.unwrap_or_else(|| "NULL".into()))
                    .collect(),
            ),
            QueryEvent::Error(e) => panic!("{text}: {}", e.message),
            QueryEvent::Finished { .. } => return rows,
            _ => {}
        }
    }
}

fn execute(session: &Arc<dyn QuerySession>, text: &str, ctx: &egui::Context) -> Execution {
    let mut exec = Execution::start(session.clone(), text.into(), 0, ctx);
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

fn message(exec: &Execution) -> String {
    exec.results[0]
        .editor
        .as_ref()
        .unwrap()
        .message()
        .unwrap_or_default()
        .to_string()
}

fn reason(exec: &Execution) -> String {
    match exec.results[0].editor.as_ref().unwrap().status().clone() {
        Status::ReadOnly(r) => r,
        other => panic!("expected read-only, got {other:?}"),
    }
}

/// O tipo e o valor do campo `field` do documento `id`, como o servidor
/// guarda.
fn stored(session: &Arc<dyn QuerySession>, id: &str, field: &str) -> Vec<String> {
    rows(
        session,
        &format!(
            "db.pedidos.aggregate([{{ $match: {{ _id: {id} }} }}, \
             {{ $project: {{ _id: 0, tipo: {{ $type: '${field}' }}, valor: '${field}' }} }}])"
        ),
    )
    .into_iter()
    .next()
    .unwrap_or_default()
}

#[test]
fn edits_and_deletes_the_documents_of_a_find() {
    let Some(session) = session("pptest_grid_edit") else {
        return;
    };
    let ctx = egui::Context::default();
    rows(
        &session,
        "db.dropDatabase()\n\
         db.pedidos.insertMany([\n\
           { _id: 1, cliente: 'Ana', total: 10, pago: false, em: ISODate('2026-01-02T03:04:05Z'), itens: [{ sku: 'a' }] },\n\
           { _id: 2, cliente: 'Bia', total: 20.5, pago: true },\n\
           { _id: ObjectId('65f000000000000000000001'), cliente: 'Caio', total: 30 },\n\
           { _id: 'x-4', cliente: 'Davi', total: 40 }\n\
         ])",
    );
    let mut exec = execute(&session, "db.pedidos.find().sort({ total: 1 })", &ctx);
    let editor = exec.results[0].editor.as_ref().unwrap();
    assert_eq!(*editor.status(), Status::Editable);
    assert_eq!(
        editor.editing().as_deref(),
        Some("pptest_grid_edit.pedidos")
    );
    assert_eq!(
        exec.results[0].columns(),
        ["_id", "cliente", "total", "pago", "em", "itens"]
    );
    assert_eq!(
        exec.results[0].grid.editable,
        vec![false, true, true, true, true, true],
        "everything but _id"
    );

    // Texto continua texto, mesmo parecendo número.
    update(&mut exec, 0, 1, Some("42"));
    assert_eq!(message(&exec), "1 row updated.");
    assert_eq!(stored(&session, "1", "cliente"), ["string", "42"]);
    assert_eq!(exec.results[0].row(0)[1].as_deref(), Some("42"));

    // Número continua número; um texto solto é recusado e nada muda.
    update(&mut exec, 0, 2, Some("15"));
    assert_eq!(stored(&session, "1", "total"), ["int", "15"]);
    update(&mut exec, 0, 2, Some("quinze"));
    assert!(
        message(&exec).contains("total holds a number"),
        "{}",
        message(&exec)
    );
    assert_eq!(stored(&session, "1", "total"), ["int", "15"]);

    // Booleano, data e lista.
    update(&mut exec, 0, 3, Some("true"));
    assert_eq!(stored(&session, "1", "pago"), ["bool", "true"]);
    update(&mut exec, 0, 4, Some("2026-05-06T07:08:09Z"));
    assert_eq!(
        stored(&session, "1", "em"),
        ["date", "2026-05-06T07:08:09Z"]
    );
    update(&mut exec, 0, 5, Some(r#"[{"sku":"b"},{"sku":"c"}]"#));
    assert_eq!(
        stored(&session, "1", "itens"),
        ["array", r#"[{"sku":"b"},{"sku":"c"}]"#]
    );

    // Campo que o documento não tinha: nasce com o tipo do que foi
    // escrito. NULL grava null.
    update(&mut exec, 1, 3, Some("true"));
    assert_eq!(stored(&session, "2", "pago"), ["bool", "true"]);
    update(&mut exec, 2, 3, Some("depois"));
    assert_eq!(
        stored(&session, "ObjectId('65f000000000000000000001')", "pago"),
        ["string", "depois"]
    );
    update(&mut exec, 3, 1, None);
    assert_eq!(stored(&session, "'x-4'", "cliente"), ["null", "NULL"]);

    // O _id não muda.
    update(&mut exec, 0, 0, Some("9"));
    assert_eq!(
        rows(&session, "db.pedidos.countDocuments({ _id: 9 })"),
        [["0"]]
    );

    // Delete: confirma com os _id e exclui só depois do Yes.
    let set = &mut exec.results[0];
    let store = set.shared_store();
    {
        let mut s = store.lock().unwrap();
        let editor = set.editor.as_mut().unwrap();
        editor.request_delete(&mut s, 2..4);
        let (count, preview) = editor.confirm_preview().unwrap();
        assert_eq!(count, 2);
        assert_eq!(preview, ["_id = 65f000000000000000000001", "_id = x-4"]);
        editor.answer_delete(false, &mut s);
        editor.request_delete(&mut s, 2..4);
        editor.answer_delete(true, &mut s);
    }
    settle(&mut exec);
    assert_eq!(message(&exec), "2 rows deleted.");
    assert_eq!(exec.results[0].len(), 2);
    assert_eq!(
        rows(&session, "db.pedidos.find({}, { _id: 1 }).sort({ _id: 1 })"),
        [["1"], ["2"]]
    );

    // Documento que sumiu por fora: avisa e não grava.
    rows(&session, "db.pedidos.deleteOne({ _id: 2 })");
    update(&mut exec, 1, 1, Some("x"));
    assert!(
        message(&exec).contains("was not found"),
        "{}",
        message(&exec)
    );
}

#[test]
fn results_that_are_not_a_find_say_why() {
    let Some(session) = session("pptest_grid_readonly") else {
        return;
    };
    let ctx = egui::Context::default();
    rows(
        &session,
        "db.dropDatabase()\ndb.pedidos.insertOne({ _id: 1, total: 10 })",
    );
    for (text, why) in [
        (
            "db.pedidos.aggregate([{ $match: {} }])",
            "only the documents of a find",
        ),
        (
            "db.pedidos.find({}, { _id: 0, total: 1 })",
            "leaves out _id",
        ),
        (
            "db.pedidos.countDocuments()",
            "only the documents of a find",
        ),
        ("show collections", "only the documents of a find"),
    ] {
        let exec = execute(&session, text, &ctx);
        let reason = reason(&exec);
        assert!(reason.contains(why), "{text}: {reason}");
        assert!(exec.results[0].grid.editable.is_empty(), "{text}");
    }
}
