//! A janela "Import CSV": escolher como ler o arquivo, casar as colunas
//! com as da tabela, ver a prévia e carregar.
//!
//! Nada aqui espera o disco ou o banco na thread da interface: a amostra
//! do arquivo, as colunas da tabela e a carga rodam em threads e chegam
//! por canais ([`ImportDialog::step`]).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use eframe::egui::{self, Align, Layout};
use pgstudio_core::query::QuerySession;
use pgstudio_i18n::{tr, trf};

use crate::csv_import::{self, ColumnKind, CsvOptions, DELIMITERS, Plan, Progress, Sample, Target};
use crate::registered::{self, Rgb};

/// Linhas mostradas na prévia.
const PREVIEW_ROWS: usize = 8;

/// O que a janela pede para a casca.
pub enum Request {
    /// Linhas para a janela Output.
    Log(Vec<String>),
    /// A carga terminou bem: linhas para a janela Output.
    Imported(Vec<String>),
    Closed,
}

/// Em que pé a janela está.
#[derive(Debug, PartialEq, Eq)]
pub enum Phase {
    /// Escolhendo opções e colunas.
    Editing,
    /// Esperando o "sim" num ambiente que pede confirmação.
    Confirming,
    Running,
    Done(String),
}

struct Running {
    rx: Receiver<Progress>,
    cancel: Arc<AtomicBool>,
    rows: u64,
    bytes: u64,
    started: Instant,
}

/// A janela "Import CSV" de uma tabela.
pub struct ImportDialog {
    session: Arc<dyn QuerySession>,
    pub path: PathBuf,
    pub options: CsvOptions,
    /// Ambiente que pede confirmação antes de alterar o banco.
    environment: Option<(String, Rgb)>,
    target: Option<Target>,
    target_rx: Option<Receiver<Result<Target, String>>>,
    sample: Option<Sample>,
    sample_rx: Option<Receiver<Result<(u8, Sample), String>>>,
    /// Para cada coluna da tabela, a coluna do arquivo (veja
    /// [`csv_import::auto_map`]).
    pub mapping: Vec<Option<usize>>,
    phase: Phase,
    running: Option<Running>,
    error: Option<String>,
}

impl ImportDialog {
    /// Abre a janela para a tabela com este id (`oid` ou `object_id`) e
    /// começa a ler o arquivo e as colunas da tabela.
    pub fn open(
        session: Arc<dyn QuerySession>,
        table: u64,
        path: PathBuf,
        environment: Option<(String, Rgb)>,
        ctx: &egui::Context,
    ) -> Self {
        let (tx, target_rx) = mpsc::channel();
        let (s, repaint) = (session.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(csv_import::load_target(&s, table));
            repaint.request_repaint();
        });
        let mut dialog = ImportDialog {
            session,
            path,
            options: CsvOptions::default(),
            environment,
            target: None,
            target_rx: Some(target_rx),
            sample: None,
            sample_rx: None,
            mapping: Vec::new(),
            phase: Phase::Editing,
            running: None,
            error: None,
        };
        dialog.read_sample(None, ctx);
        dialog
    }

    /// Lê o começo do arquivo com este separador (`None`: descobrir).
    fn read_sample(&mut self, delimiter: Option<u8>, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let (path, repaint) = (self.path.clone(), ctx.clone());
        std::thread::spawn(move || {
            let read = || {
                let delimiter = match delimiter {
                    Some(d) => d,
                    None => csv_import::sniff(&path)?,
                };
                Ok((delimiter, csv_import::sample(&path, delimiter)?))
            };
            let result = read().map_err(|e: std::io::Error| {
                trf!(
                    "Could not read {path}: {error}",
                    path = path.display(),
                    error = e
                )
            });
            let _ = tx.send(result);
            repaint.request_repaint();
        });
        self.sample = None;
        self.sample_rx = Some(rx);
    }

    /// Casa de novo as colunas, pelas opções atuais.
    fn remap(&mut self) {
        if let (Some(target), Some(sample)) = (&self.target, &self.sample) {
            let headers = sample.headers(self.options.header);
            self.mapping = csv_import::auto_map(&target.columns, &headers, self.options.header);
        }
    }

    /// Recebe o que as threads mandaram.
    fn step(&mut self) -> Option<Request> {
        if let Some(rx) = &self.target_rx
            && let Ok(result) = rx.try_recv()
        {
            self.target_rx = None;
            match result {
                Ok(target) => {
                    self.target = Some(target);
                    self.remap();
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(rx) = &self.sample_rx
            && let Ok(result) = rx.try_recv()
        {
            self.sample_rx = None;
            match result {
                Ok((delimiter, sample)) => {
                    self.options.delimiter = delimiter;
                    self.sample = Some(sample);
                    self.error = None;
                    self.remap();
                }
                Err(e) => self.error = Some(e),
            }
        }
        let mut request = None;
        if let Some(run) = &mut self.running {
            let mut last = None;
            while let Ok(p) = run.rx.try_recv() {
                match p {
                    Progress::Sent { rows, bytes } => {
                        run.rows = rows;
                        run.bytes = bytes;
                    }
                    other => last = Some(other),
                }
            }
            if let Some(end) = last {
                let seconds = run.started.elapsed().as_secs_f64();
                self.running = None;
                let table = self
                    .target
                    .as_ref()
                    .map(Target::display)
                    .unwrap_or_default();
                match end {
                    Progress::Done { rows } => {
                        let text = trf!(
                            "Imported {n} row(s) from {file} into {table} in {seconds} s.",
                            n = rows,
                            file = self.file_name(),
                            table = table,
                            seconds = format!("{seconds:.1}")
                        );
                        self.phase = Phase::Done(text.clone());
                        request = Some(Request::Imported(vec![text]));
                    }
                    Progress::Canceled => {
                        let text = trf!(
                            "Import into {table} canceled; nothing was imported.",
                            table = table
                        );
                        self.error = Some(text.clone());
                        self.phase = Phase::Editing;
                        request = Some(Request::Log(vec![text]));
                    }
                    Progress::Failed(e) => {
                        let text = trf!(
                            "Import into {table} failed; nothing was imported. {error}",
                            table = table,
                            error = e
                        );
                        self.error = Some(text.clone());
                        self.phase = Phase::Editing;
                        request = Some(Request::Log(vec![text]));
                    }
                    Progress::Sent { .. } => {}
                }
            }
        }
        request
    }

    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// Se ainda está lendo o arquivo ou as colunas da tabela.
    pub fn is_loading(&self) -> bool {
        self.target_rx.is_some() || self.sample_rx.is_some()
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn target(&self) -> Option<&Target> {
        self.target.as_ref()
    }

    /// Por que ainda não dá para importar.
    fn invalid(&self) -> Option<String> {
        let (Some(target), Some(sample)) = (&self.target, &self.sample) else {
            return Some(tr("Still reading the file and the table.").to_string());
        };
        if sample.rows(self.options.header).is_empty() {
            return Some(tr("The file has no rows to import.").to_string());
        }
        if Plan::new(target, &self.mapping).is_none() {
            return Some(tr("Choose a file column for at least one table column.").to_string());
        }
        None
    }

    /// O botão "Import": confere, pede confirmação no ambiente que pede e
    /// começa a carga.
    pub fn import(&mut self, ctx: &egui::Context) {
        if let Some(why) = self.invalid() {
            self.error = Some(why);
            return;
        }
        self.error = None;
        if self.environment.is_some() && self.phase != Phase::Confirming {
            self.phase = Phase::Confirming;
            return;
        }
        self.start(ctx);
    }

    /// Começa a carga numa thread.
    fn start(&mut self, ctx: &egui::Context) {
        let Some(plan) = self
            .target
            .as_ref()
            .and_then(|t| Plan::new(t, &self.mapping))
        else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (session, path, options) = (
            self.session.clone(),
            self.path.clone(),
            self.options.clone(),
        );
        let (flag, repaint) = (cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            csv_import::run(&session, &path, &options, &plan, &flag, &mut |p| {
                let _ = tx.send(p);
                repaint.request_repaint();
            });
        });
        self.running = Some(Running {
            rx,
            cancel,
            rows: 0,
            bytes: 0,
            started: Instant::now(),
        });
        self.phase = Phase::Running;
    }

    /// Cancela a carga em andamento (desfaz tudo).
    pub fn cancel(&mut self) {
        if let Some(run) = &self.running {
            run.cancel.store(true, Ordering::Relaxed);
            self.session.cancel();
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<Request> {
        let mut request = self.step();
        let modal = crate::theme::modal(egui::Id::new("csv_import")).show(ctx, |ui| {
            crate::fit::dialog_width(ui, 720.0);
            ui.horizontal(|ui| {
                match &self.target {
                    Some(t) => ui.heading(trf!("Import CSV into {table}", table = t.display())),
                    None => ui.heading(tr("Import CSV")),
                };
                if let Some((name, color)) = &self.environment {
                    registered::environment_badge(ui, name, *color);
                }
            });
            ui.separator();
            let editing = self.phase == Phase::Editing;
            ui.add_enabled_ui(editing, |ui| self.form(ui, ctx));
            ui.separator();
            match &self.phase {
                Phase::Running => {
                    if let (Some(run), Some(sample)) = (&self.running, &self.sample) {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(trf!("Importing... {n} row(s) sent.", n = run.rows));
                            let done = run.bytes as f32 / sample.size.max(1) as f32;
                            ui.add(
                                egui::ProgressBar::new(done.min(1.0))
                                    .show_percentage()
                                    .desired_width(200.0),
                            );
                        });
                    }
                }
                Phase::Confirming => {
                    let (environment, table) = (
                        self.environment
                            .as_ref()
                            .map(|e| e.0.clone())
                            .unwrap_or_default(),
                        self.target
                            .as_ref()
                            .map(Target::display)
                            .unwrap_or_default(),
                    );
                    ui.label(trf!(
                        "This server is in {environment}, and the import adds rows to {table}.",
                        environment = environment,
                        table = table
                    ));
                    ui.label(tr("Do you want to import the file?"));
                }
                Phase::Done(text) => {
                    ui.label(text);
                }
                Phase::Editing => {}
            }
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            let bar = egui::vec2(ui.available_width(), 26.0);
            ui.allocate_ui_with_layout(bar, Layout::right_to_left(Align::Center), |ui| match self
                .phase
            {
                Phase::Editing => {
                    if ui.button(tr("Cancel")).clicked() {
                        request = Some(Request::Closed);
                    }
                    let ready = !self.is_loading();
                    if ui
                        .add_enabled(ready, egui::Button::new(tr("Import")))
                        .clicked()
                    {
                        self.import(ctx);
                    }
                }
                Phase::Confirming => {
                    if ui.button(tr("Cancel")).clicked() {
                        self.phase = Phase::Editing;
                    }
                    if ui.button(tr("Import")).clicked() {
                        self.start(ctx);
                    }
                }
                Phase::Running => {
                    if ui.button(tr("Cancel")).clicked() {
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
        // Fechar por fora (Esc) só quando não há carga em andamento.
        if modal.should_close() && self.phase != Phase::Running && request.is_none() {
            request = Some(Request::Closed);
        }
        request
    }

    fn form(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(tr("File:"));
            ui.label(self.path.display().to_string());
        });
        ui.horizontal(|ui| {
            ui.label(tr("Delimiter:"));
            let before = self.options.delimiter;
            egui::ComboBox::from_id_salt("csv_import_delimiter")
                .selected_text(delimiter_name(before))
                .show_ui(ui, |ui| {
                    for d in DELIMITERS {
                        ui.selectable_value(&mut self.options.delimiter, d, delimiter_name(d));
                    }
                });
            if self.options.delimiter != before {
                self.read_sample(Some(self.options.delimiter), ctx);
            }
            ui.add_space(12.0);
            if ui
                .checkbox(
                    &mut self.options.header,
                    tr("The first row has the column names"),
                )
                .changed()
            {
                self.remap();
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr("Text that means NULL:"));
            ui.add(
                egui::TextEdit::singleline(&mut self.options.null_text)
                    .desired_width(140.0)
                    .hint_text(tr("(empty field)")),
            );
        });
        ui.add_space(4.0);
        let (Some(target), Some(sample)) = (&self.target, &self.sample) else {
            if self.is_loading() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Reading the file and the table..."));
                });
            }
            return;
        };
        let headers = sample.headers(self.options.header);
        let rows = sample.rows(self.options.header);
        ui.strong(tr("Columns"));
        egui::ScrollArea::vertical()
            .id_salt("csv_import_columns")
            .max_height(220.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("csv_import_mapping")
                    .num_columns(3)
                    .striped(true)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        ui.weak(tr("Table column"));
                        ui.weak(tr("Type"));
                        ui.weak(tr("File column"));
                        ui.end_row();
                        for (i, column) in target.columns.iter().enumerate() {
                            ui.label(&column.name);
                            ui.weak(&column.type_name);
                            if column.kind == ColumnKind::Computed {
                                ui.weak(tr("(computed by the database)"));
                                ui.end_row();
                                continue;
                            }
                            let Some(choice) = self.mapping.get_mut(i) else {
                                ui.end_row();
                                continue;
                            };
                            let skip = tr("(do not import)");
                            let label = |c: usize| {
                                let name = headers.get(c).cloned().unwrap_or_default();
                                match rows.first().and_then(|r| r.get(c)) {
                                    Some(f) if !f.text.is_empty() => {
                                        format!("{name}  ({})", shorten(&f.text, 24))
                                    }
                                    _ => name,
                                }
                            };
                            egui::ComboBox::from_id_salt(("csv_import_column", i))
                                .width(260.0)
                                .selected_text(match choice {
                                    Some(c) => label(*c),
                                    None => skip.to_string(),
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(choice, None, skip);
                                    for c in 0..headers.len() {
                                        ui.selectable_value(choice, Some(c), label(c));
                                    }
                                });
                            ui.end_row();
                        }
                    });
            });
        ui.add_space(4.0);
        ui.strong(tr("Preview"));
        let mapped: Vec<(usize, usize)> = self
            .mapping
            .iter()
            .enumerate()
            .filter_map(|(i, m)| Some((i, (*m)?)))
            .collect();
        if mapped.is_empty() {
            ui.weak(tr("Choose a file column for at least one table column."));
            return;
        }
        let options = &self.options;
        egui::ScrollArea::both()
            .id_salt("csv_import_preview")
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("csv_import_preview_grid")
                    .striped(true)
                    .spacing([16.0, 2.0])
                    .show(ui, |ui| {
                        for (i, _) in &mapped {
                            ui.strong(&target.columns[*i].name);
                        }
                        ui.end_row();
                        for row in rows.iter().take(PREVIEW_ROWS) {
                            for (_, c) in &mapped {
                                match row.get(*c).and_then(|f| options.value(f)) {
                                    Some(v) => ui.label(shorten(v, 40)),
                                    None => ui.weak("NULL"), // i18n-ignore
                                };
                            }
                            ui.end_row();
                        }
                    });
            });
    }
}

/// Nome do separador na lista.
fn delimiter_name(delimiter: u8) -> &'static str {
    match delimiter {
        b';' => tr("Semicolon (;)"),
        b'\t' => tr("Tab"),
        b'|' => tr("Pipe (|)"),
        _ => tr("Comma (,)"),
    }
}

/// O texto numa linha só, com no máximo `max` caracteres.
fn shorten(text: &str, max: usize) -> String {
    let mut out: String = text
        .chars()
        .take(max)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}
