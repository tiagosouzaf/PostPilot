//! Uma execução de consulta na janela de consulta: recebe os eventos da
//! sessão (em outra thread) e guarda resultados e mensagens para as abas
//! Results e Messages, como no SSMS.

use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::query::{QueryEvent, QuerySession, ServerMessage, line_col};
use pgstudio_result_grid::{GridState, ResultStore};

/// Linhas recebidas entre um pedido de frame e outro, para a contagem na
/// tela andar sem pedir um frame por linha.
const REPAINT_EVERY_ROWS: u64 = 5000;

/// Como mostrar os resultados: grade (Ctrl+D) ou texto (Ctrl+T).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResultsMode {
    #[default]
    Grid,
    Text,
}

/// Um resultado (um `SELECT`): as linhas no buffer paginado e o estado da
/// grade.
///
/// As linhas vão direto da thread do banco para o buffer, sem passar por um
/// canal: assim a memória fica limitada pelo orçamento do buffer mesmo
/// quando o servidor manda linhas mais rápido do que a tela desenha.
pub struct ResultSet {
    store: Arc<Mutex<ResultStore>>,
    pub grid: GridState,
    /// Larguras do modo texto, calculadas na primeira vez que ele aparece.
    pub text_widths: Option<Vec<usize>>,
    /// Posição da rolagem do modo texto, em linhas.
    pub text_top: f64,
}

impl ResultSet {
    fn new(store: Arc<Mutex<ResultStore>>) -> Self {
        ResultSet {
            store,
            grid: GridState::default(),
            text_widths: None,
            text_top: 0.0,
        }
    }

    /// O buffer, travado enquanto o guarda existir.
    pub fn store(&self) -> MutexGuard<'_, ResultStore> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// O buffer e o estado da grade, para desenhar.
    pub fn parts(&mut self) -> (&Mutex<ResultStore>, &mut GridState) {
        (&self.store, &mut self.grid)
    }

    pub fn columns(&self) -> Vec<String> {
        self.store().columns().to_vec()
    }

    /// Linhas recebidas até agora.
    pub fn len(&self) -> u64 {
        self.store().len()
    }

    pub fn is_empty(&self) -> bool {
        self.store().is_empty()
    }

    /// Uma linha, como texto.
    pub fn row(&self, row: u64) -> Vec<Option<String>> {
        self.store().row(row)
    }
}

/// O que a thread do banco manda para a interface.
enum Incoming {
    Event(QueryEvent),
    /// Começou um resultado; as linhas vão direto para este buffer.
    Result(Arc<Mutex<ResultStore>>),
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
    events: mpsc::Receiver<Incoming>,
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
    /// Posição e mensagem do primeiro erro, ainda não mostradas no editor.
    pending_error: Option<(usize, String)>,
    pub tab: Tab,
    pub database: String,
    /// Grade ou texto, escolhido antes de executar.
    pub mode: ResultsMode,
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
        let mut current: Option<Arc<Mutex<ResultStore>>> = None;
        let mut since_repaint = 0;
        session.execute(
            sql.clone(),
            Box::new(move |e| match e {
                QueryEvent::Row(row) => {
                    if let Some(store) = &current {
                        store.lock().unwrap_or_else(|e| e.into_inner()).push(&row);
                        since_repaint += 1;
                        if since_repaint == REPAINT_EVERY_ROWS {
                            since_repaint = 0;
                            ctx.request_repaint();
                        }
                    }
                }
                QueryEvent::Columns(columns) => {
                    let store = Arc::new(Mutex::new(ResultStore::new(columns)));
                    current = Some(store.clone());
                    let _ = tx.send(Incoming::Result(store));
                    ctx.request_repaint();
                }
                other => {
                    if matches!(
                        other,
                        QueryEvent::CommandComplete { .. } | QueryEvent::Error(_)
                    ) {
                        current = None;
                    }
                    let _ = tx.send(Incoming::Event(other));
                    ctx.request_repaint();
                }
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
            mode: ResultsMode::Grid,
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
        self.results.iter().map(ResultSet::len).sum()
    }

    /// Posição (no documento) e mensagem do erro a mostrar no editor, uma
    /// vez só.
    pub fn take_error_position(&mut self) -> Option<(usize, String)> {
        self.pending_error.take()
    }

    /// Lê os eventos que chegaram. Devolve `true` no frame em que a execução
    /// termina.
    pub fn poll(&mut self) -> bool {
        let mut finished = false;
        while let Ok(incoming) = self.events.try_recv() {
            match incoming {
                Incoming::Result(store) => {
                    self.results.push(ResultSet::new(store));
                    self.open_result = true;
                }
                Incoming::Event(event) => finished |= self.apply(event),
            }
        }
        finished
    }

    fn apply(&mut self, event: QueryEvent) -> bool {
        match event {
            // Colunas e linhas vão direto para o buffer (ver `start`).
            QueryEvent::Columns(_) | QueryEvent::Row(_) => {}
            QueryEvent::CommandComplete { rows } => {
                let had_result = std::mem::take(&mut self.open_result);
                if had_result || rows > 0 {
                    self.info(rows_affected(rows));
                }
                // Disco cheio ou sem pasta temporária: tudo fica em memória.
                let spill_error = self
                    .results
                    .last()
                    .filter(|_| had_result)
                    .and_then(|s| s.store().spill_error().map(str::to_string));
                if let Some(e) = spill_error {
                    self.info(format!(
                        "Warning: the results could not be paged to disk ({e}); they are kept in memory."
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
                    self.pending_error = position.map(|p| (p, e.message.clone()));
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

/// `(n rows affected)`, como no SSMS.
pub fn rows_affected(rows: u64) -> String {
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
