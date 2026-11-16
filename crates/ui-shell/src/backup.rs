//! "Back Up Database" (Tasks > Back Up...): as opções do `pg_dump`, o
//! progresso e o cancelamento, numa janela como a do SSMS.

use pgstudio_i18n::{tr, trf};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, Align, Layout};
use pgstudio_core::connection::ConnectionSettings;
use pgstudio_core::query::{QueryEvent, QuerySession};

use crate::pgtools::{self, Tool, ToolEvent, ToolRun};

/// Formato do arquivo (`--format`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `custom`: compactado, restaura com pg_restore (o padrão).
    Custom,
    /// `plain`: um script SQL.
    Plain,
    /// `directory`: uma pasta, um arquivo por tabela.
    Directory,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Custom, Format::Plain, Format::Directory];

    pub fn label(self) -> &'static str {
        tr(match self {
            Format::Custom => "Custom (pg_restore)",
            Format::Plain => "Plain SQL script",
            Format::Directory => "Directory",
        })
    }

    fn flag(self) -> &'static str {
        match self {
            Format::Custom => "c",
            Format::Plain => "p",
            Format::Directory => "d",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Format::Custom => ".backup",
            Format::Plain => ".sql",
            Format::Directory => "",
        }
    }
}

/// O que entra no backup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Full,
    SchemaOnly,
    DataOnly,
}

impl Content {
    pub fn label(self) -> &'static str {
        tr(match self {
            Content::Full => "Schema and data",
            Content::SchemaOnly => "Schema only",
            Content::DataOnly => "Data only",
        })
    }
}

/// As escolhas da janela.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupOptions {
    pub database: String,
    pub format: Format,
    pub content: Content,
    pub file: String,
    /// Tabelas escolhidas (`schema.tabela`, já com aspas se preciso);
    /// vazio é o banco inteiro.
    pub tables: BTreeSet<String>,
}

/// Arquivo padrão: `<home>/<banco>.backup` (ou `.sql`, ou a pasta).
pub fn default_file(database: &str, format: Format) -> String {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let safe: String = database
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    home.join(format!("{safe}{}", format.extension()))
        .display()
        .to_string()
}

/// Argumentos do `pg_dump` (sem a senha, que vai no ambiente).
pub fn backup_args(o: &BackupOptions, settings: &ConnectionSettings) -> Vec<String> {
    let mut args = pgtools::connection_args(settings);
    args.push(format!("--dbname={}", o.database));
    args.push(format!("--format={}", o.format.flag()));
    args.push(format!("--file={}", o.file.trim()));
    args.push("--verbose".to_string());
    match o.content {
        Content::Full => {}
        Content::SchemaOnly => args.push("--schema-only".to_string()),
        Content::DataOnly => args.push("--data-only".to_string()),
    }
    for t in &o.tables {
        args.push(format!("--table={t}"));
    }
    args
}

/// Tabelas do banco, como o `--table` aceita.
pub const TABLES_SQL: &str = "SELECT quote_ident(n.nspname) || '.' || quote_ident(c.relname) \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE c.relkind IN ('r', 'p') AND n.nspname <> 'information_schema' \
     AND n.nspname NOT LIKE 'pg\\_%' ORDER BY 1";

/// Estado do backup.
enum Phase {
    Editing,
    Running(ToolRun),
    Done(Result<(), String>),
}

/// O que a janela pediu à casca.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// Linhas para o Output.
    Log(Vec<String>),
    Closed,
}

/// A janela "Back Up Database".
pub struct BackupDialog {
    pub options: BackupOptions,
    settings: ConnectionSettings,
    password: String,
    tool: Result<Tool, String>,
    /// `false`: banco inteiro; `true`: só as tabelas marcadas.
    pub choose_tables: bool,
    tables: Option<Vec<String>>,
    tables_rx: Receiver<Vec<String>>,
    phase: Phase,
    /// Últimas linhas do pg_dump, para a janela.
    pub progress: Vec<String>,
    error: Option<String>,
}

impl BackupDialog {
    /// Abre para `database`; `server_version` decide se o pg_dump serve.
    /// `session` lê a lista de tabelas.
    pub fn open(
        database: String,
        settings: ConnectionSettings,
        password: String,
        server_version: &str,
        session: Arc<dyn QuerySession>,
        ctx: &egui::Context,
    ) -> Self {
        let tool = pgtools::choose(
            "pg_dump",
            pgtools::configured_dir().as_deref(),
            server_version,
        );
        let (tx, tables_rx) = mpsc::channel();
        let ctx2 = ctx.clone();
        let mut names = Vec::new();
        session.execute(
            TABLES_SQL.to_string(),
            Box::new(move |e| match e {
                QueryEvent::Row(r) => {
                    if let Some(Some(n)) = r.into_iter().next() {
                        names.push(n);
                    }
                }
                QueryEvent::Finished { .. } => {
                    let _ = tx.send(std::mem::take(&mut names));
                    ctx2.request_repaint();
                }
                _ => {}
            }),
        );
        Self {
            options: BackupOptions {
                file: default_file(&database, Format::Custom),
                database,
                format: Format::Custom,
                content: Content::Full,
                tables: BTreeSet::new(),
            },
            settings,
            password,
            tool,
            choose_tables: false,
            tables: None,
            tables_rx,
            phase: Phase::Editing,
            progress: Vec::new(),
            error: None,
        }
    }

    pub fn tool(&self) -> Result<&Tool, &str> {
        self.tool.as_ref().map_err(String::as_str)
    }

    pub fn tables(&self) -> Option<&[String]> {
        self.tables.as_deref()
    }

    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running(_))
    }

    /// Resultado, quando terminou.
    pub fn result(&self) -> Option<&Result<(), String>> {
        match &self.phase {
            Phase::Done(r) => Some(r),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// As opções que valem (sem tabelas quando "banco inteiro").
    fn effective(&self) -> BackupOptions {
        let mut o = self.options.clone();
        if !self.choose_tables {
            o.tables.clear();
        }
        o
    }

    /// A linha de comando, como o Script mostra.
    pub fn command_line(&self) -> Option<String> {
        let tool = self.tool.as_ref().ok()?;
        Some(pgtools::command_line(
            &tool.path,
            &backup_args(&self.effective(), &self.settings),
        ))
    }

    fn invalid(&self) -> Option<String> {
        if let Err(e) = &self.tool {
            return Some(e.clone());
        }
        if self.options.file.trim().is_empty() {
            return Some(tr("Choose the destination file.").into());
        }
        if self.choose_tables && self.options.tables.is_empty() {
            return Some(tr("Select at least one table, or back up the whole database.").into());
        }
        None
    }

    /// Começa o pg_dump.
    pub fn start(&mut self, ctx: &egui::Context) -> Option<Request> {
        if let Some(e) = self.invalid() {
            self.error = Some(e);
            return None;
        }
        let tool = self.tool.as_ref().ok()?.clone();
        let args = backup_args(&self.effective(), &self.settings);
        let env = pgtools::connection_env(&self.settings, &self.password);
        self.error = None;
        self.progress.clear();
        let line = pgtools::command_line(&tool.path, &args);
        match ToolRun::spawn(&tool.path, &args, &env, false, ctx) {
            Ok(run) => {
                self.phase = Phase::Running(run);
                Some(Request::Log(vec![
                    trf!(
                        "Back Up: {database} to {file}",
                        database = self.options.database,
                        file = self.options.file.trim()
                    ),
                    format!("  {line}"),
                ]))
            }
            Err(e) => {
                self.error = Some(trf!("pg_dump could not start: {e}", e = e));
                None
            }
        }
    }

    /// Para o pg_dump em andamento.
    pub fn cancel(&mut self) {
        if let Phase::Running(run) = &self.phase {
            run.cancel();
        }
    }

    /// Recebe o progresso; devolve as linhas para o Output.
    pub fn step(&mut self) -> Option<Request> {
        if let Ok(t) = self.tables_rx.try_recv() {
            self.tables = Some(t);
        }
        let Phase::Running(run) = &self.phase else {
            return None;
        };
        let mut log = Vec::new();
        let mut done = None;
        for e in run.events() {
            match e {
                ToolEvent::Line(l) => {
                    log.push(format!("  {l}"));
                    self.progress.push(l);
                }
                ToolEvent::Exit(r) => done = Some(r),
            }
        }
        if self.progress.len() > 200 {
            let extra = self.progress.len() - 200;
            self.progress.drain(..extra);
        }
        if let Some(r) = done {
            log.push(match &r {
                Ok(()) => trf!(
                    "Back Up: completed ({file}).",
                    file = self.options.file.trim()
                ),
                Err(e) => trf!("Back Up: failed ({e}).", e = e),
            });
            self.phase = Phase::Done(r);
        }
        (!log.is_empty()).then_some(Request::Log(log))
    }

    /// Desenha a janela.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Request> {
        let mut request = self.step();
        let modal = crate::theme::modal(egui::Id::new("backup_dialog")).show(ctx, |ui| {
            crate::fit::dialog_width(ui, 640.0);
            ui.heading(trf!(
                "Back Up Database - {database}",
                database = self.options.database
            ));
            ui.separator();
            let editing = matches!(self.phase, Phase::Editing);
            ui.add_enabled_ui(editing, |ui| self.form(ui));
            ui.separator();
            match &self.phase {
                Phase::Editing => {}
                Phase::Running(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr("Backing up..."));
                    });
                }
                Phase::Done(Ok(())) => {
                    ui.label(format!(
                        "{} {}",
                        egui_phosphor::regular::CHECK_CIRCLE,
                        trf!(
                            "The backup of {database} completed.",
                            database = self.options.database
                        )
                    ));
                }
                Phase::Done(Err(e)) => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        trf!("The backup failed: {e}", e = e),
                    );
                }
            }
            if !self.progress.is_empty() {
                egui::ScrollArea::vertical()
                    .id_salt("backup_progress")
                    .max_height(120.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &self.progress {
                            ui.monospace(l);
                        }
                    });
            }
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            let bar = egui::vec2(ui.available_width(), 26.0);
            ui.allocate_ui_with_layout(bar, Layout::right_to_left(Align::Center), |ui| match &self
                .phase
            {
                Phase::Editing => {
                    if ui.button(tr("Cancel")).clicked() {
                        request = Some(Request::Closed);
                    }
                    let ok = self.tool.is_ok();
                    if ui.add_enabled(ok, egui::Button::new(tr("OK"))).clicked() {
                        request = self.start(ctx).or(request.take());
                    }
                    if ui
                        .add_enabled(
                            ok,
                            egui::Button::new(format!(
                                "{} {}",
                                egui_phosphor::regular::SCROLL,
                                tr("Script")
                            )),
                        )
                        .clicked()
                        && let Some(line) = self.command_line()
                    {
                        ui.ctx().copy_text(line.clone());
                        self.error = None;
                        self.progress = vec![line];
                        request = Some(Request::Log(vec![
                            tr("Back Up: command line copied to the clipboard.").into(),
                        ]));
                    }
                }
                Phase::Running(_) => {
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
        egui::Grid::new("backup_form")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label(tr("Database:"));
                ui.label(&self.options.database);
                ui.end_row();
                ui.label(tr("pg_dump:"));
                match &self.tool {
                    Ok(t) => ui.label(format!("{} ({})", t.path.display(), t.version_text())),
                    Err(e) => ui.colored_label(ui.visuals().warn_fg_color, e),
                };
                ui.end_row();
                ui.label(tr("Backup type:"));
                ui.horizontal(|ui| {
                    for c in [Content::Full, Content::SchemaOnly, Content::DataOnly] {
                        ui.radio_value(&mut self.options.content, c, c.label());
                    }
                });
                ui.end_row();
                ui.label(tr("Format:"));
                let before = self.options.format;
                egui::ComboBox::from_id_salt("backup_format")
                    .selected_text(self.options.format.label())
                    .show_ui(ui, |ui| {
                        for f in Format::ALL {
                            ui.selectable_value(&mut self.options.format, f, f.label());
                        }
                    });
                if before != self.options.format
                    && self.options.file == default_file(&self.options.database, before)
                {
                    self.options.file = default_file(&self.options.database, self.options.format);
                }
                ui.end_row();
                ui.label(tr("Back up to:"));
                ui.add(egui::TextEdit::singleline(&mut self.options.file).desired_width(440.0));
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.choose_tables, false, tr("Whole database"));
            ui.radio_value(&mut self.choose_tables, true, tr("Selected tables"));
        });
        if self.choose_tables {
            match &self.tables {
                None => {
                    ui.spinner();
                }
                Some(tables) if tables.is_empty() => {
                    ui.weak(tr("The database has no tables."));
                }
                Some(tables) => {
                    egui::ScrollArea::vertical()
                        .id_salt("backup_tables")
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for t in tables {
                                let mut on = self.options.tables.contains(t);
                                if ui.checkbox(&mut on, t.as_str()).changed() {
                                    if on {
                                        self.options.tables.insert(t.clone());
                                    } else {
                                        self.options.tables.remove(t);
                                    }
                                }
                            }
                        });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_become_pg_dump_arguments() {
        let s = ConnectionSettings {
            host: "h".into(),
            port: 5432,
            user: "u".into(),
            ..Default::default()
        };
        let mut o = BackupOptions {
            database: "loja".into(),
            format: Format::Custom,
            content: Content::Full,
            file: " /tmp/loja.backup ".into(),
            tables: BTreeSet::new(),
        };
        assert_eq!(
            backup_args(&o, &s),
            [
                "--host=h",
                "--port=5432",
                "--username=u",
                "--no-password",
                "--dbname=loja",
                "--format=c",
                "--file=/tmp/loja.backup",
                "--verbose",
            ]
        );
        o.format = Format::Plain;
        o.content = Content::SchemaOnly;
        o.tables.insert("public.pedido".into());
        o.tables.insert("public.\"Item\"".into());
        let args = backup_args(&o, &s);
        assert!(args.contains(&"--format=p".to_string()));
        assert!(args.contains(&"--schema-only".to_string()));
        assert!(args.ends_with(&[
            "--table=public.\"Item\"".to_string(),
            "--table=public.pedido".to_string()
        ]));
    }

    #[test]
    fn default_files_follow_the_format() {
        assert!(default_file("loja", Format::Custom).ends_with("loja.backup"));
        assert!(default_file("loja", Format::Plain).ends_with("loja.sql"));
        assert!(default_file("a/b c", Format::Directory).ends_with("a_b_c"));
    }
}
