//! Registered Servers (Ctrl+Alt+G): servidores guardados em grupos, como no
//! SSMS, para conectar com dois cliques.
//!
//! A árvore fica em `registered_servers.json`, sem senha. A senha de um
//! servidor registrado com "Remember password" vai para o cofre do sistema
//! (veja [`crate::secrets`]), na mesma chave que o "Connect to Server" usa;
//! conectar um servidor registrado procura a senha lá e só pede ao usuário
//! quando não acha (ou quando não há cofre).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::connection::{ConnectionSettings, DEFAULT_DATABASE, SslMode};
use serde::{Deserialize, Serialize};

use crate::secrets::{self, Lookup, SecretStore};

/// Versão do formato de `registered_servers.json`.
pub const VERSION: u32 = 1;

/// Nome do grupo raiz, como no SSMS.
pub const ROOT_NAME: &str = "Local Server Groups";

/// Um servidor registrado.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredServer {
    /// "Registered server name": o nome mostrado na árvore.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Como conectar (sem senha).
    pub settings: ConnectionSettings,
}

/// Um grupo de servidores, com subgrupos.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerGroup {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub groups: Vec<ServerGroup>,
    #[serde(default)]
    pub servers: Vec<RegisteredServer>,
}

/// Onde está um nó da árvore: os índices dos grupos a partir da raiz e,
/// para um servidor, o índice dele no último grupo.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct NodePath {
    pub groups: Vec<usize>,
    pub server: Option<usize>,
}

impl NodePath {
    /// Um grupo (a raiz é `&[]`).
    pub fn group(groups: &[usize]) -> Self {
        NodePath {
            groups: groups.to_vec(),
            server: None,
        }
    }

    /// Um servidor dentro de um grupo.
    pub fn server(groups: &[usize], index: usize) -> Self {
        NodePath {
            groups: groups.to_vec(),
            server: Some(index),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    root: ServerGroup,
}

/// A árvore de servidores registrados e o arquivo onde ela fica.
#[derive(Clone, Debug)]
pub struct RegisteredServers {
    path: Option<PathBuf>,
    root: ServerGroup,
}

impl Default for RegisteredServers {
    fn default() -> Self {
        RegisteredServers::in_memory(ServerGroup::default())
    }
}

impl RegisteredServers {
    /// Lê o arquivo. Arquivo ausente ou ruim vira árvore vazia.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let root = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<File>(&text).ok())
            .filter(|f| f.version == VERSION)
            .map(|f| f.root)
            .unwrap_or_default();
        RegisteredServers {
            path: Some(path),
            root: ServerGroup {
                name: ROOT_NAME.into(),
                ..root
            },
        }
    }

    /// Árvore em memória, sem arquivo (testes).
    pub fn in_memory(root: ServerGroup) -> Self {
        RegisteredServers {
            path: None,
            root: ServerGroup {
                name: ROOT_NAME.into(),
                ..root
            },
        }
    }

    /// Caminho do arquivo, se houver.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// O grupo raiz ("Local Server Groups").
    pub fn root(&self) -> &ServerGroup {
        &self.root
    }

    /// Um grupo pelo caminho.
    pub fn group(&self, groups: &[usize]) -> Option<&ServerGroup> {
        groups.iter().try_fold(&self.root, |g, &i| g.groups.get(i))
    }

    fn group_mut(&mut self, groups: &[usize]) -> Option<&mut ServerGroup> {
        groups
            .iter()
            .try_fold(&mut self.root, |g, &i| g.groups.get_mut(i))
    }

    /// Um servidor pelo caminho.
    pub fn server(&self, path: &NodePath) -> Option<&RegisteredServer> {
        self.group(&path.groups)?.servers.get(path.server?)
    }

    /// Todos os servidores, em ordem da árvore.
    pub fn all_servers(&self) -> Vec<&RegisteredServer> {
        fn walk<'a>(g: &'a ServerGroup, out: &mut Vec<&'a RegisteredServer>) {
            for sub in &g.groups {
                walk(sub, out);
            }
            out.extend(&g.servers);
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    /// Cria um grupo dentro de `parent`. Devolve o caminho dele.
    pub fn add_group(
        &mut self,
        parent: &[usize],
        group: ServerGroup,
    ) -> Result<Vec<usize>, String> {
        let name = group.name.trim().to_string();
        let g = self
            .group_mut(parent)
            .ok_or("The group no longer exists.")?;
        check_name(&name, g.groups.iter().map(|g| g.name.as_str()), "group")?;
        g.groups.push(ServerGroup { name, ..group });
        let mut path = parent.to_vec();
        path.push(g.groups.len() - 1);
        self.save_or_log();
        Ok(path)
    }

    /// Troca nome e descrição de um grupo (a raiz não muda).
    pub fn update_group(
        &mut self,
        groups: &[usize],
        name: &str,
        description: &str,
    ) -> Result<(), String> {
        let Some((&last, parent)) = groups.split_last() else {
            return Err("The root group cannot be renamed.".into());
        };
        let name = name.trim().to_string();
        let p = self
            .group_mut(parent)
            .ok_or("The group no longer exists.")?;
        let others = p
            .groups
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != last)
            .map(|(_, g)| g.name.as_str());
        check_name(&name, others, "group")?;
        let g = p
            .groups
            .get_mut(last)
            .ok_or("The group no longer exists.")?;
        g.name = name;
        g.description = description.trim().to_string();
        self.save_or_log();
        Ok(())
    }

    /// Registra um servidor em `parent`. Devolve o caminho dele.
    pub fn add_server(
        &mut self,
        parent: &[usize],
        server: RegisteredServer,
    ) -> Result<NodePath, String> {
        let server = clean(server);
        let g = self
            .group_mut(parent)
            .ok_or("The group no longer exists.")?;
        check_name(
            &server.name,
            g.servers.iter().map(|s| s.name.as_str()),
            "server",
        )?;
        g.servers.push(server);
        let path = NodePath::server(parent, g.servers.len() - 1);
        self.save_or_log();
        Ok(path)
    }

    /// Troca os dados de um servidor registrado.
    pub fn update_server(
        &mut self,
        path: &NodePath,
        server: RegisteredServer,
    ) -> Result<(), String> {
        let server = clean(server);
        let index = path.server.ok_or("Not a server.")?;
        let g = self
            .group_mut(&path.groups)
            .ok_or("The group no longer exists.")?;
        let others = g
            .servers
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, s)| s.name.as_str());
        check_name(&server.name, others, "server")?;
        *g.servers
            .get_mut(index)
            .ok_or("The server no longer exists.")? = server;
        self.save_or_log();
        Ok(())
    }

    /// Apaga um servidor ou um grupo (com tudo o que há dentro). A raiz
    /// não é apagada.
    pub fn remove(&mut self, path: &NodePath) -> bool {
        let removed = match (path.server, path.groups.split_last()) {
            (Some(i), _) => self.group_mut(&path.groups).is_some_and(|g| {
                i < g.servers.len() && {
                    g.servers.remove(i);
                    true
                }
            }),
            (None, Some((&last, parent))) => self.group_mut(parent).is_some_and(|g| {
                last < g.groups.len() && {
                    g.groups.remove(last);
                    true
                }
            }),
            (None, None) => false,
        };
        if removed {
            self.save_or_log();
        }
        removed
    }

    fn save_or_log(&self) {
        if let Err(e) = self.save()
            && let Some(path) = &self.path
        {
            eprintln!("registered servers: could not save {}: {e}", path.display());
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = File {
            version: VERSION,
            root: self.root.clone(),
        };
        let text = serde_json::to_string_pretty(&file).expect("always serializable");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

fn clean(server: RegisteredServer) -> RegisteredServer {
    let name = match server.name.trim() {
        "" => server.settings.server_name(),
        n => n.to_string(),
    };
    RegisteredServer {
        name,
        description: server.description.trim().to_string(),
        ..server
    }
}

fn check_name<'a>(
    name: &str,
    mut siblings: impl Iterator<Item = &'a str>,
    what: &str,
) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("Enter a {what} name."));
    }
    if siblings.any(|s| s.eq_ignore_ascii_case(name)) {
        return Err(format!(
            "A {what} named \"{name}\" already exists in this group."
        ));
    }
    Ok(())
}

/// O que o usuário pediu na janela Registered Servers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Conectar o Object Explorer ao servidor (duplo clique ou Connect >
    /// Object Explorer).
    ConnectExplorer(ConnectionSettings),
    /// Conectar e abrir uma janela de consulta (Connect > New Query).
    NewQuery(ConnectionSettings),
}

/// Janela Registered Servers: a árvore, a seleção e os diálogos.
pub struct RegisteredPanel {
    servers: RegisteredServers,
    selected: Option<NodePath>,
    /// Grupos recolhidos (os demais aparecem abertos).
    collapsed: HashSet<Vec<usize>>,
    server_dialog: Option<ServerDialog>,
    group_dialog: Option<GroupDialog>,
    confirm_delete: Option<NodePath>,
    /// Conexão ativa, para preencher "New Server Registration".
    pub current: Option<ConnectionSettings>,
}

impl Default for RegisteredPanel {
    fn default() -> Self {
        RegisteredPanel::new(RegisteredServers::default())
    }
}

impl RegisteredPanel {
    pub fn new(servers: RegisteredServers) -> Self {
        RegisteredPanel {
            servers,
            selected: None,
            collapsed: HashSet::new(),
            server_dialog: None,
            group_dialog: None,
            confirm_delete: None,
            current: None,
        }
    }

    /// A árvore.
    pub fn servers(&self) -> &RegisteredServers {
        &self.servers
    }

    /// A árvore, para mudar sem os diálogos (testes e scripts).
    pub fn servers_mut(&mut self) -> &mut RegisteredServers {
        &mut self.servers
    }

    /// Nó selecionado.
    pub fn selected(&self) -> Option<&NodePath> {
        self.selected.as_ref()
    }

    /// Seleciona um nó.
    pub fn select(&mut self, path: NodePath) {
        self.selected = Some(path);
    }

    /// Diálogo de servidor aberto, se houver.
    pub fn server_dialog_mut(&mut self) -> Option<&mut ServerDialog> {
        self.server_dialog.as_mut()
    }

    /// Diálogo de grupo aberto, se houver.
    pub fn group_dialog_mut(&mut self) -> Option<&mut GroupDialog> {
        self.group_dialog.as_mut()
    }

    /// Se algum diálogo da janela está aberto.
    pub fn has_dialog(&self) -> bool {
        self.server_dialog.is_some() || self.group_dialog.is_some() || self.confirm_delete.is_some()
    }

    /// "New Server Registration..." no grupo `parent`, preenchido com a
    /// conexão ativa (ou o padrão).
    pub fn new_server(&mut self, parent: &[usize], store: Option<&Arc<dyn SecretStore>>) {
        let settings = self.current.clone().unwrap_or_default();
        let server = RegisteredServer {
            name: String::new(),
            description: String::new(),
            settings,
        };
        self.server_dialog = Some(ServerDialog::new(parent.to_vec(), None, &server, store));
    }

    /// "Properties" de um servidor registrado.
    pub fn edit_server(&mut self, path: &NodePath, store: Option<&Arc<dyn SecretStore>>) {
        if let Some(server) = self.servers.server(path) {
            let dialog = ServerDialog::new(path.groups.clone(), Some(path.clone()), server, store);
            self.server_dialog = Some(dialog);
        }
    }

    /// "New Server Group..." dentro de `parent`.
    pub fn new_group(&mut self, parent: &[usize]) {
        self.group_dialog = Some(GroupDialog {
            parent: parent.to_vec(),
            editing: false,
            name: String::new(),
            description: String::new(),
            error: None,
        });
    }

    fn edit_group(&mut self, groups: &[usize]) {
        let Some(g) = self.servers.group(groups) else {
            return;
        };
        self.group_dialog = Some(GroupDialog {
            parent: groups.to_vec(),
            editing: true,
            name: g.name.clone(),
            description: g.description.clone(),
            error: None,
        });
    }

    /// Desenha a árvore e devolve os pedidos de conexão.
    pub fn show(&mut self, ui: &mut egui::Ui, store: Option<&Arc<dyn SecretStore>>) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut menu: Option<Menu> = None;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(icon::DESKTOP_TOWER);
                ui.strong("Database Engine");
            });
            self.group_rows(ui, &[], 0, &mut actions, &mut menu);
            if self.servers.all_servers().is_empty() && self.servers.root().groups.is_empty() {
                ui.add_space(6.0);
                ui.weak("No registered servers. Right-click \"Local Server Groups\" and choose \"New Server Registration...\".");
            }
        });
        match menu {
            Some(Menu::NewServer(parent)) => self.new_server(&parent, store),
            Some(Menu::NewGroup(parent)) => self.new_group(&parent),
            Some(Menu::Edit(path)) if path.server.is_some() => self.edit_server(&path, store),
            Some(Menu::Edit(path)) => self.edit_group(&path.groups),
            Some(Menu::Delete(path)) => self.confirm_delete = Some(path),
            None => {}
        }
        actions
    }

    fn group_rows(
        &mut self,
        ui: &mut egui::Ui,
        groups: &[usize],
        depth: usize,
        actions: &mut Vec<Action>,
        menu: &mut Option<Menu>,
    ) {
        let Some(group) = self.servers.group(groups) else {
            return;
        };
        let expanded = !self.collapsed.contains(groups);
        let here = NodePath::group(groups);
        let is_root = groups.is_empty();
        let name = group.name.clone();
        let description = group.description.clone();
        let (subgroups, servers) = (group.groups.len(), group.servers.clone());
        let mut toggle = false;
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 16.0 + 8.0);
            let caret = if expanded {
                icon::CARET_DOWN
            } else {
                icon::CARET_RIGHT
            };
            if ui.add(egui::Button::new(caret).frame(false)).clicked() {
                toggle = true;
            }
            let folder = if expanded {
                icon::FOLDER_OPEN
            } else {
                icon::FOLDER
            };
            let label = ui.add(
                egui::Button::selectable(
                    self.selected.as_ref() == Some(&here),
                    format!("{folder} {name}"),
                )
                .frame_when_inactive(false),
            );
            let label = if description.is_empty() {
                label
            } else {
                label.on_hover_text(&description)
            };
            if label.clicked() {
                self.selected = Some(here.clone());
            }
            if label.double_clicked() {
                toggle = true;
            }
            label.context_menu(|ui| {
                self.selected = Some(here.clone());
                if ui.button("New Server Group...").clicked() {
                    *menu = Some(Menu::NewGroup(groups.to_vec()));
                    ui.close();
                }
                if ui.button("New Server Registration...").clicked() {
                    *menu = Some(Menu::NewServer(groups.to_vec()));
                    ui.close();
                }
                if !is_root {
                    ui.separator();
                    if ui.button("Delete").clicked() {
                        *menu = Some(Menu::Delete(here.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Properties").clicked() {
                        *menu = Some(Menu::Edit(here.clone()));
                        ui.close();
                    }
                }
            });
        });
        if toggle {
            if expanded {
                self.collapsed.insert(groups.to_vec());
            } else {
                self.collapsed.remove(groups);
            }
        }
        if !expanded {
            return;
        }
        for i in 0..subgroups {
            let mut sub = groups.to_vec();
            sub.push(i);
            self.group_rows(ui, &sub, depth + 1, actions, menu);
        }
        for (i, server) in servers.iter().enumerate() {
            let path = NodePath::server(groups, i);
            ui.horizontal(|ui| {
                ui.add_space((depth + 1) as f32 * 16.0 + 8.0 + 14.0);
                let label = ui.add(
                    egui::Button::selectable(
                        self.selected.as_ref() == Some(&path),
                        format!("{} {}", icon::DESKTOP_TOWER, server.name),
                    )
                    .frame_when_inactive(false),
                );
                let tip = if server.description.is_empty() {
                    format!(
                        "{} ({})",
                        server.settings.server_name(),
                        server.settings.user
                    )
                } else {
                    format!(
                        "{} ({})\n{}",
                        server.settings.server_name(),
                        server.settings.user,
                        server.description
                    )
                };
                let label = label.on_hover_text(tip);
                if label.clicked() {
                    self.selected = Some(path.clone());
                }
                if label.double_clicked() {
                    actions.push(Action::ConnectExplorer(server.settings.clone()));
                }
                label.context_menu(|ui| {
                    self.selected = Some(path.clone());
                    ui.menu_button("Connect", |ui| {
                        if ui.button("Object Explorer").clicked() {
                            actions.push(Action::ConnectExplorer(server.settings.clone()));
                            ui.close();
                        }
                        if ui.button("New Query").clicked() {
                            actions.push(Action::NewQuery(server.settings.clone()));
                            ui.close();
                        }
                    });
                    ui.separator();
                    if ui.button("Delete").clicked() {
                        *menu = Some(Menu::Delete(path.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Properties").clicked() {
                        *menu = Some(Menu::Edit(path.clone()));
                        ui.close();
                    }
                });
            });
        }
    }

    /// Desenha os diálogos abertos. Devolve uma mensagem para a status bar
    /// quando algo mudou.
    pub fn show_windows(&mut self, ctx: &egui::Context) -> Option<String> {
        let mut status = None;
        if let Some(dialog) = &mut self.server_dialog {
            match dialog.show(ctx) {
                Reply::None => {}
                Reply::Cancel => self.server_dialog = None,
                Reply::Ok => {
                    let dialog = self.server_dialog.take().expect("dialog is open");
                    match dialog.apply(&mut self.servers) {
                        Ok(path) => {
                            status = Some(format!(
                                "Registered server \"{}\" saved.",
                                self.servers
                                    .server(&path)
                                    .map(|s| s.name.as_str())
                                    .unwrap_or("")
                            ));
                            self.selected = Some(path);
                        }
                        Err(failed) => {
                            let (mut dialog, e) = *failed;
                            dialog.error = Some(e);
                            self.server_dialog = Some(dialog);
                        }
                    }
                }
            }
        }
        if let Some(dialog) = &mut self.group_dialog {
            match dialog.show(ctx) {
                Reply::None => {}
                Reply::Cancel => self.group_dialog = None,
                Reply::Ok => {
                    let result = if dialog.editing {
                        self.servers
                            .update_group(&dialog.parent, &dialog.name, &dialog.description)
                            .map(|()| dialog.parent.clone())
                    } else {
                        self.servers.add_group(
                            &dialog.parent,
                            ServerGroup {
                                name: dialog.name.clone(),
                                description: dialog.description.clone(),
                                ..Default::default()
                            },
                        )
                    };
                    match result {
                        Ok(path) => {
                            status =
                                Some(format!("Server group \"{}\" saved.", dialog.name.trim()));
                            self.selected = Some(NodePath::group(&path));
                            self.group_dialog = None;
                        }
                        Err(e) => dialog.error = Some(e),
                    }
                }
            }
        }
        if let Some(path) = self.confirm_delete.clone() {
            let name = match path.server {
                Some(_) => self.servers.server(&path).map(|s| s.name.clone()),
                None => self.servers.group(&path.groups).map(|g| g.name.clone()),
            };
            let Some(name) = name else {
                self.confirm_delete = None;
                return status;
            };
            let what = if path.server.is_some() {
                "server registration"
            } else {
                "server group and everything in it"
            };
            let mut answer = None;
            let modal = egui::Modal::new(egui::Id::new("registered_delete")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading("Delete");
                ui.add_space(4.0);
                ui.label(format!(
                    "Are you sure you want to delete the {what} \"{name}\"?"
                ));
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("No").clicked() {
                        answer = Some(false);
                    }
                    if ui.button("Yes").clicked() {
                        answer = Some(true);
                    }
                });
            });
            if modal.should_close() && answer.is_none() {
                answer = Some(false);
            }
            if let Some(yes) = answer {
                self.confirm_delete = None;
                if yes && self.servers.remove(&path) {
                    self.selected = None;
                    status = Some(format!("Deleted \"{name}\"."));
                }
            }
        }
        status
    }
}

enum Menu {
    NewServer(Vec<usize>),
    NewGroup(Vec<usize>),
    Edit(NodePath),
    Delete(NodePath),
}

enum Reply {
    None,
    Cancel,
    Ok,
}

/// "New Server Registration" e "Edit Server Registration Properties".
pub struct ServerDialog {
    parent: Vec<usize>,
    editing: Option<NodePath>,
    pub server: String,
    pub user: String,
    pub password: String,
    pub remember: bool,
    pub database: String,
    pub ssl: SslMode,
    pub name: String,
    pub description: String,
    error: Option<String>,
    store: Option<Arc<dyn SecretStore>>,
    lookup: Option<Lookup>,
    saved: bool,
}

impl ServerDialog {
    fn new(
        parent: Vec<usize>,
        editing: Option<NodePath>,
        server: &RegisteredServer,
        store: Option<&Arc<dyn SecretStore>>,
    ) -> Self {
        let s = &server.settings;
        let lookup = store.map(|st| secrets::lookup(st, secrets::password_key(s)));
        ServerDialog {
            parent,
            editing,
            server: s.server_name(),
            user: s.user.clone(),
            password: String::new(),
            remember: false,
            database: s.database.clone(),
            ssl: s.ssl,
            name: server.name.clone(),
            description: server.description.clone(),
            error: None,
            store: store.cloned(),
            lookup,
            saved: false,
        }
    }

    /// Mensagem de erro da última tentativa de salvar.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Espera a leitura da senha guardada (testes).
    pub fn wait_lookup(&mut self) {
        if let Some(l) = self.lookup.take() {
            if let Some(p) = l.wait()
                && self.password.is_empty()
            {
                self.password = p;
                self.remember = true;
                self.saved = true;
            }
        }
    }

    fn poll_lookup(&mut self) {
        let Some(found) = self.lookup.as_ref().and_then(Lookup::poll) else {
            return;
        };
        self.lookup = None;
        if let Some(p) = found
            && self.password.is_empty()
        {
            self.password = p;
            self.remember = true;
            self.saved = true;
        }
    }

    fn settings(&self) -> Result<ConnectionSettings, String> {
        let mut s = ConnectionSettings {
            user: self.user.trim().to_string(),
            database: self.database.trim().to_string(),
            ssl: self.ssl,
            ..Default::default()
        };
        s.set_server_name(&self.server)?;
        if s.user.is_empty() {
            return Err("Enter a login.".into());
        }
        if s.database.is_empty() {
            s.database = DEFAULT_DATABASE.into();
        }
        Ok(s)
    }

    /// Grava na árvore e no cofre. Em caso de erro devolve o diálogo para
    /// continuar aberto.
    fn apply(
        self,
        servers: &mut RegisteredServers,
    ) -> Result<NodePath, Box<(ServerDialog, String)>> {
        let settings = match self.settings() {
            Ok(s) => s,
            Err(e) => return Err(Box::new((self, e))),
        };
        let server = RegisteredServer {
            name: self.name.clone(),
            description: self.description.clone(),
            settings: settings.clone(),
        };
        let result = match &self.editing {
            Some(path) => servers.update_server(path, server).map(|()| path.clone()),
            None => servers.add_server(&self.parent, server),
        };
        let path = match result {
            Ok(p) => p,
            Err(e) => return Err(Box::new((self, e))),
        };
        if let Some(store) = &self.store {
            let key = secrets::password_key(&settings);
            if self.remember && !self.password.is_empty() {
                secrets::save_in_background(store, key, Some(self.password.clone()));
            } else if !self.remember && self.saved {
                secrets::save_in_background(store, key, None);
            }
        }
        Ok(path)
    }

    fn show(&mut self, ctx: &egui::Context) -> Reply {
        self.poll_lookup();
        if self.lookup.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(20));
        }
        let mut reply = Reply::None;
        let title = if self.editing.is_some() {
            "Edit Server Registration Properties"
        } else {
            "New Server Registration"
        };
        let modal = egui::Modal::new(egui::Id::new("registered_server")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(title);
            ui.add_space(4.0);
            egui::Grid::new("registered_server_fields")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    let label = ui.label("Server name:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.server)
                            .desired_width(300.0)
                            .hint_text("host or host,port"),
                    )
                    .labelled_by(label.id);
                    ui.end_row();

                    let label = ui.label("Login:");
                    ui.add(egui::TextEdit::singleline(&mut self.user).desired_width(300.0))
                        .labelled_by(label.id);
                    ui.end_row();

                    let label = ui.label("Password:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.password)
                            .password(true)
                            .desired_width(300.0),
                    )
                    .labelled_by(label.id);
                    ui.end_row();

                    ui.label("");
                    ui.checkbox(&mut self.remember, "Remember password");
                    ui.end_row();

                    let label = ui.label("Database:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.database)
                            .desired_width(300.0)
                            .hint_text(DEFAULT_DATABASE),
                    )
                    .labelled_by(label.id);
                    ui.end_row();

                    let label = ui.label("Encryption:");
                    egui::ComboBox::from_id_salt("registered_ssl")
                        .selected_text(self.ssl.label())
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            for mode in SslMode::ALL {
                                ui.selectable_value(&mut self.ssl, mode, mode.label());
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    ui.end_row();

                    let label = ui.label("Registered server name:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.name)
                            .desired_width(300.0)
                            .hint_text("same as the server name"),
                    )
                    .labelled_by(label.id);
                    ui.end_row();

                    let label = ui.label("Registered server description:");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.description)
                            .desired_rows(2)
                            .desired_width(300.0),
                    )
                    .labelled_by(label.id);
                    ui.end_row();
                });
            if let Some(error) = &self.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    reply = Reply::Cancel;
                }
                if ui.button("Save").clicked() {
                    reply = Reply::Ok;
                }
            });
        });
        if modal.should_close() && matches!(reply, Reply::None) {
            reply = Reply::Cancel;
        }
        reply
    }
}

/// "New Server Group Properties" e "Edit Server Group Properties".
pub struct GroupDialog {
    /// Grupo pai (novo) ou o próprio grupo (editando).
    parent: Vec<usize>,
    editing: bool,
    pub name: String,
    pub description: String,
    error: Option<String>,
}

impl GroupDialog {
    /// Mensagem de erro da última tentativa de salvar.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn show(&mut self, ctx: &egui::Context) -> Reply {
        let mut reply = Reply::None;
        let title = if self.editing {
            "Edit Server Group Properties"
        } else {
            "New Server Group Properties"
        };
        let modal = egui::Modal::new(egui::Id::new("registered_group")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading(title);
            ui.add_space(4.0);
            egui::Grid::new("registered_group_fields")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    let label = ui.label("Group name:");
                    ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(260.0))
                        .labelled_by(label.id);
                    ui.end_row();
                    let label = ui.label("Group description:");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.description)
                            .desired_rows(2)
                            .desired_width(260.0),
                    )
                    .labelled_by(label.id);
                    ui.end_row();
                });
            if let Some(error) = &self.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    reply = Reply::Cancel;
                }
                if ui.button("OK").clicked() {
                    reply = Reply::Ok;
                }
            });
        });
        if modal.should_close() && matches!(reply, Reply::None) {
            reply = Reply::Cancel;
        }
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, host: &str) -> RegisteredServer {
        RegisteredServer {
            name: name.into(),
            description: String::new(),
            settings: ConnectionSettings {
                host: host.into(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn groups_and_servers_nest_and_names_are_unique_per_group() {
        let mut r = RegisteredServers::default();
        let prod = r
            .add_group(
                &[],
                ServerGroup {
                    name: " Production ".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(r.group(&prod).unwrap().name, "Production");
        assert!(
            r.add_group(
                &[],
                ServerGroup {
                    name: "production".into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(r.add_group(&[], ServerGroup::default()).is_err());
        let eu = r
            .add_group(
                &prod,
                ServerGroup {
                    name: "EU".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(eu, vec![0, 0]);
        let a = r.add_server(&eu, server("", "db1.example.com")).unwrap();
        assert_eq!(r.server(&a).unwrap().name, "db1.example.com");
        assert!(
            r.add_server(&eu, server("DB1.example.com", "other"))
                .is_err()
        );
        r.add_server(&[], server("local", "localhost")).unwrap();
        let names: Vec<_> = r.all_servers().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["db1.example.com", "local"]);

        r.update_group(&eu, "Europe", "west").unwrap();
        assert_eq!(r.group(&eu).unwrap().description, "west");
        assert!(r.update_group(&[], "x", "").is_err());
        r.update_server(&a, server("primary", "db1.example.com"))
            .unwrap();
        assert_eq!(r.server(&a).unwrap().name, "primary");

        assert!(r.remove(&NodePath::group(&prod)));
        assert_eq!(r.all_servers().len(), 1);
        assert!(!r.remove(&NodePath::group(&[])));
    }

    #[test]
    fn survives_a_restart_without_passwords_and_ignores_garbage() {
        let dir = std::env::temp_dir().join(format!("pgstudio-registered-{}", std::process::id()));
        let path = dir.join("registered_servers.json");
        let _ = std::fs::remove_file(&path);
        let mut r = RegisteredServers::open(&path);
        assert_eq!(r.root().name, ROOT_NAME);
        let g = r
            .add_group(
                &[],
                ServerGroup {
                    name: "Dev".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        r.add_server(&g, server("dev db", "dev.example.com"))
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("password"), "{text}");
        let again = RegisteredServers::open(&path);
        assert_eq!(again.root(), r.root());

        std::fs::write(&path, "[1, 2").unwrap();
        assert!(RegisteredServers::open(&path).all_servers().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_a_registration_remembers_the_password() {
        let store: Arc<dyn SecretStore> = Arc::new(crate::secrets::MemoryStore::default());
        let mut panel = RegisteredPanel {
            current: Some(ConnectionSettings {
                host: "db".into(),
                user: "alice".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        panel.new_server(&[], Some(&store));
        let mut dialog = panel.server_dialog.take().unwrap();
        dialog.wait_lookup();
        assert_eq!(dialog.server, "db");
        dialog.password = "pw".into();
        dialog.remember = true;
        let path = dialog.apply(&mut panel.servers).ok().unwrap();
        assert_eq!(panel.servers.server(&path).unwrap().name, "db");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while store.get("alice@db:5432").unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        // Editar mostra a senha guardada; desmarcar esquece.
        panel.edit_server(&path, Some(&store));
        let mut dialog = panel.server_dialog.take().unwrap();
        dialog.wait_lookup();
        assert_eq!((dialog.password.as_str(), dialog.remember), ("pw", true));
        dialog.remember = false;
        dialog.apply(&mut panel.servers).ok().unwrap();
        while store.get("alice@db:5432").unwrap().is_some() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}
