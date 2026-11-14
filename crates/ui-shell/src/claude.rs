//! A ponte entre o Claude e a interface (marco 7.1).
//!
//! As chamadas de ferramenta chegam pelo [`Inbox`] do
//! `pgstudio-automation` (do servidor MCP ou do painel do Claude) e são
//! atendidas aqui, na thread da interface, a cada frame, com o estado de
//! sempre da casca: servidores conectados, janelas de consulta, editor e
//! resultados.
//!
//! Ler é livre: o SQL de leitura roda numa conexão própria, dentro de uma
//! transação `READ ONLY`. O que pode alterar alguma coisa entra numa fila e
//! só roda depois do "Run" no diálogo de confirmação, que mostra o SQL
//! inteiro. Escrever no editor não pergunta: o texto entra selecionado e o
//! Ctrl+Z desfaz. Cada chamada deixa uma linha na janela Output.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc;

use eframe::egui;
use pgstudio_automation::collect::{self, Collector, DEFAULT_ROWS, MAX_BYTES, MAX_ROWS};
use pgstudio_automation::safety::read_only_batch;
use pgstudio_automation::{
    Client, EditMode, Inbox, Origin, Pending, Reply, Responder, Safety, ToolCall, catalog, classify,
};
use pgstudio_core::connection::Connected;
use pgstudio_core::query::QueryService;
use serde_json::{Value, json};

use crate::execution::{Execution, MessageKind};
use crate::servers::server_key;
use crate::{Ask, ShellApp, panels};

/// Pedidos atendidos por frame, para um cliente apressado não travar a
/// interface.
const CALLS_PER_FRAME: usize = 32;

/// Estado da ponte dentro da casca.
#[derive(Default)]
pub struct Bridge {
    inbox: Option<Inbox>,
    /// SQL que altera alguma coisa, esperando o usuário (o primeiro está no
    /// diálogo).
    confirms: VecDeque<Confirmation>,
    /// Janelas de consulta executando a pedido do Claude.
    waits: Vec<WindowWait>,
    /// Fim das consultas em conexão própria, para a janela Output.
    done_tx: Option<mpsc::Sender<String>>,
    done_rx: Option<mpsc::Receiver<String>>,
}

impl Bridge {
    /// Se o diálogo de confirmação está aberto.
    pub fn is_confirming(&self) -> bool {
        !self.confirms.is_empty()
    }
}

/// SQL esperando a confirmação.
struct Confirmation {
    origin: Origin,
    sql: String,
    safety: Safety,
    /// "servidor, banco" ou a janela de consulta.
    target: String,
    action: Confirmed,
}

enum Confirmed {
    /// Rodar numa conexão própria.
    Sql {
        queries: Arc<dyn QueryService>,
        database: String,
        max_rows: usize,
        responder: Responder,
    },
    /// Executar a janela de consulta (F5).
    Window {
        doc: u32,
        max_rows: usize,
        responder: Responder,
    },
}

impl Confirmed {
    fn responder(&self) -> &Responder {
        match self {
            Confirmed::Sql { responder, .. } | Confirmed::Window { responder, .. } => responder,
        }
    }
}

/// Uma janela de consulta executando para o Claude.
struct WindowWait {
    doc: u32,
    max_rows: usize,
    responder: Responder,
    origin: Origin,
    /// Já foi vista executando (o "Include Actual Execution Plan" pode
    /// fazer outra pergunta antes de começar).
    armed: bool,
}

/// Servidor e banco onde uma ferramenta roda.
struct Target {
    queries: Arc<dyn QueryService>,
    key: String,
    database: String,
}

impl ShellApp {
    /// Liga a ponte e devolve o lado de quem chama (o servidor MCP, o
    /// painel e os testes). Uma chamada nova troca a anterior.
    pub fn automation(&mut self, ctx: &egui::Context) -> Client {
        let repaint = ctx.clone();
        let (client, inbox) =
            pgstudio_automation::hub::channel(Arc::new(move || repaint.request_repaint()));
        let (tx, rx) = mpsc::channel();
        self.claude.inbox = Some(inbox);
        self.claude.done_tx = Some(tx);
        self.claude.done_rx = Some(rx);
        client
    }

    /// SQL e motivo do diálogo de confirmação aberto (testes).
    pub fn claude_confirmation(&self) -> Option<(&str, String)> {
        self.claude
            .confirms
            .front()
            .map(|c| (c.sql.as_str(), c.safety.describe()))
    }

    /// Responde o diálogo de confirmação aberto, como os botões "Run" e
    /// "Don't Run".
    pub fn answer_claude_confirmation(&mut self, ctx: &egui::Context, run: bool) {
        let Some(c) = self.claude.confirms.pop_front() else {
            return;
        };
        if !run {
            self.output.push(format!(
                "{}: you declined to run the SQL.",
                c.origin.label()
            ));
            c.action.responder().send(Reply::error(
                "The user declined to run this SQL in PGStudio.",
            ));
            return;
        }
        self.output.push(format!(
            "{}: you allowed: {}",
            c.origin.label(),
            one_line(&c.sql)
        ));
        match c.action {
            Confirmed::Sql {
                queries,
                database,
                max_rows,
                responder,
            } => {
                let collector = Collector::new(max_rows);
                self.claude_run_sql(
                    ctx,
                    c.origin,
                    "run_query",
                    &queries,
                    &database,
                    c.sql,
                    collector,
                    responder,
                );
            }
            Confirmed::Window {
                doc,
                max_rows,
                responder,
            } => self.claude_start_window(ctx, doc, max_rows, responder, c.origin),
        }
    }

    /// Uma vez por frame: atende os pedidos novos e acompanha as janelas
    /// executando.
    pub(crate) fn claude_step(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.claude.done_rx {
            while let Ok(line) = rx.try_recv() {
                self.output.push(line);
            }
        }
        for _ in 0..CALLS_PER_FRAME {
            let Some(pending) = self.claude.inbox.as_ref().and_then(Inbox::try_next) else {
                break;
            };
            self.claude_handle(ctx, pending);
        }
        self.claude_poll_windows();
    }

    fn claude_handle(&mut self, ctx: &egui::Context, pending: Pending) {
        let Pending {
            tool,
            args,
            origin,
            responder,
        } = pending;
        let call = match ToolCall::parse(&tool, &args) {
            Ok(c) => c,
            Err(e) => {
                responder.send(Reply::error(e));
                return;
            }
        };
        let who = origin.label();
        match call {
            ToolCall::ListServers => {
                self.output.push(format!("{who}: list_servers"));
                responder.send(self.claude_list_servers());
            }
            ToolCall::ListDatabases { server } => self.claude_catalog(
                ctx,
                origin,
                "list_databases",
                server,
                None,
                responder,
                |_| (catalog::list_databases(), &[]),
            ),
            ToolCall::ListObjects {
                server,
                database,
                schema,
            } => self.claude_catalog(
                ctx,
                origin,
                "list_objects",
                server,
                database,
                responder,
                move |_| (catalog::list_objects(schema.as_deref()), &[]),
            ),
            ToolCall::DescribeTable {
                table,
                server,
                database,
            } => self.claude_catalog(
                ctx,
                origin,
                "describe_table",
                server,
                database,
                responder,
                move |_| {
                    (
                        catalog::describe_table(&table),
                        catalog::DESCRIBE_TABLE_LABELS,
                    )
                },
            ),
            ToolCall::DescribeFunction {
                function,
                server,
                database,
            } => self.claude_catalog(
                ctx,
                origin,
                "describe_function",
                server,
                database,
                responder,
                move |_| (catalog::describe_function(&function), &[]),
            ),
            ToolCall::RunQuery {
                sql,
                server,
                database,
                max_rows,
            } => {
                let target = match self.claude_target(server.as_deref(), database) {
                    Ok(t) => t,
                    Err(e) => return responder.send(Reply::error(e)),
                };
                let max_rows = max_rows.unwrap_or(DEFAULT_ROWS);
                let safety = classify(&sql);
                if safety.is_read_only() {
                    let collector = Collector::new(max_rows).wrapped();
                    let batch = read_only_batch(&sql);
                    self.output
                        .push(format!("{who}: run_query (read only): {}", one_line(&sql)));
                    self.claude_run_sql(
                        ctx,
                        origin,
                        "run_query",
                        &target.queries,
                        &target.database,
                        batch,
                        collector,
                        responder,
                    );
                } else {
                    self.output.push(format!(
                        "{who}: run_query needs your confirmation: {}",
                        one_line(&sql)
                    ));
                    self.claude.confirms.push_back(Confirmation {
                        origin,
                        sql,
                        safety,
                        target: format!("{}, database {}", target.key, target.database),
                        action: Confirmed::Sql {
                            queries: target.queries,
                            database: target.database,
                            max_rows,
                            responder,
                        },
                    });
                }
            }
            ToolCall::ListQueryWindows => {
                self.output.push(format!("{who}: list_query_windows"));
                responder.send(self.claude_list_windows());
            }
            ToolCall::GetQueryWindow { window } => {
                self.output.push(format!("{who}: get_query_window"));
                responder.send(match self.claude_window(window) {
                    Ok(id) => self.claude_read_window(ctx, id),
                    Err(e) => Reply::error(e),
                });
            }
            ToolCall::EditQueryWindow { text, mode, window } => {
                let reply = match self.claude_window(window) {
                    Ok(id) => {
                        let title = self
                            .documents
                            .get(id)
                            .map(|d| d.name().to_string())
                            .unwrap_or_default();
                        self.output
                            .push(format!("{who}: edited {title} (Ctrl+Z undoes it)"));
                        self.claude_edit_window(ctx, id, &text, mode)
                    }
                    Err(e) => Reply::error(e),
                };
                responder.send(reply);
            }
            ToolCall::OpenQueryWindow {
                text,
                server,
                database,
            } => {
                let reply = match self.claude_target(server.as_deref(), database) {
                    Ok(t) => {
                        let id = self
                            .documents
                            .new_script(text.unwrap_or_default(), Some(t.database.clone()));
                        self.doc_servers.insert(id, t.key.clone());
                        self.activate_document(id);
                        let title = self
                            .documents
                            .get(id)
                            .map(|d| d.name().to_string())
                            .unwrap_or_default();
                        self.output.push(format!("{who}: opened {title}"));
                        Reply::ok(&json!({
                            "window": id,
                            "title": title,
                            "server": t.key,
                            "database": t.database,
                        }))
                    }
                    Err(e) => Reply::error(e),
                };
                responder.send(reply);
            }
            ToolCall::ExecuteQueryWindow { window, max_rows } => {
                let id = match self.claude_window(window) {
                    Ok(id) => id,
                    Err(e) => return responder.send(Reply::error(e)),
                };
                let max_rows = max_rows.unwrap_or(DEFAULT_ROWS);
                let Some(doc) = self.documents.get(id) else {
                    return responder.send(Reply::error("The query window was closed."));
                };
                if doc.execution.as_ref().is_some_and(Execution::is_running) {
                    return responder.send(Reply::error(
                        "That query window is already executing. Wait for it, or use get_query_results later.",
                    ));
                }
                let sql = pgstudio_sql_editor::selected_text(ctx, panels::editor_id(id), &doc.text)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| doc.text.clone());
                if sql.trim().is_empty() {
                    return responder.send(Reply::error("The query window is empty."));
                }
                let safety = classify(&sql);
                if safety.is_read_only() {
                    self.output
                        .push(format!("{who}: executing {} (read only)", doc.name()));
                    self.claude_start_window(ctx, id, max_rows, responder, origin);
                } else {
                    let target = format!(
                        "query window {} ({})",
                        doc.name(),
                        self.document_servers()
                            .get(&id)
                            .cloned()
                            .unwrap_or_default()
                    );
                    self.output.push(format!(
                        "{who}: executing {} needs your confirmation",
                        doc.name()
                    ));
                    self.claude.confirms.push_back(Confirmation {
                        origin,
                        sql,
                        safety,
                        target,
                        action: Confirmed::Window {
                            doc: id,
                            max_rows,
                            responder,
                        },
                    });
                }
            }
            ToolCall::GetQueryResults { window, max_rows } => {
                self.output.push(format!("{who}: get_query_results"));
                let reply = match self.claude_window(window) {
                    Ok(id) => match self.documents.get(id).and_then(|d| d.execution.as_ref()) {
                        Some(exec) => {
                            Reply::ok(&execution_json(exec, max_rows.unwrap_or(DEFAULT_ROWS)))
                        }
                        None => Reply::error("That query window has not been executed yet."),
                    },
                    Err(e) => Reply::error(e),
                };
                responder.send(reply);
            }
        }
    }

    /// Ferramentas de catálogo: SQL fixo, só leitura, sem perguntar.
    #[allow(clippy::too_many_arguments)]
    fn claude_catalog(
        &mut self,
        ctx: &egui::Context,
        origin: Origin,
        tool: &'static str,
        server: Option<String>,
        database: Option<String>,
        responder: Responder,
        sql: impl FnOnce(&Target) -> (String, &'static [&'static str]),
    ) {
        let target = match self.claude_target(server.as_deref(), database) {
            Ok(t) => t,
            Err(e) => return responder.send(Reply::error(e)),
        };
        let (sql, labels) = sql(&target);
        self.output.push(format!(
            "{}: {tool} ({}, {})",
            origin.label(),
            target.key,
            target.database
        ));
        let collector = Collector::new(MAX_ROWS).wrapped().labels(labels);
        self.claude_run_sql(
            ctx,
            origin,
            tool,
            &target.queries,
            &target.database,
            read_only_batch(&sql),
            collector,
            responder,
        );
    }

    /// Roda `batch` numa conexão própria e responde no fim, da thread do
    /// banco.
    #[allow(clippy::too_many_arguments)]
    fn claude_run_sql(
        &mut self,
        ctx: &egui::Context,
        origin: Origin,
        tool: &'static str,
        queries: &Arc<dyn QueryService>,
        database: &str,
        batch: String,
        mut collector: Collector,
        responder: Responder,
    ) {
        let session = queries.open(database);
        // A sessão vive até o fim da execução; depois a conexão fecha (e
        // uma transação abortada vai junto).
        let keep = session.clone();
        let done = self.claude.done_tx.clone();
        let repaint = ctx.clone();
        let mut keep = Some(keep);
        session.execute(
            batch,
            Box::new(move |event| {
                collector.push(event);
                if collector.is_finished() && keep.take().is_some() {
                    let reply = Reply {
                        text: serde_json::to_string_pretty(&collector.to_json())
                            .unwrap_or_default(),
                        is_error: collector.failed(),
                    };
                    if let Some(done) = &done {
                        let _ = done.send(format!(
                            "{}: {tool} finished: {}",
                            origin.label(),
                            collector.summary()
                        ));
                    }
                    responder.send(reply);
                    repaint.request_repaint();
                }
            }),
        );
    }

    /// F5 na janela `id` para o Claude; a resposta vai no fim.
    fn claude_start_window(
        &mut self,
        ctx: &egui::Context,
        id: u32,
        max_rows: usize,
        responder: Responder,
        origin: Origin,
    ) {
        if self.documents.get(id).is_none() {
            return responder.send(Reply::error("The query window was closed."));
        }
        self.activate_document(id);
        self.execute_query(ctx, Ask::Run);
        let running = self
            .documents
            .get(id)
            .and_then(|d| d.execution.as_ref())
            .is_some_and(Execution::is_running);
        if !running && self.actual_confirm.is_none() {
            let status = self.status.clone();
            return responder.send(Reply::error(format!(
                "The query window was not executed: {status}"
            )));
        }
        self.claude.waits.push(WindowWait {
            doc: id,
            max_rows,
            responder,
            origin,
            armed: running,
        });
    }

    fn claude_poll_windows(&mut self) {
        let mut i = 0;
        while i < self.claude.waits.len() {
            let w = &mut self.claude.waits[i];
            let exec = self.documents.get(w.doc).and_then(|d| d.execution.as_ref());
            let running = exec.is_some_and(Execution::is_running);
            if running {
                w.armed = true;
                i += 1;
                continue;
            }
            if !w.armed && self.actual_confirm.is_some() {
                i += 1;
                continue;
            }
            let w = self.claude.waits.remove(i);
            let reply = match exec {
                None => Reply::error("The query window was closed."),
                Some(_) if !w.armed => Reply::error("The user did not execute the query window."),
                Some(exec) => {
                    let v = execution_json(exec, w.max_rows);
                    self.output.push(format!(
                        "{}: execution finished: {}",
                        w.origin.label(),
                        exec.outcome().label()
                    ));
                    Reply {
                        is_error: v.get("error").is_some(),
                        text: serde_json::to_string_pretty(&v).unwrap_or_default(),
                    }
                }
            };
            w.responder.send(reply);
        }
    }

    /// Diálogo de confirmação do SQL que altera alguma coisa.
    pub(crate) fn claude_confirm_window(&mut self, ctx: &egui::Context) {
        let Some(c) = self.claude.confirms.front() else {
            return;
        };
        let mut answer = None;
        let pending = self.claude.confirms.len();
        let modal = egui::Modal::new(egui::Id::new("claude_confirm")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(format!("{} wants to run SQL", c.origin.label()));
            ui.label(c.safety.describe());
            ui.label(format!("Where: {}", c.target));
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    egui::ScrollArea::both()
                        .max_height(280.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&c.sql).monospace())
                                    .selectable(true)
                                    .extend(),
                            );
                        });
                });
            if pending > 1 {
                ui.label(format!("{} more request(s) waiting.", pending - 1));
            }
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Don't Run").clicked() {
                    answer = Some(false);
                }
                if ui.button("Run").clicked() {
                    answer = Some(true);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(run) = answer {
            self.answer_claude_confirmation(ctx, run);
        }
    }

    /// Conexões abertas, a ativa primeiro.
    fn claude_connections(&self) -> Vec<(&Connected, bool)> {
        self.connection
            .iter()
            .map(|c| (c, true))
            .chain(
                self.servers
                    .parked
                    .iter()
                    .filter_map(|p| p.connection.as_ref())
                    .map(|c| (c, false)),
            )
            .collect()
    }

    fn claude_list_servers(&self) -> Reply {
        let servers: Vec<Value> = self
            .claude_connections()
            .into_iter()
            .map(|(c, active)| {
                json!({
                    "server": server_key(&c.settings),
                    "name": c.settings.server_name(),
                    "user": c.settings.user,
                    "default_database": c.settings.database,
                    "postgres_version": c.server_version,
                    "active": active,
                })
            })
            .collect();
        if servers.is_empty() {
            return Reply::error(NOT_CONNECTED);
        }
        Reply::ok(&json!({ "servers": servers }))
    }

    /// Servidor e banco de uma ferramenta: os pedidos, ou os da janela
    /// ativa, ou os do servidor ativo.
    fn claude_target(
        &self,
        server: Option<&str>,
        database: Option<String>,
    ) -> Result<Target, String> {
        let connections = self.claude_connections();
        if connections.is_empty() {
            return Err(NOT_CONNECTED.into());
        }
        let window_key = self
            .active_doc
            .and_then(|id| self.doc_servers.get(&id))
            .cloned();
        let (connection, from_window) = match server {
            Some(wanted) => {
                let wanted = wanted.trim();
                let found = connections.iter().find(|(c, _)| {
                    server_key(&c.settings) == wanted
                        || c.settings.server_name().eq_ignore_ascii_case(wanted)
                        || c.settings.host.eq_ignore_ascii_case(wanted)
                });
                match found {
                    Some((c, _)) => {
                        let key = server_key(&c.settings);
                        (*c, window_key.as_deref() == Some(key.as_str()))
                    }
                    None => {
                        let known: Vec<String> = connections
                            .iter()
                            .map(|(c, _)| server_key(&c.settings))
                            .collect();
                        return Err(format!(
                            "Server \"{wanted}\" is not connected in PGStudio. Connected: {}.",
                            known.join(", ")
                        ));
                    }
                }
            }
            None => {
                let by_window = window_key.as_deref().and_then(|k| {
                    connections
                        .iter()
                        .find(|(c, _)| server_key(&c.settings) == k)
                });
                match by_window {
                    Some((c, _)) => (*c, true),
                    None => (connections[0].0, false),
                }
            }
        };
        let database = match database {
            Some(db) => db,
            None if from_window => self
                .active_doc
                .and_then(|id| self.document_database(id))
                .unwrap_or_else(|| connection.settings.database.clone()),
            None => connection.settings.database.clone(),
        };
        Ok(Target {
            queries: connection.queries.clone(),
            key: server_key(&connection.settings),
            database,
        })
    }

    /// A janela pedida, ou a ativa.
    fn claude_window(&self, window: Option<u32>) -> Result<u32, String> {
        match window.or(self.active_doc) {
            Some(id) if self.documents.get(id).is_some() => Ok(id),
            Some(id) => Err(format!(
                "There is no query window {id}. Use list_query_windows."
            )),
            None => Err("No query window is open. Use open_query_window to open one.".into()),
        }
    }

    fn claude_list_windows(&self) -> Reply {
        let labels = self.document_servers();
        let windows: Vec<Value> = self
            .documents
            .ids()
            .into_iter()
            .filter_map(|id| {
                let d = self.documents.get(id)?;
                Some(json!({
                    "window": id,
                    "title": d.name(),
                    "file": d.path().map(|p| p.display().to_string()),
                    "server": self.doc_servers.get(&id),
                    "server_label": labels.get(&id),
                    "database": self.document_database(id),
                    "unsaved_changes": d.is_dirty(),
                    "executing": d.execution.as_ref().is_some_and(Execution::is_running),
                    "active": self.active_doc == Some(id),
                }))
            })
            .collect();
        Reply::ok(&json!({ "windows": windows }))
    }

    fn claude_read_window(&self, ctx: &egui::Context, id: u32) -> Reply {
        let Some(d) = self.documents.get(id) else {
            return Reply::error("The query window was closed.");
        };
        let selection =
            pgstudio_sql_editor::selection(ctx, panels::editor_id(id)).and_then(|(a, b)| {
                let (a, b) = (a.min(b), a.max(b));
                (a != b).then(|| {
                    json!({
                        "start": a,
                        "end": b,
                        "text": d.text.chars().skip(a).take(b - a).collect::<String>(),
                    })
                })
            });
        let last = d.execution.as_ref().map(|e| {
            json!({
                "outcome": e.outcome().label(),
                "result_sets": e.results.len(),
                "rows": e.total_rows(),
                "errors": e.messages.iter().filter(|m| m.kind == MessageKind::Error).map(|m| m.text.clone()).collect::<Vec<_>>(),
            })
        });
        Reply::ok(&json!({
            "window": id,
            "title": d.name(),
            "server": self.doc_servers.get(&id),
            "database": self.document_database(id),
            "text": d.text,
            "selection": selection,
            "cursor": d.cursor.map(|(line, col)| json!({ "line": line, "column": col })),
            "last_execution": last,
        }))
    }

    fn claude_edit_window(
        &mut self,
        ctx: &egui::Context,
        id: u32,
        text: &str,
        mode: EditMode,
    ) -> Reply {
        let editor = panels::editor_id(id);
        let selection = pgstudio_sql_editor::selection(ctx, editor);
        let Some(doc) = self.documents.get_mut(id) else {
            return Reply::error("The query window was closed.");
        };
        let len = doc.text.chars().count();
        let (start, end, insert) = match mode {
            EditMode::ReplaceAll => (0, len, text.to_string()),
            EditMode::ReplaceSelection => match selection {
                Some((a, b)) => (a.min(b).min(len), a.max(b).min(len), text.to_string()),
                None => (0, 0, text.to_string()),
            },
            EditMode::InsertAtCursor => {
                let at = selection.map_or(0, |(_, c)| c.min(len));
                (at, at, text.to_string())
            }
            EditMode::Append => {
                let sep = if doc.text.is_empty() || doc.text.ends_with('\n') {
                    ""
                } else {
                    "\n"
                };
                (len, len, format!("{sep}{text}"))
            }
        };
        let a = pgstudio_sql_editor::edit::byte_of(&doc.text, start);
        let b = pgstudio_sql_editor::edit::byte_of(&doc.text, end);
        doc.text.replace_range(a..b, &insert);
        let inserted = insert.chars().count();
        // O texto novo fica selecionado, para o usuário ver o que mudou.
        pgstudio_sql_editor::set_selection(ctx, editor, (start, start + inserted));
        let total = doc.text.chars().count();
        self.activate_document(id);
        Reply::ok(&json!({
            "window": id,
            "selection": { "start": start, "end": start + inserted },
            "length": total,
        }))
    }
}

const NOT_CONNECTED: &str = "No server is connected in PGStudio. Ask the user to connect \
(File > Connect Object Explorer, or the Registered Servers window).";

/// SQL numa linha só, cortado, para a janela Output.
fn one_line(sql: &str) -> String {
    let flat: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 120 {
        format!("{}…", flat.chars().take(120).collect::<String>())
    } else {
        flat
    }
}

/// Resultados e mensagens de uma execução de janela, com os mesmos
/// limites do `run_query`.
fn execution_json(exec: &Execution, max_rows: usize) -> Value {
    let max_rows = max_rows.clamp(1, MAX_ROWS);
    let mut bytes = 0;
    let results: Vec<Value> = exec
        .results
        .iter()
        .map(|set| {
            let total = set.len();
            let mut rows = Vec::new();
            let mut truncated = false;
            for r in 0..total {
                if rows.len() >= max_rows || bytes >= MAX_BYTES {
                    truncated = true;
                    break;
                }
                let row: Vec<Option<String>> = set
                    .row(r)
                    .into_iter()
                    .map(|v| v.map(|s| cut_cell(&s)))
                    .collect();
                bytes += row
                    .iter()
                    .map(|v| v.as_ref().map_or(4, |s| s.len()) + 3)
                    .sum::<usize>();
                rows.push(row);
            }
            let mut v = json!({ "columns": set.columns(), "rows": rows, "row_count": total });
            if truncated {
                v["truncated"] = json!(true);
            }
            v
        })
        .collect();
    let messages: Vec<String> = exec
        .messages
        .iter()
        .filter(|m| m.kind != MessageKind::Error)
        .map(|m| m.text.clone())
        .collect();
    let errors: Vec<String> = exec
        .messages
        .iter()
        .filter(|m| m.kind == MessageKind::Error)
        .map(|m| m.text.clone())
        .collect();
    let mut v = json!({
        "outcome": exec.outcome().label(),
        "database": exec.database,
        "elapsed_ms": exec.elapsed().as_millis() as u64,
        "results": results,
        "messages": messages,
    });
    if !errors.is_empty() {
        v["error"] = json!(errors.join("\n"));
    }
    if !exec.plans.is_empty() {
        v["plans"] = json!(exec.plans.len());
    }
    v
}

fn cut_cell(s: &str) -> String {
    if s.len() <= collect::MAX_CELL {
        return s.to_string();
    }
    let mut end = collect::MAX_CELL;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
