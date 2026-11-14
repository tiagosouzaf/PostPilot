//! O workspace na casca: abrir, criar e fechar, ligar as consultas aos
//! servidores do workspace ao abrir e ao salvar.

use std::path::{Path, PathBuf};

use pgstudio_core::connection::ConnectionSettings;
use pgstudio_i18n::{tr, trf};

use super::panel::Action;
use super::{QueryLink, Workspace, find_server_by_key, link_settings};
use crate::ShellApp;
use crate::panels::Panel;
use crate::registered;
use crate::servers::server_key;

impl ShellApp {
    /// Lembra o último workspace aberto neste arquivo e reabre o que
    /// estiver lá.
    pub fn with_workspace_memory(mut self, file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        let last = std::fs::read_to_string(&file)
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        self.workspace_memory = Some(file);
        if let Some(root) = last {
            let root = PathBuf::from(root);
            if let Ok(ws) = Workspace::open(&root) {
                self.workspace_panel.open(ws);
                self.after_workspace_opened();
            }
        }
        self
    }

    /// Workspace Explorer.
    pub fn workspace_panel(&self) -> &super::panel::WorkspacePanel {
        &self.workspace_panel
    }

    /// Workspace Explorer, mutável.
    pub fn workspace_panel_mut(&mut self) -> &mut super::panel::WorkspacePanel {
        &mut self.workspace_panel
    }

    /// O workspace aberto.
    pub fn current_workspace(&self) -> Option<&Workspace> {
        self.workspace_panel.workspace()
    }

    /// Abre o workspace da pasta e mostra o Workspace Explorer.
    pub fn open_workspace(&mut self, root: &Path) -> bool {
        match Workspace::open(root) {
            Ok(ws) => {
                self.status = format!("Opened workspace {}.", ws.name());
                self.workspace_panel.open(ws);
                self.after_workspace_opened();
                self.workspace.open(Panel::WorkspaceExplorer);
                self.remember_workspace();
                true
            }
            Err(e) => {
                self.status = e;
                false
            }
        }
    }

    /// Cria um workspace na pasta (o nome é o da pasta) e o abre.
    pub fn create_workspace(&mut self, root: &Path) -> bool {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Workspace".into());
        match Workspace::create(root, &name) {
            Ok(ws) => {
                self.status = format!("Created workspace {} in {}.", ws.name(), root.display());
                self.workspace_panel.open(ws);
                self.after_workspace_opened();
                self.workspace.open(Panel::WorkspaceExplorer);
                self.remember_workspace();
                true
            }
            Err(e) => {
                self.status = e;
                false
            }
        }
    }

    /// Depois de abrir um workspace: o "Remember password" passa a usar o
    /// cofre dele e a senha mestra é pedida (ou vem do cofre do sistema).
    fn after_workspace_opened(&mut self) {
        if self.secrets.is_none() {
            self.secrets = Some(std::sync::Arc::new(
                super::passwords::WorkspaceSecrets::new(None, self.workspace_panel.vault_slot()),
            ));
        }
        let store = self.secrets.clone();
        if let Some(p) = self.workspace_panel.passwords_mut() {
            p.auto_unlock(store.as_ref());
        }
    }

    /// File > Close Workspace. As janelas de consulta continuam abertas.
    pub fn close_workspace(&mut self) {
        if self.workspace_panel.workspace().is_none() {
            self.status = "No workspace is open.".to_string();
            return;
        }
        self.workspace_panel.close();
        self.remember_workspace();
        self.status = "Workspace closed.".to_string();
    }

    /// Clona `url` numa pasta nova dentro de `parent` (com o nome do
    /// repositório) numa thread separada; ao terminar, abre o workspace.
    pub fn clone_workspace(&mut self, url: &str, parent: &Path) {
        let dest = parent.join(super::git::repo_name(url));
        let busy = std::fs::read_dir(&dest).is_ok_and(|mut d| d.next().is_some());
        if busy {
            self.status = format!("{} already exists and is not empty.", dest.display());
            return;
        }
        self.status = format!("Cloning {url}...");
        self.workspace_panel.start_clone(url, &dest);
        self.workspace.open(Panel::WorkspaceExplorer);
    }

    /// Fim do clone: abre o workspace clonado. Um repositório sem workspace
    /// (recém-criado no GitHub, por exemplo) vira um.
    pub fn finish_clone(&mut self, result: Result<PathBuf, super::git::GitError>) {
        match result {
            Ok(root) if root.join(super::MANIFEST).exists() => {
                self.open_workspace(&root);
            }
            Ok(root) => {
                self.create_workspace(&root);
            }
            Err(e) => self.status = format!("Clone failed: {e}"),
        }
    }

    pub(crate) fn ask_new_workspace(&mut self) {
        if let Some(root) = self.dialogs.folder_path("New Workspace") {
            self.create_workspace(&root);
        }
    }

    pub(crate) fn ask_open_workspace(&mut self) {
        if let Some(root) = self.dialogs.folder_path("Open Workspace") {
            self.open_workspace(&root);
        }
    }

    fn remember_workspace(&self) {
        let Some(file) = &self.workspace_memory else {
            return;
        };
        let text = self
            .workspace_panel
            .workspace()
            .map(|w| w.root().display().to_string())
            .unwrap_or_default();
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(file, text) {
            eprintln!("workspace: could not save {}: {e}", file.display());
        }
    }

    /// Conexão (usuário, servidor, porta e banco) de um servidor conectado.
    fn connected_settings(&self, key: &str) -> Option<ConnectionSettings> {
        self.connection
            .iter()
            .chain(
                self.servers
                    .parked
                    .iter()
                    .filter_map(|p| p.connection.as_ref()),
            )
            .map(|c| &c.settings)
            .find(|s| server_key(s) == key)
            .cloned()
    }

    /// A ligação que uma consulta salva em `path` deve levar: o servidor da
    /// janela, se ele estiver no workspace, e o banco dela. `None` fora do
    /// workspace ou quando o servidor não é do workspace.
    pub fn query_link(&self, id: u32, path: &Path) -> Option<QueryLink> {
        let ws = self.workspace_panel.workspace()?;
        if !ws.contains(path.parent().unwrap_or(path)) {
            return None;
        }
        let key = self
            .doc_servers
            .get(&id)
            .cloned()
            .or_else(|| self.connection.as_ref().map(|c| server_key(&c.settings)))?;
        let servers = self.workspace_panel.servers()?.servers().root();
        let server = find_server_by_key(servers, &key)?;
        let database = self
            .documents
            .get(id)
            .and_then(|d| d.database.clone())
            .or_else(|| self.connected_settings(&key).map(|s| s.database))
            .filter(|d| !d.is_empty());
        Some(QueryLink {
            server: server.name.clone(),
            database,
        })
    }

    /// Antes de salvar: dentro do workspace, põe a ligação na primeira
    /// linha. Devolve a ligação gravada.
    pub(crate) fn link_before_save(&mut self, id: u32, path: &Path) -> Option<QueryLink> {
        let link = self.query_link(id, path)?;
        let doc = self.documents.get_mut(id)?;
        if QueryLink::read(&doc.text).as_ref() != Some(&link) {
            doc.text = link.apply(&doc.text);
        }
        Some(link)
    }

    /// Depois de salvar: atualiza o Workspace Explorer e explica quando a
    /// consulta ficou sem ligação.
    pub(crate) fn after_save(&mut self, path: &Path, link: Option<QueryLink>) {
        let Some(ws) = self.workspace_panel.workspace() else {
            return;
        };
        if !ws.contains(path.parent().unwrap_or(path)) {
            return;
        }
        self.workspace_panel.refresh();
        self.status = match link {
            Some(l) => match l.database {
                Some(db) => format!("Item(s) Saved. Linked to {} / {db}.", l.server),
                None => format!("Item(s) Saved. Linked to {}.", l.server),
            },
            None => "Item(s) Saved. Register this server in the workspace to link the query to it."
                .to_string(),
        };
    }

    /// Ao abrir uma consulta do workspace com ligação: a janela fica no
    /// banco dela e o servidor dela conecta (ou vem para a frente).
    pub(crate) fn connect_linked_query(&mut self, id: u32) {
        let Some(ws) = self.workspace_panel.workspace() else {
            return;
        };
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        if !doc.path().is_some_and(|p| ws.contains(p)) {
            return;
        }
        let Some(link) = QueryLink::read(&doc.text) else {
            return;
        };
        let Some(servers) = self.workspace_panel.servers() else {
            return;
        };
        let Some(settings) = link_settings(servers.servers().root(), &link) else {
            self.status = format!(
                "The query is linked to \"{}\", which is not a server of this workspace.",
                link.server
            );
            return;
        };
        if let Some(doc) = self.documents.get_mut(id) {
            doc.database = link.database.clone();
        }
        self.doc_servers.insert(id, server_key(&settings));
        if self.connector.is_some() {
            self.connect_registered(&settings, false);
        }
    }

    /// "New Query" numa pasta do workspace: cria o arquivo (já ligado ao
    /// servidor ativo, se ele for do workspace) e abre.
    pub fn new_workspace_query(&mut self, folder: &Path) {
        let path = Workspace::new_query_path(folder);
        let link = self.connection.as_ref().and_then(|c| {
            let servers = self.workspace_panel.servers()?.servers().root();
            let server = find_server_by_key(servers, &server_key(&c.settings))?;
            Some(QueryLink {
                server: server.name.clone(),
                database: Some(c.settings.database.clone()).filter(|d| !d.is_empty()),
            })
        });
        let text = link.map(|l| l.line() + "\n").unwrap_or_default();
        if let Err(e) = std::fs::write(&path, text) {
            self.status = format!("Could not create {}: {e}", path.display());
            return;
        }
        self.workspace_panel.refresh();
        self.open_file(&path);
    }

    /// "New Server Registration" na raiz dos servidores do workspace.
    pub fn new_workspace_server(&mut self) {
        let store = self.secrets.clone();
        self.workspace_panel.new_server(store.as_ref());
    }

    /// Traz os Registered Servers desta máquina para o workspace aberto
    /// (grupos de mesmo nome se juntam, um servidor de mesmo nome é
    /// substituído). Com as senhas do workspace destrancadas, as senhas
    /// lembradas deles são copiadas para lá, numa thread; o cofre do sistema
    /// continua com as dele. Devolve a mensagem para a status bar.
    pub fn import_registered_into_workspace(&mut self) -> Result<String, String> {
        let local = self.registered.servers().root().clone();
        let keys: Vec<String> = self
            .registered
            .servers()
            .all_servers()
            .into_iter()
            .map(|s| crate::secrets::password_key(&s.settings))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if keys.is_empty() {
            return Err(tr("There are no Registered Servers on this computer to import.").into());
        }
        let unlocked = self
            .workspace_panel
            .passwords()
            .is_some_and(|p| p.is_unlocked());
        let Some(panel) = self.workspace_panel.servers_mut() else {
            return Err(tr("No workspace is open.").into());
        };
        let n = panel.servers_mut().import_tree(&[], local)?;
        let imported = if n == 1 {
            tr("Imported 1 server into the workspace.").to_string()
        } else {
            trf!("Imported {n} servers into the workspace.", n = n)
        };
        match (&self.secrets, unlocked) {
            (Some(store), true) => {
                self.workspace_panel.copy_passwords(keys, store.clone());
                Ok(trf!(
                    "{imported} Copying their saved passwords to the workspace passwords...",
                    imported = imported
                ))
            }
            _ => Ok(trf!(
                "{imported} Turn on or unlock the workspace passwords to bring their saved passwords too.",
                imported = imported
            )),
        }
    }

    /// Uma vez por frame: trata os pedidos e os diálogos do Workspace
    /// Explorer.
    pub(crate) fn workspace_step(&mut self, ctx: &eframe::egui::Context, actions: Vec<Action>) {
        if let Some(panel) = self.workspace_panel.servers_mut() {
            panel.current = self.connection.as_ref().map(|c| c.settings.clone());
        }
        for action in actions {
            match action {
                Action::New => self.ask_new_workspace(),
                Action::Open => self.ask_open_workspace(),
                Action::OpenQuery(path) => {
                    self.open_file(&path);
                }
                Action::NewQuery(folder) => self.new_workspace_query(&folder),
                Action::ImportRegistered => {
                    self.status = match self.import_registered_into_workspace() {
                        Ok(s) | Err(s) => s,
                    }
                }
                Action::Servers(registered::Action::ConnectExplorer(s)) => {
                    self.connect_registered(&s, false)
                }
                Action::Servers(registered::Action::NewQuery(s)) => {
                    self.connect_registered(&s, true)
                }
                Action::Servers(registered::Action::Import(group)) => {
                    let Some(path) = self.dialogs.open_path() else {
                        continue;
                    };
                    let Some(panel) = self.workspace_panel.servers_mut() else {
                        continue;
                    };
                    self.status = match panel.servers_mut().import(&group, &path) {
                        Ok(1) => tr("Imported 1 server into the workspace.").to_string(),
                        Ok(n) => trf!("Imported {n} servers into the workspace.", n = n),
                        Err(e) => e,
                    };
                }
                Action::Servers(registered::Action::Export(group)) => {
                    let Some(path) = self.dialogs.save_path("RegisteredServers.json", None) else {
                        continue;
                    };
                    let Some(panel) = self.workspace_panel.servers() else {
                        continue;
                    };
                    self.status = match panel.servers().export(&group, &path) {
                        Ok(()) => format!(
                            "Exported servers to {} (passwords are not exported).",
                            path.display()
                        ),
                        Err(e) => e,
                    };
                }
            }
        }
        if let Some(status) = self
            .workspace_panel
            .show_windows(ctx, self.secrets.as_ref())
        {
            self.status = status;
        }
        if let Some(url) = self.workspace_panel.take_clone_request()
            && let Some(parent) = self.dialogs.folder_path("Clone Workspace into")
        {
            self.clone_workspace(&url, &parent);
        }
        if let Some(result) = self.workspace_panel.take_cloned() {
            self.finish_clone(result);
        }
    }
}
