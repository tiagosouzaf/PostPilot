//! Workspace Explorer: o painel do workspace, como o Solution Explorer do
//! SSMS. Em cima, os servidores do workspace (a mesma árvore do Registered
//! Servers, gravada em `servers.json`); embaixo, as consultas de `queries/`.

use pgstudio_i18n::{tr, tr_noop, trf};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use egui_phosphor::regular as icon;

use super::git::{Done, Git, GitError, Job};
use super::passwords::{Passwords, SharedVault, WorkspaceSecrets};
use super::source_control::SourceControl;
use super::{Folder, Workspace};
use crate::registered::{self, RegisteredPanel};
use crate::secrets::SecretStore;

/// O que o usuário pediu no Workspace Explorer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// File > New > Workspace...
    New,
    /// File > Open > Workspace...
    Open,
    /// Abrir uma consulta (duplo clique ou "Open").
    OpenQuery(PathBuf),
    /// "New Query" numa pasta: criar o arquivo nela e abrir.
    NewQuery(PathBuf),
    /// Pedido da árvore de servidores do workspace.
    Servers(registered::Action),
    /// Trazer os Registered Servers desta máquina (e as senhas lembradas)
    /// para o workspace.
    ImportRegistered,
}

struct Opened {
    workspace: Workspace,
    servers: RegisteredPanel,
    queries: Folder,
    /// Pastas recolhidas (as demais aparecem abertas).
    collapsed: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    git: SourceControl,
    passwords: Passwords,
}

/// "Clone Workspace" esperando a URL.
struct CloneDialog {
    url: String,
    error: Option<String>,
}

/// "New Folder" esperando o nome.
struct NewFolder {
    parent: PathBuf,
    name: String,
    error: Option<String>,
}

/// O painel Workspace Explorer.
#[derive(Default)]
pub struct WorkspacePanel {
    opened: Option<Opened>,
    new_folder: Option<NewFolder>,
    message: Option<String>,
    clone: Option<CloneDialog>,
    clone_job: Option<Job>,
    /// URL confirmada no "Clone Workspace", esperando a pasta.
    pending_clone: Option<String>,
    /// Cofre de senhas do workspace aberto, dividido com o "Remember
    /// password".
    vault: SharedVault,
    /// Cópia das senhas do cofre do sistema para o do workspace, rodando.
    copying: Option<std::thread::JoinHandle<Result<usize, String>>>,
}

impl WorkspacePanel {
    /// Passa a mostrar este workspace.
    pub fn open(&mut self, workspace: Workspace) {
        let servers = RegisteredPanel::new(workspace.servers());
        let queries = workspace.queries();
        let git = SourceControl::new(workspace.root());
        let passwords = Passwords::new(workspace.root(), self.vault.clone());
        self.opened = Some(Opened {
            workspace,
            servers,
            queries,
            collapsed: HashSet::new(),
            selected: None,
            git,
            passwords,
        });
        self.new_folder = None;
    }

    /// Fecha o workspace.
    pub fn close(&mut self) {
        self.opened = None;
        let mut slot = self.vault.lock().expect("vault lock");
        slot.vault = None;
        slot.keys.clear();
        drop(slot);
        self.new_folder = None;
    }

    /// O workspace aberto.
    pub fn workspace(&self) -> Option<&Workspace> {
        self.opened.as_ref().map(|o| &o.workspace)
    }

    /// A árvore de servidores do workspace.
    pub fn servers(&self) -> Option<&RegisteredPanel> {
        self.opened.as_ref().map(|o| &o.servers)
    }

    /// A árvore de servidores do workspace, mutável.
    pub fn servers_mut(&mut self) -> Option<&mut RegisteredPanel> {
        self.opened.as_mut().map(|o| &mut o.servers)
    }

    /// As consultas, como lidas da última vez.
    pub fn queries(&self) -> Option<&Folder> {
        self.opened.as_ref().map(|o| &o.queries)
    }

    /// Lê de novo as consultas e os servidores do disco (depois de salvar,
    /// de um pull do git ou de mudanças fora do PostPilot).
    pub fn refresh(&mut self) {
        if let Some(o) = &mut self.opened {
            o.git.refresh();
            o.queries = o.workspace.queries();
            if !o.servers.has_dialog() {
                let current = o.servers.current.take();
                o.servers = RegisteredPanel::new(o.workspace.servers());
                o.servers.current = current;
            }
        }
    }

    /// O cofre do workspace, dividido com o "Remember password".
    pub fn vault_slot(&self) -> SharedVault {
        self.vault.clone()
    }

    /// As senhas do workspace.
    pub fn passwords(&self) -> Option<&Passwords> {
        self.opened.as_ref().map(|o| &o.passwords)
    }

    /// As senhas do workspace, mutável.
    pub fn passwords_mut(&mut self) -> Option<&mut Passwords> {
        self.opened.as_mut().map(|o| &mut o.passwords)
    }

    /// O git do workspace.
    pub fn source_control(&self) -> Option<&SourceControl> {
        self.opened.as_ref().map(|o| &o.git)
    }

    /// O git do workspace, mutável.
    pub fn source_control_mut(&mut self) -> Option<&mut SourceControl> {
        self.opened.as_mut().map(|o| &mut o.git)
    }

    /// Abre "Clone Workspace".
    pub fn open_clone(&mut self) {
        self.clone = Some(CloneDialog {
            url: String::new(),
            error: None,
        });
    }

    /// Clona `url` em `dest` numa thread separada; o fim chega por
    /// [`Self::take_cloned`].
    pub fn start_clone(&mut self, url: &str, dest: &Path) {
        if self.clone_job.is_some() {
            self.message = Some(tr("Wait for the current clone.").into());
            return;
        }
        let (url, dest) = (url.to_string(), dest.to_path_buf());
        self.clone_job = Some(Job::spawn(tr_noop("Cloning..."), move || {
            Done::Cloned(Git::clone(&url, &dest).map(|g| g.root().to_path_buf()))
        }));
    }

    /// URL confirmada no "Clone Workspace" (a casca pede a pasta).
    pub fn take_clone_request(&mut self) -> Option<String> {
        self.pending_clone.take()
    }

    /// Se um clone está rodando.
    pub fn is_cloning(&self) -> bool {
        self.clone_job.is_some()
    }

    /// O clone que terminou, se algum.
    pub fn take_cloned(&mut self) -> Option<Result<PathBuf, GitError>> {
        let done = self.clone_job.as_ref()?.poll()?;
        self.clone_job = None;
        match done {
            Done::Cloned(r) => Some(r),
            _ => None,
        }
    }

    /// Espera o clone (testes).
    pub fn wait_clone(&mut self) -> Option<Result<PathBuf, GitError>> {
        let job = self.clone_job.take()?;
        match job.wait() {
            Done::Cloned(r) => Some(r),
            Done::Status(Err(e)) => Some(Err(e)),
            _ => None,
        }
    }

    /// Abre "New Folder" dentro de `parent`.
    pub fn new_folder(&mut self, parent: &Path) {
        self.new_folder = Some(NewFolder {
            parent: parent.to_path_buf(),
            name: tr("New Folder").into(),
            error: None,
        });
    }

    /// Se "New Folder" está aberto.
    pub fn is_naming_folder(&self) -> bool {
        self.new_folder.is_some()
    }

    /// Confirma "New Folder" com este nome (testes e o botão OK).
    pub fn create_folder(&mut self, name: &str) -> Result<PathBuf, String> {
        let Some(dialog) = &mut self.new_folder else {
            return Err(tr("New Folder is not open.").into());
        };
        let name = name.trim();
        let bad = name.is_empty()
            || name == "."
            || name == ".."
            || name.starts_with('.')
            || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']);
        if bad {
            let e = tr("Enter a valid folder name.").to_string();
            dialog.error = Some(e.clone());
            return Err(e);
        }
        let path = dialog.parent.join(name);
        if path.exists() {
            let e = trf!("\"{name}\" already exists.", name = name);
            dialog.error = Some(e.clone());
            return Err(e);
        }
        if let Err(e) = std::fs::create_dir_all(&path) {
            let e = trf!("Could not create the folder: {e}", e = e);
            dialog.error = Some(e.clone());
            return Err(e);
        }
        self.new_folder = None;
        self.refresh();
        Ok(path)
    }

    /// Copia, numa thread, as senhas lembradas destas chaves (`usuario@host:porta`)
    /// de `from` para o cofre do workspace, que precisa estar destrancado.
    /// Senhas que já estão no cofre não mudam.
    pub fn copy_passwords(&mut self, keys: Vec<String>, from: Arc<dyn SecretStore>) {
        let slot = self.vault.clone();
        self.copying = Some(std::thread::spawn(move || {
            let mut copied = 0;
            for key in keys {
                {
                    let mut slot = slot.lock().expect("vault lock");
                    let Some(v) = slot.vault.as_mut().filter(|v| v.is_unlocked()) else {
                        return Err(tr("The workspace passwords are locked.").into());
                    };
                    if v.get(&key).ok().flatten().is_some() {
                        continue;
                    }
                }
                // O cofre do sistema pode demorar: fora da trava.
                let Ok(Some(secret)) = from.get(&key) else {
                    continue;
                };
                let mut slot = slot.lock().expect("vault lock");
                let Some(v) = slot.vault.as_mut().filter(|v| v.is_unlocked()) else {
                    return Err(tr("The workspace passwords are locked.").into());
                };
                v.set(&key, &secret)
                    .map_err(|e| super::passwords::vault_error_text(&e))?;
                copied += 1;
            }
            Ok(copied)
        }));
    }

    /// Se a cópia das senhas está rodando.
    pub fn is_copying(&self) -> bool {
        self.copying.is_some()
    }

    /// Espera a cópia das senhas (testes) e devolve a mensagem dela.
    pub fn wait_copy(&mut self) -> Option<String> {
        self.finish_copy()
    }

    fn finish_copy(&mut self) -> Option<String> {
        let handle = self.copying.take()?;
        let result = handle
            .join()
            .unwrap_or_else(|_| Err(tr("Copying the passwords failed.").into()));
        Some(match result {
            Ok(1) => tr("Copied 1 saved password to the workspace passwords.").into(),
            Ok(n) => trf!(
                "Copied {n} saved passwords to the workspace passwords.",
                n = n
            ),
            Err(e) => e,
        })
    }

    /// O cofre que a árvore de servidores do workspace usa: as senhas dos
    /// servidores dela vão para o cofre do workspace (quando destrancado),
    /// até as de um servidor que acabou de ser registrado.
    fn workspace_store(
        &self,
        store: Option<&Arc<dyn SecretStore>>,
    ) -> Option<Arc<dyn SecretStore>> {
        store.map(|s| {
            Arc::new(WorkspaceSecrets::workspace_only(
                Some(s.clone()),
                self.vault.clone(),
            )) as Arc<dyn SecretStore>
        })
    }

    /// Abre o "New Server Registration" na raiz da árvore do workspace,
    /// como o menu de contexto dela.
    pub fn new_server(&mut self, store: Option<&Arc<dyn SecretStore>>) {
        let store = self.workspace_store(store);
        if let Some(o) = &mut self.opened {
            o.servers.new_server(&[], store.as_ref());
        }
    }

    /// Desenha o painel e devolve os pedidos.
    pub fn show(&mut self, ui: &mut egui::Ui, store: Option<&Arc<dyn SecretStore>>) -> Vec<Action> {
        let mut actions = Vec::new();
        let workspace_store = self.workspace_store(store);
        let store = workspace_store.as_ref();
        let Some(o) = &mut self.opened else {
            ui.add_space(6.0);
            ui.weak(tr("No workspace is open. A workspace is a folder, usually a git repository, with your servers and queries."));
            ui.add_space(6.0);
            if ui
                .button(format!("{} {}", icon::FOLDER_PLUS, tr("New Workspace...")))
                .clicked()
            {
                actions.push(Action::New);
            }
            if ui
                .button(format!("{} {}", icon::FOLDER_OPEN, tr("Open Workspace...")))
                .clicked()
            {
                actions.push(Action::Open);
            }
            if ui
                .button(format!("{} {}", icon::GIT_BRANCH, tr("Clone Workspace...")))
                .clicked()
            {
                self.open_clone();
            }
            if let Some(job) = &self.clone_job {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak(tr(job.label));
                });
            }
            return actions;
        };
        let mut refresh = false;
        ui.horizontal(|ui| {
            ui.strong(format!("{} {}", icon::BRIEFCASE, o.workspace.name()))
                .on_hover_text(o.workspace.root().display().to_string());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(icon::ARROWS_CLOCKWISE).frame(false))
                    .on_hover_text(tr("Refresh"))
                    .clicked()
                {
                    refresh = true;
                }
            });
        });
        o.git.show(ui);
        o.passwords.show(ui);
        ui.separator();

        // Servidores em cima, num retângulo próprio: a árvore do Registered
        // Servers tem rolagem que ocupa todo o espaço que recebe.
        let width = ui.available_width();
        let height = (ui.available_height() * 0.4).max(80.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("Servers")).small().weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(format!(
                                "{} {}",
                                icon::DOWNLOAD_SIMPLE,
                                tr("Import from this computer")
                            ))
                            .small(),
                        )
                        .frame(false),
                    )
                    .on_hover_text(tr(
                        "Import Registered Servers from this computer, with their saved passwords",
                    ))
                    .clicked()
                {
                    actions.push(Action::ImportRegistered);
                }
            });
        });
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("workspace_servers")
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        actions.extend(
            o.servers
                .show(&mut child, store)
                .into_iter()
                .map(Action::Servers),
        );
        ui.separator();

        ui.label(egui::RichText::new(tr("Queries")).small().weak());
        let mut folder_menu: Option<PathBuf> = None;
        egui::ScrollArea::both()
            .id_salt("workspace_queries")
            .auto_shrink(false)
            .show(ui, |ui| {
                let root = o.queries.clone();
                folder_rows(ui, o, &root, 0, &mut actions, &mut folder_menu);
                if root.count() == 0 && root.folders.is_empty() {
                    ui.add_space(4.0);
                    ui.weak(tr("No queries yet. Right-click \"Queries\" and choose \"New Query\", or save a query inside the queries folder."));
                }
            });
        if refresh {
            self.refresh();
        }
        if let Some(parent) = folder_menu {
            self.new_folder(&parent);
        }
        actions
    }

    /// Desenha os diálogos (os do Registered Servers e o "New Folder").
    /// Devolve uma mensagem para a status bar quando algo mudou.
    pub fn show_windows(
        &mut self,
        ctx: &egui::Context,
        store: Option<&Arc<dyn SecretStore>>,
        ai: Option<&pgstudio_automation::llm::Backend>,
    ) -> Option<String> {
        let mut status = self.message.take();
        if let Some(o) = &mut self.opened
            && let Some(s) = o.servers.show_windows(ctx)
        {
            status = Some(s);
        }
        if let Some(o) = &mut self.opened {
            if let Some(s) = o.git.show_windows(ctx, ai) {
                status = Some(s);
            }
            if let Some(s) = o.passwords.show_windows(ctx, store) {
                status = Some(s);
            }
            // Chaves dos servidores do workspace: o "Remember password"
            // deles vai para o cofre do workspace.
            let keys = o
                .servers
                .servers()
                .all_servers()
                .into_iter()
                .map(|s| crate::secrets::password_key(&s.settings))
                .collect();
            self.vault.lock().expect("vault lock").keys = keys;
            if o.git.take_files_changed() {
                o.queries = o.workspace.queries();
                if !o.servers.has_dialog() {
                    let current = o.servers.current.take();
                    o.servers = RegisteredPanel::new(o.workspace.servers());
                    o.servers.current = current;
                }
            }
        }
        if self.copying.as_ref().is_some_and(|h| h.is_finished()) {
            status = self.finish_copy().or(status);
        } else if self.copying.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.clone_job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut clone_ok = None;
        let mut clone_cancel = false;
        if let Some(dialog) = &mut self.clone {
            crate::theme::modal(egui::Id::new("workspace_clone")).show(ctx, |ui| {
                ui.set_width(460.0);
                ui.heading(tr("Clone Workspace"));
                ui.label(tr("Repository URL, for example git@github.com:you/sql.git:"));
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut dialog.url).desired_width(f32::INFINITY),
                );
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    clone_ok = Some(dialog.url.clone());
                }
                ui.weak(tr("Next you choose the folder where the clone goes. Git uses your SSH keys and credential helper."));
                if let Some(e) = &dialog.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        clone_cancel = true;
                    }
                    if ui.button(tr("Clone")).clicked() {
                        clone_ok = Some(dialog.url.clone());
                    }
                });
            });
        }
        if clone_cancel {
            self.clone = None;
        } else if let Some(url) = clone_ok
            && let Some(dialog) = &mut self.clone
        {
            if url.trim().is_empty() {
                dialog.error = Some(tr("Enter the repository URL.").into());
            } else {
                self.clone = None;
                self.pending_clone = Some(url.trim().to_string());
            }
        }
        let mut ok = None;
        let mut cancel = false;
        if let Some(dialog) = &mut self.new_folder {
            crate::theme::modal(egui::Id::new("workspace_new_folder")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading(tr("New Folder"));
                ui.label(tr("Folder name:"));
                let edit = ui.text_edit_singleline(&mut dialog.name);
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ok = Some(dialog.name.clone());
                }
                if let Some(e) = &dialog.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(tr("OK")).clicked() {
                        ok = Some(dialog.name.clone());
                    }
                });
            });
        }
        if cancel {
            self.new_folder = None;
        } else if let Some(name) = ok
            && let Ok(path) = self.create_folder(&name)
        {
            status = Some(trf!("Created {path}.", path = path.display()));
        }
        status
    }
}

fn folder_rows(
    ui: &mut egui::Ui,
    o: &mut Opened,
    folder: &Folder,
    depth: usize,
    actions: &mut Vec<Action>,
    folder_menu: &mut Option<PathBuf>,
) {
    let expanded = !o.collapsed.contains(&folder.path);
    let mut toggle = false;
    let name = if depth == 0 {
        tr("Queries").to_string()
    } else {
        folder.name.clone()
    };
    ui.horizontal(|ui| {
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
        let label = ui.add(
            egui::Button::selectable(
                o.selected.as_deref() == Some(folder.path.as_path()),
                format!("{glyph} {name}"),
            )
            .frame_when_inactive(false),
        );
        if label.clicked() {
            o.selected = Some(folder.path.clone());
        }
        if label.double_clicked() {
            toggle = true;
        }
        label.context_menu(|ui| {
            o.selected = Some(folder.path.clone());
            if ui.button(tr("New Query")).clicked() {
                actions.push(Action::NewQuery(folder.path.clone()));
                ui.close();
            }
            if ui.button(tr("New Folder...")).clicked() {
                *folder_menu = Some(folder.path.clone());
                ui.close();
            }
        });
    });
    if toggle {
        if expanded {
            o.collapsed.insert(folder.path.clone());
        } else {
            o.collapsed.remove(&folder.path);
        }
    }
    if !expanded {
        return;
    }
    for sub in &folder.folders {
        folder_rows(ui, o, sub, depth + 1, actions, folder_menu);
    }
    for file in &folder.files {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add_space((depth + 1) as f32 * 16.0 + 4.0 + 14.0);
            let label = ui.add(
                egui::Button::selectable(
                    o.selected.as_deref() == Some(file.as_path()),
                    format!("{} {name}", icon::FILE_SQL),
                )
                .frame_when_inactive(false),
            );
            if label.clicked() {
                o.selected = Some(file.clone());
            }
            if label.double_clicked() {
                actions.push(Action::OpenQuery(file.clone()));
            }
            label.context_menu(|ui| {
                o.selected = Some(file.clone());
                if ui.button(tr("Open")).clicked() {
                    actions.push(Action::OpenQuery(file.clone()));
                    ui.close();
                }
            });
        });
    }
}
