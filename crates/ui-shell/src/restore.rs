//! "Restore Database" (Tasks > Restore...): o `pg_restore` com o conteúdo
//! do arquivo (`pg_restore -l`) para escolher, banco novo ou existente e
//! as opções clean e create.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, Align, Layout};
use pgstudio_core::connection::ConnectionSettings;
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_core::schema::quote_ident;

use crate::pgtools::{self, Tool, ToolEvent, ToolRun};

/// Uma linha do índice do arquivo (`pg_restore -l`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// A linha como veio, para o `--use-list`.
    pub line: String,
    /// Número da entrada (o primeiro campo).
    pub id: u32,
    /// `TABLE`, `TABLE DATA`, `INDEX`, `FK CONSTRAINT`...
    pub kind: String,
    pub schema: String,
    pub name: String,
}

/// Tipos de entrada com mais de uma palavra, para separar o tipo do resto.
const MULTI_WORD_KINDS: &[&str] = &[
    "TABLE DATA",
    "SEQUENCE SET",
    "SEQUENCE OWNED BY",
    "FK CONSTRAINT",
    "DEFAULT ACL",
    "MATERIALIZED VIEW",
    "EVENT TRIGGER",
    "FOREIGN TABLE",
    "TEXT SEARCH",
    "LARGE OBJECT",
    "BLOB DATA",
];

/// Lê a saída de `pg_restore -l`: `1234; 1259 16385 TABLE public cliente
/// postgres`. Comentários (`;`) ficam de fora.
pub fn parse_list(text: &str) -> Vec<Entry> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with(';') && !l.trim().is_empty())
        .filter_map(|line| {
            let (id, rest) = line.split_once(';')?;
            let id = id.trim().parse().ok()?;
            // Depois do ';': catálogo, oid e o resto.
            let mut words = rest.split_whitespace();
            words.next()?;
            words.next()?;
            let rest: Vec<&str> = words.collect();
            let joined = rest.join(" ");
            let multi = MULTI_WORD_KINDS
                .iter()
                .find(|k| joined.starts_with(&format!("{k} ")))
                .map(|k| k.split(' ').count());
            let n = multi.unwrap_or(1);
            let kind = rest.get(..n)?.join(" ");
            let schema = rest.get(n).map(|s| s.to_string()).unwrap_or_default();
            // O último campo é o dono; o nome é o que fica no meio.
            let name = match rest.len() {
                len if len > n + 2 => rest[n + 1..len - 1].join(" "),
                len if len == n + 2 => rest[n + 1].to_string(),
                _ => String::new(),
            };
            Some(Entry {
                line: line.to_string(),
                id,
                kind,
                schema: if schema == "-" { String::new() } else { schema },
                name,
            })
        })
        .collect()
}

/// Para onde restaurar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Um banco que já existe.
    Existing(String),
    /// Um banco novo com este nome (criado antes do pg_restore).
    New(String),
    /// `--create`: o banco com o nome do arquivo, criado pelo pg_restore.
    FromArchive,
}

/// As escolhas da janela.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreOptions {
    pub file: String,
    pub target: Target,
    /// `--clean --if-exists`: apaga os objetos antes de recriar.
    pub clean: bool,
    /// `--no-owner`: os objetos ficam com quem restaura.
    pub no_owner: bool,
    /// `--single-transaction`: tudo ou nada.
    pub single_transaction: bool,
    /// `--use-list`: só estas entradas (vazio: todas).
    pub list_file: Option<PathBuf>,
}

/// Argumentos do `pg_restore`. Com `--create`, ele conecta no banco da
/// conexão para criar o do arquivo.
pub fn restore_args(o: &RestoreOptions, settings: &ConnectionSettings) -> Vec<String> {
    let mut args = pgtools::connection_args(settings);
    match &o.target {
        Target::Existing(db) | Target::New(db) => args.push(format!("--dbname={db}")),
        Target::FromArchive => {
            args.push(format!("--dbname={}", settings.database));
            args.push("--create".to_string());
        }
    }
    if o.clean {
        args.push("--clean".to_string());
        args.push("--if-exists".to_string());
    }
    if o.no_owner {
        args.push("--no-owner".to_string());
    }
    if o.single_transaction {
        args.push("--single-transaction".to_string());
    }
    if let Some(list) = &o.list_file {
        args.push(format!("--use-list={}", list.display()));
    }
    args.push("--verbose".to_string());
    args.push(o.file.trim().to_string());
    args
}

/// O arquivo do `--use-list`: as entradas escolhidas, na ordem do índice.
pub fn use_list(entries: &[Entry], chosen: &BTreeSet<u32>) -> String {
    let mut out = String::new();
    for e in entries.iter().filter(|e| chosen.contains(&e.id)) {
        out.push_str(&e.line);
        out.push('\n');
    }
    out
}

enum Phase {
    Editing,
    /// Lendo o índice do arquivo.
    Listing(ToolRun),
    /// Criando o banco novo.
    Creating(Receiver<Result<(), String>>),
    Running(ToolRun),
    Done(Result<(), String>),
}

/// O que a janela pediu à casca.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Log(Vec<String>),
    Closed,
    /// Restaurou: atualizar a pasta Databases.
    Restored(Vec<String>),
}

/// A janela "Restore Database".
pub struct RestoreDialog {
    pub options: RestoreOptions,
    settings: ConnectionSettings,
    password: String,
    session: Arc<dyn QuerySession>,
    tool: Result<Tool, String>,
    databases: Option<Vec<String>>,
    databases_rx: Receiver<Vec<String>>,
    entries: Option<Vec<Entry>>,
    /// Entradas marcadas (todas, depois de listar).
    pub chosen: BTreeSet<u32>,
    phase: Phase,
    pub progress: Vec<String>,
    error: Option<String>,
    /// O texto do campo "New database".
    pub new_name: String,
    /// Arquivo listado por último.
    listed: String,
}

impl RestoreDialog {
    /// Abre a janela; `database` é o banco de destino sugerido (None:
    /// banco novo). `session` é do banco da conexão.
    pub fn open(
        database: Option<String>,
        settings: ConnectionSettings,
        password: String,
        server_version: &str,
        session: Arc<dyn QuerySession>,
        ctx: &egui::Context,
    ) -> Self {
        let tool = pgtools::choose(
            "pg_restore",
            pgtools::configured_dir().as_deref(),
            server_version,
        );
        let (tx, databases_rx) = mpsc::channel();
        let ctx2 = ctx.clone();
        let mut names = Vec::new();
        session.execute(
            "SELECT datname FROM pg_database WHERE datallowconn AND NOT datistemplate ORDER BY 1"
                .to_string(),
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
        let target = match database {
            Some(db) => Target::Existing(db),
            None => Target::New(String::new()),
        };
        Self {
            options: RestoreOptions {
                file: String::new(),
                target,
                clean: false,
                no_owner: false,
                single_transaction: false,
                list_file: None,
            },
            settings,
            password,
            session,
            tool,
            databases: None,
            databases_rx,
            entries: None,
            chosen: BTreeSet::new(),
            phase: Phase::Editing,
            progress: Vec::new(),
            error: None,
            new_name: String::new(),
            listed: String::new(),
        }
    }

    pub fn tool(&self) -> Result<&Tool, &str> {
        self.tool.as_ref().map_err(String::as_str)
    }

    pub fn databases(&self) -> Option<&[String]> {
        self.databases.as_deref()
    }

    pub fn entries(&self) -> Option<&[Entry]> {
        self.entries.as_deref()
    }

    pub fn is_listing(&self) -> bool {
        matches!(self.phase, Phase::Listing(_))
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Listing(_) | Phase::Creating(_) | Phase::Running(_)
        )
    }

    pub fn result(&self) -> Option<&Result<(), String>> {
        match &self.phase {
            Phase::Done(r) => Some(r),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Lê o índice do arquivo com `pg_restore -l`.
    pub fn list(&mut self, ctx: &egui::Context) {
        let Ok(tool) = &self.tool else {
            return;
        };
        let file = self.options.file.trim().to_string();
        if file.is_empty() {
            self.error = Some("Choose the backup file.".into());
            return;
        }
        self.error = None;
        self.entries = None;
        self.listed = file.clone();
        match ToolRun::spawn(&tool.path, &["--list".into(), file], &[], true, ctx) {
            Ok(run) => self.phase = Phase::Listing(run),
            Err(e) => self.error = Some(format!("pg_restore could not start: {e}")),
        }
    }

    fn invalid(&self) -> Option<String> {
        if let Err(e) = &self.tool {
            return Some(e.clone());
        }
        if self.options.file.trim().is_empty() {
            return Some("Choose the backup file.".into());
        }
        if self.entries.is_none() || self.listed != self.options.file.trim() {
            return Some("List the contents of the file first.".into());
        }
        if self.chosen.is_empty() {
            return Some("Select at least one item to restore.".into());
        }
        match &self.options.target {
            Target::New(n) if n.trim().is_empty() => Some("Name the new database.".into()),
            Target::Existing(n) if n.is_empty() => Some("Choose the destination database.".into()),
            _ => None,
        }
    }

    /// A linha de comando, como o Script mostra.
    pub fn command_line(&self) -> Option<String> {
        let tool = self.tool.as_ref().ok()?;
        let mut o = self.options.clone();
        if self.uses_list() {
            o.list_file = Some(PathBuf::from("<selected items>"));
        }
        Some(pgtools::command_line(
            &tool.path,
            &restore_args(&o, &self.settings),
        ))
    }

    /// Nem tudo marcado: vai um `--use-list`.
    fn uses_list(&self) -> bool {
        self.entries
            .as_ref()
            .is_some_and(|e| e.len() != self.chosen.len())
    }

    /// Começa: cria o banco novo, se for o caso, e roda o pg_restore.
    pub fn start(&mut self, ctx: &egui::Context) -> Option<Request> {
        if let Some(e) = self.invalid() {
            self.error = Some(e);
            return None;
        }
        self.error = None;
        self.progress.clear();
        if self.uses_list() {
            let entries = self.entries.as_deref().unwrap_or_default();
            let path = std::env::temp_dir().join(format!(
                "pgstudio-restore-{}-{}.list",
                std::process::id(),
                entries.len()
            ));
            if let Err(e) = std::fs::write(&path, use_list(entries, &self.chosen)) {
                self.error = Some(format!("The list of items could not be written: {e}"));
                return None;
            }
            self.options.list_file = Some(path);
        } else {
            self.options.list_file = None;
        }
        let mut log = vec![format!(
            "Restore: {} to {}",
            self.options.file.trim(),
            match &self.options.target {
                Target::Existing(db) | Target::New(db) => db.clone(),
                Target::FromArchive => "the database named in the file".into(),
            }
        )];
        if let Target::New(name) = &self.options.target {
            let (tx, rx) = mpsc::channel();
            let ctx2 = ctx.clone();
            let mut error = None;
            self.session.execute(
                format!("CREATE DATABASE {}", quote_ident(name.trim())),
                Box::new(move |e| match e {
                    QueryEvent::Error(e) => error = Some(e.message),
                    QueryEvent::Finished { .. } => {
                        let _ = tx.send(error.take().map_or(Ok(()), Err));
                        ctx2.request_repaint();
                    }
                    _ => {}
                }),
            );
            log.push(format!("  CREATE DATABASE {}", quote_ident(name.trim())));
            self.phase = Phase::Creating(rx);
            return Some(Request::Log(log));
        }
        log.extend(self.run(ctx));
        Some(Request::Log(log))
    }

    /// Roda o pg_restore em si.
    fn run(&mut self, ctx: &egui::Context) -> Vec<String> {
        let Ok(tool) = self.tool.clone() else {
            return Vec::new();
        };
        let mut o = self.options.clone();
        if let Target::New(n) = &o.target {
            o.target = Target::New(n.trim().to_string());
        }
        let args = restore_args(&o, &self.settings);
        let env = pgtools::connection_env(&self.settings, &self.password);
        let line = pgtools::command_line(&tool.path, &args);
        match ToolRun::spawn(&tool.path, &args, &env, false, ctx) {
            Ok(run) => {
                self.phase = Phase::Running(run);
                vec![format!("  {line}")]
            }
            Err(e) => {
                let e = format!("pg_restore could not start: {e}");
                self.phase = Phase::Done(Err(e.clone()));
                vec![format!("Restore: {e}")]
            }
        }
    }

    pub fn cancel(&mut self) {
        if let Phase::Running(run) | Phase::Listing(run) = &self.phase {
            run.cancel();
        }
    }

    /// Recebe o progresso.
    pub fn step(&mut self, ctx: &egui::Context) -> Option<Request> {
        if let Ok(d) = self.databases_rx.try_recv() {
            if let Target::Existing(db) = &self.options.target
                && db.is_empty()
                && let Some(first) = d.first()
            {
                self.options.target = Target::Existing(first.clone());
            }
            self.databases = Some(d);
        }
        let mut log = Vec::new();
        let mut next = None;
        match &self.phase {
            Phase::Editing | Phase::Done(_) => return None,
            Phase::Listing(run) => {
                let mut errors = Vec::new();
                for e in run.events() {
                    match e {
                        ToolEvent::Line(l) => errors.push(l),
                        ToolEvent::Exit(Ok(())) => {
                            let text = run.stdout.lock().map(|t| t.clone()).unwrap_or_default();
                            let entries = parse_list(&text);
                            self.chosen = entries.iter().map(|e| e.id).collect();
                            self.entries = Some(entries);
                            next = Some(Phase::Editing);
                        }
                        ToolEvent::Exit(Err(e)) => {
                            let detail = errors.join(" ");
                            self.error = Some(if detail.is_empty() {
                                format!("The file could not be read ({e}).")
                            } else {
                                detail
                            });
                            next = Some(Phase::Editing);
                        }
                    }
                }
                self.progress.extend(errors);
            }
            Phase::Creating(rx) => match rx.try_recv() {
                Ok(Ok(())) => {
                    log.extend(self.run(ctx));
                }
                Ok(Err(e)) => {
                    log.push(format!("Restore: failed ({e})."));
                    next = Some(Phase::Done(Err(e)));
                }
                Err(_) => {}
            },
            Phase::Running(run) => {
                for e in run.events() {
                    match e {
                        ToolEvent::Line(l) => {
                            log.push(format!("  {l}"));
                            self.progress.push(l);
                        }
                        ToolEvent::Exit(r) => {
                            log.push(match &r {
                                Ok(()) => "Restore: completed.".to_string(),
                                Err(e) => format!("Restore: failed ({e})."),
                            });
                            next = Some(Phase::Done(r));
                        }
                    }
                }
            }
        }
        if self.progress.len() > 200 {
            let extra = self.progress.len() - 200;
            self.progress.drain(..extra);
        }
        let finished_ok = matches!(next, Some(Phase::Done(Ok(()))));
        if let Some(p) = next {
            self.phase = p;
        }
        if finished_ok {
            if let Some(f) = self.options.list_file.take() {
                let _ = std::fs::remove_file(f);
            }
            return Some(Request::Restored(log));
        }
        (!log.is_empty()).then_some(Request::Log(log))
    }

    /// Desenha a janela.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Request> {
        let mut request = self.step(ctx);
        let modal = egui::Modal::new(egui::Id::new("restore_dialog")).show(ctx, |ui| {
            ui.set_width(680.0);
            ui.heading("Restore Database");
            ui.separator();
            let editing = matches!(self.phase, Phase::Editing);
            ui.add_enabled_ui(editing, |ui| self.form(ui, ctx));
            ui.separator();
            match &self.phase {
                Phase::Editing => {}
                Phase::Listing(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading the backup file...");
                    });
                }
                Phase::Creating(_) | Phase::Running(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Restoring...");
                    });
                }
                Phase::Done(Ok(())) => {
                    ui.label(format!(
                        "{} The restore completed.",
                        egui_phosphor::regular::CHECK_CIRCLE
                    ));
                }
                Phase::Done(Err(e)) => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("The restore failed: {e}"),
                    );
                }
            }
            if !self.progress.is_empty() {
                egui::ScrollArea::vertical()
                    .id_salt("restore_progress")
                    .max_height(110.0)
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
                    if ui.button("Cancel").clicked() {
                        request = Some(Request::Closed);
                    }
                    let ok = self.tool.is_ok();
                    if ui.add_enabled(ok, egui::Button::new("OK")).clicked() {
                        request = self.start(ctx).or(request.take());
                    }
                    if ui
                        .add_enabled(
                            ok,
                            egui::Button::new(format!("{} Script", egui_phosphor::regular::SCROLL)),
                        )
                        .clicked()
                        && let Some(line) = self.command_line()
                    {
                        ui.ctx().copy_text(line.clone());
                        self.progress = vec![line];
                        request = Some(Request::Log(vec![
                            "Restore: command line copied to the clipboard.".into(),
                        ]));
                    }
                }
                Phase::Listing(_) | Phase::Running(_) => {
                    if ui.button("Stop").clicked() {
                        self.cancel();
                    }
                }
                Phase::Creating(_) => {}
                Phase::Done(_) => {
                    if ui.button("Close").clicked() {
                        request = Some(Request::Closed);
                    }
                }
            });
        });
        if modal.should_close() && request.is_none() && !self.is_busy() {
            request = Some(Request::Closed);
        }
        request
    }

    fn form(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Grid::new("restore_form")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("pg_restore:");
                match &self.tool {
                    Ok(t) => ui.label(format!("{} ({})", t.path.display(), t.version_text())),
                    Err(e) => ui.colored_label(ui.visuals().warn_fg_color, e),
                };
                ui.end_row();
                ui.label("Backup file:");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.options.file)
                            .hint_text("custom or directory backup")
                            .desired_width(420.0),
                    );
                    if ui.button("List Contents").clicked() {
                        self.list(ctx);
                    }
                });
                ui.end_row();
                ui.label("Destination:");
                ui.vertical(|ui| {
                    let existing = matches!(self.options.target, Target::Existing(_));
                    let new = matches!(self.options.target, Target::New(_));
                    ui.horizontal(|ui| {
                        if ui.radio(existing, "Existing database").clicked() && !existing {
                            let first = self
                                .databases
                                .as_ref()
                                .and_then(|d| d.first().cloned())
                                .unwrap_or_default();
                            self.options.target = Target::Existing(first);
                        }
                        if let Target::Existing(db) = &mut self.options.target {
                            egui::ComboBox::from_id_salt("restore_db")
                                .selected_text(db.as_str())
                                .show_ui(ui, |ui| {
                                    for d in self.databases.iter().flatten() {
                                        ui.selectable_value(db, d.clone(), d.as_str());
                                    }
                                });
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.radio(new, "New database").clicked() && !new {
                            self.options.target = Target::New(self.new_name.clone());
                        }
                        if let Target::New(name) = &mut self.options.target {
                            ui.add(egui::TextEdit::singleline(name).desired_width(200.0));
                            self.new_name = name.clone();
                        }
                    });
                    let from = self.options.target == Target::FromArchive;
                    if ui
                        .radio(from, "Database named in the file (--create)")
                        .clicked()
                    {
                        self.options.target = Target::FromArchive;
                    }
                });
                ui.end_row();
                ui.label("Options:");
                ui.vertical(|ui| {
                    ui.checkbox(
                        &mut self.options.clean,
                        "Drop the objects before recreating them (--clean)",
                    );
                    ui.checkbox(
                        &mut self.options.no_owner,
                        "Do not restore the owners (--no-owner)",
                    );
                    ui.checkbox(
                        &mut self.options.single_transaction,
                        "Restore in a single transaction",
                    );
                });
                ui.end_row();
            });
        if let Some(entries) = &self.entries {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.strong(format!(
                    "Contents: {} of {} items selected",
                    self.chosen.len(),
                    entries.len()
                ));
                if ui.small_button("All").clicked() {
                    self.chosen = entries.iter().map(|e| e.id).collect();
                }
                if ui.small_button("None").clicked() {
                    self.chosen.clear();
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("restore_entries")
                .max_height(180.0)
                .show(ui, |ui| {
                    for e in entries {
                        let mut on = self.chosen.contains(&e.id);
                        let what = if e.schema.is_empty() {
                            e.name.clone()
                        } else if e.name.contains(' ') {
                            format!("{} ({})", e.name, e.schema)
                        } else {
                            format!("{}.{}", e.schema, e.name)
                        };
                        if ui
                            .checkbox(&mut on, format!("{}  {what}", e.kind))
                            .changed()
                        {
                            if on {
                                self.chosen.insert(e.id);
                            } else {
                                self.chosen.remove(&e.id);
                            }
                        }
                    }
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "\
;
; Archive created at 2026-11-14 12:00:00 UTC
;     dbname: loja
;
215; 1259 16385 TABLE public cliente postgres
216; 1259 16390 TABLE public Pedido do Mês postgres
3350; 0 16385 TABLE DATA public cliente postgres
3351; 0 0 SEQUENCE OWNED BY public cliente_id_seq postgres
3200; 2606 16389 CONSTRAINT public cliente cliente_pkey postgres
3210; 2606 16400 FK CONSTRAINT public pedido pedido_cliente_id_fkey postgres
5; 2615 2200 SCHEMA - public pg_database_owner
";

    #[test]
    fn the_archive_list_is_read() {
        let mut e = parse_list(LIST);
        assert_eq!(e.len(), 7);
        let owned = e.remove(3);
        assert_eq!(
            (owned.kind.as_str(), owned.name.as_str()),
            ("SEQUENCE OWNED BY", "cliente_id_seq")
        );
        assert_eq!(
            (
                e[0].id,
                e[0].kind.as_str(),
                e[0].schema.as_str(),
                e[0].name.as_str()
            ),
            (215, "TABLE", "public", "cliente")
        );
        assert_eq!(e[1].name, "Pedido do Mês");
        assert_eq!(e[2].kind, "TABLE DATA");
        assert_eq!(e[3].name, "cliente cliente_pkey");
        assert_eq!(e[4].kind, "FK CONSTRAINT");
        assert_eq!(e[5].kind, "SCHEMA");
        assert_eq!(e[5].name, "public");
        let chosen: BTreeSet<u32> = [215, 3350].into();
        assert_eq!(
            use_list(&e, &chosen),
            "215; 1259 16385 TABLE public cliente postgres\n\
             3350; 0 16385 TABLE DATA public cliente postgres\n"
        );
    }

    #[test]
    fn options_become_pg_restore_arguments() {
        let s = ConnectionSettings {
            host: "h".into(),
            port: 5432,
            user: "u".into(),
            database: "postgres".into(),
            ..Default::default()
        };
        let mut o = RestoreOptions {
            file: "/tmp/loja.backup".into(),
            target: Target::Existing("loja".into()),
            clean: true,
            no_owner: true,
            single_transaction: false,
            list_file: None,
        };
        assert_eq!(
            restore_args(&o, &s)[4..],
            [
                "--dbname=loja",
                "--clean",
                "--if-exists",
                "--no-owner",
                "--verbose",
                "/tmp/loja.backup"
            ]
        );
        o.target = Target::FromArchive;
        o.clean = false;
        o.no_owner = false;
        o.single_transaction = true;
        o.list_file = Some("/tmp/l".into());
        assert_eq!(
            restore_args(&o, &s)[4..],
            [
                "--dbname=postgres",
                "--create",
                "--single-transaction",
                "--use-list=/tmp/l",
                "--verbose",
                "/tmp/loja.backup"
            ]
        );
    }
}
