//! Uma execução de consulta na janela de consulta: recebe os eventos da
//! sessão (em outra thread) e guarda resultados e mensagens para as abas
//! Results e Messages, como no SSMS.

use pgstudio_i18n::{tr, trf};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::Engine;
use pgstudio_core::query::{QueryEvent, QuerySession, ServerMessage, line_col};
use pgstudio_execution_plan::{Plan, PlanView};
use pgstudio_result_grid::export::{Format, Writer};
use pgstudio_result_grid::grid::INSERT_TABLE;
use pgstudio_result_grid::xlsx::Workbook;
use pgstudio_result_grid::{GridState, ResultStore};

use crate::result_edit::ResultEditor;

/// Linhas recebidas entre um pedido de frame e outro, para a contagem na
/// tela andar sem pedir um frame por linha.
const REPAINT_EVERY_ROWS: u64 = 5000;

/// Como mostrar os resultados: grade (Ctrl+D), texto (Ctrl+T) ou direto
/// num arquivo (Ctrl+Shift+F).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResultsMode {
    #[default]
    Grid,
    Text,
    File,
}

/// Um resultado (um `SELECT`): as linhas no buffer paginado e o estado da
/// grade.
///
/// As linhas vão direto da thread do banco para o buffer, sem passar por um
/// canal: assim a memória fica limitada pelo orçamento do buffer mesmo
/// quando o servidor manda linhas mais rápido do que a tela desenha.
pub struct ResultSet {
    pub(crate) store: Arc<Mutex<ResultStore>>,
    pub grid: GridState,
    /// Larguras do modo texto, calculadas quando ele aparece e de novo
    /// enquanto chegam linhas da amostra.
    pub text_widths: Option<Vec<usize>>,
    /// Linhas da amostra quando as larguras foram calculadas.
    pub text_width_rows: u64,
    /// Posição da rolagem do modo texto, em linhas.
    pub text_top: f64,
    /// Edição na grade (só depois que a execução termina).
    pub editor: Option<ResultEditor>,
}

impl ResultSet {
    fn new(store: Arc<Mutex<ResultStore>>) -> Self {
        ResultSet {
            store,
            grid: GridState::default(),
            text_widths: None,
            text_width_rows: 0,
            text_top: 0.0,
            editor: None,
        }
    }

    /// O buffer, travado enquanto o guarda existir.
    pub fn store(&self) -> MutexGuard<'_, ResultStore> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// O buffer compartilhado (para exportar fora da thread da interface).
    pub fn shared_store(&self) -> Arc<Mutex<ResultStore>> {
        self.store.clone()
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
        tr(self.english_label())
    }

    /// O mesmo texto sempre em inglês, para o que vai ao Claude Code.
    pub fn english_label(self) -> &'static str {
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
    /// "Execution plan", com os planos do Ctrl+L.
    Plan,
}

/// Um trecho do SQL enviado que veio do documento: `len` caracteres a
/// partir de `sent` no texto enviado correspondem aos que começam em
/// `original` no texto escolhido pelo usuário.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub sent: usize,
    pub original: usize,
    pub len: usize,
}

/// O que vai para o servidor: o texto escolhido (seleção ou documento) ou
/// uma versão dele com comandos em volta (como `EXPLAIN` antes de cada
/// comando), com os trechos que permitem levar a posição de um erro de
/// volta ao documento.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub sent: String,
    pub segments: Vec<Segment>,
    /// Os comandos, um por resultado esperado, para os cabeçalhos dos
    /// planos.
    pub statements: Vec<String>,
}

impl Batch {
    /// O texto como está.
    pub fn plain(sql: &str) -> Self {
        Batch {
            sent: sql.to_string(),
            segments: vec![Segment {
                sent: 0,
                original: 0,
                len: sql.chars().count(),
            }],
            statements: Vec::new(),
        }
    }

    /// SQL montado pelo PGStudio (como o do Alt+F1), que não está no
    /// documento: os erros não apontam para nenhuma posição.
    pub fn generated(sql: &str) -> Self {
        Batch {
            sent: sql.to_string(),
            segments: Vec::new(),
            statements: Vec::new(),
        }
    }

    /// `EXPLAIN (options)` antes de cada comando de `sql`, como o Ctrl+L.
    /// Sem comandos (só espaço e comentários), devolve `None`.
    pub fn explain(sql: &str, options: &str) -> Option<Self> {
        let ranges = pgstudio_sql_editor::lexer::statements(sql);
        if ranges.is_empty() {
            return None;
        }
        let mut sent = String::new();
        let mut sent_chars = 0;
        let mut segments = Vec::new();
        let mut statements = Vec::new();
        for (i, r) in ranges.into_iter().enumerate() {
            if i > 0 {
                sent.push_str(";\n");
                sent_chars += 2;
            }
            let prefix = format!("EXPLAIN ({options}) ");
            sent_chars += prefix.chars().count();
            sent.push_str(&prefix);
            let body = &sql[r.clone()];
            let len = body.chars().count();
            segments.push(Segment {
                sent: sent_chars,
                original: sql[..r.start].chars().count(),
                len,
            });
            sent.push_str(body);
            sent_chars += len;
            statements.push(body.to_string());
        }
        Some(Batch {
            sent,
            segments,
            statements,
        })
    }

    /// SQL Server: o lote entre `SET <option> ON` e `OFF` (`SHOWPLAN_XML`
    /// para o plano estimado, `STATISTICS XML` para o real), cada um no
    /// seu lote como o servidor exige. Com `roll_back`, tudo roda numa
    /// transação desfeita no fim. Sem comandos, devolve `None`.
    pub fn showplan(sql: &str, option: &str, roll_back: bool) -> Option<Self> {
        if pgstudio_sql_editor::lexer::statements(sql).is_empty() {
            return None;
        }
        let mut prefix = format!("SET {option} ON\nGO\n");
        let mut suffix = format!("\nGO\nSET {option} OFF");
        if roll_back {
            prefix.push_str("BEGIN TRANSACTION\nGO\n");
            suffix = format!("\nGO\nIF @@TRANCOUNT > 0 ROLLBACK TRANSACTION{suffix}");
        }
        Some(Batch {
            sent: format!("{prefix}{sql}{suffix}"),
            segments: vec![Segment {
                sent: prefix.chars().count(),
                original: 0,
                len: sql.chars().count(),
            }],
            statements: Vec::new(),
        })
    }

    /// Põe o lote entre `BEGIN` e `ROLLBACK`, para o `EXPLAIN ANALYZE` de
    /// comandos que alteram dados não deixar rastro.
    pub fn in_rolled_back_transaction(mut self) -> Self {
        const BEGIN: &str = "BEGIN;\n";
        let shift = BEGIN.chars().count();
        self.sent = format!("{BEGIN}{};\nROLLBACK", self.sent);
        for s in &mut self.segments {
            s.sent += shift;
        }
        self
    }

    /// Posição (em caracteres, a partir de 0) no texto escolhido para uma
    /// posição no texto enviado. Uma posição dentro do que foi acrescentado
    /// vai para o começo do comando seguinte.
    pub fn original_position(&self, sent: usize) -> usize {
        let i = self.segments.partition_point(|s| s.sent <= sent);
        let prev = i.checked_sub(1).map(|i| self.segments[i]);
        let next = self.segments.get(i);
        match prev {
            Some(s) if sent - s.sent <= s.len || next.is_none() => {
                s.original + (sent - s.sent).min(s.len)
            }
            _ => next.map_or(0, |n| n.original),
        }
    }
}

/// Se algum comando de `sql` altera dados (`INSERT`, `UPDATE`, `DELETE`
/// ou `MERGE`, também dentro de um `WITH`), o que pede confirmação antes
/// do `EXPLAIN ANALYZE`, que executa o comando.
pub fn changes_data(sql: &str) -> bool {
    use pgstudio_sql_editor::lexer::{TokenKind, tokenize};
    tokenize(sql).iter().any(|t| {
        t.kind == TokenKind::Keyword
            && matches!(
                sql[t.range.clone()].to_ascii_lowercase().as_str(),
                "insert" | "update" | "delete" | "merge"
            )
    })
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
    /// Os trechos do texto enviado e os comandos, para levar erros ao
    /// documento e dar nome aos planos.
    batch: Batch,
    /// Ctrl+L: cada resultado é um plano em JSON, mostrado na aba
    /// Execution plan em vez da grade.
    plan_mode: bool,
    /// Plano real do SQL Server: os resultados dos comandos ficam, e os
    /// planos vêm em resultados à parte.
    keep_results: bool,
    pub plans: Vec<(Plan, PlanView)>,
    /// O lote roda entre `BEGIN` e `ROLLBACK` (plano real de DML).
    rolled_back: bool,
    /// O servidor mandou `Finished`: um erro muda o resultado antes disso,
    /// mas o lote só acaba aqui.
    finished: bool,
    /// Results to File: os resultados vão para este arquivo, não para a
    /// grade.
    file: Option<PathBuf>,
    /// O lote rodou em vários servidores (veja [`crate::multi`]): a grade
    /// é só leitura.
    pub multi: bool,
    ctx: egui::Context,
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
        let batch = Batch::plain(&sql);
        Self::start_batch(session, batch, sql, offset, ctx)
    }

    /// Ctrl+L: pede o plano estimado de cada comando de `sql`, sem rodar
    /// nada. `None` quando não há comando.
    pub fn start_plan(
        session: Arc<dyn QuerySession>,
        sql: String,
        offset: usize,
        ctx: &egui::Context,
    ) -> Option<Self> {
        let batch = match session.engine() {
            Engine::PostgreSql | Engine::Redis => Batch::explain(&sql, "FORMAT JSON")?,
            Engine::SqlServer => Batch::showplan(&sql, "SHOWPLAN_XML", false)?,
        };
        let mut exec = Self::start_batch(session, batch, sql, offset, ctx);
        exec.plan_mode = true;
        exec.tab = Tab::Plan;
        Some(exec)
    }

    /// Manda `batch.sent` para a sessão; `sql` é o texto escolhido, que
    /// começa no caractere `offset` do documento.
    pub fn start_batch(
        session: Arc<dyn QuerySession>,
        batch: Batch,
        sql: String,
        offset: usize,
        ctx: &egui::Context,
    ) -> Self {
        let (tx, events) = mpsc::channel();
        let ui_ctx = ctx.clone();
        let ctx = ctx.clone();
        let mut current: Option<Arc<Mutex<ResultStore>>> = None;
        let mut since_repaint = 0;
        session.execute(
            batch.sent.clone(),
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
            batch,
            plan_mode: false,
            keep_results: false,
            plans: Vec::new(),
            rolled_back: false,
            finished: false,
            file: None,
            multi: false,
            ctx: ui_ctx,
        }
    }

    /// Results to File: roda `sql` escrevendo cada resultado em `path`
    /// (no formato da extensão), direto da thread do banco, sem guardar as
    /// linhas. Resultados seguidos ficam separados por uma linha em branco.
    pub fn start_to_file(
        session: Arc<dyn QuerySession>,
        sql: String,
        offset: usize,
        path: &Path,
        ctx: &egui::Context,
    ) -> std::io::Result<Self> {
        let file = std::fs::File::create(path)?;
        let format = Format::from_path(path);
        let batch = Batch::plain(&sql);
        let (tx, events) = mpsc::channel();
        let ui_ctx = ctx.clone();
        let ctx = ctx.clone();
        let shown = path.display().to_string();
        let mut sink = if format == Format::Xlsx {
            FileSink::Book(Some(Workbook::new(BufWriter::new(file), tr("Results"))))
        } else {
            FileSink::Text {
                idle: Some(BufWriter::new(file)),
                writer: None,
                format,
                results: 0,
            }
        };
        let mut failed: Option<String> = None;
        session.execute(
            batch.sent.clone(),
            Box::new(move |e| match e {
                QueryEvent::Columns(columns) => {
                    if let Err(e) = sink.columns(columns) {
                        failed = Some(e.to_string());
                    }
                }
                QueryEvent::Row(row) => {
                    if let Err(e) = sink.row(&row) {
                        failed = Some(e.to_string());
                    }
                }
                QueryEvent::Finished { canceled } => {
                    match sink.finish() {
                        Ok(0) => {}
                        Ok(n) => {
                            let _ = tx.send(Incoming::Event(QueryEvent::Notice(ServerMessage {
                                severity: "WARNING".into(),
                                message: truncated_text(n),
                                ..Default::default()
                            })));
                        }
                        Err(e) => failed = Some(e.to_string()),
                    }
                    if let Some(e) = failed.take() {
                        let message = trf!(
                            "Could not write the results to {path}: {error}",
                            path = shown,
                            error = e
                        );
                        let _ = tx.send(Incoming::Event(QueryEvent::Error(ServerMessage::error(
                            message,
                        ))));
                    }
                    let _ = tx.send(Incoming::Event(QueryEvent::Finished { canceled }));
                    ctx.request_repaint();
                }
                other => {
                    if matches!(
                        other,
                        QueryEvent::CommandComplete { .. } | QueryEvent::Error(_)
                    ) && let Err(e) = sink.end_result()
                    {
                        failed = Some(e.to_string());
                    }
                    // O fim de um resultado conta como resultado para o
                    // "(n rows affected)".
                    let _ = tx.send(Incoming::Event(other));
                    ctx.request_repaint();
                }
            }),
        );
        Ok(Execution {
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
            tab: Tab::Messages,
            mode: ResultsMode::File,
            batch,
            plan_mode: false,
            keep_results: false,
            plans: Vec::new(),
            rolled_back: false,
            finished: false,
            file: Some(path.to_path_buf()),
            multi: false,
            ctx: ui_ctx,
        })
    }

    /// Arquivo do Results to File, se for.
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Ctrl+M ligado: roda cada comando com `EXPLAIN (ANALYZE, BUFFERS,
    /// FORMAT JSON)` e mostra os planos reais. Com `roll_back`, tudo roda
    /// numa transação desfeita no fim. `None` quando não há comando.
    pub fn start_actual_plan(
        session: Arc<dyn QuerySession>,
        sql: String,
        offset: usize,
        roll_back: bool,
        ctx: &egui::Context,
    ) -> Option<Self> {
        // No SQL Server o comando roda normalmente e o plano real vem junto
        // com os resultados, como no SSMS.
        let sql_server = session.engine() == Engine::SqlServer;
        let batch = if sql_server {
            Batch::showplan(&sql, "STATISTICS XML", roll_back)?
        } else {
            let batch = Batch::explain(&sql, "ANALYZE, BUFFERS, FORMAT JSON")?;
            if roll_back {
                batch.in_rolled_back_transaction()
            } else {
                batch
            }
        };
        let mut exec = Self::start_batch(session, batch, sql, offset, ctx);
        exec.keep_results = sql_server;
        exec.plan_mode = true;
        exec.rolled_back = roll_back;
        exec.tab = Tab::Plan;
        Some(exec)
    }

    /// Roda SQL montado pelo PGStudio (Alt+F1) mostrando os resultados,
    /// sem ligar erros ao texto do documento.
    pub fn start_generated(
        session: Arc<dyn QuerySession>,
        sql: String,
        ctx: &egui::Context,
    ) -> Self {
        let batch = Batch::generated(&sql);
        Self::start_batch(session, batch, String::new(), 0, ctx)
    }

    /// Se a execução é de plano (Ctrl+L).
    pub fn is_plan(&self) -> bool {
        self.plan_mode
    }

    /// Pede o cancelamento.
    pub fn cancel(&self) {
        if self.is_running() {
            self.session.cancel();
        }
    }

    pub fn is_running(&self) -> bool {
        !self.finished
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
            QueryEvent::CommandComplete { .. }
                if self.plan_mode
                    && (!self.keep_results || (self.open_result && self.last_is_showplan())) =>
            {
                if std::mem::take(&mut self.open_result) {
                    self.take_plan();
                }
            }
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
                    self.info(trf!(
                        "Warning: the results could not be paged to disk ({error}); they are kept in memory.",
                        error = e
                    ));
                }
            }
            QueryEvent::Notice(n) => self.messages.push(Message {
                // Sem nível (o PRINT do SQL Server): só o texto, como no SSMS.
                text: if n.severity.is_empty() {
                    describe(&n)
                } else {
                    format!("{}: {}", n.severity, describe(&n))
                },
                kind: MessageKind::Notice,
                position: None,
            }),
            QueryEvent::Error(e) => {
                self.open_result = false;
                let original = e
                    .position
                    .filter(|_| !self.batch.segments.is_empty())
                    .map(|p| self.batch.original_position(p.saturating_sub(1)));
                let position = original.map(|p| self.offset + p);
                let header = match original {
                    Some(p) => {
                        let (line, col) = line_col(&self.sql, p);
                        trf!(
                            "Msg {code}, Level {level}, Line {line}, Column {col}",
                            code = code(&e),
                            level = e.severity,
                            line = line,
                            col = col
                        )
                    }
                    None => trf!(
                        "Msg {code}, Level {level}",
                        code = code(&e),
                        level = e.severity
                    ),
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
                self.finished = true;
                self.elapsed = Some(self.started.elapsed());
                if self.rolled_back {
                    // Um erro ou o cancelamento param o lote antes do
                    // ROLLBACK, com a transação ainda aberta.
                    if canceled || self.outcome == Outcome::Failed {
                        self.session.execute("ROLLBACK".into(), Box::new(|_| {}));
                    }
                    self.info(tr("The changes made by the statements were rolled back.").into());
                }

                if canceled {
                    self.outcome = Outcome::Canceled;
                    self.info(tr("Query was cancelled by user.").into());
                } else if self.outcome == Outcome::Running {
                    self.outcome = Outcome::Succeeded;
                    if !self.messages.iter().any(|m| m.kind == MessageKind::Info) {
                        self.info(tr("Commands completed successfully.").into());
                    }
                }
                if let Some(path) = &self.file
                    && self.outcome != Outcome::Failed
                {
                    let text = trf!("The results were saved to {path}.", path = path.display());
                    self.info(text);
                }
                self.info(String::new());
                self.info(trf!(
                    "Total execution time: {time}",
                    time = format_elapsed(self.elapsed(), true)
                ));
                // Erros vão para a aba Messages, como no SSMS.
                if self.plan_mode {
                    if self.plans.is_empty() {
                        self.tab = Tab::Messages;
                    }
                } else if self.results.is_empty() || self.outcome == Outcome::Failed {
                    self.tab = Tab::Messages;
                }
                self.start_editors();
                return true;
            }
        }
        false
    }

    /// Depois da execução, vê se a grade pode ser editada: só quando o
    /// lote é um comando que só lê e trouxe um resultado; o resto fica
    /// só leitura com o motivo.
    fn start_editors(&mut self) {
        if self.plan_mode || self.file.is_some() || self.results.is_empty() {
            return;
        }
        let statements = pgstudio_sql_editor::lexer::statements(&self.sql);
        let reason = if self.multi {
            Some(tr(
                "The results are read-only: they come from multiple servers.",
            ))
        } else if self.batch.segments.is_empty() {
            Some(tr(
                "The results are read-only: they were produced by PostPilot, not by a query.",
            ))
        } else if self.outcome != Outcome::Succeeded {
            Some(tr(
                "The results are read-only: the query did not complete successfully.",
            ))
        } else if statements.len() != 1 || self.results.len() != 1 {
            Some(tr(
                "The results are read-only: the batch has more than one statement. \
                 Run the SELECT by itself to edit its results.",
            ))
        } else if changes_data(&self.sql) {
            Some(tr("The results are read-only: the statement changes data."))
        } else {
            None
        };
        let session = self.session.clone();
        match reason {
            Some(reason) => {
                for set in &mut self.results {
                    set.editor = Some(ResultEditor::read_only(session.clone(), reason, &self.ctx));
                }
            }
            None => {
                let sql = self.sql[statements[0].clone()].to_string();
                self.results[0].editor = Some(ResultEditor::new(session, &sql, &self.ctx));
            }
        }
    }

    /// Se o último resultado é um plano do SQL Server (uma coluna com o
    /// Showplan XML).
    fn last_is_showplan(&self) -> bool {
        self.results.last().is_some_and(|set| {
            matches!(set.store().columns(), [c] if c == pgstudio_execution_plan::SHOWPLAN_COLUMN)
        })
    }

    /// O resultado que acabou de chegar é a saída de um `EXPLAIN (FORMAT
    /// JSON)` ou um Showplan XML: vira um plano e sai da lista de
    /// resultados.
    fn take_plan(&mut self) {
        let Some(set) = self.results.pop() else {
            return;
        };
        let json: String = {
            let mut store = set.store();
            (0..store.len())
                .filter_map(|r| store.row(r).into_iter().next().flatten())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let statement = self
            .batch
            .statements
            .get(self.plans.len())
            .cloned()
            .unwrap_or_default();
        let parsed = match self.session.engine() {
            Engine::PostgreSql | Engine::Redis => pgstudio_execution_plan::parse(&json, &statement),
            Engine::SqlServer => pgstudio_execution_plan::parse_showplan(&json),
        };
        match parsed {
            Ok(plans) => {
                for plan in plans {
                    let view = PlanView::new(&plan);
                    self.plans.push((plan, view));
                }
            }
            Err(e) => self.messages.push(Message {
                text: e.to_string(),
                kind: MessageKind::Error,
                position: None,
            }),
        }
    }

    /// Acrescenta uma linha informativa à aba Messages (o que a edição na
    /// grade gravou, por exemplo).
    pub fn push_info(&mut self, text: String) {
        self.info(text);
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
        text.push_str(&format!("\n{}: {d}", tr("DETAIL")));
    }
    if let Some(h) = &m.hint {
        text.push_str(&format!("\n{}: {h}", tr("HINT")));
    }
    text
}

/// `(n rows affected)`, como no SSMS.
pub fn rows_affected(rows: u64) -> String {
    if rows == 1 {
        tr("(1 row affected)").into()
    } else {
        trf!("({rows} rows affected)", rows = rows)
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

/// Aviso de células cortadas num arquivo do Excel.
pub fn truncated_text(cells: u64) -> String {
    trf!(
        "{n} cell(s) longer than the 32,767 characters an Excel cell holds were cut.",
        n = cells
    )
}

/// Para onde vai o Results to File.
enum FileSink {
    /// CSV, TSV, JSON ou INSERT: um resultado depois do outro, com uma
    /// linha em branco entre eles.
    Text {
        idle: Option<BufWriter<std::fs::File>>,
        writer: Option<Writer<BufWriter<std::fs::File>>>,
        format: Format,
        results: usize,
    },
    /// Excel: uma planilha por resultado. `None` depois de um erro.
    Book(Option<Workbook<BufWriter<std::fs::File>>>),
}

impl FileSink {
    /// Começa um resultado.
    fn columns(&mut self, columns: Vec<String>) -> std::io::Result<()> {
        match self {
            FileSink::Text {
                idle,
                writer,
                format,
                results,
            } => {
                let Some(mut out) = idle.take() else {
                    return Ok(());
                };
                if *results > 0 {
                    writeln!(out)?;
                }
                *results += 1;
                *writer = Some(Writer::new(out, *format, columns, INSERT_TABLE)?);
                Ok(())
            }
            FileSink::Book(book) => match book {
                Some(b) => b.sheet(&columns).inspect_err(|_| *book = None),
                None => Ok(()),
            },
        }
    }

    fn row(&mut self, row: &[Option<String>]) -> std::io::Result<()> {
        match self {
            FileSink::Text { writer, .. } => match writer {
                Some(w) => w.row(row).inspect_err(|_| *writer = None),
                None => Ok(()),
            },
            FileSink::Book(book) => match book {
                Some(b) => b.row(row).inspect_err(|_| *book = None),
                None => Ok(()),
            },
        }
    }

    /// Fecha o resultado atual.
    fn end_result(&mut self) -> std::io::Result<()> {
        if let FileSink::Text { idle, writer, .. } = self
            && let Some(w) = writer.take()
        {
            *idle = Some(w.finish()?);
        }
        Ok(())
    }

    /// Fecha o arquivo. Devolve as células cortadas (Excel).
    fn finish(&mut self) -> std::io::Result<u64> {
        self.end_result()?;
        match self {
            FileSink::Text { idle, .. } => {
                if let Some(out) = idle {
                    out.flush()?;
                }
                Ok(0)
            }
            FileSink::Book(book) => match book.take() {
                Some(b) => {
                    let cut = b.truncated();
                    b.finish()?.flush()?;
                    Ok(cut)
                }
                None => Ok(0),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explain_batch_maps_positions_back() {
        let sql = "SELECT 1;\n  SELECT bad FROM t;";
        let b = Batch::explain(sql, "FORMAT JSON").unwrap();
        assert_eq!(
            b.sent,
            "EXPLAIN (FORMAT JSON) SELECT 1;\nEXPLAIN (FORMAT JSON) SELECT bad FROM t"
        );
        assert_eq!(b.statements, ["SELECT 1", "SELECT bad FROM t"]);
        let bad = b.sent.find("bad").unwrap();
        assert_eq!(b.original_position(bad), sql.find("bad").unwrap());
        // Dentro do EXPLAIN acrescentado: começo do comando.
        assert_eq!(b.original_position(40), sql.find("SELECT bad").unwrap());
        assert_eq!(b.original_position(3), 0);
        assert!(Batch::explain(" -- nada", "FORMAT JSON").is_none());
        let plain = Batch::plain("SELECT 1");
        assert_eq!(plain.original_position(7), 7);
    }

    #[test]
    fn rolled_back_batches_keep_positions() {
        let sql = "UPDATE t SET bad = 1";
        let b = Batch::explain(sql, "ANALYZE, FORMAT JSON")
            .unwrap()
            .in_rolled_back_transaction();
        assert_eq!(
            b.sent,
            "BEGIN;\nEXPLAIN (ANALYZE, FORMAT JSON) UPDATE t SET bad = 1;\nROLLBACK"
        );
        let bad = b.sent.find("bad").unwrap();
        assert_eq!(b.original_position(bad), sql.find("bad").unwrap());
    }

    #[test]
    fn data_changes_are_detected_by_keyword() {
        assert!(changes_data("update t set a = 1"));
        assert!(changes_data(
            "WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x"
        ));
        assert!(changes_data(
            "select 1; merge into t using s on true when matched then do nothing"
        ));
        assert!(!changes_data("select 'update' as \"delete\" -- insert"));
        assert!(!changes_data("select * from updates"));
    }

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
