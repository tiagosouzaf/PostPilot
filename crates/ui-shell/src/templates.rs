//! Template Explorer (Ctrl+Alt+T): os modelos que vêm com o PostPilot, para
//! PostgreSQL e SQL Server, e os trechos do próprio usuário.
//!
//! Um trecho do usuário é um arquivo `.sql` numa pasta livre:
//!
//! - com um workspace aberto, em `snippets/` na raiz dele, para ir pelo Git
//!   para as outras máquinas (um arquivo por trecho, bom para diff e merge);
//! - sempre, em `<config>/PostPilot/snippets/`, só neste computador.
//!
//! O painel pesquisa pelo nome, pela pasta e pelo conteúdo, mostra o SQL do
//! item selecionado e o põe no editor com duplo clique. Editar um trecho é
//! abrir o arquivo dele numa janela de consulta e salvar.
//!
//! O que fica na sessão (veja [`crate::session`]): o texto da pesquisa e as
//! pastas abertas. O painel em si volta com o layout.

pub mod builtin;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::CommandId;
use pgstudio_core::connection::Engine;
use pgstudio_i18n::{tr, trf};
use serde::{Deserialize, Serialize};

use crate::ShellApp;
use crate::panels::Panel;
use crate::repo::Folder;
use crate::repo::search::{fold, terms};

pub use builtin::Builtin;

/// Pasta dos trechos dentro de um workspace.
pub const SNIPPETS: &str = "snippets";

/// De quanto em quanto tempo as pastas são lidas de novo com o painel à
/// mostra (um `git pull` ou um arquivo salvo aparece sozinho).
const REREAD_EVERY: Duration = Duration::from_secs(2);

/// O que o usuário pediu no Template Explorer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Pôr o SQL no editor ativo (ou numa janela nova, se não há editor).
    Insert(String),
    /// Abrir o SQL numa janela de consulta nova.
    OpenNew(String),
    /// Abrir o arquivo do trecho para editar.
    Edit(PathBuf),
    /// "New Snippet" nesta pasta: a casca traz a seleção do editor.
    NewFromEditor(PathBuf),
}

/// O que a sessão guarda do painel.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplatesSession {
    /// O que está escrito na pesquisa.
    pub search: String,
    /// Pastas abertas (veja [`TemplatesPanel::expanded`]).
    pub expanded: Vec<String>,
}

/// Uma raiz de trechos do usuário.
#[derive(Clone, Debug)]
struct Root {
    /// Texto da raiz na árvore, já traduzido.
    label: String,
    tree: Folder,
}

/// O item selecionado.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Selected {
    /// Índice em [`builtin::ALL`].
    Builtin(usize),
    File(PathBuf),
    /// Pasta, pela chave dela.
    Folder(String),
}

enum Dialog {
    /// Trecho novo: pasta, nome e SQL.
    Snippet {
        folder: PathBuf,
        name: String,
        sql: String,
        error: Option<String>,
    },
    Folder {
        parent: PathBuf,
        name: String,
        error: Option<String>,
    },
    Rename {
        path: PathBuf,
        name: String,
        error: Option<String>,
    },
    Delete {
        path: PathBuf,
    },
}

/// O painel Template Explorer.
pub struct TemplatesPanel {
    /// Pasta dos trechos deste computador.
    local: Option<PathBuf>,
    /// Nome e pasta de trechos do workspace aberto.
    workspace: Option<(String, PathBuf)>,
    roots: Vec<Root>,
    /// Texto de cada trecho do usuário, para a pesquisa e a prévia.
    texts: HashMap<PathBuf, String>,
    read_at: Option<Instant>,
    /// O que está escrito na pesquisa.
    pub search: String,
    /// Chaves das pastas abertas: `builtin:<servidor>`,
    /// `builtin:<servidor>:<pasta>` e, nas do usuário, o caminho. As
    /// pastas do usuário começam abertas e entram como `closed:<caminho>`
    /// quando fechadas.
    expanded: BTreeSet<String>,
    selected: Option<Selected>,
    dialog: Option<Dialog>,
    message: Option<String>,
    /// Servidor da conexão ativa: os modelos dele aparecem primeiro.
    pub engine: Option<Engine>,
    focus_search: bool,
}

impl Default for TemplatesPanel {
    fn default() -> Self {
        TemplatesPanel {
            local: None,
            workspace: None,
            roots: Vec::new(),
            texts: HashMap::new(),
            read_at: None,
            search: String::new(),
            expanded: Engine::ALL.into_iter().map(engine_key).collect(),
            selected: None,
            dialog: None,
            message: None,
            engine: None,
            focus_search: false,
        }
    }
}

fn engine_key(engine: Engine) -> String {
    match engine {
        Engine::PostgreSql => "builtin:postgresql".into(),
        Engine::SqlServer => "builtin:sqlserver".into(),
        Engine::Redis => "builtin:redis".into(),
        Engine::MongoDb => "builtin:mongodb".into(),
    }
}

fn folder_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Confere um nome de trecho ou de pasta.
fn valid_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    let bad = name.is_empty()
        || name.starts_with('.')
        || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']);
    if bad {
        Err(tr("Enter a valid name.").to_string())
    } else {
        Ok(name)
    }
}

/// Se todos os termos aparecem em `hay` (já sem acentos e em minúsculas).
fn has_all(hay: &str, terms: &[String]) -> bool {
    terms.iter().all(|t| hay.contains(t.as_str()))
}

impl Builtin {
    /// Se o modelo atende à pesquisa: pelo servidor, pela pasta, pelo nome
    /// (no idioma da interface e em inglês) ou pelo SQL.
    pub fn matches(&self, terms: &[String]) -> bool {
        let hay = fold(&format!(
            "{} {} {} {} {} {}",
            self.engine.label(),
            tr(self.folder),
            self.folder,
            tr(self.name),
            self.name,
            self.sql
        ));
        has_all(&hay, terms)
    }
}

impl TemplatesPanel {
    /// Guarda os trechos deste computador em `dir`.
    pub fn set_local_dir(&mut self, dir: impl Into<PathBuf>) {
        self.local = Some(dir.into());
        self.reload();
    }

    /// Passa a mostrar (ou deixa de mostrar) os trechos de um workspace:
    /// o nome dele e a raiz.
    pub fn set_workspace(&mut self, workspace: Option<(&str, &Path)>) {
        let new = workspace.map(|(name, root)| (name.to_string(), root.join(SNIPPETS)));
        if new != self.workspace {
            self.workspace = new;
            self.reload();
        }
    }

    /// Lê de novo as pastas de trechos.
    pub fn reload(&mut self) {
        self.read_at = Some(Instant::now());
        self.roots.clear();
        self.texts.clear();
        if let Some((_, dir)) = &self.workspace {
            self.roots.push(Root {
                label: tr("Workspace Snippets").to_string(),
                tree: Folder::read(dir, String::new()),
            });
        }
        if let Some(dir) = &self.local {
            let label = if self.workspace.is_some() {
                tr("My Snippets (this computer)")
            } else {
                tr("My Snippets")
            };
            self.roots.push(Root {
                label: label.to_string(),
                tree: Folder::read(dir, String::new()),
            });
        }
        let mut files = Vec::new();
        for root in &self.roots {
            collect(&root.tree, &mut files);
        }
        for file in files {
            if let Ok(bytes) = std::fs::read(&file) {
                self.texts.insert(file, crate::repo::search::decode(&bytes));
            }
        }
    }

    /// Onde um trecho novo vai por padrão: a pasta selecionada (ou a do
    /// trecho selecionado), senão o workspace, senão este computador.
    pub fn default_folder(&self) -> Option<PathBuf> {
        match &self.selected {
            Some(Selected::File(path)) => path.parent().map(Path::to_path_buf),
            Some(Selected::Folder(key)) if !key.starts_with("builtin:") => Some(PathBuf::from(key)),
            _ => self.roots.first().map(|r| r.tree.path.clone()),
        }
    }

    /// Os trechos do usuário, por caminho, de todas as raízes.
    pub fn snippets(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for root in &self.roots {
            collect(&root.tree, &mut files);
        }
        files
    }

    /// O SQL de um trecho do usuário.
    pub fn snippet_text(&self, path: &Path) -> Option<&str> {
        self.texts.get(path).map(String::as_str)
    }

    /// Os termos da pesquisa atual (vazio sem pesquisa).
    fn terms(&self) -> Vec<String> {
        terms(&self.search)
    }

    fn file_matches(&self, path: &Path, terms: &[String]) -> bool {
        let text = self.texts.get(path).map_or("", String::as_str);
        // O caminho inteiro entra: a pasta também conta.
        let hay = fold(&format!("{} {text}", path.with_extension("").display()));
        has_all(&hay, terms)
    }

    /// Nomes do que a pesquisa atual acha (todos, sem pesquisa): modelos
    /// embutidos no idioma da interface e trechos pelo nome do arquivo.
    pub fn found(&self) -> Vec<String> {
        let terms = self.terms();
        let mut out: Vec<String> = builtin::ALL
            .iter()
            .filter(|t| t.matches(&terms))
            .map(|t| tr(t.name).to_string())
            .collect();
        out.extend(
            self.snippets()
                .iter()
                .filter(|f| self.file_matches(f, &terms))
                .map(|f| file_stem(f)),
        );
        out
    }

    /// Põe o foco na pesquisa no próximo frame.
    pub fn focus(&mut self) {
        self.focus_search = true;
    }

    /// Abre "New Snippet" nesta pasta, com este SQL.
    pub fn new_snippet(&mut self, folder: &Path, sql: &str) {
        self.dialog = Some(Dialog::Snippet {
            folder: folder.to_path_buf(),
            name: String::new(),
            sql: sql.to_string(),
            error: None,
        });
    }

    /// Se há um diálogo do painel aberto.
    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some()
    }

    /// Grava um trecho em `folder` e devolve o arquivo.
    pub fn save_snippet(
        &mut self,
        folder: &Path,
        name: &str,
        sql: &str,
    ) -> Result<PathBuf, String> {
        let name = valid_name(name)?;
        if sql.trim().is_empty() {
            return Err(tr("The snippet is empty.").to_string());
        }
        let name = name.strip_suffix(".sql").unwrap_or(name);
        let path = folder.join(format!("{name}.sql"));
        if path.exists() {
            return Err(trf!("\"{name}\" already exists.", name = name));
        }
        let io = |e: std::io::Error| trf!("Could not save the snippet: {e}", e = e);
        std::fs::create_dir_all(folder).map_err(io)?;
        let mut text = sql.to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        std::fs::write(&path, text).map_err(io)?;
        self.open_folder(folder);
        self.selected = Some(Selected::File(path.clone()));
        self.reload();
        Ok(path)
    }

    /// Deixa uma pasta do usuário aberta (elas começam abertas; a chave
    /// marca as fechadas).
    fn open_folder(&mut self, folder: &Path) {
        self.expanded
            .remove(&format!("closed:{}", folder_key(folder)));
    }

    /// Cria uma pasta de trechos dentro de `parent`.
    pub fn create_folder(&mut self, parent: &Path, name: &str) -> Result<PathBuf, String> {
        let name = valid_name(name)?;
        let path = parent.join(name);
        if path.exists() {
            return Err(trf!("\"{name}\" already exists.", name = name));
        }
        std::fs::create_dir_all(&path)
            .map_err(|e| trf!("Could not create the folder: {e}", e = e))?;
        // O git não guarda pasta vazia.
        let _ = std::fs::write(path.join(".gitkeep"), "");
        self.open_folder(parent);
        self.selected = Some(Selected::Folder(folder_key(&path)));
        self.reload();
        Ok(path)
    }

    /// Troca o nome de um trecho ou de uma pasta.
    pub fn rename(&mut self, path: &Path, name: &str) -> Result<PathBuf, String> {
        let name = valid_name(name)?;
        let is_dir = path.is_dir();
        let file_name = if is_dir {
            name.to_string()
        } else {
            format!("{}.sql", name.strip_suffix(".sql").unwrap_or(name))
        };
        let to = path.with_file_name(file_name);
        if to == path {
            return Ok(to);
        }
        if to.exists() {
            return Err(trf!("\"{name}\" already exists.", name = name));
        }
        std::fs::rename(path, &to).map_err(|e| trf!("Could not rename: {e}", e = e))?;
        if is_dir {
            // As pastas fechadas lá dentro continuam fechadas.
            let old = format!("closed:{}", folder_key(path));
            let moved: Vec<String> = self
                .expanded
                .iter()
                .filter(|k| {
                    k.strip_prefix("closed:")
                        .is_some_and(|p| Path::new(p).starts_with(path))
                })
                .cloned()
                .collect();
            for key in moved {
                self.expanded.remove(&key);
                let rest = key[old.len()..].to_string();
                self.expanded
                    .insert(format!("closed:{}{rest}", folder_key(&to)));
            }
            self.selected = Some(Selected::Folder(folder_key(&to)));
        } else {
            self.selected = Some(Selected::File(to.clone()));
        }
        self.reload();
        Ok(to)
    }

    /// Apaga um trecho ou uma pasta (com tudo o que há nela).
    pub fn delete(&mut self, path: &Path) -> Result<(), String> {
        let result = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        result.map_err(|e| trf!("Could not delete: {e}", e = e))?;
        self.selected = None;
        self.reload();
        Ok(())
    }

    /// O estado que a sessão guarda.
    pub fn session(&self) -> TemplatesSession {
        TemplatesSession {
            search: self.search.clone(),
            expanded: self.expanded.iter().cloned().collect(),
        }
    }

    /// Volta ao estado de uma sessão.
    pub fn restore(&mut self, session: TemplatesSession) {
        self.search = session.search;
        self.expanded = session.expanded.into_iter().collect();
    }

    /// Nome e SQL do item selecionado, para a prévia.
    fn preview(&self) -> Option<(String, String)> {
        match self.selected.as_ref()? {
            Selected::Builtin(i) => {
                let t = builtin::ALL.get(*i)?;
                Some((tr(t.name).to_string(), t.sql.to_string()))
            }
            Selected::File(path) => Some((file_stem(path), self.texts.get(path)?.clone())),
            Selected::Folder(_) => None,
        }
    }

    /// Desenha o painel.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        if self.read_at.is_none_or(|at| at.elapsed() >= REREAD_EVERY) {
            self.reload();
        }
        let mut actions = Vec::new();
        let width = ui.clip_rect().intersect(ui.max_rect()).width();
        let target = self.default_folder();
        ui.horizontal(|ui| {
            let new = egui::Button::new(format!("{} {}", icon::PLUS, tr("New Snippet")));
            let response = ui.add_enabled(target.is_some(), new).on_hover_text(tr(
                "Save your own snippet. It starts with the text selected in the editor.",
            ));
            if response.clicked()
                && let Some(folder) = &target
            {
                actions.push(Action::NewFromEditor(folder.clone()));
            }
            let folder = egui::Button::new(icon::FOLDER_PLUS);
            if ui
                .add_enabled(target.is_some(), folder)
                .on_hover_text(tr("New Folder"))
                .clicked()
                && let Some(parent) = &target
            {
                self.dialog = Some(Dialog::Folder {
                    parent: parent.clone(),
                    name: String::new(),
                    error: None,
                });
            }
            if ui
                .add(egui::Button::new(icon::ARROWS_CLOCKWISE).frame(false))
                .on_hover_text(tr("Refresh"))
                .clicked()
            {
                self.reload();
            }
        });
        self.search_ui(ui, width);
        ui.separator();

        if let Some((name, sql)) = self.preview() {
            let height = ui.available_height();
            egui::Panel::bottom(egui::Id::new("templates_preview"))
                .resizable(true)
                .default_size(height * 0.4)
                .min_size(70.0)
                .show(ui, |ui| {
                    // O painel costuma ser estreito: o nome numa linha
                    // (cortado se não couber) e os botões na de baixo.
                    ui.add(egui::Label::new(egui::RichText::new(&name).strong()).truncate()); // i18n-ignore
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if ui
                                .button(tr("Insert"))
                                .on_hover_text(tr("Insert into the active query window"))
                                .clicked()
                            {
                                actions.push(Action::Insert(sql.clone()));
                            }
                            if ui
                                .button(tr("New Window"))
                                .on_hover_text(tr("Open in a new query window"))
                                .clicked()
                            {
                                actions.push(Action::OpenNew(sql.clone()));
                            }
                        });
                    });
                    egui::ScrollArea::both()
                        .id_salt("templates_preview_scroll")
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(sql.as_str()).monospace()) // i18n-ignore
                                    .extend(),
                            );
                        });
                });
        }

        let terms = self.terms();
        let searching = !terms.is_empty();
        egui::ScrollArea::vertical()
            .id_salt("templates_tree")
            .auto_shrink(false)
            .show(ui, |ui| {
                for i in 0..self.roots.len() {
                    let root = self.roots[i].clone();
                    self.user_rows(ui, &root.tree, Some(&root.label), 0, &terms, &mut actions);
                }
                if self.roots.is_empty() {
                    ui.weak(tr(
                        "Open a workspace to keep your own snippets and take them to your other computers through Git.",
                    ));
                } else if self.workspace.is_none() && !searching {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(tr(
                            "Open a workspace to keep snippets in Git and take them to your other computers.",
                        ))
                        .small()
                        .weak(),
                    );
                }
                ui.add_space(4.0);
                let mut engines = Engine::ALL.to_vec();
                if let Some(current) = self.engine {
                    engines.sort_by_key(|e| *e != current);
                }
                for engine in engines {
                    self.builtin_rows(ui, engine, &terms, &mut actions);
                }
            });
        actions
    }

    fn search_ui(&mut self, ui: &mut egui::Ui, width: f32) {
        let row = egui::vec2(width, ui.spacing().interact_size.y);
        let layout = egui::Layout::right_to_left(egui::Align::Center);
        ui.allocate_ui_with_layout(row, layout, |ui| {
            ui.set_max_width(width);
            let clear = !self.search.trim().is_empty()
                && ui
                    .add(egui::Button::new(icon::X).frame(false))
                    .on_hover_text(tr("Clear the search"))
                    .clicked();
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .id_salt("templates_search")
                    .hint_text(format!(
                        "{} {}",
                        icon::MAGNIFYING_GLASS,
                        tr("Search templates by name or content")
                    ))
                    .desired_width(f32::INFINITY),
            );
            if field.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.search.clear();
            }
            if clear {
                self.search.clear();
                field.request_focus();
            }
            if std::mem::take(&mut self.focus_search) {
                field.request_focus();
            }
        });
        if self.search.trim().is_empty() {
            return;
        }
        let text = match self.found().len() {
            0 => tr("No templates found.").to_string(),
            1 => tr("1 template found.").to_string(),
            n => trf!("{n} templates found.", n = n),
        };
        ui.label(egui::RichText::new(text).small().weak());
    }

    /// Linha de pasta: devolve a resposta do rótulo. Alterna abrir e fechar
    /// (fora da pesquisa, que mostra tudo aberto).
    fn folder_row(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        label: &str,
        depth: usize,
        searching: bool,
    ) -> (bool, egui::Response) {
        // As pastas do usuário começam abertas: a sessão guarda as fechadas.
        let closed_key = (!key.starts_with("builtin:")).then(|| format!("closed:{key}"));
        let state_open = match &closed_key {
            Some(closed) => !self.expanded.contains(closed),
            None => self.expanded.contains(key),
        };
        let expanded = searching || state_open;
        let mut toggle = false;
        let response = ui
            .horizontal(|ui| {
                ui.add_space(depth as f32 * 16.0 + 4.0);
                let caret = if expanded {
                    icon::CARET_DOWN
                } else {
                    icon::CARET_RIGHT
                };
                if ui.add(egui::Button::new(caret).frame(false)).clicked() {
                    toggle = true;
                }
                let glyph = if expanded {
                    icon::FOLDER_OPEN
                } else {
                    icon::FOLDER
                };
                let selected = self.selected == Some(Selected::Folder(key.to_string()));
                let label = ui.add(
                    egui::Button::selectable(selected, format!("{glyph} {label}"))
                        .frame_when_inactive(false),
                );
                if label.clicked() {
                    self.selected = Some(Selected::Folder(key.to_string()));
                }
                if label.double_clicked() {
                    toggle = true;
                }
                label
            })
            .inner;
        if toggle && !searching {
            // Para uma pasta do usuário, a chave marca "fechada".
            let (state_key, insert) = match closed_key {
                Some(closed) => (closed, expanded),
                None => (key.to_string(), !expanded),
            };
            if insert {
                self.expanded.insert(state_key);
            } else {
                self.expanded.remove(&state_key);
            }
        }
        (expanded, response)
    }

    /// Linha de um modelo ou trecho.
    fn item_row(
        &mut self,
        ui: &mut egui::Ui,
        item: Selected,
        label: &str,
        sql: &str,
        depth: usize,
        actions: &mut Vec<Action>,
    ) -> egui::Response {
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 16.0 + 4.0 + 14.0);
            let row = ui.add(
                egui::Button::selectable(
                    self.selected.as_ref() == Some(&item),
                    format!("{} {label}", icon::FILE_SQL),
                )
                .frame_when_inactive(false),
            );
            if row.clicked() {
                self.selected = Some(item.clone());
            }
            if row.double_clicked() {
                actions.push(Action::Insert(sql.to_string()));
            }
            row
        })
        .inner
    }

    /// Os itens de menu comuns a modelos e trechos.
    fn item_menu(ui: &mut egui::Ui, sql: &str, actions: &mut Vec<Action>) {
        if ui.button(tr("Insert")).clicked() {
            actions.push(Action::Insert(sql.to_string()));
            ui.close();
        }
        if ui.button(tr("Open in New Query Window")).clicked() {
            actions.push(Action::OpenNew(sql.to_string()));
            ui.close();
        }
        if ui.button(tr("Copy")).clicked() {
            ui.ctx().copy_text(sql.to_string());
            ui.close();
        }
    }

    fn builtin_rows(
        &mut self,
        ui: &mut egui::Ui,
        engine: Engine,
        terms: &[String],
        actions: &mut Vec<Action>,
    ) {
        let searching = !terms.is_empty();
        let any = |folder: Option<&str>| {
            builtin::ALL.iter().any(|t| {
                t.engine == engine && folder.is_none_or(|f| f == t.folder) && t.matches(terms)
            })
        };
        if searching && !any(None) {
            return;
        }
        let key = engine_key(engine);
        let (open, _) = self.folder_row(ui, &key, engine.label(), 0, searching);
        if !open {
            return;
        }
        for folder in builtin::folders(engine) {
            if searching && !any(Some(folder)) {
                continue;
            }
            let key = format!("{key}:{folder}");
            let (open, _) = self.folder_row(ui, &key, tr(folder), 1, searching);
            if !open {
                continue;
            }
            for (i, t) in builtin::ALL.iter().enumerate() {
                if t.engine != engine || t.folder != folder || !t.matches(terms) {
                    continue;
                }
                let row = self.item_row(ui, Selected::Builtin(i), tr(t.name), t.sql, 2, actions);
                let target = self.roots.first().map(|r| r.tree.path.clone());
                row.context_menu(|ui| {
                    self.selected = Some(Selected::Builtin(i));
                    Self::item_menu(ui, t.sql, actions);
                    ui.separator();
                    let copy = egui::Button::new(tr("Save a Copy in My Snippets..."));
                    if ui.add_enabled(target.is_some(), copy).clicked() {
                        if let Some(folder) = &target {
                            self.new_snippet(folder, t.sql);
                            if let Some(Dialog::Snippet { name, .. }) = &mut self.dialog {
                                *name = tr(t.name).to_string();
                            }
                        }
                        ui.close();
                    }
                });
            }
        }
    }

    /// Se a pasta (ou alguma subpasta) tem trecho que atende à pesquisa.
    fn folder_matches(&self, folder: &Folder, terms: &[String]) -> bool {
        folder.files.iter().any(|f| self.file_matches(f, terms))
            || folder.folders.iter().any(|f| self.folder_matches(f, terms))
    }

    fn user_rows(
        &mut self,
        ui: &mut egui::Ui,
        folder: &Folder,
        root_label: Option<&str>,
        depth: usize,
        terms: &[String],
        actions: &mut Vec<Action>,
    ) {
        let searching = !terms.is_empty();
        if searching && !self.folder_matches(folder, terms) {
            return;
        }
        let key = folder_key(&folder.path);
        let label = root_label.unwrap_or(&folder.name);
        let (open, row) = self.folder_row(ui, &key, label, depth, searching);
        row.context_menu(|ui| {
            self.selected = Some(Selected::Folder(key.clone()));
            if ui.button(tr("New Snippet...")).clicked() {
                actions.push(Action::NewFromEditor(folder.path.clone()));
                ui.close();
            }
            if ui.button(tr("New Folder...")).clicked() {
                self.dialog = Some(Dialog::Folder {
                    parent: folder.path.clone(),
                    name: String::new(),
                    error: None,
                });
                ui.close();
            }
            if depth > 0 {
                ui.separator();
                if ui.button(tr("Rename...")).clicked() {
                    self.dialog = Some(Dialog::Rename {
                        path: folder.path.clone(),
                        name: folder.name.clone(),
                        error: None,
                    });
                    ui.close();
                }
                if ui.button(tr("Delete...")).clicked() {
                    self.dialog = Some(Dialog::Delete {
                        path: folder.path.clone(),
                    });
                    ui.close();
                }
            }
        });
        if !open {
            return;
        }
        for sub in &folder.folders {
            self.user_rows(ui, sub, None, depth + 1, terms, actions);
        }
        for file in &folder.files {
            if searching && !self.file_matches(file, terms) {
                continue;
            }
            let sql = self.texts.get(file).cloned().unwrap_or_default();
            let name = file_stem(file);
            let row = self.item_row(
                ui,
                Selected::File(file.clone()),
                &name,
                &sql,
                depth + 1,
                actions,
            );
            row.context_menu(|ui| {
                self.selected = Some(Selected::File(file.clone()));
                Self::item_menu(ui, &sql, actions);
                ui.separator();
                if ui.button(tr("Edit")).clicked() {
                    actions.push(Action::Edit(file.clone()));
                    ui.close();
                }
                if ui.button(tr("Rename...")).clicked() {
                    self.dialog = Some(Dialog::Rename {
                        path: file.clone(),
                        name: name.clone(),
                        error: None,
                    });
                    ui.close();
                }
                if ui.button(tr("Delete...")).clicked() {
                    self.dialog = Some(Dialog::Delete { path: file.clone() });
                    ui.close();
                }
            });
        }
        if depth == 0 && folder.count() == 0 && folder.folders.is_empty() && !searching {
            ui.horizontal(|ui| {
                ui.add_space(38.0);
                ui.weak(tr("No snippets yet. Use New Snippet to save one."));
            });
        }
    }

    /// Os diálogos do painel. Devolve um texto para a status bar.
    pub fn show_windows(&mut self, ctx: &egui::Context) -> Option<String> {
        let mut status = self.message.take();
        let Some(mut dialog) = self.dialog.take() else {
            return status;
        };
        // `Some(true)`: confirmar; `Some(false)`: cancelar.
        let mut answer = None;
        let buttons = |ui: &mut egui::Ui, ok: &str, answer: &mut Option<bool>| {
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Cancel")).clicked() {
                    *answer = Some(false);
                }
                if ui.button(ok).clicked() {
                    *answer = Some(true);
                }
            });
        };
        let name_field = |ui: &mut egui::Ui, name: &mut String, answer: &mut Option<bool>| {
            let edit = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                *answer = Some(true);
            }
            // O foco começa no nome.
            if ui.memory(|m| m.focused().is_none()) {
                edit.request_focus();
            }
        };
        let show_error = |ui: &mut egui::Ui, error: &Option<String>| {
            if let Some(e) = error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        };
        match &mut dialog {
            Dialog::Snippet {
                folder,
                name,
                sql,
                error,
            } => {
                crate::theme::modal(egui::Id::new("templates_new_snippet")).show(ctx, |ui| {
                    ui.set_width(520.0);
                    ui.heading(tr("New Snippet"));
                    ui.label(tr("Name:"));
                    name_field(ui, name, &mut answer);
                    ui.label(tr("SQL:"));
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(sql)
                                    .code_editor()
                                    .desired_rows(10)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    ui.weak(trf!("Saved in {folder}.", folder = folder.display()));
                    show_error(ui, error);
                    buttons(ui, tr("Save"), &mut answer);
                });
            }
            Dialog::Folder { name, error, .. } => {
                crate::theme::modal(egui::Id::new("templates_new_folder")).show(ctx, |ui| {
                    ui.set_width(320.0);
                    ui.heading(tr("New Folder"));
                    ui.label(tr("Folder name:"));
                    name_field(ui, name, &mut answer);
                    show_error(ui, error);
                    buttons(ui, tr("OK"), &mut answer);
                });
            }
            Dialog::Rename { name, error, .. } => {
                crate::theme::modal(egui::Id::new("templates_rename")).show(ctx, |ui| {
                    ui.set_width(320.0);
                    ui.heading(tr("Rename"));
                    ui.label(tr("Name:"));
                    name_field(ui, name, &mut answer);
                    show_error(ui, error);
                    buttons(ui, tr("OK"), &mut answer);
                });
            }
            Dialog::Delete { path } => {
                crate::theme::modal(egui::Id::new("templates_delete")).show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.heading(tr("Delete"));
                    let name = if path.is_dir() {
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    } else {
                        file_stem(path)
                    };
                    ui.label(if path.is_dir() {
                        trf!(
                            "Delete the folder \"{name}\" and every snippet in it?",
                            name = name
                        )
                    } else {
                        trf!("Delete the snippet \"{name}\"?", name = name)
                    });
                    buttons(ui, tr("Delete"), &mut answer);
                });
            }
        }
        match answer {
            None => self.dialog = Some(dialog),
            Some(false) => {}
            Some(true) => {
                let result = match &dialog {
                    Dialog::Snippet {
                        folder, name, sql, ..
                    } => self
                        .save_snippet(folder, name, sql)
                        .map(|p| trf!("Snippet saved in {path}.", path = p.display())),
                    Dialog::Folder { parent, name, .. } => self
                        .create_folder(parent, name)
                        .map(|p| trf!("Created {path}.", path = p.display())),
                    Dialog::Rename { path, name, .. } => self
                        .rename(path, name)
                        .map(|p| trf!("Renamed to {path}.", path = p.display())),
                    Dialog::Delete { path } => self
                        .delete(path)
                        .map(|()| trf!("Deleted {path}.", path = path.display())),
                };
                match result {
                    Ok(s) => status = Some(s),
                    Err(e) => match &mut dialog {
                        Dialog::Snippet { error, .. }
                        | Dialog::Folder { error, .. }
                        | Dialog::Rename { error, .. } => {
                            *error = Some(e);
                            self.dialog = Some(dialog);
                        }
                        Dialog::Delete { .. } => status = Some(e),
                    },
                }
            }
        }
        status
    }
}

fn collect(folder: &Folder, out: &mut Vec<PathBuf>) {
    for sub in &folder.folders {
        collect(sub, out);
    }
    out.extend(folder.files.iter().cloned());
}

impl ShellApp {
    /// Guarda os trechos do usuário deste computador em `dir`
    /// (`<config>/PostPilot/snippets`).
    pub fn with_snippets_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.templates.set_local_dir(dir);
        self
    }

    /// O Template Explorer.
    pub fn templates(&self) -> &TemplatesPanel {
        &self.templates
    }

    /// O Template Explorer, mutável.
    pub fn templates_mut(&mut self) -> &mut TemplatesPanel {
        &mut self.templates
    }

    /// Ctrl+Alt+T: mostra o painel com o foco na pesquisa.
    pub(crate) fn open_template_explorer(&mut self) {
        self.sync_templates();
        self.workspace.open(Panel::TemplateExplorer);
        self.templates.focus();
    }

    /// O texto selecionado no editor ativo ou, sem seleção, a janela toda.
    fn editor_text_for_snippet(&self, ctx: &egui::Context) -> String {
        let Some(id) = self.active_doc else {
            return String::new();
        };
        let Some(doc) = self.documents.get(id) else {
            return String::new();
        };
        pgstudio_sql_editor::selected_text(ctx, crate::panels::editor_id(id), &doc.text)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| doc.text.clone())
    }

    /// "Save as Snippet": abre "New Snippet" com o que está selecionado.
    pub(crate) fn save_selection_as_snippet(&mut self, ctx: &egui::Context) {
        self.sync_templates();
        let Some(folder) = self.templates.default_folder() else {
            self.status = tr("Open a workspace to save snippets.").to_string();
            return;
        };
        let sql = self.editor_text_for_snippet(ctx);
        self.workspace.open(Panel::TemplateExplorer);
        self.templates.new_snippet(&folder, &sql);
    }

    /// Workspace e servidor atuais, para o painel.
    fn sync_templates(&mut self) {
        let workspace = self
            .workspace_panel
            .workspace()
            .map(|w| (w.name().to_string(), w.root().to_path_buf()));
        self.templates
            .set_workspace(workspace.as_ref().map(|(n, r)| (n.as_str(), r.as_path())));
        self.templates.engine = self.connection.as_ref().map(|c| c.settings.engine);
    }

    /// Põe o SQL de um modelo no editor ativo; sem editor, numa janela
    /// nova.
    pub fn insert_template(&mut self, ctx: &egui::Context, sql: &str) {
        match self
            .active_doc
            .filter(|id| self.documents.get(*id).is_some())
        {
            Some(doc) => self.insert_sql(ctx, doc, sql.trim_end_matches('\n')),
            None => self.open_template(sql),
        }
    }

    /// Abre o SQL de um modelo numa janela de consulta nova.
    pub fn open_template(&mut self, sql: &str) {
        let id = self
            .documents
            .new_script(sql.to_string(), self.default_database());
        self.activate_document(id);
    }

    /// Atende ao que o painel pediu e mostra os diálogos dele.
    pub(crate) fn templates_step(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        self.sync_templates();
        for action in actions {
            match action {
                Action::Insert(sql) => self.insert_template(ctx, &sql),
                Action::OpenNew(sql) => self.open_template(&sql),
                Action::Edit(path) => {
                    self.open_file(&path);
                }
                Action::NewFromEditor(folder) => {
                    let sql = self.editor_text_for_snippet(ctx);
                    self.templates.new_snippet(&folder, &sql);
                }
            }
        }
        if let Some(status) = self.templates.show_windows(ctx) {
            self.status = status;
        }
    }

    /// Só para os menus: o comando do painel.
    pub(crate) fn template_command(&mut self, ctx: &egui::Context, cmd: CommandId) {
        match cmd {
            CommandId::SaveAsSnippet => self.save_selection_as_snippet(ctx),
            _ => self.open_template_explorer(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("postpilot-templates-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn search_finds_builtins_by_name_folder_engine_and_sql() {
        let mut panel = TemplatesPanel::default();
        assert_eq!(panel.found().len(), builtin::ALL.len());
        panel.search = "pg_blocking_pids".into();
        assert_eq!(panel.found(), vec!["Blocked Sessions"]);
        panel.search = "sql server upsert".into();
        assert_eq!(panel.found(), vec!["Insert or Update (Upsert)"]);
        panel.search = "MONITORING postgresql sizes".into();
        assert_eq!(panel.found(), vec!["Table Sizes", "Database Sizes"]);
        panel.search = "nothing like this".into();
        assert!(panel.found().is_empty());
    }

    #[test]
    fn snippets_are_files_in_folders_found_by_name_folder_and_content() {
        let root = dir("files");
        let mut panel = TemplatesPanel::default();
        panel.set_local_dir(&root);
        let folder = panel.create_folder(&root, "Clientes").unwrap();
        let file = panel
            .save_snippet(
                &folder,
                "Versão do sistema",
                "select versao from parametros",
            )
            .unwrap();
        assert_eq!(file, folder.join("Versão do sistema.sql"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "select versao from parametros\n"
        );
        assert_eq!(panel.snippets(), vec![file.clone()]);
        // Mesmo nome, nome inválido e trecho vazio são recusados.
        assert!(
            panel
                .save_snippet(&folder, "Versão do sistema", "x")
                .is_err()
        );
        assert!(panel.save_snippet(&folder, "a/b", "x").is_err());
        assert!(panel.save_snippet(&folder, "vazio", "  ").is_err());

        for (query, found) in [("versao", true), ("clientes", true), ("parametros", true)] {
            panel.search = query.into();
            assert_eq!(
                panel.found().contains(&"Versão do sistema".to_string()),
                found
            );
        }
        panel.search = "pedidos".into();
        assert!(panel.found().is_empty());

        let renamed = panel.rename(&file, "Versão").unwrap();
        assert_eq!(renamed, folder.join("Versão.sql"));
        assert!(!file.exists());
        panel
            .expanded
            .insert(format!("closed:{}", folder_key(&folder)));
        let moved = panel.rename(&folder, "Todos").unwrap();
        // A pasta fechada continua fechada com o nome novo.
        assert!(
            panel
                .session()
                .expanded
                .contains(&format!("closed:{}", folder_key(&moved)))
        );
        assert_eq!(panel.snippets(), vec![moved.join("Versão.sql")]);
        panel.delete(&moved).unwrap();
        assert!(panel.snippets().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn workspace_snippets_come_first_and_are_the_default_destination() {
        let local = dir("local");
        let ws = dir("ws");
        let mut panel = TemplatesPanel::default();
        panel.set_local_dir(&local);
        assert_eq!(panel.default_folder(), Some(local.clone()));
        panel.set_workspace(Some(("sql", &ws)));
        assert_eq!(panel.default_folder(), Some(ws.join(SNIPPETS)));
        let file = panel
            .save_snippet(&ws.join(SNIPPETS), "a", "select 1;\n")
            .unwrap();
        assert_eq!(file, ws.join("snippets").join("a.sql"));
        panel.set_workspace(None);
        assert!(panel.snippets().is_empty());
        let _ = std::fs::remove_dir_all(&local);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn session_round_trips() {
        let mut panel = TemplatesPanel {
            search: "índice".into(),
            ..Default::default()
        };
        panel.expanded.insert("builtin:postgresql:Tables".into());
        let saved = panel.session();
        let mut other = TemplatesPanel::default();
        other.restore(saved.clone());
        assert_eq!(other.session(), saved);
    }
}
