//! Uma execução de consulta na janela de consulta: recebe os eventos da
//! sessão (em outra thread) e guarda resultados e mensagens para as abas
//! Results e Messages, como no SSMS.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::query::{QueryEvent, QuerySession, ServerMessage, line_col};

/// Linhas guardadas por resultado. A grade com buffer paginado (marco 2.5)
/// tira este limite; até lá, o resto só é contado.
pub const MAX_ROWS: usize = 100_000;

/// Um resultado (um `SELECT`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultSet {
    pub columns: Vec<String>,
    /// Até [`MAX_ROWS`] linhas, como texto; `None` é NULL.
    pub rows: Vec<Vec<Option<String>>>,
    /// Linhas recebidas, inclusive as que passaram do limite.
    pub total: u64,
}

/// Uma linha da aba Messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub kind: MessageKind,
    /// Caractere do documento onde está o erro, para levar o cursor até lá.
    pub position: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Notice,
    Error,
}

/// Como a execução terminou (ou se ainda roda).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl Outcome {
    /// Texto da faixa de status da janela de consulta, como no SSMS.
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Running => "Executing query...",
            Outcome::Succeeded => "Query executed successfully.",
            Outcome::Failed => "Query completed with errors.",
            Outcome::Canceled => "Query was canceled.",
        }
    }
}

/// Aba visível no painel de resultados.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Results,
    Messages,
}

/// Uma execução, do F5 até o fim.
pub struct Execution {
    events: mpsc::Receiver<QueryEvent>,
    session: Arc<dyn QuerySession>,
    /// O SQL enviado e onde ele começa no documento (em caracteres), para
    /// traduzir a posição dos erros.
    sql: String,
    offset: usize,
    pub results: Vec<ResultSet>,
    pub messages: Vec<Message>,
    /// O último evento abriu um resultado que ainda não terminou.
    open_result: bool,
    started: Instant,
    elapsed: Option<Duration>,
    outcome: Outcome,
    /// Posição do primeiro erro, ainda não mostrada no editor.
    pending_error: Option<usize>,
    pub tab: Tab,
    pub database: String,
}

impl std::fmt::Debug for Execution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Execution")
            .field("outcome", &self.outcome)
            .field("results", &self.results.len())
            .field("messages", &self.messages)
            .finish_non_exhaustive()
    }
}

impl Execution {
    /// Manda `sql` (que começa no caractere `offset` do documento) para a
    /// sessão. Cada evento pede um novo frame.
    pub fn start(
        session: Arc<dyn QuerySession>,
        sql: String,
        offset: usize,
        ctx: &egui::Context,
    ) -> Self {
        let (tx, events) = mpsc::channel();
        let ctx = ctx.clone();
        session.execute(
            sql.clone(),
            Box::new(move |e| {
                let _ = tx.send(e);
                ctx.request_repaint();
            }),
        );
        Execution {
            events,
            database: session.database(),
            session,
            sql,
            offset,
            results: Vec::new(),
            messages: Vec::new(),
            open_result: false,
            started: Instant::now(),
            elapsed: None,
            outcome: Outcome::Running,
            pending_error: None,
            tab: Tab::Results,
        }
    }

    /// Pede o cancelamento.
    pub fn cancel(&self) {
        if self.is_running() {
            self.session.cancel();
        }
    }

    pub fn is_running(&self) -> bool {
        self.outcome == Outcome::Running
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// Tempo de execução (até agora, se ainda roda).
    pub fn elapsed(&self) -> Duration {
        self.elapsed.unwrap_or_else(|| self.started.elapsed())
    }

    /// Linhas recebidas em todos os resultados.
    pub fn total_rows(&self) -> u64 {
        self.results.iter().map(|r| r.total).sum()
    }

    /// Posição (no documento) do erro a mostrar no editor, uma vez só.
    pub fn take_error_position(&mut self) -> Option<usize> {
        self.pending_error.take()
    }

    /// Lê os eventos que chegaram. Devolve `true` no frame em que a execução
    /// termina.
    pub fn poll(&mut self) -> bool {
        let mut finished = false;
        while let Ok(event) = self.events.try_recv() {
            finished |= self.apply(event);
        }
        finished
    }

    fn apply(&mut self, event: QueryEvent) -> bool {
        match event {
            QueryEvent::Columns(columns) => {
                self.results.push(ResultSet {
                    columns,
                    ..Default::default()
                });
                self.open_result = true;
            }
            QueryEvent::Row(row) => {
                if let Some(set) = self.results.last_mut().filter(|_| self.open_result) {
                    set.total += 1;
                    if set.rows.len() < MAX_ROWS {
                        set.rows.push(row);
                    }
                }
            }
            QueryEvent::CommandComplete { rows } => {
                let had_result = std::mem::take(&mut self.open_result);
                if had_result || rows > 0 {
                    self.info(rows_affected(rows));
                }
                let truncated = had_result
                    && self
                        .results
                        .last()
                        .is_some_and(|s| s.total > s.rows.len() as u64);
                if truncated {
                    self.info(format!(
                        "Only the first {MAX_ROWS} rows are shown in Results."
                    ));
                }
            }
            QueryEvent::Notice(n) => self.messages.push(Message {
                text: format!("{}: {}", n.severity, describe(&n)),
                kind: MessageKind::Notice,
                position: None,
            }),
            QueryEvent::Error(e) => {
                self.open_result = false;
                let position = e.position.map(|p| self.offset + p.saturating_sub(1));
                let header = match e.position {
                    Some(p) => {
                        let (line, col) = line_col(&self.sql, p.saturating_sub(1));
                        format!(
                            "Msg {}, Level {}, Line {line}, Column {col}",
                            code(&e),
                            e.severity
                        )
                    }
                    None => format!("Msg {}, Level {}", code(&e), e.severity),
                };
                self.messages.push(Message {
                    text: format!("{header}\n{}", describe(&e)),
                    kind: MessageKind::Error,
                    position,
                });
                if self.pending_error.is_none() {
                    self.pending_error = position;
                }
                self.outcome = Outcome::Failed;
            }
            QueryEvent::Finished { canceled } => {
                self.elapsed = Some(self.started.elapsed());
                if canceled {
                    self.outcome = Outcome::Canceled;
                    self.info("Query was cancelled by user.".into());
                } else if self.outcome == Outcome::Running {
                    self.outcome = Outcome::Succeeded;
                    if !self.messages.iter().any(|m| m.kind == MessageKind::Info) {
                        self.info("Commands completed successfully.".into());
                    }
                }
                self.info(String::new());
                self.info(format!(
                    "Total execution time: {}",
                    format_elapsed(self.elapsed(), true)
                ));
                // Erros vão para a aba Messages, como no SSMS.
                if self.results.is_empty() || self.outcome == Outcome::Failed {
                    self.tab = Tab::Messages;
                }
                return true;
            }
        }
        false
    }

    fn info(&mut self, text: String) {
        self.messages.push(Message {
            text,
            kind: MessageKind::Info,
            position: None,
        });
    }
}

fn code(e: &ServerMessage) -> &str {
    if e.code.is_empty() { "0" } else { &e.code }
}

/// Mensagem com DETAIL e HINT, quando houver.
fn describe(m: &ServerMessage) -> String {
    let mut text = m.message.clone();
    if let Some(d) = &m.detail {
        text.push_str(&format!("\nDETAIL: {d}"));
    }
    if let Some(h) = &m.hint {
        text.push_str(&format!("\nHINT: {h}"));
    }
    text
}

fn rows_affected(rows: u64) -> String {
    if rows == 1 {
        "(1 row affected)".into()
    } else {
        format!("({rows} rows affected)")
    }
}

/// `00:00:01` na faixa de status; com `millis`, `00:00:01.234`.
pub fn format_elapsed(d: Duration, millis: bool) -> String {
    let s = d.as_secs();
    let base = format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60);
    if millis {
        format!("{base}.{:03}", d.subsec_millis())
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_is_formatted_like_ssms() {
        assert_eq!(
            format_elapsed(Duration::from_millis(1234), false),
            "00:00:01"
        );
        assert_eq!(
            format_elapsed(Duration::from_millis(3_723_456), true),
            "01:02:03.456"
        );
    }
}
