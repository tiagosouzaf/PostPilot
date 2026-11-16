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

use pgstudio_i18n::{tr, trf};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use eframe::egui;
use pgstudio_automation::collect::{self, Collector, DEFAULT_ROWS, MAX_BYTES, MAX_ROWS};
use pgstudio_automation::endpoint::Listener;
use pgstudio_automation::safety::{classify_for, read_only_batch_for};
use pgstudio_automation::{
    Client, EditMode, Inbox, Origin, Pending, Reply, Responder, ToolCall, catalog,
};
use pgstudio_core::connection::{Connected, Engine};
use pgstudio_core::query::QueryService;
use serde_json::{Value, json};

use crate::execution::{Execution, MessageKind};
use crate::registered::{AiAccess, Rgb, ServerTag};
use crate::servers::server_key;
use crate::{Ask, ShellApp, panels};

mod insight;

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
    /// Lado de quem chama, criado uma vez e dividido entre o MCP e o
    /// painel.
    client: Option<Client>,
    /// "Allow Claude Code (MCP)" ligado: o canal local escutando.
    listener: Option<Listener>,
    /// A execução de janela em andamento já foi confirmada no diálogo do
    /// Claude.
    confirmed_run: bool,
    /// Arquivo do canal local (`mcp-endpoint.json`).
    endpoint: Option<PathBuf>,
    /// Onde a escolha "Allow Claude Code" fica salva (`claude.json`).
    settings: Option<PathBuf>,
}

impl Bridge {
    /// Se o diálogo de confirmação está aberto.
    pub fn is_confirming(&self) -> bool {
        !self.confirms.is_empty()
    }

    /// Se o usuário já confirmou, no diálogo do Claude, a execução de
    /// janela que está começando.
    pub(crate) fn confirmed_run(&self) -> bool {
        self.confirmed_run
    }

    /// Se o Claude Code pode conectar (o canal local está aberto).
    pub fn is_listening(&self) -> bool {
        self.listener.is_some()
    }
}

/// O que fica em `claude.json`.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Settings {
    #[serde(default)]
    allow_claude_code: bool,
}

/// Um pedido esperando a confirmação.
struct Confirmation {
    origin: Origin,
    /// Título do diálogo, como "Claude wants to run SQL".
    title: String,
    /// O SQL (ou, na atividade, o que vai ser lido), mostrado inteiro.
    sql: String,
    /// Por que pede confirmação.
    reason: String,
    /// "servidor, banco" ou a janela de consulta.
    target: String,
    /// Ambiente do servidor (nome e cor), para a etiqueta do diálogo.
    environment: Option<(String, Rgb)>,
    responder: Responder,
    action: Confirmed,
}

enum Confirmed {
    /// Rodar numa conexão própria. `read_only`: o lote só lê e roda no
    /// embrulho de leitura.
    Sql {
        target: Target,
        max_rows: usize,
        read_only: bool,
    },
    /// Executar a janela de consulta (F5).
    Window { doc: u32, max_rows: usize },
    /// Plano real: executa o comando numa transação desfeita.
    Plan { target: Target, read_only: bool },
    /// Ler a atividade do servidor.
    Activity {
        target: Target,
        include_idle: bool,
        top_statements: bool,
    },
}

/// O que uma ferramenta faz no servidor, para a regra do ambiente.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Touch {
    /// Lê o esquema ou pede um plano estimado: nada roda, nenhum dado sai.
    Schema,
    /// Roda SQL ou traz dados (linhas, consultas das outras sessões).
    Data,
}

/// O que a regra do ambiente manda fazer com um pedido.
enum Gate {
    /// Segue a regra de sempre.
    Open,
    /// Pede confirmação mesmo sendo leitura.
    Ask,
    /// Recusa; o texto vai para o modelo.
    Blocked(String),
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
    /// O motor do servidor: decide o SQL do catálogo e do embrulho de
    /// leitura.
    engine: Engine,
    /// Ambiente e regra da IA do servidor, se está registrado.
    tag: Option<ServerTag>,
}

impl Target {
    /// "servidor, banco", para o diálogo.
    fn label(&self) -> String {
        trf!(
            "{server}, database {database}",
            server = self.key,
            database = self.database
        )
    }
}

/// Nome e cor do ambiente, para a etiqueta.
fn badge(tag: Option<&ServerTag>) -> Option<(String, Rgb)> {
    tag.and_then(|t| Some((t.environment.clone()?, t.color)))
}

/// A regra do ambiente para um pedido.
fn gate(tag: Option<&ServerTag>, touch: Touch) -> Gate {
    let Some(tag) = tag else {
        return Gate::Open;
    };
    match (tag.ai, touch) {
        (AiAccess::Free, _) | (AiAccess::Confirm, Touch::Schema) => Gate::Open,
        (AiAccess::Confirm, Touch::Data) => Gate::Ask,
        (AiAccess::Blocked, _) => Gate::Blocked(format!(
            "The user has blocked AI access to servers in the {} environment in PostPilot. \
             Do not retry on this server or work around it; tell the user. They can change it \
             in the server list: Environments.",
            tag.environment.as_deref().unwrap_or("this")
        )),
    }
}

/// Frase do diálogo quando o que pede confirmação é o ambiente.
fn environment_reason(tag: Option<&ServerTag>, what: &str) -> String {
    let environment = tag.and_then(|t| t.environment.clone()).unwrap_or_default();
    trf!(
        "{what} Servers in {environment} ask before the AI runs anything.",
        what = what,
        environment = environment
    )
}

impl ShellApp {
    /// Liga a ponte e devolve o lado de quem chama (o servidor MCP, o
    /// painel e os testes). Uma chamada nova troca a anterior.
    pub fn automation(&mut self, ctx: &egui::Context) -> Client {
        if let Some(client) = &self.claude.client {
            return client.clone();
        }
        let repaint = ctx.clone();
        let (client, inbox) =
            pgstudio_automation::hub::channel(Arc::new(move || repaint.request_repaint()));
        let (tx, rx) = mpsc::channel();
        self.claude.inbox = Some(inbox);
        self.claude.done_tx = Some(tx);
        self.claude.done_rx = Some(rx);
        self.claude.client = Some(client.clone());
        client
    }

    /// Usa estes arquivos para o canal do `postpilot mcp` e para a escolha
    /// "Allow Claude Code (MCP)"; se ela estava ligada, abre o canal.
    pub fn with_claude_code(
        mut self,
        ctx: &egui::Context,
        endpoint: PathBuf,
        settings: PathBuf,
    ) -> Self {
        let saved: Settings = std::fs::read_to_string(&settings)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        self.claude.endpoint = Some(endpoint);
        self.claude.settings = Some(settings);
        if saved.allow_claude_code {
            self.start_claude_code(ctx);
        }
        self
    }

    /// Tools > "Allow Claude Code (MCP)": abre ou fecha o canal local e
    /// guarda a escolha.
    pub fn set_claude_code_access(&mut self, ctx: &egui::Context, on: bool) {
        if on {
            self.start_claude_code(ctx);
        } else if self.claude.listener.take().is_some() {
            self.status = tr("Claude Code can no longer reach PostPilot.").to_string();
            self.output
                .push(tr("Claude Code: access turned off.").to_string());
        }
        if let Some(path) = &self.claude.settings {
            let text = serde_json::to_string_pretty(&Settings {
                allow_claude_code: self.claude.listener.is_some(),
            })
            .unwrap_or_default();
            let saved = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|_| std::fs::write(path, text));
            if let Err(e) = saved {
                eprintln!("claude: could not save {}: {e}", path.display());
            }
        }
    }

    fn start_claude_code(&mut self, ctx: &egui::Context) {
        if self.claude.listener.is_some() {
            return;
        }
        let Some(path) = self.claude.endpoint.clone() else {
            self.status = tr("Claude Code access is not available in this mode.").to_string();
            return;
        };
        let client = self.automation(ctx);
        match Listener::start(client, path) {
            Ok(l) => {
                self.output.push(trf!(
                    "Claude Code: access on. Add it once with: claude mcp add postpilot -- postpilot mcp (channel {channel})",
                    channel = l.path().display()
                ));
                self.status = tr("Claude Code can now use PostPilot.").to_string();
                self.claude.listener = Some(l);
            }
            Err(e) => {
                let text = trf!("Could not turn on Claude Code access: {e}", e = e);
                self.output.push(text.clone());
                self.status = text;
            }
        }
    }

    /// Canal local do Claude Code, se ligado (testes).
    pub fn claude_code_listener(&self) -> Option<&Listener> {
        self.claude.listener.as_ref()
    }

    /// Indicador na status bar enquanto o Claude Code pode conectar.
    pub(crate) fn claude_status(&self, ui: &mut egui::Ui, color: egui::Color32) {
        let Some(l) = &self.claude.listener else {
            return;
        };
        ui.separator();
        let busy = l.running() > 0 || self.claude.is_confirming();
        let text = if busy {
            tr("Claude Code: working")
        } else {
            tr("Claude Code: on")
        };
        ui.label(egui::RichText::new(text).color(color))
            .on_hover_text(trf!(
                "Claude Code can use PostPilot through `postpilot mcp`. {calls} call(s) so far. \
             Turn it off in Tools > Allow Claude Code (MCP).",
                calls = l.calls()
            ));
    }

    /// SQL e motivo do diálogo de confirmação aberto (testes).
    pub fn claude_confirmation(&self) -> Option<(&str, String)> {
        self.claude
            .confirms
            .front()
            .map(|c| (c.sql.as_str(), c.reason.clone()))
    }

    /// Ambiente mostrado no diálogo de confirmação aberto (testes).
    pub fn claude_confirmation_environment(&self) -> Option<&str> {
        let c = self.claude.confirms.front()?;
        c.environment.as_ref().map(|(name, _)| name.as_str())
    }

    /// Responde o diálogo de confirmação aberto, como os botões "Run" e
    /// "Don't Run".
    pub fn answer_claude_confirmation(&mut self, ctx: &egui::Context, run: bool) {
        let Some(c) = self.claude.confirms.pop_front() else {
            return;
        };
        if !run {
            self.output.push(trf!(
                "{who}: you declined to run the SQL.",
                who = c.origin.label()
            ));
            c.responder.send(Reply::error(
                "The user declined to run this in PostPilot. Do not retry it.",
            ));
            return;
        }
        let responder = c.responder;
        self.output.push(trf!(
            "{who}: you allowed: {sql}",
            who = c.origin.label(),
            sql = one_line(&c.sql)
        ));
        match c.action {
            Confirmed::Sql {
                target,
                max_rows,
                read_only,
            } => {
                let (collector, batch) = if read_only {
                    (
                        Collector::new(max_rows).wrapped_for(target.engine),
                        read_only_batch_for(target.engine, &c.sql),
                    )
                } else {
                    (Collector::new(max_rows), c.sql)
                };
                self.claude_run_sql(
                    ctx,
                    c.origin,
                    "run_query",
                    &target.queries,
                    &target.database,
                    batch,
                    collector,
                    responder,
                );
            }
            Confirmed::Window { doc, max_rows } => {
                self.claude_start_window(ctx, doc, max_rows, responder, c.origin, true)
            }
            Confirmed::Plan { target, read_only } => {
                self.claude_run_plan(ctx, c.origin, &target, &c.sql, true, read_only, responder)
            }
            Confirmed::Activity {
                target,
                include_idle,
                top_statements,
            } => self.claude_run_activity(
                ctx,
                c.origin,
                &target,
                include_idle,
                top_statements,
                responder,
            ),
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
                |t| match t.engine {
                    Engine::PostgreSql => (catalog::list_databases(), &[]),
                    Engine::SqlServer => (catalog::tsql::list_databases(), &[]),
                },
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
                move |t| match t.engine {
                    Engine::PostgreSql => (catalog::list_objects(schema.as_deref()), &[]),
                    Engine::SqlServer => (catalog::tsql::list_objects(schema.as_deref()), &[]),
                },
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
                move |t| {
                    (
                        match t.engine {
                            Engine::PostgreSql => catalog::describe_table(&table),
                            Engine::SqlServer => catalog::tsql::describe_table(&table),
                        },
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
                move |t| match t.engine {
                    Engine::PostgreSql => (catalog::describe_function(&function), &[]),
                    Engine::SqlServer => (catalog::tsql::describe_function(&function), &[]),
                },
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
                let safety = classify_for(target.engine, &sql);
                let ask = match gate(target.tag.as_ref(), Touch::Data) {
                    Gate::Blocked(e) => return responder.send(Reply::error(e)),
                    Gate::Ask => true,
                    Gate::Open => false,
                };
                if safety.is_read_only() && !ask {
                    let collector = Collector::new(max_rows).wrapped_for(target.engine);
                    let batch = read_only_batch_for(target.engine, &sql);
                    self.output.push(trf!(
                        "{who}: run_query (read only): {sql}",
                        who = who,
                        sql = one_line(&sql)
                    ));
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
                    self.output.push(trf!(
                        "{who}: run_query needs your confirmation: {sql}",
                        who = who,
                        sql = one_line(&sql)
                    ));
                    let read_only = safety.is_read_only();
                    self.claude.confirms.push_back(Confirmation {
                        origin,
                        title: trf!("{who} wants to run SQL", who = who),
                        sql,
                        reason: if read_only {
                            environment_reason(target.tag.as_ref(), &safety.describe())
                        } else {
                            safety.describe()
                        },
                        target: target.label(),
                        environment: badge(target.tag.as_ref()),
                        responder,
                        action: Confirmed::Sql {
                            target,
                            max_rows,
                            read_only,
                        },
                    });
                }
            }
            ToolCall::ExplainQuery {
                sql,
                analyze,
                server,
                database,
            } => {
                let target = match self.claude_target(server.as_deref(), database) {
                    Ok(t) => t,
                    Err(e) => return responder.send(Reply::error(e)),
                };
                // O estimado não roda nada; o real executa o comando.
                let touch = if analyze { Touch::Data } else { Touch::Schema };
                let ask = match gate(target.tag.as_ref(), touch) {
                    Gate::Blocked(e) => return responder.send(Reply::error(e)),
                    Gate::Ask => true,
                    Gate::Open => false,
                };
                let safety = classify_for(target.engine, &sql);
                let read_only = safety.is_read_only();
                if !analyze || (read_only && !ask) {
                    self.output.push(trf!(
                        "{who}: explain_query ({kind}): {sql}",
                        who = who,
                        kind = if analyze { "actual" } else { "estimated" },
                        sql = one_line(&sql)
                    ));
                    self.claude_run_plan(ctx, origin, &target, &sql, analyze, read_only, responder);
                } else {
                    self.output.push(trf!(
                        "{who}: explain_query needs your confirmation: {sql}",
                        who = who,
                        sql = one_line(&sql)
                    ));
                    let what = trf!(
                        "{safety} The actual plan runs the statement to measure it, then rolls it back.",
                        safety = safety.describe()
                    );
                    self.claude.confirms.push_back(Confirmation {
                        origin,
                        title: trf!("{who} wants to run SQL for its actual plan", who = who),
                        sql,
                        reason: if read_only {
                            environment_reason(target.tag.as_ref(), &what)
                        } else {
                            what
                        },
                        target: target.label(),
                        environment: badge(target.tag.as_ref()),
                        responder,
                        action: Confirmed::Plan { target, read_only },
                    });
                }
            }
            ToolCall::GetActivity {
                server,
                include_idle,
                top_statements,
            } => {
                let target = match self.claude_target(server.as_deref(), None) {
                    Ok(t) => t,
                    Err(e) => return responder.send(Reply::error(e)),
                };
                match gate(target.tag.as_ref(), Touch::Data) {
                    Gate::Blocked(e) => responder.send(Reply::error(e)),
                    Gate::Open => self.claude_run_activity(
                        ctx,
                        origin,
                        &target,
                        include_idle,
                        top_statements,
                        responder,
                    ),
                    Gate::Ask => {
                        self.output.push(trf!(
                            "{who}: get_activity needs your confirmation",
                            who = who
                        ));
                        self.claude.confirms.push_back(Confirmation {
                            origin,
                            title: trf!("{who} wants to read the server activity", who = who),
                            sql: tr("Sessions with their current SQL, locks and who is blocking whom, as the Activity Monitor shows.").to_string(),
                            reason: environment_reason(
                                target.tag.as_ref(),
                                tr("This only reads the server activity."),
                            ),
                            target: target.key.clone(),
                            environment: badge(target.tag.as_ref()),
                            responder,
                            action: Confirmed::Activity {
                                target,
                                include_idle,
                                top_statements,
                            },
                        });
                    }
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
                        self.output.push(trf!(
                            "{who}: edited {title} (Ctrl+Z undoes it)",
                            who = who,
                            title = title
                        ));
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
                        self.output
                            .push(trf!("{who}: opened {title}", who = who, title = title));
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
                let safety = classify_for(self.document_engine(id).unwrap_or_default(), &sql);
                let tag = self.claude_window_tag(id);
                let ask = match gate(tag.as_ref(), Touch::Data) {
                    Gate::Blocked(e) => return responder.send(Reply::error(e)),
                    Gate::Ask => true,
                    Gate::Open => false,
                };
                if safety.is_read_only() && !ask {
                    self.output.push(trf!(
                        "{who}: executing {title} (read only)",
                        who = who,
                        title = doc.name()
                    ));
                    self.claude_start_window(ctx, id, max_rows, responder, origin, false);
                } else {
                    let target = trf!(
                        "query window {title} ({server})",
                        title = doc.name(),
                        server = self
                            .document_servers()
                            .get(&id)
                            .cloned()
                            .unwrap_or_default()
                    );
                    self.output.push(trf!(
                        "{who}: executing {title} needs your confirmation",
                        who = who,
                        title = doc.name()
                    ));
                    self.claude.confirms.push_back(Confirmation {
                        origin,
                        title: trf!("{who} wants to run SQL", who = who),
                        sql,
                        reason: if safety.is_read_only() {
                            environment_reason(tag.as_ref(), &safety.describe())
                        } else {
                            safety.describe()
                        },
                        target,
                        environment: badge(tag.as_ref()),
                        responder,
                        action: Confirmed::Window { doc: id, max_rows },
                    });
                }
            }
            ToolCall::GetQueryResults { window, max_rows } => {
                self.output.push(format!("{who}: get_query_results"));
                let blocked = self.claude_window(window).ok().and_then(|id| {
                    match gate(self.claude_window_tag(id).as_ref(), Touch::Data) {
                        Gate::Blocked(e) => Some(e),
                        _ => None,
                    }
                });
                if let Some(e) = blocked {
                    return responder.send(Reply::error(e));
                }
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
        if let Gate::Blocked(e) = gate(target.tag.as_ref(), Touch::Schema) {
            return responder.send(Reply::error(e));
        }
        let (sql, labels) = sql(&target);
        self.output.push(format!(
            "{}: {tool} ({}, {})",
            origin.label(),
            target.key,
            target.database
        ));
        let collector = Collector::new(MAX_ROWS)
            .wrapped_for(target.engine)
            .labels(labels);
        self.claude_run_sql(
            ctx,
            origin,
            tool,
            &target.queries,
            &target.database,
            read_only_batch_for(target.engine, &sql),
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
                        let _ = done.send(trf!(
                            "{who}: {tool} finished: {summary}",
                            who = origin.label(),
                            tool = tool,
                            summary = collector.summary()
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
        confirmed: bool,
    ) {
        if self.documents.get(id).is_none() {
            return responder.send(Reply::error("The query window was closed."));
        }
        self.activate_document(id);
        // Quem já respondeu o diálogo do Claude (que mostra o ambiente) não
        // responde de novo o do ambiente.
        self.claude.confirmed_run = confirmed;
        self.execute_query(ctx, Ask::Run);
        self.claude.confirmed_run = false;
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
                    self.output.push(trf!(
                        "{who}: execution finished: {outcome}",
                        who = w.origin.label(),
                        outcome = exec.outcome().label()
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
        let modal = crate::theme::modal(egui::Id::new("claude_confirm")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(&c.title);
            ui.label(&c.reason);
            ui.horizontal(|ui| {
                ui.label(trf!("Where: {target}", target = c.target));
                if let Some((name, color)) = &c.environment {
                    crate::registered::environment_badge(ui, name, *color);
                }
            });
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(280.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&c.sql).monospace())
                                    .selectable(true)
                                    .wrap(),
                            );
                        });
                });
            if pending > 1 {
                ui.label(trf!("{n} more request(s) waiting.", n = pending - 1));
            }
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Don't Run")).clicked() {
                    answer = Some(false);
                }
                if ui.button(tr("Run")).clicked() {
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
                let tag = self.server_tag(&server_key(&c.settings));
                json!({
                    "environment": tag.as_ref().and_then(|t| t.environment.clone()),
                    "ai_access": tag.as_ref().map_or(AiAccess::Free, |t| t.ai).id(),
                    "server": server_key(&c.settings),
                    "name": c.settings.server_name(),
                    "user": c.settings.user,
                    "default_database": c.settings.database,
                    "engine": match c.settings.engine {
                        Engine::PostgreSql => "postgresql",
                        Engine::SqlServer => "sqlserver",
                    },
                    "version": c.server_version,
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
                            "Server \"{wanted}\" is not connected in PostPilot. Connected: {}.",
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
        let key = server_key(&connection.settings);
        Ok(Target {
            queries: connection.queries.clone(),
            tag: self.server_tag(&key),
            key,
            database,
            engine: connection.settings.engine,
        })
    }

    /// Ambiente e regra da IA do servidor de uma janela de consulta.
    fn claude_window_tag(&self, id: u32) -> Option<ServerTag> {
        let key = self
            .doc_servers
            .get(&id)
            .cloned()
            .or_else(|| self.connection.as_ref().map(|c| server_key(&c.settings)))?;
        self.server_tag(&key)
    }

    /// Pede o plano de `sql` numa conexão própria e responde com o texto
    /// de cada plano.
    #[allow(clippy::too_many_arguments)]
    fn claude_run_plan(
        &mut self,
        ctx: &egui::Context,
        origin: Origin,
        target: &Target,
        sql: &str,
        analyze: bool,
        read_only: bool,
        responder: Responder,
    ) {
        let engine = target.engine;
        let Some(batch) = insight::plan_batch(engine, sql, analyze, read_only) else {
            return responder.send(Reply::error("There is no statement to explain."));
        };
        let statements = insight::statements(sql);
        self.claude_gather(
            ctx,
            &target.queries,
            &target.database,
            batch,
            insight::Gatherer::plans(),
            move |g| {
                let (v, failed) = insight::plan_reply(engine, &statements, analyze, g);
                let summary = match v["error"].as_str() {
                    Some(e) => format!("error: {}", one_line(e)),
                    None => format!("{} plan(s)", v["plans"].as_array().map_or(0, Vec::len)),
                };
                let reply = Reply {
                    text: serde_json::to_string_pretty(&v).unwrap_or_default(),
                    is_error: failed,
                };
                (reply, summary)
            },
            origin,
            "explain_query",
            responder,
        );
    }

    /// Lê a atividade do servidor (as consultas do Activity Monitor) e
    /// responde com sessões, bloqueios e locks.
    fn claude_run_activity(
        &mut self,
        ctx: &egui::Context,
        origin: Origin,
        target: &Target,
        include_idle: bool,
        top_statements: bool,
        responder: Responder,
    ) {
        self.output
            .push(format!("{}: get_activity ({})", origin.label(), target.key));
        let server = target.key.clone();
        self.claude_gather(
            ctx,
            &target.queries,
            &target.database,
            crate::activity::refresh_sql(target.engine, top_statements),
            insight::Gatherer::all(),
            move |g| match insight::activity_reply(g, include_idle, top_statements) {
                Ok(mut v) => {
                    v["server"] = json!(server);
                    let summary = format!(
                        "{} session(s), {} blocked",
                        v["summary"]["sessions"], v["summary"]["blocked"]
                    );
                    (Reply::ok(&v), summary)
                }
                Err(e) => {
                    let summary = format!("error: {}", one_line(&e));
                    (Reply::error(e), summary)
                }
            },
            origin,
            "get_activity",
            responder,
        );
    }

    /// Roda `batch` numa conexão própria, junta os resultados inteiros e
    /// responde com o que `finish` montar (a resposta e o resumo para a
    /// janela Output).
    #[allow(clippy::too_many_arguments)]
    fn claude_gather(
        &mut self,
        ctx: &egui::Context,
        queries: &Arc<dyn QueryService>,
        database: &str,
        batch: String,
        mut gatherer: insight::Gatherer,
        finish: impl FnOnce(&insight::Gatherer) -> (Reply, String) + Send + 'static,
        origin: Origin,
        tool: &'static str,
        responder: Responder,
    ) {
        let session = queries.open(database);
        let done = self.claude.done_tx.clone();
        let repaint = ctx.clone();
        // A sessão vive até o fim da execução; depois a conexão fecha.
        let mut keep = Some((session.clone(), finish));
        session.execute(
            batch,
            Box::new(move |event| {
                gatherer.push(event);
                if gatherer.is_finished()
                    && let Some((_, finish)) = keep.take()
                {
                    let (reply, summary) = finish(&gatherer);
                    if let Some(done) = &done {
                        let _ = done.send(trf!(
                            "{who}: {tool} finished: {summary}",
                            who = origin.label(),
                            tool = tool,
                            summary = summary
                        ));
                    }
                    responder.send(reply);
                    repaint.request_repaint();
                }
            }),
        );
    }

    /// A janela pedida, ou a ativa.
    pub(crate) fn claude_window(&self, window: Option<u32>) -> Result<u32, String> {
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
                "outcome": e.outcome().english_label(),
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

    pub(crate) fn claude_edit_window(
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

const NOT_CONNECTED: &str = "No server is connected in PostPilot. Ask the user to connect \
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
        "outcome": exec.outcome().english_label(),
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
