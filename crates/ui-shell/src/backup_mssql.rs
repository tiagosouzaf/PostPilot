//! "Back Up Database" e "Restore Database" do SQL Server: o `BACKUP` e o
//! `RESTORE` rodam no servidor e o arquivo `.bak` fica no disco dele (não
//! há um `pg_dump` que traga o backup para o computador de quem usa o
//! app). As janelas mostram o progresso (`STATS`), deixam parar e têm o
//! Script, como as do SSMS.
//!
//! Aqui também ficam o SQL das duas operações ([`backup_sql`] e
//! [`restore_sql`]) e a leitura do que há num arquivo de backup
//! (`RESTORE HEADERONLY` e `RESTORE FILELISTONLY`).

use pgstudio_i18n::{tr, trf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::{self, Align, Layout};
use pgstudio_core::query::{QueryEvent, QuerySession};

/// Tipo do backup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupKind {
    Full,
    /// Só o que mudou desde o último backup completo.
    Differential,
    /// O log de transações (o banco precisa do recovery model FULL).
    Log,
}

impl BackupKind {
    pub fn label(self) -> &'static str {
        tr(match self {
            BackupKind::Full => "Full",
            BackupKind::Differential => "Differential",
            BackupKind::Log => "Transaction log",
        })
    }

    /// Como o SSMS chama o backup no `NAME`.
    fn title(self) -> &'static str {
        match self {
            BackupKind::Full => "Full Database Backup",
            BackupKind::Differential => "Differential Database Backup",
            BackupKind::Log => "Transaction Log Backup",
        }
    }
}

/// O que a janela "Back Up Database" monta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupOptions {
    pub database: String,
    /// Caminho do arquivo no disco do servidor.
    pub file: String,
    pub kind: BackupKind,
    /// Não entra na sequência de backups (só para o completo e o de log).
    pub copy_only: bool,
    pub compression: bool,
    pub checksum: bool,
    /// Substitui o que há no arquivo (`INIT`); sem ela, acrescenta.
    pub overwrite: bool,
}

fn lit(text: &str) -> String {
    format!("N'{}'", text.replace('\'', "''"))
}

fn bracket(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

/// O `BACKUP` das opções, com `STATS = 10` para o progresso.
pub fn backup_sql(o: &BackupOptions) -> String {
    let what = match o.kind {
        BackupKind::Log => "LOG",
        _ => "DATABASE",
    };
    let mut with = Vec::new();
    if o.kind == BackupKind::Differential {
        with.push("DIFFERENTIAL".to_string());
    }
    if o.copy_only && o.kind != BackupKind::Differential {
        with.push("COPY_ONLY".to_string());
    }
    if o.compression {
        with.push("COMPRESSION".to_string());
    }
    if o.checksum {
        with.push("CHECKSUM".to_string());
    }
    with.push(if o.overwrite { "INIT" } else { "NOINIT" }.to_string());
    with.push(format!(
        "NAME = {}",
        lit(&format!("{}-{}", o.database, o.kind.title()))
    ));
    with.push("STATS = 10".to_string());
    format!(
        "BACKUP {what} {} TO DISK = {} WITH {}",
        bracket(&o.database),
        lit(o.file.trim()),
        with.join(", ")
    )
}

/// Junta uma pasta do servidor e um nome de arquivo, com a barra que a
/// pasta usa (Windows ou Linux).
pub fn server_path(dir: &str, file: &str) -> String {
    let sep = if dir.contains('\\') { '\\' } else { '/' };
    let dir = dir.trim_end_matches(['\\', '/']);
    format!("{dir}{sep}{file}")
}

/// As pastas padrão do servidor (backup, dados e log), numa linha.
/// A pasta de backups só existe como propriedade a partir do SQL Server
/// 2019; antes disso a sugestão é a pasta de dados.
const DEFAULTS_SQL: &str = "SELECT CAST(COALESCE(SERVERPROPERTY('InstanceDefaultBackupPath'), \
     SERVERPROPERTY('InstanceDefaultDataPath')) AS nvarchar(260)), \
     CAST(SERVERPROPERTY('InstanceDefaultDataPath') AS nvarchar(260)), \
     CAST(SERVERPROPERTY('InstanceDefaultLogPath') AS nvarchar(260))";

/// Um backup dentro de um arquivo (`RESTORE HEADERONLY`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupSet {
    /// O `FILE = n` do RESTORE.
    pub position: u32,
    pub name: String,
    pub database: String,
    pub kind: BackupKind,
    pub finished: String,
}

impl BackupSet {
    pub fn label(&self) -> String {
        format!(
            "{}: {} - {} ({})",
            self.position,
            self.database,
            self.kind.label(),
            self.finished
        )
    }
}

/// Um arquivo do banco dentro do backup (`RESTORE FILELISTONLY`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupFile {
    pub logical: String,
    /// `true` para o log; os outros são de dados.
    pub log: bool,
}

/// Um resultado com os nomes das colunas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

impl Table {
    fn text(&self, row: &[Option<String>], column: &str) -> String {
        self.columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(column))
            .and_then(|i| row.get(i).cloned().flatten())
            .unwrap_or_default()
    }
}

/// Os backups do arquivo, do `RESTORE HEADERONLY`.
pub fn parse_header(t: &Table) -> Vec<BackupSet> {
    t.rows
        .iter()
        .filter_map(|r| {
            let kind = match t.text(r, "BackupType").trim() {
                "1" => BackupKind::Full,
                "5" => BackupKind::Differential,
                "2" => BackupKind::Log,
                _ => return None,
            };
            Some(BackupSet {
                position: t.text(r, "Position").trim().parse().ok()?,
                name: t.text(r, "BackupName"),
                database: t.text(r, "DatabaseName"),
                kind,
                finished: t.text(r, "BackupFinishDate"),
            })
        })
        .collect()
}

/// Os arquivos do banco, do `RESTORE FILELISTONLY`.
pub fn parse_files(t: &Table) -> Vec<BackupFile> {
    t.rows
        .iter()
        .map(|r| BackupFile {
            logical: t.text(r, "LogicalName"),
            log: t.text(r, "Type").trim() == "L",
        })
        .collect()
}

pub fn header_sql(file: &str) -> String {
    format!("RESTORE HEADERONLY FROM DISK = {}", lit(file.trim()))
}

pub fn files_sql(file: &str, position: u32) -> String {
    format!(
        "RESTORE FILELISTONLY FROM DISK = {} WITH FILE = {position}",
        lit(file.trim())
    )
}

/// O que a janela "Restore Database" monta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreOptions {
    /// Caminho do `.bak` no disco do servidor.
    pub file: String,
    /// Banco de destino (novo, ou um existente com `replace`).
    pub database: String,
    /// Qual backup do arquivo.
    pub position: u32,
    /// Sobrescreve um banco que já existe (`REPLACE`).
    pub replace: bool,
    /// Derruba as conexões do banco existente antes.
    pub close_connections: bool,
    /// Deixa o banco pronto para uso (`RECOVERY`); sem ela, espera mais
    /// backups (`NORECOVERY`).
    pub recovery: bool,
}

/// Para onde vai cada arquivo do backup: as pastas padrão do servidor, com
/// o nome do banco de destino (`vendas.mdf`, `vendas_2.ndf`,
/// `vendas_log.ldf`).
pub fn relocation(
    files: &[BackupFile],
    database: &str,
    data_dir: &str,
    log_dir: &str,
) -> Vec<(String, String)> {
    let (mut data, mut logs) = (0, 0);
    files
        .iter()
        .map(|f| {
            let path = if f.log {
                logs += 1;
                let n = if logs == 1 {
                    String::new()
                } else {
                    format!("_{logs}")
                };
                server_path(log_dir, &format!("{database}_log{n}.ldf"))
            } else {
                data += 1;
                if data == 1 {
                    server_path(data_dir, &format!("{database}.mdf"))
                } else {
                    server_path(data_dir, &format!("{database}_{data}.ndf"))
                }
            };
            (f.logical.clone(), path)
        })
        .collect()
}

/// O `RESTORE` das opções; `moves` vem de [`relocation`].
pub fn restore_sql(o: &RestoreOptions, moves: &[(String, String)]) -> String {
    let db = o.database.trim();
    let mut sql = String::new();
    if o.close_connections {
        sql.push_str(&format!(
            "IF DB_ID({}) IS NOT NULL ALTER DATABASE {} SET SINGLE_USER WITH ROLLBACK IMMEDIATE;\n",
            lit(db),
            bracket(db)
        ));
    }
    let mut with = vec![format!("FILE = {}", o.position)];
    for (logical, path) in moves {
        with.push(format!("MOVE {} TO {}", lit(logical), lit(path)));
    }
    if o.replace {
        with.push("REPLACE".to_string());
    }
    with.push(if o.recovery { "RECOVERY" } else { "NORECOVERY" }.to_string());
    with.push("STATS = 10".to_string());
    sql.push_str(&format!(
        "RESTORE DATABASE {} FROM DISK = {} WITH {}",
        bracket(db),
        lit(o.file.trim()),
        with.join(", ")
    ));
    sql
}

/// Devolve o banco ao uso normal se um RESTORE que falhou o deixou em
/// SINGLE_USER.
fn reopen_sql(database: &str) -> String {
    format!(
        "IF DATABASEPROPERTYEX({}, 'UserAccess') = 'SINGLE_USER' \
         ALTER DATABASE {} SET MULTI_USER",
        lit(database.trim()),
        bracket(database.trim())
    )
}

/// O `NN` de "NN percent processed.".
fn percent(message: &str) -> Option<u32> {
    let (n, rest) = message.trim().split_once(' ')?;
    rest.starts_with("percent")
        .then(|| n.parse().ok())
        .flatten()
}

/// O que as janelas pedem à casca.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// Linhas para o Output.
    Log(Vec<String>),
    /// O SQL para uma janela de consulta (no master).
    Script(String),
    /// O restore terminou: a lista de bancos mudou.
    Restored(Vec<String>),
    Closed,
}

enum Msg {
    /// Resultados de uma leitura (com a etiqueta de quem pediu).
    Read(&'static str, Result<Vec<Table>, String>),
    Line(String),
    Done(Result<(), String>),
}

enum Phase {
    Editing,
    Running,
    Done(Result<(), String>),
}

/// Roda `sql` e manda os resultados com `tag`.
fn read(
    session: &Arc<dyn QuerySession>,
    sql: String,
    tag: &'static str,
    tx: Sender<Msg>,
    ctx: &egui::Context,
) {
    let ctx = ctx.clone();
    let mut tables: Vec<Table> = Vec::new();
    let mut error = None;
    session.execute(
        sql,
        Box::new(move |event| match event {
            QueryEvent::Columns(columns) => tables.push(Table {
                columns,
                rows: Vec::new(),
            }),
            QueryEvent::Row(r) => {
                if let Some(t) = tables.last_mut() {
                    t.rows.push(r);
                }
            }
            QueryEvent::Error(e) => {
                error.get_or_insert(e.message);
            }
            QueryEvent::Finished { .. } => {
                let reply = match error.take() {
                    Some(e) => Err(e),
                    None => Ok(std::mem::take(&mut tables)),
                };
                let _ = tx.send(Msg::Read(tag, reply));
                ctx.request_repaint();
            }
            _ => {}
        }),
    );
}

/// Roda o BACKUP ou o RESTORE: os avisos viram linhas de progresso.
/// `on_error` roda depois de um erro (para desfazer o SINGLE_USER).
fn run(
    session: &Arc<dyn QuerySession>,
    sql: String,
    mut on_error: Option<String>,
    tx: Sender<Msg>,
    ctx: &egui::Context,
) {
    let ctx = ctx.clone();
    let again = session.clone();
    let mut error: Option<String> = None;
    session.execute(
        sql,
        Box::new(move |event| {
            match event {
                QueryEvent::Notice(n) => {
                    let _ = tx.send(Msg::Line(n.message));
                }
                QueryEvent::Error(e) => {
                    // O primeiro erro é o que explica; os outros são
                    // "RESTORE DATABASE is terminating abnormally".
                    if error.is_none() {
                        error = Some(e.message);
                    }
                }
                QueryEvent::Finished { canceled } => {
                    let result = match error.take() {
                        Some(e) => Err(e),
                        None if canceled => Err(tr("Stopped.").to_string()),
                        None => Ok(()),
                    };
                    if result.is_err()
                        && let Some(sql) = on_error.take()
                    {
                        again.execute(sql, Box::new(|_| {}));
                    }
                    let _ = tx.send(Msg::Done(result));
                }
                _ => {}
            }
            ctx.request_repaint();
        }),
    );
}

/// O que as duas janelas têm em comum: o progresso e o fim.
struct Progress {
    phase: Phase,
    percent: Option<u32>,
    lines: Vec<String>,
}

impl Progress {
    fn new() -> Self {
        Progress {
            phase: Phase::Editing,
            percent: None,
            lines: Vec::new(),
        }
    }

    fn start(&mut self) {
        self.phase = Phase::Running;
        self.percent = Some(0);
        self.lines.clear();
    }

    fn line(&mut self, l: String) {
        if let Some(p) = percent(&l) {
            self.percent = Some(p);
        }
        self.lines.push(l);
    }

    /// O estado e as linhas, abaixo do formulário.
    fn show(&self, ui: &mut egui::Ui, running: &str, done: &str, failed: &str) {
        match &self.phase {
            Phase::Editing => {}
            Phase::Running => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(running);
                    ui.add(
                        egui::ProgressBar::new(self.percent.unwrap_or(0) as f32 / 100.0)
                            .show_percentage()
                            .desired_width(220.0),
                    );
                });
            }
            Phase::Done(Ok(())) => {
                // i18n-ignore
                ui.label(format!("{} {done}", egui_phosphor::regular::CHECK_CIRCLE));
            }
            Phase::Done(Err(e)) => {
                // i18n-ignore
                ui.colored_label(ui.visuals().error_fg_color, format!("{failed} {e}"));
            }
        }
        if !self.lines.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("backup_progress")
                .max_height(110.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for l in &self.lines {
                        ui.monospace(l);
                    }
                });
        }
    }
}

fn script_button(ui: &mut egui::Ui) -> bool {
    ui.button(format!(
        "{} {}",
        egui_phosphor::regular::SCROLL,
        tr("Script")
    ))
    .clicked()
}

/// A janela "Back Up Database" do SQL Server.
pub struct BackupDialog {
    pub options: BackupOptions,
    /// Sessão no master, que roda o BACKUP.
    session: Arc<dyn QuerySession>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    progress: Progress,
    error: Option<String>,
}

impl BackupDialog {
    /// Abre para `database`; lê a pasta de backups do servidor para
    /// sugerir o arquivo.
    pub fn open(database: String, session: Arc<dyn QuerySession>, ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        read(
            &session,
            DEFAULTS_SQL.to_string(),
            "defaults",
            tx.clone(),
            ctx,
        );
        BackupDialog {
            options: BackupOptions {
                file: String::new(),
                database,
                kind: BackupKind::Full,
                copy_only: false,
                compression: false,
                checksum: true,
                overwrite: true,
            },
            session,
            tx,
            rx,
            progress: Progress::new(),
            error: None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.progress.phase, Phase::Running)
    }

    /// Como acabou (None: ainda não rodou ou está rodando).
    pub fn result(&self) -> Option<&Result<(), String>> {
        match &self.progress.phase {
            Phase::Done(r) => Some(r),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn percent(&self) -> Option<u32> {
        self.progress.percent
    }

    fn invalid(&self) -> Option<&'static str> {
        self.options
            .file
            .trim()
            .is_empty()
            .then(|| tr("Enter the backup file path (on the server)."))
    }

    /// Começa o backup.
    pub fn start(&mut self, ctx: &egui::Context) -> Option<Request> {
        if let Some(why) = self.invalid() {
            self.error = Some(why.to_string());
            return None;
        }
        self.error = None;
        let sql = backup_sql(&self.options);
        self.progress.start();
        run(&self.session, sql.clone(), None, self.tx.clone(), ctx);
        Some(Request::Log(vec![
            trf!(
                "Back Up: {database} to {file} (on the server)...",
                database = self.options.database,
                file = self.options.file.trim()
            ),
            format!("  {sql}"),
        ]))
    }

    pub fn cancel(&mut self) {
        if self.is_running() {
            self.session.cancel();
        }
    }

    /// Recebe o progresso; devolve as linhas para o Output.
    pub fn step(&mut self) -> Option<Request> {
        let mut log = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Read(_, Ok(tables)) => {
                    let dir = tables
                        .first()
                        .and_then(|t| t.rows.first())
                        .and_then(|r| r.first().cloned().flatten())
                        .unwrap_or_default();
                    if self.options.file.is_empty() && !dir.is_empty() {
                        self.options.file =
                            server_path(&dir, &format!("{}.bak", self.options.database));
                    }
                }
                Msg::Read(_, Err(e)) => self.error = Some(e),
                Msg::Line(l) => {
                    log.push(format!("  {l}"));
                    self.progress.line(l);
                }
                Msg::Done(r) => {
                    log.push(match &r {
                        Ok(()) => trf!(
                            "Back Up: completed ({file}).",
                            file = self.options.file.trim()
                        ),
                        Err(e) => trf!("Back Up: failed ({e}).", e = e),
                    });
                    self.progress.phase = Phase::Done(r);
                }
            }
        }
        (!log.is_empty()).then_some(Request::Log(log))
    }

    /// Desenha a janela.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Request> {
        let mut request = self.step();
        let modal = crate::theme::modal(egui::Id::new("backup_mssql")).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.heading(trf!(
                "Back Up Database - {database}",
                database = self.options.database
            ));
            ui.separator();
            let editing = matches!(self.progress.phase, Phase::Editing);
            ui.add_enabled_ui(editing, |ui| self.form(ui));
            ui.separator();
            self.progress.show(
                ui,
                tr("Backing up..."),
                &trf!(
                    "The backup of {database} completed.",
                    database = self.options.database
                ),
                tr("The backup failed:"),
            );
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            let bar = egui::vec2(ui.available_width(), 26.0);
            ui.allocate_ui_with_layout(bar, Layout::right_to_left(Align::Center), |ui| match &self
                .progress
                .phase
            {
                Phase::Editing => {
                    if ui.button(tr("Cancel")).clicked() {
                        request = Some(Request::Closed);
                    }
                    if ui.button(tr("OK")).clicked() {
                        request = self.start(ctx).or(request.take());
                    }
                    if script_button(ui) {
                        match self.invalid() {
                            Some(why) => self.error = Some(why.to_string()),
                            None => {
                                request = Some(Request::Script(format!(
                                    "{}\n",
                                    backup_sql(&self.options)
                                )));
                            }
                        }
                    }
                }
                Phase::Running => {
                    if ui.button(tr("Stop")).clicked() {
                        self.cancel();
                    }
                }
                Phase::Done(_) => {
                    if ui.button(tr("Close")).clicked() {
                        request = Some(Request::Closed);
                    }
                }
            });
        });
        if modal.should_close() && request.is_none() && !self.is_running() {
            request = Some(Request::Closed);
        }
        request
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        let o = &mut self.options;
        egui::Grid::new("backup_mssql_form")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label(tr("Database:"));
                ui.label(&o.database);
                ui.end_row();
                ui.label(tr("Backup type:"));
                ui.horizontal(|ui| {
                    for kind in [BackupKind::Full, BackupKind::Differential, BackupKind::Log] {
                        ui.radio_value(&mut o.kind, kind, kind.label());
                    }
                });
                ui.end_row();
                ui.label(tr("File on the server:"));
                ui.add(egui::TextEdit::singleline(&mut o.file).desired_width(460.0));
                ui.end_row();
            });
        ui.weak(tr(
            "SQL Server writes the backup on the server's own disk, not on this computer.",
        ));
        ui.add_space(6.0);
        ui.checkbox(
            &mut o.overwrite,
            tr("Overwrite the backups already in the file"),
        );
        ui.add_enabled(
            o.kind != BackupKind::Differential,
            egui::Checkbox::new(
                &mut o.copy_only,
                tr("Copy-only backup (does not affect the backup sequence)"),
            ),
        );
        ui.checkbox(&mut o.compression, tr("Compress the backup"));
        ui.checkbox(&mut o.checksum, tr("Verify page checksums"));
    }
}

/// A janela "Restore Database" do SQL Server.
pub struct RestoreDialog {
    pub options: RestoreOptions,
    /// Sessão no master.
    session: Arc<dyn QuerySession>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Pastas padrão de dados e de log do servidor.
    dirs: (String, String),
    /// Os backups do arquivo, depois de lido.
    sets: Vec<BackupSet>,
    files: Vec<BackupFile>,
    /// Lendo o arquivo.
    listing: bool,
    /// Se o nome do destino ainda é o sugerido (segue o backup escolhido).
    suggested: bool,
    progress: Progress,
    error: Option<String>,
}

impl RestoreDialog {
    /// Abre com `database` de destino sugerido (None: o do backup).
    pub fn open(
        database: Option<String>,
        session: Arc<dyn QuerySession>,
        ctx: &egui::Context,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        read(
            &session,
            DEFAULTS_SQL.to_string(),
            "defaults",
            tx.clone(),
            ctx,
        );
        RestoreDialog {
            options: RestoreOptions {
                file: String::new(),
                database: database.clone().unwrap_or_default(),
                position: 1,
                replace: database.is_some(),
                close_connections: database.is_some(),
                recovery: true,
            },
            session,
            tx,
            rx,
            dirs: Default::default(),
            sets: Vec::new(),
            files: Vec::new(),
            listing: false,
            suggested: database.is_none(),
            progress: Progress::new(),
            error: None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.progress.phase, Phase::Running)
    }

    pub fn is_listing(&self) -> bool {
        self.listing
    }

    pub fn result(&self) -> Option<&Result<(), String>> {
        match &self.progress.phase {
            Phase::Done(r) => Some(r),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn sets(&self) -> &[BackupSet] {
        &self.sets
    }

    pub fn files(&self) -> &[BackupFile] {
        &self.files
    }

    /// Lê os backups do arquivo.
    pub fn list(&mut self, ctx: &egui::Context) {
        if self.options.file.trim().is_empty() {
            self.error = Some(tr("Enter the backup file path (on the server).").to_string());
            return;
        }
        self.error = None;
        self.listing = true;
        self.sets.clear();
        self.files.clear();
        read(
            &self.session,
            header_sql(&self.options.file),
            "header",
            self.tx.clone(),
            ctx,
        );
    }

    /// Escolhe o backup `position` do arquivo e lê os arquivos dele.
    pub fn choose(&mut self, position: u32, ctx: &egui::Context) {
        self.options.position = position;
        if self.suggested
            && let Some(set) = self.sets.iter().find(|s| s.position == position)
        {
            self.options.database = set.database.clone();
        }
        self.listing = true;
        self.files.clear();
        read(
            &self.session,
            files_sql(&self.options.file, position),
            "files",
            self.tx.clone(),
            ctx,
        );
    }

    /// Para onde vão os arquivos do banco.
    pub fn moves(&self) -> Vec<(String, String)> {
        relocation(
            &self.files,
            self.options.database.trim(),
            &self.dirs.0,
            &self.dirs.1,
        )
    }

    fn invalid(&self) -> Option<&'static str> {
        if self.files.is_empty() {
            Some(tr("Read the backup file first."))
        } else if self.options.database.trim().is_empty() {
            Some(tr("Enter the destination database."))
        } else {
            None
        }
    }

    /// O SQL do restore, se a janela estiver pronta.
    pub fn sql(&self) -> Option<String> {
        self.invalid()
            .is_none()
            .then(|| restore_sql(&self.options, &self.moves()))
    }

    /// Começa o restore.
    pub fn start(&mut self, ctx: &egui::Context) -> Option<Request> {
        if let Some(why) = self.invalid() {
            self.error = Some(why.to_string());
            return None;
        }
        self.error = None;
        let sql = restore_sql(&self.options, &self.moves());
        self.progress.start();
        let reopen = self
            .options
            .close_connections
            .then(|| reopen_sql(&self.options.database));
        run(&self.session, sql.clone(), reopen, self.tx.clone(), ctx);
        let mut log = vec![trf!(
            "Restore: {file} into {database}...",
            file = self.options.file.trim(),
            database = self.options.database.trim()
        )];
        log.extend(sql.lines().map(|l| format!("  {l}")));
        Some(Request::Log(log))
    }

    pub fn cancel(&mut self) {
        if self.is_running() {
            self.session.cancel();
        }
    }

    /// Recebe as leituras e o progresso.
    pub fn step(&mut self, ctx: &egui::Context) -> Option<Request> {
        let mut log = Vec::new();
        let mut restored = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Read("defaults", Ok(tables)) => {
                    if let Some(r) = tables.first().and_then(|t| t.rows.first()) {
                        let cell = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
                        self.dirs = (cell(1), cell(2));
                        if self.options.file.is_empty() && !cell(0).is_empty() {
                            self.options.file = server_path(&cell(0), "");
                        }
                    }
                }
                Msg::Read("header", Ok(tables)) => {
                    self.sets = tables.first().map(parse_header).unwrap_or_default();
                    // O último backup completo, como o SSMS sugere.
                    let pick = self
                        .sets
                        .iter()
                        .rev()
                        .find(|s| s.kind == BackupKind::Full)
                        .or(self.sets.last())
                        .map(|s| s.position);
                    match pick {
                        Some(position) => self.choose(position, ctx),
                        None => {
                            self.listing = false;
                            self.error = Some(tr("The file has no backups.").to_string());
                        }
                    }
                }
                Msg::Read(_, Ok(tables)) => {
                    self.listing = false;
                    self.files = tables.first().map(parse_files).unwrap_or_default();
                }
                Msg::Read(_, Err(e)) => {
                    self.listing = false;
                    self.error = Some(e);
                }
                Msg::Line(l) => {
                    log.push(format!("  {l}"));
                    self.progress.line(l);
                }
                Msg::Done(r) => {
                    log.push(match &r {
                        Ok(()) => trf!(
                            "Restore: completed ({database}).",
                            database = self.options.database.trim()
                        ),
                        Err(e) => trf!("Restore: failed ({e}).", e = e),
                    });
                    restored = r.is_ok();
                    self.progress.phase = Phase::Done(r);
                }
            }
        }
        if restored {
            Some(Request::Restored(log))
        } else {
            (!log.is_empty()).then_some(Request::Log(log))
        }
    }

    /// Desenha a janela.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Request> {
        let mut request = self.step(ctx);
        let modal = crate::theme::modal(egui::Id::new("restore_mssql")).show(ctx, |ui| {
            ui.set_width(660.0);
            ui.heading(tr("Restore Database"));
            ui.separator();
            let editing = matches!(self.progress.phase, Phase::Editing);
            ui.add_enabled_ui(editing, |ui| self.form(ui, ctx));
            ui.separator();
            self.progress.show(
                ui,
                tr("Restoring..."),
                &trf!(
                    "The restore of {database} completed.",
                    database = self.options.database.trim()
                ),
                tr("The restore failed:"),
            );
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            let bar = egui::vec2(ui.available_width(), 26.0);
            ui.allocate_ui_with_layout(bar, Layout::right_to_left(Align::Center), |ui| match &self
                .progress
                .phase
            {
                Phase::Editing => {
                    if ui.button(tr("Cancel")).clicked() {
                        request = Some(Request::Closed);
                    }
                    if ui.button(tr("OK")).clicked() {
                        request = self.start(ctx).or(request.take());
                    }
                    if script_button(ui) {
                        match self.sql() {
                            Some(sql) => request = Some(Request::Script(format!("{sql}\n"))),
                            None => self.error = self.invalid().map(str::to_string),
                        }
                    }
                }
                Phase::Running => {
                    if ui.button(tr("Stop")).clicked() {
                        self.cancel();
                    }
                }
                Phase::Done(_) => {
                    if ui.button(tr("Close")).clicked() {
                        request = Some(Request::Closed);
                    }
                }
            });
        });
        if modal.should_close() && request.is_none() && !self.is_running() {
            request = Some(Request::Closed);
        }
        request
    }

    fn form(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut list = false;
        let mut choose = None;
        egui::Grid::new("restore_mssql_form")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label(tr("File on the server:"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.options.file).desired_width(400.0));
                    if ui
                        .add_enabled(!self.listing, egui::Button::new(tr("Read")))
                        .clicked()
                    {
                        list = true;
                    }
                    if self.listing {
                        ui.spinner();
                    }
                });
                ui.end_row();
                ui.label(tr("Backup:"));
                let current = self
                    .sets
                    .iter()
                    .find(|s| s.position == self.options.position)
                    .map(BackupSet::label)
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("restore_set")
                    .selected_text(current)
                    .width(460.0)
                    .show_ui(ui, |ui| {
                        for s in &self.sets {
                            if ui
                                .selectable_label(s.position == self.options.position, s.label())
                                .clicked()
                            {
                                choose = Some(s.position);
                            }
                        }
                    });
                ui.end_row();
                ui.label(tr("Destination database:"));
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.options.database).desired_width(300.0),
                    )
                    .changed()
                {
                    self.suggested = false;
                }
                ui.end_row();
            });
        ui.weak(tr(
            "SQL Server reads the backup from the server's own disk, not from this computer.",
        ));
        if !self.files.is_empty() {
            ui.add_space(4.0);
            ui.strong(tr("Restore the database files as"));
            for (logical, path) in self.moves() {
                ui.horizontal(|ui| {
                    ui.monospace(logical);
                    ui.label(egui_phosphor::regular::ARROW_RIGHT);
                    ui.monospace(path);
                });
            }
        }
        ui.add_space(6.0);
        ui.checkbox(
            &mut self.options.replace,
            tr("Overwrite the existing database (WITH REPLACE)"),
        );
        ui.checkbox(
            &mut self.options.close_connections,
            tr("Close existing connections to the destination database"),
        );
        ui.checkbox(
            &mut self.options.recovery,
            tr("Leave the database ready to use (RECOVERY)"),
        );
        if list {
            self.list(ctx);
        }
        if let Some(position) = choose {
            self.choose(position, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> BackupOptions {
        BackupOptions {
            database: "vendas".into(),
            file: "/var/opt/mssql/data/vendas.bak".into(),
            kind: BackupKind::Full,
            copy_only: false,
            compression: false,
            checksum: true,
            overwrite: true,
        }
    }

    #[test]
    fn backup_commands_follow_the_options() {
        let mut o = options();
        assert_eq!(
            backup_sql(&o),
            "BACKUP DATABASE [vendas] TO DISK = N'/var/opt/mssql/data/vendas.bak' \
             WITH CHECKSUM, INIT, NAME = N'vendas-Full Database Backup', STATS = 10"
        );
        o.kind = BackupKind::Differential;
        o.copy_only = true;
        o.compression = true;
        o.checksum = false;
        o.overwrite = false;
        // O diferencial não pode ser copy-only.
        assert_eq!(
            backup_sql(&o),
            "BACKUP DATABASE [vendas] TO DISK = N'/var/opt/mssql/data/vendas.bak' \
             WITH DIFFERENTIAL, COMPRESSION, NOINIT, \
             NAME = N'vendas-Differential Database Backup', STATS = 10"
        );
        o.kind = BackupKind::Log;
        o.database = "a]b".into();
        o.file = "C:\\Backup\\it's.trn".into();
        assert_eq!(
            backup_sql(&o),
            "BACKUP LOG [a]]b] TO DISK = N'C:\\Backup\\it''s.trn' \
             WITH COPY_ONLY, COMPRESSION, NOINIT, \
             NAME = N'a]b-Transaction Log Backup', STATS = 10"
        );
    }

    #[test]
    fn server_paths_use_the_server_separator() {
        assert_eq!(
            server_path("/var/opt/mssql/data/", "x.bak"),
            "/var/opt/mssql/data/x.bak"
        );
        assert_eq!(
            server_path("C:\\MSSQL\\Backup", "x.bak"),
            "C:\\MSSQL\\Backup\\x.bak"
        );
    }

    #[test]
    fn restore_relocates_the_files_to_the_destination() {
        let files = [
            BackupFile {
                logical: "vendas".into(),
                log: false,
            },
            BackupFile {
                logical: "vendas_log".into(),
                log: true,
            },
            BackupFile {
                logical: "arquivo".into(),
                log: false,
            },
        ];
        let moves = relocation(&files, "copia", "/data", "/log/");
        assert_eq!(
            moves,
            [
                ("vendas".to_string(), "/data/copia.mdf".to_string()),
                ("vendas_log".to_string(), "/log/copia_log.ldf".to_string()),
                ("arquivo".to_string(), "/data/copia_2.ndf".to_string()),
            ]
        );
        let mut o = RestoreOptions {
            file: "/data/vendas.bak".into(),
            database: "copia".into(),
            position: 2,
            replace: false,
            close_connections: false,
            recovery: true,
        };
        assert_eq!(
            restore_sql(&o, &moves[..2]),
            "RESTORE DATABASE [copia] FROM DISK = N'/data/vendas.bak' WITH FILE = 2, \
             MOVE N'vendas' TO N'/data/copia.mdf', MOVE N'vendas_log' TO N'/log/copia_log.ldf', \
             RECOVERY, STATS = 10"
        );
        o.replace = true;
        o.close_connections = true;
        o.recovery = false;
        assert_eq!(
            restore_sql(&o, &[]),
            "IF DB_ID(N'copia') IS NOT NULL ALTER DATABASE [copia] SET SINGLE_USER \
             WITH ROLLBACK IMMEDIATE;\n\
             RESTORE DATABASE [copia] FROM DISK = N'/data/vendas.bak' WITH FILE = 2, \
             REPLACE, NORECOVERY, STATS = 10"
        );
    }

    #[test]
    fn header_and_file_list_are_read_by_column_name() {
        let header = Table {
            columns: [
                "BackupName",
                "BackupType",
                "Position",
                "DatabaseName",
                "BackupFinishDate",
            ]
            .map(String::from)
            .to_vec(),
            rows: vec![
                ["a", "1", "1", "vendas", "2026-11-15 10:00:00"]
                    .map(|v| Some(v.to_string()))
                    .to_vec(),
                ["b", "5", "2", "vendas", "2026-11-15 11:00:00"]
                    .map(|v| Some(v.to_string()))
                    .to_vec(),
            ],
        };
        let sets = parse_header(&header);
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[1].kind, BackupKind::Differential);
        assert_eq!(sets[1].position, 2);
        let files = Table {
            columns: ["LogicalName", "PhysicalName", "Type"]
                .map(String::from)
                .to_vec(),
            rows: vec![
                ["vendas", "/x/vendas.mdf", "D"]
                    .map(|v| Some(v.to_string()))
                    .to_vec(),
                ["vendas_log", "/x/vendas_log.ldf", "L"]
                    .map(|v| Some(v.to_string()))
                    .to_vec(),
            ],
        };
        let files = parse_files(&files);
        assert!(!files[0].log && files[1].log);
        assert_eq!(percent("40 percent processed."), Some(40));
        assert_eq!(percent("Processed 328 pages for database"), None);
    }
}
