//! O painel do Claude (marco 7.4): uma tool window de conversa dentro do
//! PostPilot.
//!
//! Por baixo roda o Claude Code instalado na máquina (veja
//! `pgstudio_automation::agent`), com o login dele, e as ferramentas são as
//! mesmas do MCP: o painel abre um canal local só dele e passa ao `claude`
//! o `postpilot mcp` apontando para esse canal. As chamadas aparecem na
//! conversa como cartões, e o que altera dados passa pelo mesmo diálogo de
//! confirmação.
//!
//! Aqui fica o estado e o desenho; quem age (abrir o processo, mandar a
//! mensagem, inserir SQL no editor) é a casca, pelas [`PanelAction`].

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use pgstudio_automation::agent::{self, Agent, AgentEvent};
use pgstudio_automation::chat_agent::{self, ChatAgent};
use pgstudio_automation::codex::{self, CodexAgent};
use pgstudio_automation::endpoint::{self, Listener};
use pgstudio_automation::llm::Backend;
use pgstudio_automation::{EditMode, Origin};
use pgstudio_core::CommandId;
use pgstudio_i18n::{tr, tr_noop, trf};
use serde_json::Value;

/// Linhas de stderr guardadas para explicar uma falha.
const STDERR_LINES: usize = 12;
/// Quanto do resultado de uma ferramenta o cartão mostra.
const CARD_TEXT: usize = 4000;

/// Um item da conversa.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// O que o usuário escreveu.
    User(String),
    /// Resposta do Claude (chega aos poucos).
    Assistant(String),
    /// Uma ferramenta chamada, com a resposta quando chegar.
    Tool {
        id: String,
        name: String,
        input: Value,
        result: Option<(bool, String)>,
    },
    /// Aviso do painel (erro, parada, Claude Code não instalado).
    Note(String),
}

/// O que o painel pede à casca.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelAction {
    Send(String),
    Stop,
    NewChat,
    /// Inserir este SQL na janela de consulta ativa.
    InsertSql(String),
    /// Abrir Tools > Options na página dos provedores de IA.
    Settings,
}

/// Quem responde no painel: o Claude Code instalado ou um provedor por
/// chave de API. Os dois falam por [`AgentEvent`].
#[derive(Debug)]
pub(crate) enum PanelAgent {
    Code(Agent),
    Api(ChatAgent),
    /// O ChatGPT pela conta, pelo Codex instalado.
    Codex(CodexAgent),
}

impl PanelAgent {
    fn try_recv(&self) -> Option<AgentEvent> {
        match self {
            PanelAgent::Code(a) => a.try_recv(),
            PanelAgent::Api(a) => a.try_recv(),
            PanelAgent::Codex(a) => a.try_recv(),
        }
    }

    fn send(&mut self, text: &str) -> std::io::Result<()> {
        match self {
            PanelAgent::Code(a) => a.send(text),
            PanelAgent::Api(a) => a.send(text),
            PanelAgent::Codex(a) => a.send(text),
        }
    }
}

/// Estado do painel.
#[derive(Default)]
pub struct ClaudePanel {
    pub(crate) items: Vec<Item>,
    pub input: String,
    pub(crate) agent: Option<PanelAgent>,
    /// Nome do provedor em uso; vazio com o Claude Code.
    pub(crate) provider: String,
    /// Para onde vai o que o assistente lê (o servidor do provedor).
    pub(crate) provider_host: String,
    /// O provedor é o ChatGPT pela conta (Codex), sem chave de API.
    pub(crate) by_account: bool,
    /// Sessão do Claude Code, para continuar depois de Parar.
    pub(crate) session: Option<String>,
    pub(crate) model: String,
    /// Esperando a vez do Claude acabar.
    pub(crate) busy: bool,
    /// O último item de resposta ainda recebe pedaços.
    streaming: bool,
    pub(crate) stderr: Vec<String>,
    /// Canal local do painel (o `postpilot mcp` do painel fala com ele).
    pub(crate) listener: Option<Listener>,
    /// Programa no lugar do `claude` (testes).
    pub(crate) program: Option<std::path::PathBuf>,
    /// Executável do PostPilot para o `postpilot mcp` (testes usam outro).
    pub(crate) exe: Option<std::path::PathBuf>,
    /// Pasta do canal e do diretório de trabalho do `claude` (padrão: a
    /// pasta de configuração do PostPilot).
    pub(crate) dir: Option<std::path::PathBuf>,
    focus_input: bool,
}

impl ClaudePanel {
    /// A conversa até agora.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Se o Claude está respondendo.
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Põe o foco na caixa de mensagem no próximo frame.
    pub fn focus(&mut self) {
        self.focus_input = true;
    }

    pub(crate) fn note(&mut self, text: impl Into<String>) {
        self.streaming = false;
        self.items.push(Item::Note(text.into()));
    }

    /// Recebe os eventos do processo. Devolve `true` se algo mudou.
    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Some(event) = self.agent.as_ref().and_then(PanelAgent::try_recv) {
            changed = true;
            self.apply(event);
        }
        changed
    }

    pub(crate) fn apply(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Started { session_id, model } => {
                if !session_id.is_empty() {
                    self.session = Some(session_id);
                }
                self.model = model;
            }
            AgentEvent::TextDelta(t) => match self.items.last_mut() {
                Some(Item::Assistant(text)) if self.streaming => text.push_str(&t),
                _ => {
                    self.items.push(Item::Assistant(t));
                    self.streaming = true;
                }
            },
            AgentEvent::Text(full) => {
                match self.items.last_mut() {
                    Some(Item::Assistant(text)) if self.streaming => *text = full,
                    _ => self.items.push(Item::Assistant(full)),
                }
                self.streaming = false;
            }
            AgentEvent::ToolUse { id, name, input } => {
                self.streaming = false;
                self.items.push(Item::Tool {
                    id,
                    name,
                    input,
                    result: None,
                });
            }
            AgentEvent::ToolResult { id, is_error, text } => {
                for item in self.items.iter_mut().rev() {
                    if let Item::Tool {
                        id: tid, result, ..
                    } = item
                        && *tid == id
                    {
                        *result = Some((is_error, text));
                        break;
                    }
                }
            }
            AgentEvent::TurnDone {
                is_error,
                text,
                session_id,
                ..
            } => {
                self.busy = false;
                self.streaming = false;
                if !session_id.is_empty() {
                    self.session = Some(session_id);
                }
                if is_error && self.provider.is_empty() {
                    self.note(trf!("Claude Code reported an error: {text}", text = text));
                } else if is_error {
                    self.note(trf!(
                        "{provider} reported an error: {text}",
                        provider = self.provider,
                        text = text
                    ));
                }
            }
            AgentEvent::Stderr(line) => {
                self.stderr.push(line);
                if self.stderr.len() > STDERR_LINES {
                    self.stderr.remove(0);
                }
            }
            AgentEvent::Exited(_) => {
                self.agent = None;
                if self.busy {
                    self.busy = false;
                    let detail = if self.stderr.is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", self.stderr.join("\n"))
                    };
                    let mut note = tr(
                        "Claude Code stopped before answering. If it asks you to sign in, run `claude` in a terminal once.",
                    )
                    .to_string();
                    note.push_str(&detail);
                    self.note(note);
                }
            }
        }
    }

    /// Desenha o painel e devolve o que o usuário pediu.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Vec<PanelAction> {
        let mut actions = Vec::new();
        // A largura é a da área do painel; nada de dentro pode alargá-la.
        let width = ui.available_width();
        ui.set_max_width(width);
        ui.horizontal(|ui| {
            let name = if self.provider.is_empty() {
                "Claude"
            } else {
                self.provider.as_str()
            };
            // i18n-ignore
            let title = ui.strong(format!("{} {name}", egui_phosphor::regular::SPARKLE));
            if !self.model.is_empty() {
                title.on_hover_text(trf!("Model: {model}", model = self.model));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(format!(
                        "{} {}",
                        egui_phosphor::regular::PLUS,
                        tr("New chat")
                    ))
                    .on_hover_text(tr("Start a new conversation"))
                    .clicked()
                {
                    actions.push(PanelAction::NewChat);
                }
                if ui
                    .button(egui_phosphor::regular::GEAR)
                    .on_hover_text(tr(
                        "Choose the AI provider (Tools > Options > AI Providers)",
                    ))
                    .clicked()
                {
                    actions.push(PanelAction::Settings);
                }
                if self.busy
                    && ui
                        .button(format!("{} {}", egui_phosphor::regular::STOP, tr("Stop")))
                        .clicked()
                {
                    actions.push(PanelAction::Stop);
                }
            });
        });
        ui.separator();
        let input_height = 120.0;
        let transcript = (ui.available_height() - input_height).max(80.0);
        egui::ScrollArea::vertical()
            .id_salt("claude_transcript")
            .max_height(transcript)
            .max_width(width)
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.items.is_empty() {
                    ui.add_space(8.0);
                    ui.weak(tr(
                        "Ask about your databases, or ask for a query. Claude sees your servers, schema and query windows through PostPilot, and asks before changing anything.",
                    ));
                }
                for (i, item) in self.items.iter().enumerate() {
                    ui.push_id(i, |ui| show_item(ui, item, &mut actions));
                    ui.add_space(6.0);
                }
                if self.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        if self.provider.is_empty() {
                            ui.weak(tr("Claude is working…"));
                        } else {
                            ui.weak(trf!("{provider} is working…", provider = self.provider));
                        }
                    });
                }
            });
        ui.separator();
        let edit = egui::TextEdit::multiline(&mut self.input)
            .id(egui::Id::new("claude_input"))
            .hint_text(if self.provider.is_empty() {
                tr("Message Claude").to_string()
            } else {
                trf!("Message {provider}", provider = self.provider)
            })
            .desired_rows(3)
            .desired_width(f32::INFINITY);
        let response = ui
            .add(edit)
            .on_hover_text(tr("Enter sends; Shift+Enter starts a new line"));
        if std::mem::take(&mut self.focus_input) {
            response.request_focus();
        }
        let enter = response.has_focus()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        // Nada aqui pode alargar o painel: rótulos quebram a linha.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let send = ui.add_enabled(
                !self.busy && !self.input.trim().is_empty(),
                egui::Button::new(format!(
                    "{} {}",
                    egui_phosphor::regular::PAPER_PLANE_RIGHT,
                    tr("Send")
                )),
            );
            if (send.clicked() || enter) && !self.busy && !self.input.trim().is_empty() {
                let text = std::mem::take(&mut self.input).trim().to_string();
                actions.push(PanelAction::Send(text));
                response.request_focus();
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                let notice = if self.provider.is_empty() {
                    tr("Uses your Claude Code sign-in. What Claude reads is sent to Anthropic.")
                        .to_string()
                } else if self.by_account {
                    tr("Uses your ChatGPT account, through Codex. What the assistant reads is sent to OpenAI.")
                        .to_string()
                } else {
                    trf!(
                        "Uses your {provider} API key. What the assistant reads is sent to {host}.",
                        provider = self.provider,
                        host = self.provider_host
                    )
                };
                ui.add(egui::Label::new(egui::RichText::new(notice).small().weak()).wrap());
            });
        });
        actions
    }
}

fn show_item(ui: &mut egui::Ui, item: &Item, actions: &mut Vec<PanelAction>) {
    match item {
        Item::User(text) => {
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .corner_radius(6)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(text).selectable(true).wrap());
                });
        }
        Item::Assistant(text) => show_markdown(ui, text, actions),
        Item::Tool {
            name,
            input,
            result,
            ..
        } => {
            let (icon, color) = match result {
                None => (
                    egui_phosphor::regular::CIRCLE_NOTCH,
                    ui.visuals().weak_text_color(),
                ),
                Some((false, _)) => (
                    egui_phosphor::regular::CHECK,
                    ui.visuals().weak_text_color(),
                ),
                Some((true, _)) => (egui_phosphor::regular::WARNING, ui.visuals().warn_fg_color),
            };
            // O cabeçalho não quebra a linha: corta para caber no painel.
            let font = egui::TextStyle::Body.resolve(ui.style());
            let char_width = ui.fonts_mut(|f| f.glyph_width(&font, 'n')).max(1.0);
            let fits = ((ui.available_width() - 40.0) / char_width).max(8.0) as usize;
            let label = format!("{icon} {}", tool_label(name, input)); // i18n-ignore
            let label = if label.chars().count() > fits {
                let mut cut: String = label.chars().take(fits.saturating_sub(1)).collect();
                cut.push('…');
                cut
            } else {
                label
            };
            let header = egui::RichText::new(label).color(color);
            egui::CollapsingHeader::new(header)
                .id_salt("tool")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                serde_json::to_string_pretty(input).unwrap_or_default(),
                            )
                            .monospace()
                            .small(),
                        )
                        .wrap(),
                    );
                    if let Some((_, text)) = result {
                        ui.separator();
                        let shown: String = text.chars().take(CARD_TEXT).collect();
                        ui.add(
                            egui::Label::new(egui::RichText::new(shown).monospace().small())
                                .selectable(true)
                                .wrap(),
                        );
                    }
                });
        }
        Item::Note(text) => {
            ui.add(
                egui::Label::new(egui::RichText::new(text).color(ui.visuals().warn_fg_color))
                    .wrap(),
            );
        }
    }
}

/// Nome amigável de cada ferramenta nos cartões.
const TOOL_LABELS: &[(&str, &str)] = &[
    ("list_servers", tr_noop("Listed servers")),
    ("list_databases", tr_noop("Listed databases")),
    ("list_objects", tr_noop("Listed objects")),
    ("describe_table", tr_noop("Read table")),
    ("describe_function", tr_noop("Read function")),
    ("run_query", tr_noop("Ran query")),
    ("list_query_windows", tr_noop("Listed query windows")),
    ("get_query_window", tr_noop("Read query window")),
    ("edit_query_window", tr_noop("Edited query window")),
    ("open_query_window", tr_noop("Opened query window")),
    ("execute_query_window", tr_noop("Executed query window")),
    ("get_query_results", tr_noop("Read results")),
];

/// Texto curto do cartão de uma ferramenta: o nome e o argumento principal.
pub fn tool_label(name: &str, input: &Value) -> String {
    let arg = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
    let detail = match name {
        "describe_table" => arg("table"),
        "describe_function" => arg("function"),
        "list_objects" => arg("schema").or_else(|| arg("database")),
        "list_databases" => arg("server"),
        "run_query" => arg("sql").map(|s| one_line(&s, 24)),
        "edit_query_window" => arg("mode").or(Some("replace_selection".into())),
        "open_query_window" => arg("database"),
        _ => None,
    };
    let friendly = TOOL_LABELS
        .iter()
        .find(|(tool, _)| *tool == name)
        .map_or(name, |(_, label)| tr(label));
    match detail {
        Some(d) if !d.is_empty() => format!("{friendly}: {d}"),
        _ => friendly.to_string(),
    }
}

fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

/// Markdown simples: blocos ``` em fonte fixa com "Insert" e "Copy",
/// títulos e negrito sem os marcadores.
fn show_markdown(ui: &mut egui::Ui, text: &str, actions: &mut Vec<PanelAction>) {
    let mut code: Option<(String, String)> = None;
    let mut paragraph = String::new();
    let flush = |ui: &mut egui::Ui, p: &mut String| {
        if !p.trim().is_empty() {
            ui.add(egui::Label::new(p.trim_end()).selectable(true).wrap());
        }
        p.clear();
    };
    for line in text.lines() {
        let fence = line.trim_start().strip_prefix("```");
        match (&mut code, fence) {
            (None, Some(lang)) => {
                flush(ui, &mut paragraph);
                code = Some((lang.trim().to_lowercase(), String::new()));
            }
            (Some(_), Some(_)) => {
                let (lang, body) = code.take().expect("dentro de um bloco");
                show_code(ui, &lang, &body, actions);
            }
            (Some((_, body)), None) => {
                body.push_str(line);
                body.push('\n');
            }
            (None, None) if line.trim_start().starts_with('|') => {
                flush(ui, &mut paragraph);
                let row = line.trim();
                if !row.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                    ui.add(
                        egui::Label::new(egui::RichText::new(row).monospace().small())
                            .selectable(true)
                            .wrap(),
                    );
                }
            }
            (None, None) => {
                let clean = line.replace("**", "").replace('`', "");
                if let Some(h) = clean.trim_start().strip_prefix('#') {
                    flush(ui, &mut paragraph);
                    ui.strong(h.trim_start_matches('#').trim());
                } else {
                    paragraph.push_str(&clean);
                    paragraph.push('\n');
                }
            }
        }
    }
    if let Some((lang, body)) = code {
        show_code(ui, &lang, &body, actions);
    }
    flush(ui, &mut paragraph);
}

fn show_code(ui: &mut egui::Ui, lang: &str, body: &str, actions: &mut Vec<PanelAction>) {
    let body = body.trim_end();
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(4)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(egui::RichText::new(body).monospace())
                    .selectable(true)
                    .wrap(),
            );
            ui.horizontal(|ui| {
                if matches!(lang, "" | "sql" | "postgresql" | "pgsql" | "tsql" | "mssql")
                    && ui
                        .small_button(format!(
                            "{} {}",
                            egui_phosphor::regular::ARROW_SQUARE_IN,
                            tr("Insert")
                        ))
                        .on_hover_text(tr("Insert into the active query window"))
                        .clicked()
                {
                    actions.push(PanelAction::InsertSql(body.to_string()));
                }
                if ui
                    .small_button(format!("{} {}", egui_phosphor::regular::COPY, tr("Copy")))
                    .clicked()
                {
                    ui.ctx().copy_text(body.to_string());
                }
            });
        });
}

/// O que dizer quando o `claude` não é encontrado.
pub const NOT_INSTALLED: &str = tr_noop(
    "Claude Code was not found. Install it (https://claude.com/claude-code), run `claude` once in a terminal to sign in, then try again. If it is installed somewhere else, set PGSTUDIO_CLAUDE to its path. Or connect another AI (OpenAI, OpenRouter, GLM) in Tools > Options > AI Providers.",
);

/// O que dizer quando o ChatGPT pela conta está escolhido e o `codex` não
/// é encontrado.
pub const CODEX_NOT_INSTALLED: &str = tr_noop(
    "ChatGPT by account needs Codex, OpenAI's official program, and it was not found. Open Tools > Options > AI Providers to install it and sign in. If it is installed somewhere else, set PGSTUDIO_CODEX to its path.",
);

/// Quanto da seleção vai na linha de contexto.
const CONTEXT_SELECTION: usize = 600;

impl crate::ShellApp {
    /// Troca o `claude`, o executável do `postpilot mcp` e a pasta do
    /// painel (testes e quem empacota).
    pub fn with_claude_panel(
        mut self,
        program: Option<PathBuf>,
        exe: Option<PathBuf>,
        dir: Option<PathBuf>,
    ) -> Self {
        self.ai.set_program(program.clone());
        self.claude_panel.program = program;
        self.claude_panel.exe = exe;
        self.claude_panel.dir = dir;
        self
    }

    /// O painel do Claude (testes).
    pub fn claude_panel(&self) -> &ClaudePanel {
        &self.claude_panel
    }

    /// Recebe o que o `claude` mandou e atende os pedidos do painel.
    pub(crate) fn claude_panel_step(&mut self, ctx: &egui::Context, actions: Vec<PanelAction>) {
        self.claude_panel.poll();
        for action in actions {
            match action {
                PanelAction::Send(text) => self.claude_panel_send(ctx, &text),
                PanelAction::Stop => {
                    match self.claude_panel.agent.take() {
                        // O processo morre; a próxima mensagem retoma a
                        // sessão.
                        Some(PanelAgent::Code(mut agent)) => agent.stop(),
                        // A conversa fica; só a resposta é interrompida.
                        Some(PanelAgent::Api(mut agent)) => {
                            agent.stop();
                            self.claude_panel.agent = Some(PanelAgent::Api(agent));
                        }
                        Some(PanelAgent::Codex(mut agent)) => {
                            agent.stop();
                            self.claude_panel.agent = Some(PanelAgent::Codex(agent));
                        }
                        None => {}
                    }
                    self.claude_panel.busy = false;
                    self.claude_panel.note(tr("Stopped."));
                }
                PanelAction::Settings => self.open_ai_options(),
                PanelAction::NewChat => {
                    self.claude_panel.agent = None;
                    let panel = &mut self.claude_panel;
                    panel.items.clear();
                    panel.session = None;
                    panel.busy = false;
                    panel.stderr.clear();
                    panel.focus();
                }
                PanelAction::InsertSql(sql) => match self.claude_window(None).or_else(|_| {
                    // Sem janela aberta: abre uma, como Nova Consulta.
                    self.execute(ctx, CommandId::NewQuery);
                    self.claude_window(None)
                }) {
                    Ok(id) => {
                        self.claude_edit_window(ctx, id, &sql, EditMode::ReplaceSelection);
                    }
                    Err(_) => self
                        .claude_panel
                        .note(tr("Open a query window first, then press Insert again.")),
                },
            }
        }
    }

    /// Manda a mensagem, abrindo o `claude` se preciso.
    pub fn claude_panel_send(&mut self, ctx: &egui::Context, text: &str) {
        self.claude_panel_send_with(ctx, text, "");
    }

    /// Manda `text` (o que a conversa mostra) com `instructions` a mais só
    /// para o Claude (as ações rápidas).
    fn claude_panel_send_with(&mut self, ctx: &egui::Context, text: &str, instructions: &str) {
        let context = self.claude_panel_context(ctx);
        self.claude_panel.items.push(Item::User(text.to_string()));
        self.claude_panel.stderr.clear();
        if self.claude_panel.agent.is_none()
            && let Err(e) = self.claude_panel_start(ctx)
        {
            self.claude_panel.note(e);
            return;
        }
        let message = if instructions.is_empty() {
            format!("{context}\n\n{text}")
        } else {
            format!("{context}\n\n{text}\n\n{instructions}")
        };
        let sent = self
            .claude_panel
            .agent
            .as_mut()
            .map(|a| a.send(&message))
            .unwrap_or_else(|| Err(std::io::Error::other("not running")));
        match sent {
            Ok(()) => self.claude_panel.busy = true,
            Err(e) => {
                self.claude_panel.agent = None;
                let who = self.ai.active_label();
                self.claude_panel.note(trf!(
                    "Could not talk to {who}: {error}",
                    who = who,
                    error = e
                ));
            }
        }
    }

    /// Abre o painel e pergunta, se o Claude não estiver ocupado.
    fn claude_ask(&mut self, ctx: &egui::Context, text: &str, instructions: &str) {
        self.workspace.open(crate::panels::Panel::Claude);
        if self.claude_panel.busy {
            self.claude_panel
                .note(tr("Claude is still answering. Wait for it, or press Stop."));
            return;
        }
        self.claude_panel_send_with(ctx, text, instructions);
    }

    /// Explicar, Otimizar e Corrigir, do menu de contexto do editor e do
    /// menu Query.
    pub(crate) fn claude_quick_action(&mut self, ctx: &egui::Context, cmd: CommandId) {
        let Some(id) = self
            .active_doc
            .filter(|id| self.documents.get(*id).is_some())
        else {
            self.status = tr("Open a query window first.").to_string();
            return;
        };
        let selected = pgstudio_sql_editor::selection(ctx, crate::panels::editor_id(id))
            .is_some_and(|(a, b)| a != b);
        let target = if selected {
            format!("the selected text of query window {id}")
        } else {
            format!("the query in query window {id}")
        };
        let mode = if selected {
            "replace_selection"
        } else {
            "replace_all"
        };
        let (text, instructions) = match cmd {
            CommandId::ClaudeExplain => (
                if selected {
                    tr("Explain the selected SQL.")
                } else {
                    tr("Explain this query.")
                },
                format!(
                    "Read {target} with get_query_window and explain what it does, step by \
                     step, in plain words. Look at the tables it uses if that helps. Do not \
                     change the editor and do not run it."
                ),
            ),
            CommandId::ClaudeOptimize => (
                if selected {
                    tr("Optimize the selected SQL.")
                } else {
                    tr("Optimize this query.")
                },
                format!(
                    "Read {target} with get_query_window. Look at the tables, indexes and row \
                     counts it touches (describe_table, and on PostgreSQL EXPLAIN without ANALYZE \
                     through run_query if useful). Write the improved version into the editor with \
                     edit_query_window, mode {mode}, keeping the same results. Then say briefly \
                     what changed and why. If it is already fine, say so and leave the editor \
                     alone. Suggest indexes in your answer, do not create them."
                ),
            ),
            _ => {
                let errors = self
                    .documents
                    .get(id)
                    .and_then(|d| d.execution.as_ref())
                    .map(|e| {
                        e.messages
                            .iter()
                            .filter(|m| m.kind == crate::execution::MessageKind::Error)
                            .map(|m| m.text.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let error = if errors.is_empty() {
                    "The last execution reported no error; ask the user which error they mean."
                        .to_string()
                } else {
                    format!("The last execution failed with: {}", errors.join(" | "))
                };
                (
                    tr("Fix the error in this query."),
                    format!(
                        "{error} Read {target} with get_query_window, find the cause (check \
                         names with describe_table or list_objects), and write the corrected \
                         SQL into the editor with edit_query_window, mode {mode}. Do not run \
                         it. Say in one or two sentences what was wrong."
                    ),
                )
            }
        };
        self.claude_ask(ctx, text, &instructions);
    }

    /// "Ask Claude" numa tabela ou view do Object Explorer.
    pub(crate) fn claude_ask_about(
        &mut self,
        ctx: &egui::Context,
        object: &pgstudio_core::metadata::ObjectNode,
        database: Option<String>,
    ) {
        let text = trf!("Tell me about {name}.", name = object.name);
        let database = database.unwrap_or_else(|| "the current database".into());
        let instructions = format!(
            "{name} is a {kind} in database {database} (selected in Object Explorer). Use \
             describe_table on it, and run_query for its row count and a few sample rows. \
             Explain what it holds, its keys and how it relates to other tables, and suggest \
             two or three useful queries in ```sql blocks. Do not change anything.",
            name = object.name,
            kind = object.kind.english_label().to_lowercase(),
        );
        self.claude_ask(ctx, &text, &instructions);
    }

    fn claude_panel_start(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let program = match self.ai.backend() {
            Some(Backend::Api { name, conn }) => {
                let client = self.automation(ctx);
                let repaint = ctx.clone();
                self.claude_panel.provider = name;
                self.claude_panel.provider_host = conn.host().to_string();
                self.claude_panel.by_account = false;
                self.claude_panel.agent = Some(PanelAgent::Api(ChatAgent::start(
                    conn,
                    chat_agent::PANEL_PROMPT,
                    &pgstudio_automation::tools::all(),
                    Arc::new(move |tool, args| client.call(tool, args, Origin::Assistant)),
                    Arc::new(move || repaint.request_repaint()),
                )));
                return Ok(());
            }
            Some(Backend::Codex { program }) => {
                let (dir, exe, endpoint_path) = self.claude_panel_channel(ctx)?;
                self.claude_panel.provider = pgstudio_automation::llm::CHATGPT_LABEL.to_string();
                self.claude_panel.provider_host.clear();
                self.claude_panel.by_account = true;
                let repaint = ctx.clone();
                self.claude_panel.agent = Some(PanelAgent::Codex(CodexAgent::start(
                    program,
                    codex::mcp_overrides(&exe, &endpoint_path),
                    chat_agent::PANEL_PROMPT,
                    dir.join("codex-panel"),
                    Arc::new(move || repaint.request_repaint()),
                )));
                return Ok(());
            }
            Some(Backend::ClaudeCode { program }) => program,
            None if self.ai.settings().active_provider().is_some() => {
                return Err(trf!(
                    "{provider} has no API key yet. Add it in Tools > Options > AI Providers.",
                    provider = self.ai.active_label()
                ));
            }
            None if self.ai.settings().uses_chatgpt() => {
                return Err(tr(CODEX_NOT_INSTALLED).to_string());
            }
            None => return Err(tr(NOT_INSTALLED).to_string()),
        };
        self.claude_panel.provider.clear();
        self.claude_panel.provider_host.clear();
        let (dir, exe, endpoint_path) = self.claude_panel_channel(ctx)?;
        let config = agent::mcp_config(&exe, &endpoint_path);
        let args = agent::panel_args(&config, self.claude_panel.session.as_deref(), None);
        let repaint = ctx.clone();
        let started = Agent::start(
            &program,
            &args,
            &dir.join("claude-panel"),
            Arc::new(move || repaint.request_repaint()),
        );
        match started {
            Ok(a) => {
                self.claude_panel.agent = Some(PanelAgent::Code(a));
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(tr(NOT_INSTALLED).to_string())
            }
            Err(e) => Err(trf!(
                "Could not start Claude Code ({program}): {error}",
                program = program.display(),
                error = e
            )),
        }
    }

    /// Abre (uma vez) o canal local do painel, por onde o `postpilot mcp`
    /// do Claude Code ou do Codex fala com a interface. Devolve a pasta do
    /// painel, o executável do PostPilot e o arquivo do canal.
    fn claude_panel_channel(
        &mut self,
        ctx: &egui::Context,
    ) -> Result<(PathBuf, PathBuf, PathBuf), String> {
        let dir = self
            .claude_panel
            .dir
            .clone()
            .or_else(|| endpoint::default_path().and_then(|p| p.parent().map(PathBuf::from)))
            .unwrap_or_else(std::env::temp_dir);
        if self.claude_panel.listener.is_none() {
            let client = self.automation(ctx);
            let path = dir.join(format!("panel-endpoint-{}.json", std::process::id()));
            let listener = Listener::start_as(client, path, Origin::Panel).map_err(|e| {
                trf!(
                    "Could not open the local channel for Claude: {error}",
                    error = e
                )
            })?;
            self.claude_panel.listener = Some(listener);
        }
        let endpoint_path = self
            .claude_panel
            .listener
            .as_ref()
            .map(|l| l.path().to_path_buf())
            .unwrap_or_default();
        let exe = match self.claude_panel.exe.clone() {
            Some(e) => e,
            None => std::env::current_exe()
                .map_err(|e| trf!("Could not find the PostPilot program: {error}", error = e))?,
        };
        Ok((dir, exe, endpoint_path))
    }

    /// A linha `[PostPilot context]` de cada mensagem.
    fn claude_panel_context(&self, ctx: &egui::Context) -> String {
        let Some(id) = self
            .active_doc
            .filter(|id| self.documents.get(*id).is_some())
        else {
            let server = if self.connection.is_some() {
                format!("connected to {}", self.connection_label())
            } else {
                "not connected to a server".to_string()
            };
            return format!("[PostPilot context] No query window is open; PostPilot is {server}.");
        };
        let doc = self.documents.get(id).expect("filtrado acima");
        let server = self
            .document_servers()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "no server".into());
        let database = self.document_database(id).unwrap_or_else(|| "none".into());
        let selection = pgstudio_sql_editor::selection(ctx, crate::panels::editor_id(id))
            .map(|(a, b)| (a.min(b), a.max(b)))
            .filter(|(a, b)| a != b)
            .map(|(a, b)| {
                let picked: String = doc.text.chars().skip(a).take(b - a).collect();
                let mut cut: String = picked.chars().take(CONTEXT_SELECTION).collect();
                if picked.chars().count() > CONTEXT_SELECTION {
                    cut.push('…');
                }
                format!(" Selected text (chars {a}-{b}): {cut}")
            })
            .unwrap_or_else(|| " Nothing is selected.".into());
        format!(
            "[PostPilot context] Active query window {id} \"{}\" on {server}, database {database}.{selection}",
            doc.name()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_build_the_transcript() {
        let mut p = ClaudePanel {
            busy: true,
            ..Default::default()
        };
        p.apply(AgentEvent::Started {
            session_id: "s".into(),
            model: "m".into(),
        });
        p.apply(AgentEvent::TextDelta("Vou ".into()));
        p.apply(AgentEvent::TextDelta("ver.".into()));
        p.apply(AgentEvent::Text("Vou ver.".into()));
        p.apply(AgentEvent::ToolUse {
            id: "1".into(),
            name: "describe_table".into(),
            input: json!({"table": "pedido"}),
        });
        p.apply(AgentEvent::ToolResult {
            id: "1".into(),
            is_error: false,
            text: "{}".into(),
        });
        p.apply(AgentEvent::TextDelta("Pronto".into()));
        p.apply(AgentEvent::TurnDone {
            is_error: false,
            text: "Pronto".into(),
            cost_usd: None,
            session_id: "s".into(),
        });
        assert_eq!(p.items.len(), 3);
        assert_eq!(p.items[0], Item::Assistant("Vou ver.".into()));
        assert!(matches!(
            &p.items[1],
            Item::Tool {
                result: Some((false, _)),
                ..
            }
        ));
        assert_eq!(p.items[2], Item::Assistant("Pronto".into()));
        assert!(!p.busy);
        assert_eq!(p.session.as_deref(), Some("s"));
    }

    #[test]
    fn exit_while_busy_explains() {
        let mut p = ClaudePanel {
            busy: true,
            ..Default::default()
        };
        p.apply(AgentEvent::Stderr(
            "Invalid API key · Please run /login".into(),
        ));
        p.apply(AgentEvent::Exited(Some(1)));
        assert!(!p.busy);
        let Some(Item::Note(n)) = p.items.last() else {
            panic!()
        };
        assert!(n.contains("/login"));
    }

    #[test]
    fn every_tool_has_a_translated_label() {
        use pgstudio_i18n::{Language, has_translation};
        for tool in pgstudio_automation::tools::all() {
            let (_, label) = TOOL_LABELS
                .iter()
                .find(|(t, _)| *t == tool.name)
                .unwrap_or_else(|| panic!("sem rótulo: {}", tool.name));
            assert!(
                has_translation(Language::PortugueseBrazil, None, label),
                "{label}"
            );
        }
        assert!(has_translation(
            Language::PortugueseBrazil,
            None,
            NOT_INSTALLED
        ));
    }

    #[test]
    fn labels() {
        assert_eq!(
            tool_label("describe_table", &json!({"table": "pedido"})),
            "Read table: pedido"
        );
        assert_eq!(
            tool_label("run_query", &json!({"sql": "select\n  1"})),
            "Ran query: select 1"
        );
        assert_eq!(tool_label("list_servers", &json!({})), "Listed servers");
    }
}
