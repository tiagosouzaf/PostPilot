//! Histórico de consultas executadas: o que rodou, em qual servidor e banco,
//! quando, quanto demorou e como terminou.
//!
//! Fica só neste computador, em `<config>/PostPilot/history.jsonl` (uma
//! linha de JSON por execução, acrescentada no fim), nunca no workspace do
//! Git. Guarda no máximo [`MAX_ENTRIES`] execuções e [`MAX_BYTES`] no
//! arquivo; as mais antigas saem primeiro.
//!
//! **Senhas não entram aqui.** O servidor é guardado pela chave
//! `usuario@host:porta`, e o texto passa por [`mask_passwords`] antes de ser
//! gravado: o valor depois de `PASSWORD` (e parecidos) e o `password=` de
//! uma connection string viram `***`.
//!
//! A janela (View > Query History) lista as execuções, da mais nova para a
//! mais antiga, com uma pesquisa que olha o texto, o servidor, o banco e o
//! erro; um duplo clique reabre a consulta numa janela nova.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::connection::Engine;
use pgstudio_i18n::{tr, trf};
use pgstudio_sql_editor::lexer::{Dialect, TokenKind, tokenize_in};
use serde::{Deserialize, Serialize};

use crate::execution::{Outcome, format_elapsed};
use crate::repo::search::{fold, terms};

/// Quantas execuções o histórico guarda.
pub const MAX_ENTRIES: usize = 5000;

/// Tamanho máximo do arquivo, mais ou menos.
pub const MAX_BYTES: usize = 20 * 1024 * 1024;

/// Texto máximo guardado de uma execução, em caracteres. Um script maior
/// fica só com o começo (e a marca `truncated`).
pub const MAX_SQL_CHARS: usize = 100_000;

/// Quanto do texto a janela mostra ao lado (o resto abre na janela de
/// consulta).
const PREVIEW_CHARS: usize = 20_000;

/// Como a execução terminou.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    #[default]
    Succeeded,
    Failed,
    Canceled,
}

impl Status {
    fn from_outcome(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Failed => Status::Failed,
            Outcome::Canceled => Status::Canceled,
            Outcome::Succeeded | Outcome::Running => Status::Succeeded,
        }
    }

    /// Texto curto para a lista.
    pub fn label(self) -> &'static str {
        match self {
            Status::Succeeded => tr("Succeeded"),
            Status::Failed => tr("Failed"),
            Status::Canceled => tr("Canceled"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Status::Succeeded => icon::CHECK_CIRCLE,
            Status::Failed => icon::X_CIRCLE,
            Status::Canceled => icon::PROHIBIT,
        }
    }

    fn color(self, ui: &egui::Ui) -> egui::Color32 {
        match self {
            Status::Succeeded if ui.visuals().dark_mode => egui::Color32::from_rgb(110, 200, 120),
            Status::Succeeded => egui::Color32::from_rgb(30, 130, 50),
            Status::Failed => ui.visuals().error_fg_color,
            Status::Canceled => ui.visuals().warn_fg_color,
        }
    }
}

/// Uma execução.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// Quando terminou, em segundos desde 1970 (UTC).
    pub when: u64,
    /// Chave do servidor (`usuario@host:porta`). Sem senha.
    pub server: String,
    /// Nome do servidor como aparecia na janela ("servidor (usuário)").
    pub server_name: String,
    pub engine: Engine,
    pub database: String,
    /// Nome da janela de consulta (`SQLQuery1.sql` ou o arquivo).
    pub window: String,
    /// O texto executado, com as senhas mascaradas.
    pub sql: String,
    /// O texto era maior do que [`MAX_SQL_CHARS`] e ficou só o começo.
    pub truncated: bool,
    pub elapsed_ms: u64,
    pub status: Status,
    /// Linhas devolvidas ou alteradas.
    pub rows: u64,
    /// O primeiro erro, se falhou.
    pub error: Option<String>,
    /// Número da entrada nesta execução do app (para a seleção da janela).
    #[serde(skip)]
    id: u64,
    /// Tudo o que a pesquisa olha, sem acentos e em minúsculas.
    #[serde(skip)]
    folded: String,
}

impl Entry {
    /// Monta a entrada de uma execução que acabou de terminar, já com as
    /// senhas mascaradas e o texto no tamanho máximo.
    #[allow(clippy::too_many_arguments)]
    pub fn finished(
        server: &str,
        server_name: &str,
        engine: Engine,
        database: &str,
        window: &str,
        sql: &str,
        elapsed: Duration,
        outcome: Outcome,
        rows: u64,
        error: Option<&str>,
    ) -> Self {
        let masked = mask_passwords(sql, engine);
        let truncated = masked.chars().count() > MAX_SQL_CHARS;
        let sql = if truncated {
            masked.chars().take(MAX_SQL_CHARS).collect()
        } else {
            masked
        };
        Entry {
            when: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            server: server.to_string(),
            server_name: server_name.to_string(),
            engine,
            database: database.to_string(),
            window: window.to_string(),
            sql,
            truncated,
            elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
            status: Status::from_outcome(outcome),
            rows,
            error: error.map(|e| mask_passwords(e, engine)),
            id: 0,
            folded: String::new(),
        }
    }

    /// Número da entrada (vale enquanto o app está aberto).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Servidor para mostrar: o nome, ou a chave quando não há nome.
    pub fn server_label(&self) -> &str {
        if self.server_name.is_empty() {
            &self.server
        } else {
            &self.server_name
        }
    }

    /// Data e hora locais, `2026-11-15 22:41:07`.
    pub fn when_label(&self) -> String {
        local_time(self.when)
    }

    /// Duração como na faixa de status da janela de consulta.
    pub fn elapsed_label(&self) -> String {
        format_elapsed(Duration::from_millis(self.elapsed_ms), true)
    }

    /// O começo do texto numa linha só, para a lista.
    pub fn snippet(&self) -> String {
        let head: String = self.sql.chars().take(400).collect();
        head.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn prepare(&mut self, id: u64) {
        self.id = id;
        self.folded = fold(&format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.sql,
            self.server,
            self.server_name,
            self.database,
            self.window,
            self.error.as_deref().unwrap_or_default(),
            self.when_label(),
        ));
    }

    /// Se todos os termos (já sem acentos e em minúsculas) aparecem.
    pub fn matches(&self, terms: &[String]) -> bool {
        terms.iter().all(|t| self.folded.contains(t.as_str()))
    }
}

/// Segundos desde 1970 como data e hora no fuso do sistema.
pub fn local_time(secs: u64) -> String {
    match chrono::DateTime::from_timestamp(secs.min(i64::MAX as u64) as i64, 0) {
        Some(utc) => utc
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        None => String::new(),
    }
}

/// Troca por `***` as senhas dos comandos do Redis: tudo depois de `AUTH`
/// (também no `HELLO ... AUTH usuario senha` e no `MIGRATE`), o valor de
/// `requirepass` e `masterauth`, e as senhas do `ACL SETUSER` (`>senha`,
/// `<senha`, `#hash`).
fn mask_redis_passwords(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let ending = &line[body.len()..];
        let tokens = tokenize_in(body, Dialect::Redis);
        let words: Vec<String> = tokens
            .iter()
            .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
            .map(|t| body[t.range.clone()].to_ascii_lowercase())
            .collect();
        let acl = words.first().is_some_and(|w| w == "acl")
            && words.get(1).is_some_and(|w| w == "setuser");
        // Depois de AUTH tudo é segredo; depois de requirepass, a próxima
        // palavra.
        let mut all = false;
        let mut next = false;
        for t in &tokens {
            let piece = &body[t.range.clone()];
            if matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment) {
                out.push_str(piece);
                continue;
            }
            let word = piece.to_ascii_lowercase();
            if all || next || (acl && piece.starts_with(['>', '<', '#'])) {
                out.push_str("***");
                next = false;
            } else {
                out.push_str(piece);
                all = word == "auth";
                next = matches!(word.as_str(), "requirepass" | "masterauth");
            }
        }
        out.push_str(ending);
    }
    out
}

/// Troca por `***` o que parece senha no SQL:
///
/// - o texto (ou o hash) depois de `PASSWORD`, `OLD_PASSWORD`, `SECRET` e
///   `IDENTIFIED BY`, com ou sem `=`: `ALTER ROLE x PASSWORD 'abc'`,
///   `CREATE LOGIN x WITH PASSWORD = N'abc'`;
/// - o valor de `password=` e `pwd=` dentro de um texto, como a connection
///   string de um `dblink` ou `OPENROWSET`.
///
/// É o melhor esforço: uma senha escrita de outro jeito (num `INSERT` numa
/// tabela de usuários, por exemplo) não tem como ser reconhecida.
pub fn mask_passwords(sql: &str, engine: Engine) -> String {
    let dialect = match engine {
        Engine::PostgreSql => Dialect::Postgres,
        Engine::SqlServer => Dialect::TSql,
        Engine::Redis => return mask_redis_passwords(sql),
        // Uma consulta do MongoDB não leva a senha da conexão; senhas
        // dentro de `createUser` ficam como foram escritas.
        // Nem uma requisição do Elasticsearch.
        Engine::MongoDb | Engine::Elasticsearch => return sql.to_string(),
    };
    let mut out = String::with_capacity(sql.len());
    // Depois de PASSWORD (e do `=`), o próximo texto é a senha.
    let mut armed = false;
    let mut identified = false;
    let mut copied = 0;
    for t in tokenize_in(sql, dialect) {
        let text = &sql[t.range.clone()];
        let replacement = match t.kind {
            TokenKind::Whitespace | TokenKind::Comment => None,
            TokenKind::Keyword | TokenKind::Identifier => {
                let word = text.to_ascii_lowercase();
                let by = identified && word == "by";
                identified = word == "identified";
                armed = by
                    || matches!(word.as_str(), "password" | "old_password" | "secret")
                    // `N'...'` com o N separado do texto.
                    || (armed && word == "n");
                None
            }
            TokenKind::Operator if armed && text == "=" => None,
            TokenKind::String | TokenKind::Number if armed => {
                armed = false;
                Some(if t.kind == TokenKind::String {
                    "'***'".to_string()
                } else {
                    "***".to_string()
                })
            }
            TokenKind::String => {
                let masked = mask_connection_string(text);
                (masked != text).then_some(masked)
            }
            _ => {
                armed = false;
                identified = false;
                None
            }
        };
        if let Some(r) = replacement {
            out.push_str(&sql[copied..t.range.start]);
            out.push_str(&r);
            copied = t.range.end;
        }
    }
    out.push_str(&sql[copied..]);
    out
}

/// Mascara o valor de `password=` / `pwd=` num texto.
fn mask_connection_string(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut out = String::new();
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        let key = ["password", "pwd"]
            .iter()
            .find(|k| lower[i..].starts_with(**k))
            .filter(|_| i == 0 || !bytes[i - 1].is_ascii_alphanumeric());
        let Some(key) = key else {
            // Só começos de caractere, para o corte abaixo ser válido.
            i += 1;
            while i < bytes.len() && !lower.is_char_boundary(i) {
                i += 1;
            }
            continue;
        };
        let mut j = i + key.len();
        while j < bytes.len() && bytes[j] == b' ' {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'=' {
            i += key.len();
            continue;
        }
        j += 1;
        while j < bytes.len() && bytes[j] == b' ' {
            j += 1;
        }
        let start = j;
        while j < bytes.len() && !matches!(bytes[j], b' ' | b';' | b'\'' | b'"' | b'\n' | b'\t') {
            j += 1;
        }
        if j > start {
            out.push_str(&text[copied..start]);
            out.push_str("***");
            copied = j;
        }
        i = j.max(i + key.len());
    }
    out.push_str(&text[copied..]);
    out
}

/// O histórico em memória e o arquivo dele.
#[derive(Debug, Default)]
pub struct HistoryStore {
    /// Sem arquivo, o histórico vale só enquanto o app está aberto.
    path: Option<PathBuf>,
    /// Da mais antiga para a mais nova.
    entries: Vec<Entry>,
    /// Tamanho aproximado do arquivo.
    bytes: usize,
    next_id: u64,
    /// Muda a cada alteração, para a janela refazer a lista.
    generation: u64,
    /// Problema ao ler ou gravar, para avisar o usuário.
    pub error: Option<String>,
}

impl HistoryStore {
    /// Lê `path`. Sem arquivo, começa vazio; linhas que não dá para ler
    /// são puladas.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut store = HistoryStore {
            path: Some(path.clone()),
            ..Default::default()
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                for line in text.lines().filter(|l| !l.trim().is_empty()) {
                    if let Ok(mut entry) = serde_json::from_str::<Entry>(line) {
                        store.next_id += 1;
                        entry.prepare(store.next_id);
                        store.bytes += line.len() + 1;
                        store.entries.push(entry);
                    }
                }
                if store.over_limit() {
                    store.trim();
                    store.rewrite();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => store.error = Some(e.to_string()),
        }
        store
    }

    /// Arquivo do histórico, se houver.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// As execuções, da mais antiga para a mais nova.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Guarda uma execução e a acrescenta ao arquivo.
    pub fn add(&mut self, mut entry: Entry) {
        self.next_id += 1;
        entry.prepare(self.next_id);
        let line = serde_json::to_string(&entry).unwrap_or_default();
        self.bytes += line.len() + 1;
        self.entries.push(entry);
        self.generation += 1;
        if self.over_limit() {
            self.trim();
            self.rewrite();
        } else if let Some(path) = &self.path {
            let appended = (|| {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                file.write_all(format!("{line}\n").as_bytes())
            })();
            self.error = appended.err().map(|e| e.to_string());
        }
    }

    /// Apaga uma execução.
    pub fn remove(&mut self, id: u64) {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        if self.entries.len() != before {
            self.generation += 1;
            self.rewrite();
        }
    }

    /// Apaga tudo.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.generation += 1;
        self.rewrite();
    }

    fn over_limit(&self) -> bool {
        self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES
    }

    /// Tira as mais antigas, com folga para não regravar o arquivo a cada
    /// execução depois de chegar no limite.
    fn trim(&mut self) {
        let sizes: Vec<usize> = self
            .entries
            .iter()
            .map(|e| serde_json::to_string(e).map_or(0, |l| l.len() + 1))
            .collect();
        let mut bytes: usize = sizes.iter().sum();
        let mut drop = self.entries.len().saturating_sub(MAX_ENTRIES * 9 / 10);
        if bytes > MAX_BYTES {
            let mut n = 0;
            while n < sizes.len() && bytes > MAX_BYTES * 9 / 10 {
                bytes -= sizes[n];
                n += 1;
            }
            drop = drop.max(n);
        }
        // A mais nova sempre fica.
        let drop = drop.min(self.entries.len().saturating_sub(1));
        self.entries.drain(..drop);
        self.generation += 1;
    }

    /// Regrava o arquivo inteiro (ao lado, depois renomeia).
    fn rewrite(&mut self) {
        let mut text = String::new();
        for e in &self.entries {
            if let Ok(line) = serde_json::to_string(e) {
                text.push_str(&line);
                text.push('\n');
            }
        }
        self.bytes = text.len();
        let Some(path) = &self.path else {
            return;
        };
        let written = (|| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut tmp = path.as_os_str().to_owned();
            tmp.push(".tmp");
            let tmp = PathBuf::from(tmp);
            std::fs::write(&tmp, &text)?;
            std::fs::rename(&tmp, path)
        })();
        self.error = written.err().map(|e| e.to_string());
    }
}

/// O histórico e a janela dele.
#[derive(Debug, Default)]
pub struct History {
    pub store: HistoryStore,
    /// O que está escrito na pesquisa (volta ao reabrir o app).
    pub filter: String,
    selected: Option<u64>,
    /// Posições (em `store.entries`) das execuções achadas, a mais nova
    /// primeiro, e para qual pesquisa e versão do histórico valem.
    shown: Vec<usize>,
    shown_for: Option<(String, u64)>,
    confirm_clear: bool,
}

impl History {
    /// Um histórico gravado neste arquivo.
    pub fn with_store(store: HistoryStore) -> Self {
        History {
            store,
            ..Default::default()
        }
    }

    /// As execuções que a pesquisa atual mostra, a mais nova primeiro.
    pub fn shown(&mut self) -> Vec<&Entry> {
        self.refresh();
        self.shown.iter().map(|i| &self.store.entries[*i]).collect()
    }

    /// A execução selecionada na janela.
    pub fn selected(&self) -> Option<&Entry> {
        let id = self.selected?;
        self.store.entries.iter().find(|e| e.id == id)
    }

    fn refresh(&mut self) {
        let key = (self.filter.trim().to_string(), self.store.generation);
        if self.shown_for.as_ref() == Some(&key) {
            return;
        }
        let terms = terms(&key.0);
        self.shown = (0..self.store.entries.len())
            .rev()
            .filter(|i| self.store.entries[*i].matches(&terms))
            .collect();
        self.shown_for = Some(key);
    }

    /// Desenha a janela. Devolve a execução que o usuário pediu para
    /// reabrir numa janela de consulta.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Entry> {
        self.refresh();
        let mut open: Option<u64> = None;
        let mut open_first = false;
        ui.horizontal(|ui| {
            let layout = egui::Layout::right_to_left(egui::Align::Center);
            ui.with_layout(layout, |ui| {
                let clear = ui
                    .add_enabled(
                        !self.store.is_empty(),
                        egui::Button::new(format!("{} {}", icon::TRASH, tr("Clear History…"))),
                    )
                    .on_hover_text(tr("Delete every execution kept in the history."));
                if clear.clicked() {
                    self.confirm_clear = true;
                }
                let total = self.store.len();
                let count = if self.filter.trim().is_empty() {
                    trf!("{n} executions", n = total)
                } else {
                    trf!(
                        "{found} of {n} executions",
                        found = self.shown.len(),
                        n = total
                    )
                };
                ui.weak(count);
                let erase = !self.filter.is_empty()
                    && ui
                        .add(egui::Button::new(icon::X).frame(false))
                        .on_hover_text(tr("Clear the search"))
                        .clicked();
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .id_salt("query_history_search")
                        .hint_text(format!(
                            "{} {}",
                            icon::MAGNIFYING_GLASS,
                            tr("Search the history by text, server, database or error")
                        ))
                        .desired_width(f32::INFINITY),
                );
                if field.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.filter.clear();
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    open_first = true;
                }
                if erase {
                    self.filter.clear();
                    field.request_focus();
                }
            });
        });
        self.refresh();
        if self.confirm_clear {
            ui.horizontal(|ui| {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    trf!(
                        "Delete all {n} executions of the history? This cannot be undone.",
                        n = self.store.len()
                    ),
                );
                if ui.button(tr("Delete All")).clicked() {
                    self.store.clear();
                    self.selected = None;
                    self.confirm_clear = false;
                }
                if ui.button(tr("Cancel")).clicked() {
                    self.confirm_clear = false;
                }
            });
            self.refresh();
        }
        if let Some(e) = &self.store.error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                trf!("The history could not be saved: {error}", error = e),
            );
        }
        ui.separator();
        if self.store.is_empty() {
            ui.weak(tr(
                "No query has been executed yet. Every execution is kept here, on this computer only.",
            ));
            return None;
        }
        if self.shown.is_empty() {
            ui.weak(tr("No execution found."));
            return None;
        }
        if open_first {
            open = self.shown.first().map(|i| self.store.entries[*i].id);
        }
        // O detalhe da execução selecionada fica ao lado quando cabe.
        let mut delete = None;
        let entry = self.selected().cloned();
        let mut on_detail = |action: Option<DetailAction>, id: u64| match action {
            Some(DetailAction::Open) => open = Some(id),
            Some(DetailAction::Delete) => delete = Some(id),
            None => {}
        };
        // O detalhe fica ao lado quando a janela é larga e embaixo quando
        // não é.
        let beside = ui.available_width() >= 900.0;
        if beside && let Some(entry) = &entry {
            egui::Panel::right(egui::Id::new("query_history_details"))
                .resizable(true)
                .default_size(ui.available_width() * 0.42)
                .min_size(260.0)
                .show(ui, |ui| on_detail(details(ui, entry, false), entry.id));
        }
        let below = entry.as_ref().filter(|_| !beside);
        let list_height = match below {
            Some(_) => (ui.available_height() * 0.45).max(80.0),
            None => ui.available_height(),
        };
        let widths = Widths::for_width(ui.available_width());
        header(ui, &widths);
        let row_h = ui.spacing().interact_size.y;
        let mut clicked = None;
        let mut double_clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("query_history_rows")
            .max_height(list_height - row_h)
            .auto_shrink(false)
            .show_rows(ui, row_h, self.shown.len(), |ui, range| {
                for i in range {
                    let entry = &self.store.entries[self.shown[i]];
                    let selected = self.selected == Some(entry.id);
                    let response = row(ui, entry, selected, &widths, row_h);
                    if response.clicked() {
                        clicked = Some(entry.id);
                    }
                    if response.double_clicked() {
                        double_clicked = Some(entry.id);
                    }
                }
            });
        if let Some(entry) = below {
            ui.separator();
            on_detail(details(ui, entry, true), entry.id);
        }
        if clicked.is_some() {
            self.selected = clicked;
        }
        if double_clicked.is_some() {
            open = double_clicked;
        }
        if let Some(id) = delete {
            self.store.remove(id);
            self.selected = None;
        }
        let id = open?;
        self.selected = Some(id);
        self.selected().cloned()
    }
}

/// Larguras das colunas da lista. As de servidor e banco somem quando a
/// janela é estreita.
struct Widths {
    when: f32,
    server: f32,
    database: f32,
    elapsed: f32,
    rows: f32,
}

impl Widths {
    fn for_width(width: f32) -> Self {
        Widths {
            when: 136.0,
            server: if width >= 460.0 {
                (width * 0.16).clamp(110.0, 240.0)
            } else {
                0.0
            },
            database: if width >= 620.0 {
                (width * 0.1).clamp(70.0, 150.0)
            } else {
                0.0
            },
            elapsed: 84.0,
            rows: if width >= 860.0 { 56.0 } else { 0.0 },
        }
    }
}

const STATUS_WIDTH: f32 = 20.0;

/// Uma célula de largura fixa, com o texto cortado se não couber.
fn cell(ui: &mut egui::Ui, width: f32, height: f32, text: impl Into<egui::WidgetText>) {
    if width <= 0.0 {
        return;
    }
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(egui::vec2(width, height), layout, |ui| {
        ui.set_min_width(width);
        ui.add(egui::Label::new(text).truncate());
    });
}

fn header(ui: &mut egui::Ui, w: &Widths) {
    let h = ui.text_style_height(&egui::TextStyle::Body);
    ui.horizontal(|ui| {
        let weak = |t: &str| egui::RichText::new(t).weak();
        cell(ui, STATUS_WIDTH, h, "");
        cell(ui, w.when, h, weak(tr("Executed at")));
        cell(ui, w.server, h, weak(tr("Server")));
        cell(ui, w.database, h, weak(tr("Database")));
        cell(ui, w.elapsed, h, weak(tr("Duration")));
        cell(ui, w.rows, h, weak(tr("Rows")));
        ui.add(egui::Label::new(weak(tr("Query"))).truncate());
    });
}

fn row(ui: &mut egui::Ui, e: &Entry, selected: bool, w: &Widths, h: f32) -> egui::Response {
    ui.horizontal(|ui| {
        let status = egui::RichText::new(e.status.icon()).color(e.status.color(ui));
        ui.allocate_ui_with_layout(
            egui::vec2(STATUS_WIDTH, h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(STATUS_WIDTH);
                ui.label(status).on_hover_text(e.status.label());
            },
        );
        cell(ui, w.when, h, e.when_label());
        cell(ui, w.server, h, e.server_label());
        cell(ui, w.database, h, e.database.as_str());
        cell(ui, w.elapsed, h, e.elapsed_label());
        cell(ui, w.rows, h, e.rows.to_string());
        let text = egui::RichText::new(e.snippet()).monospace();
        let width = ui.available_width();
        // Justificado à esquerda: o botão ocupa o resto da linha.
        let layout = egui::Layout::top_down_justified(egui::Align::Min);
        ui.allocate_ui_with_layout(egui::vec2(width, h), layout, |ui| {
            ui.add(egui::Button::selectable(selected, text).truncate())
        })
        .inner
        .on_hover_ui(|ui| {
            ui.set_max_width(560.0);
            ui.label(format!(
                "{} · {} · {} · {}",
                e.when_label(),
                e.server_label(),
                e.database,
                e.elapsed_label()
            ));
            if let Some(error) = &e.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            let head: String = e.sql.chars().take(1200).collect();
            ui.monospace(head);
            ui.weak(tr("Double-click to open it in a new query window."));
        })
    })
    .inner
}

enum DetailAction {
    Open,
    Delete,
}

/// O painel ao lado: os dados da execução e o texto.
fn details(ui: &mut egui::Ui, e: &Entry, compact: bool) -> Option<DetailAction> {
    let mut action = None;
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        let open = egui::Button::new(format!(
            "{} {}",
            icon::ARROW_SQUARE_OUT,
            tr("Open in New Query Window")
        ));
        if ui.add(open).clicked() {
            action = Some(DetailAction::Open);
        }
        if ui
            .button(format!("{} {}", icon::COPY, tr("Copy")))
            .clicked()
        {
            ui.ctx().copy_text(e.sql.clone());
        }
        if ui
            .button(format!("{} {}", icon::TRASH, tr("Delete")))
            .on_hover_text(tr("Remove this execution from the history."))
            .clicked()
        {
            action = Some(DetailAction::Delete);
        }
    });
    ui.separator();
    if compact {
        // Embaixo da lista: tudo numa linha, para sobrar altura para o texto.
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                e.status.color(ui),
                format!("{} {}", e.status.icon(), e.status.label()),
            );
            let mut parts = vec![
                e.when_label(),
                e.server_label().to_string(),
                e.database.clone(),
                e.elapsed_label(),
                trf!("{n} rows", n = e.rows),
            ];
            if !e.window.is_empty() {
                parts.push(e.window.clone());
            }
            ui.label(parts.join("  ·  "));
        });
    } else {
        egui::Grid::new("query_history_fields")
            .num_columns(2)
            .spacing([10.0, 3.0])
            .show(ui, |ui| {
                let field = |ui: &mut egui::Ui, name: &str, value: egui::RichText| {
                    ui.weak(name);
                    ui.add(egui::Label::new(value).wrap());
                    ui.end_row();
                };
                field(ui, tr("Result"), {
                    egui::RichText::new(format!("{} {}", e.status.icon(), e.status.label()))
                        .color(e.status.color(ui))
                });
                field(ui, tr("Executed at"), e.when_label().into());
                field(ui, tr("Server"), e.server_label().into());
                field(ui, tr("Database"), e.database.as_str().into());
                field(ui, tr("Duration"), e.elapsed_label().into());
                field(ui, tr("Rows"), e.rows.to_string().into());
                if !e.window.is_empty() {
                    field(ui, tr("Window"), e.window.as_str().into());
                }
            });
    }
    if let Some(error) = &e.error {
        ui.add_space(2.0);
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if !compact {
        ui.separator();
    }
    let long = e.sql.chars().count() > PREVIEW_CHARS;
    if e.truncated {
        ui.weak(tr(
            "The script was too long: only its beginning was kept in the history.",
        ));
    } else if long {
        ui.weak(tr(
            "Showing the beginning of the text; open it to see everything.",
        ));
    }
    let preview: String = if long {
        e.sql.chars().take(PREVIEW_CHARS).collect()
    } else {
        e.sql.clone()
    };
    egui::ScrollArea::both()
        .id_salt("query_history_sql")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut preview.as_str())
                    .id_salt("query_history_sql_text")
                    .code_editor()
                    .desired_width(f32::INFINITY),
            );
        });
    action
}

/// Uma execução que acabou de terminar numa janela de consulta, antes de
/// virar uma [`Entry`].
#[derive(Debug)]
pub(crate) struct Finished {
    pub doc: u32,
    pub window: String,
    pub sql: String,
    pub database: String,
    pub elapsed: Duration,
    pub outcome: Outcome,
    pub rows: u64,
    pub error: Option<String>,
    /// Rodou em vários servidores de uma vez (veja [`crate::multi`]).
    pub multi: bool,
}

impl crate::ShellApp {
    /// Passa a gravar o histórico de consultas neste arquivo (e mostra o
    /// que já está nele).
    pub fn with_history(mut self, store: HistoryStore) -> Self {
        if let Some(e) = &store.error {
            eprintln!("history: {e}");
        }
        self.history.store = store;
        self
    }

    /// O histórico de consultas executadas.
    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    /// Abre a janela Query History.
    pub fn show_query_history(&mut self) {
        self.workspace.open(crate::panels::Panel::QueryHistory);
    }

    /// Guarda no histórico a execução que terminou na janela `run.doc`.
    pub(crate) fn record_history(&mut self, run: Finished) {
        let (key, name, engine) = if run.multi {
            // Vários servidores: fica "N servers" no lugar do servidor, e a
            // consulta reaberta roda no servidor ativo.
            let label = self
                .multi_labels()
                .into_iter()
                .map(|(doc, label, _)| (doc, label))
                .chain(self.federated_labels())
                .find(|(doc, _)| *doc == run.doc)
                .map(|(_, label)| label)
                .unwrap_or_default();
            (String::new(), label, Engine::default())
        } else {
            let key = self.doc_servers.get(&run.doc).cloned().or_else(|| {
                self.connection
                    .as_ref()
                    .map(|c| crate::servers::server_key(&c.settings))
            });
            // Sem servidor (o de demonstração), nada rodou.
            let Some(key) = key else {
                return;
            };
            let name = self
                .document_servers()
                .get(&run.doc)
                .cloned()
                .unwrap_or_else(|| key.clone());
            let engine = self.engine_of_key(&key).unwrap_or_default();
            (key, name, engine)
        };
        self.history.store.add(Entry::finished(
            &key,
            &name,
            engine,
            &run.database,
            &run.window,
            &run.sql,
            run.elapsed,
            run.outcome,
            run.rows,
            run.error.as_deref(),
        ));
    }

    /// Reabre uma execução do histórico numa janela de consulta nova, no
    /// mesmo banco e, se ele ainda estiver conectado, no mesmo servidor.
    pub fn open_history_entry(&mut self, entry: &Entry) {
        let database = (!entry.database.is_empty()).then(|| entry.database.clone());
        let same_server = !entry.server.is_empty() && self.engine_of_key(&entry.server).is_some();
        let id = self.documents.new_script(
            entry.sql.clone(),
            // O banco só vale no servidor de onde veio.
            database.filter(|_| same_server),
        );
        if same_server {
            self.doc_servers.insert(id, entry.server.clone());
        }
        self.activate_document(id);
        self.status = if same_server {
            trf!(
                "Query from the history opened on {server}.",
                server = entry.server_label()
            )
        } else {
            trf!(
                "Query from the history opened; {server} is not connected, so it will run on the active server.",
                server = entry.server_label()
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_passwords_are_masked() {
        let m = |t: &str| mask_passwords(t, Engine::Redis);
        assert_eq!(m("AUTH s3nha\nGET a"), "AUTH ***\nGET a");
        assert_eq!(m("auth app \"minha senha\""), "auth *** ***");
        assert_eq!(
            m("HELLO 3 AUTH app s3nha SETNAME x"),
            "HELLO 3 AUTH *** *** *** ***"
        );
        assert_eq!(
            m("CONFIG SET requirepass s3nha"),
            "CONFIG SET requirepass ***"
        );
        assert_eq!(
            m("ACL SETUSER app on >s3nha ~* +@all"),
            "ACL SETUSER app on *** ~* +@all"
        );
        assert_eq!(m("SET senha:1 abc\r\n"), "SET senha:1 abc\r\n");
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "postpilot-history-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("history.jsonl")
    }

    fn entry(sql: &str, server: &str, database: &str) -> Entry {
        Entry::finished(
            server,
            "",
            Engine::PostgreSql,
            database,
            "SQLQuery1.sql",
            sql,
            Duration::from_millis(42),
            Outcome::Succeeded,
            3,
            None,
        )
    }

    #[test]
    fn executions_survive_a_restart() {
        let path = temp_file("restart");
        let mut store = HistoryStore::open(&path);
        store.add(entry("select 1", "ana@db1:5432", "vendas"));
        let mut failed = entry("select * from nada", "ana@db2:5432", "estoque");
        failed.status = Status::Failed;
        failed.error = Some("relation \"nada\" does not exist".into());
        store.add(failed);
        assert!(store.error.is_none());

        let again = HistoryStore::open(&path);
        assert_eq!(again.len(), 2);
        let last = &again.entries()[1];
        assert_eq!(last.sql, "select * from nada");
        assert_eq!(last.server, "ana@db2:5432");
        assert_eq!(last.database, "estoque");
        assert_eq!(last.status, Status::Failed);
        assert_eq!(last.elapsed_ms, 42);
        assert_eq!(last.rows, 3);
        assert!(last.error.as_deref().unwrap().contains("nada"));
    }

    #[test]
    fn search_looks_at_text_server_database_and_error() {
        let mut history = History::default();
        history
            .store
            .add(entry("select * from clientes", "ana@db1:5432", "vendas"));
        history.store.add(entry(
            "update produtos set preço = 1",
            "ana@db2:5432",
            "estoque",
        ));
        let found = |h: &mut History, text: &str| {
            h.filter = text.to_string();
            h.shown().iter().map(|e| e.sql.clone()).collect::<Vec<_>>()
        };
        // A mais nova primeiro.
        assert_eq!(found(&mut history, "").len(), 2);
        assert!(found(&mut history, "")[0].starts_with("update"));
        assert_eq!(found(&mut history, "CLIENTES"), ["select * from clientes"]);
        // Sem acento acha com acento; servidor e banco também contam.
        assert_eq!(found(&mut history, "preco").len(), 1);
        assert_eq!(found(&mut history, "db2").len(), 1);
        assert_eq!(found(&mut history, "vendas select").len(), 1);
        assert!(found(&mut history, "vendas update").is_empty());
    }

    #[test]
    fn passwords_are_masked() {
        let pg = |sql| mask_passwords(sql, Engine::PostgreSql);
        assert_eq!(
            pg("ALTER ROLE ana WITH PASSWORD 's3gr3do';"),
            "ALTER ROLE ana WITH PASSWORD '***';"
        );
        assert_eq!(
            pg("create user x encrypted password\n  'abc' valid until 'infinity'"),
            "create user x encrypted password\n  '***' valid until 'infinity'"
        );
        assert_eq!(
            pg("select * from dblink('host=h user=u password=abc123 dbname=d', 'select 1')"),
            "select * from dblink('host=h user=u password=*** dbname=d', 'select 1')"
        );
        let ms = |sql| mask_passwords(sql, Engine::SqlServer);
        assert_eq!(
            ms("CREATE LOGIN ana WITH PASSWORD = N'S3gr3do!', CHECK_POLICY = OFF"),
            "CREATE LOGIN ana WITH PASSWORD = '***', CHECK_POLICY = OFF"
        );
        assert_eq!(
            ms("ALTER LOGIN ana WITH PASSWORD = 'nova' OLD_PASSWORD = 'velha'"),
            "ALTER LOGIN ana WITH PASSWORD = '***' OLD_PASSWORD = '***'"
        );
        assert_eq!(
            ms("select * from openrowset('SQLNCLI', 'Server=s;Uid=u;Pwd=abc;', 'select 1')"),
            "select * from openrowset('SQLNCLI', 'Server=s;Uid=u;Pwd=***;', 'select 1')"
        );
        // O que não é senha fica como está.
        let plain = "select password from usuarios where nome = 'ana'";
        assert_eq!(pg(plain), plain);
    }

    #[test]
    fn the_file_never_has_the_password() {
        let path = temp_file("password");
        let mut store = HistoryStore::open(&path);
        store.add(entry(
            "alter role ana password 'muito-secreta'",
            "ana@db1:5432",
            "postgres",
        ));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("muito-secreta"), "{text}");
        assert!(text.contains("alter role ana password"));
    }

    #[test]
    fn old_executions_leave_when_the_history_is_full() {
        let path = temp_file("full");
        let mut store = HistoryStore::open(&path);
        for i in 0..MAX_ENTRIES + 1 {
            store.entries.push(entry(&format!("select {i}"), "s", "d"));
        }
        store.add(entry("select 'a mais nova'", "s", "d"));
        assert!(store.len() <= MAX_ENTRIES);
        assert_eq!(store.entries().last().unwrap().sql, "select 'a mais nova'");
        // O arquivo foi regravado com o que ficou.
        assert_eq!(HistoryStore::open(&path).len(), store.len());
    }

    #[test]
    fn a_huge_script_keeps_only_the_beginning() {
        let sql = "select 1;\n".repeat(MAX_SQL_CHARS / 5);
        let e = entry(&sql, "s", "d");
        assert!(e.truncated);
        assert_eq!(e.sql.chars().count(), MAX_SQL_CHARS);
    }

    #[test]
    fn bad_lines_are_skipped_and_entries_can_be_removed() {
        let path = temp_file("bad");
        std::fs::write(
            &path,
            "isto não é json\n{\"sql\":\"select 1\",\"when\":1}\n\n",
        )
        .unwrap();
        let mut store = HistoryStore::open(&path);
        assert_eq!(store.len(), 1);
        let id = store.entries()[0].id();
        store.remove(id);
        assert!(store.is_empty());
        assert!(HistoryStore::open(&path).is_empty());
    }
}
