//! Execução das janelas de consulta no Elasticsearch.
//!
//! O texto é lido por [`crate::console`]; cada requisição vai ao servidor
//! e a resposta vira [`QueryEvent`]s por [`crate::render`]. Um erro para
//! a execução, como no SQL.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use pgstudio_core::connection::{
    ELASTICSEARCH_DATABASE, Engine, REDIS_IN_TRANSACTION, REDIS_LIST_DATABASES,
};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage};

use crate::console::{self, Request};
use crate::http::{EsClient, HttpError, Reply};
use crate::render;

/// Abre uma [`EsQuerySession`] por janela de consulta.
pub struct EsQueryService {
    client: EsClient,
}

impl EsQueryService {
    pub fn new(client: EsClient) -> Self {
        EsQueryService { client }
    }
}

impl QueryService for EsQueryService {
    fn engine(&self) -> Engine {
        Engine::Elasticsearch
    }

    fn open(&self, _database: &str) -> Arc<dyn QuerySession> {
        Arc::new(EsQuerySession {
            client: self.client.clone(),
            shared: Arc::new(Shared::default()),
        })
    }
}

/// A sessão de uma janela. Não guarda conexão: cada requisição é uma
/// chamada HTTP (o `Agent` reaproveita as conexões).
pub struct EsQuerySession {
    client: EsClient,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    /// Uma execução por vez.
    running: Mutex<()>,
    /// O pedido de cancelamento da execução em andamento.
    cancel: Mutex<Option<Arc<AtomicBool>>>,
}

impl QuerySession for EsQuerySession {
    fn execute(&self, text: String, mut sink: QuerySink) {
        let shared = self.shared.clone();
        let client = self.client.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut c) = shared.cancel.lock() {
            *c = Some(cancel.clone());
        }
        std::thread::spawn(move || {
            let _running = shared.running.lock();
            let canceled = !run(&client, &text, &cancel, &mut sink);
            if let Ok(mut c) = shared.cancel.lock()
                && c.as_ref().is_some_and(|c| Arc::ptr_eq(c, &cancel))
            {
                *c = None;
            }
            sink(QueryEvent::Finished { canceled });
        });
    }

    fn cancel(&self) {
        if let Some(cancel) = self.shared.cancel.lock().ok().and_then(|c| c.clone()) {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    fn database(&self) -> String {
        ELASTICSEARCH_DATABASE.to_string()
    }

    fn engine(&self) -> Engine {
        Engine::Elasticsearch
    }
}

/// Posição (em caracteres, a partir de 1) do byte `at` de `text`.
fn position(text: &str, at: usize) -> usize {
    text.char_indices().take_while(|(i, _)| *i < at).count() + 1
}

fn error_at(message: String, code: String, text: &str, at: usize) -> QueryEvent {
    QueryEvent::Error(ServerMessage {
        severity: "ERROR".into(),
        code,
        message,
        position: Some(position(text, at)),
        ..Default::default()
    })
}

/// O caminho como vai ao servidor: um `_cat` sem formato pede JSON, para
/// virar uma grade.
pub fn effective_path(path: &str) -> String {
    let (main, query) = path.split_once('?').unwrap_or((path, ""));
    let is_cat = console::api(path) == "_cat" && main.trim_end_matches('/') != "/_cat";
    let chosen = query
        .split('&')
        .any(|p| p.starts_with("format=") || p == "help" || p.starts_with("help="));
    if !is_cat || chosen {
        return path.to_string();
    }
    match query {
        "" => format!("{main}?format=json"),
        q => format!("{main}?{q}&format=json"),
    }
}

/// Manda `request` numa thread e espera a resposta olhando o
/// cancelamento. `None` se cancelaram (a chamada é largada).
fn send(
    client: &EsClient,
    request: &Request,
    cancel: &AtomicBool,
) -> Option<Result<Reply, HttpError>> {
    let (tx, rx) = mpsc::channel();
    let client = client.clone();
    let method = request.method.clone();
    let path = effective_path(&request.path);
    let body = console::body(request);
    std::thread::spawn(move || {
        let body = body.as_ref().map(|(text, ty)| (text.as_str(), *ty));
        let _ = tx.send(client.request(&method, &path, body));
    });
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => return Some(result),
            Err(mpsc::RecvTimeoutError::Timeout) if cancel.load(Ordering::Relaxed) => return None,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Some(Err(HttpError::Network("request thread stopped".into())));
            }
        }
    }
}

/// Roda o texto. `false` se foi cancelado.
fn run(client: &EsClient, text: &str, cancel: &AtomicBool, sink: &mut QuerySink) -> bool {
    // O que a interface pergunta por SQL nos outros motores.
    match text.trim() {
        REDIS_LIST_DATABASES => {
            sink(QueryEvent::Columns(vec!["name".into()]));
            sink(QueryEvent::Row(vec![Some(ELASTICSEARCH_DATABASE.into())]));
            sink(QueryEvent::CommandComplete { rows: 1 });
            return true;
        }
        REDIS_IN_TRANSACTION => {
            sink(QueryEvent::Columns(vec!["in_transaction".into()]));
            sink(QueryEvent::Row(vec![Some("0".into())]));
            sink(QueryEvent::CommandComplete { rows: 1 });
            return true;
        }
        _ => {}
    }
    let requests = match console::parse(text) {
        Ok(r) => r,
        Err(e) => {
            sink(error_at(e.message, String::new(), text, e.at));
            return true;
        }
    };
    for request in &requests {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let Some(result) = send(client, request, cancel) else {
            return false;
        };
        let shown = match result {
            Ok(reply) => render::show(&reply, &mut **sink),
            Err(e) => Err((e.message().to_string(), String::new())),
        };
        if let Err((message, code)) = shown {
            sink(error_at(message, code, text, request.start));
            return true;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cat_asks_for_json_unless_told_otherwise() {
        assert_eq!(effective_path("/_cat/indices"), "/_cat/indices?format=json");
        assert_eq!(
            effective_path("/_cat/indices?v&s=index"),
            "/_cat/indices?v&s=index&format=json"
        );
        assert_eq!(
            effective_path("/_cat/indices?format=txt"),
            "/_cat/indices?format=txt"
        );
        assert_eq!(effective_path("/_cat/indices?help"), "/_cat/indices?help");
        assert_eq!(effective_path("/_cat"), "/_cat");
        assert_eq!(effective_path("/a/_search?q=x"), "/a/_search?q=x");
    }
}
