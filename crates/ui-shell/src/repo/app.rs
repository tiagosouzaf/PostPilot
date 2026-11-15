//! O workspace na casca: abrir, criar e fechar, ligar as consultas aos
//! servidores do workspace ao abrir e ao salvar.

use pgstudio_i18n::{tr, trf};
use std::path::{Path, PathBuf};

use pgstudio_core::connection::ConnectionSettings;

use super::panel::Action;
use super::{QueryLink, Workspace, find_server_by_key, link_settings};
use crate::ShellApp;
use crate::file_dialogs::Question;
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
                self.status = trf!("Opened workspace {name}.", name = ws.name());
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
            .unwrap_or_else(|| tr("Workspace").into());
        match Workspace::create(root, &name) {
            Ok(ws) => {
                self.status = trf!(
                    "Created workspace {name} in {folder}.",
                    name = ws.name(),
                    folder = root.display()
                );
                self.workspace_panel.open(ws);
                self.after_workspace_opened();
                self.workspace.open(Panel::WorkspaceExplorer);
                self.remember_workspace();
                // Um workspace novo nasce sem conexões: oferece trazer as
                // desta máquina, para elas irem no primeiro commit.
                let local = self.importable_connections();
                self.workspace_panel.offer_import(local);
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
            self.status = tr("No workspace is open.").to_string();
            return;
        }
        self.workspace_panel.close();
        self.remember_workspace();
        self.status = tr("Workspace closed.").to_string();
    }

    /// Clona `url` numa pasta nova dentro de `parent` (com o nome do
    /// repositório) numa thread separada; ao terminar, abre o workspace.
    pub fn clone_workspace(&mut self, url: &str, parent: &Path) {
        let dest = parent.join(super::git::repo_name(url));
        let busy = std::fs::read_dir(&dest).is_ok_and(|mut d| d.next().is_some());
        if busy {
            self.status = trf!(
                "{folder} already exists and is not empty.",
                folder = dest.display()
            );
            return;
        }
        self.status = trf!("Cloning {url}...", url = url);
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
            Err(e) => self.status = trf!("Clone failed: {e}", e = e),
        }
    }

    pub(crate) fn ask_new_workspace(&mut self) {
        let question = Question::Folder {
            title: tr("New Workspace").into(),
        };
        self.ask_path(question, |app, _, root| {
            if let Some(root) = root {
                app.create_workspace(&root);
            }
        });
    }

    pub(crate) fn ask_open_workspace(&mut self) {
        let question = Question::Folder {
            title: tr("Open Workspace").into(),
        };
        self.ask_path(question, |app, _, root| {
            if let Some(root) = root {
                app.open_workspace(&root);
            }
        });
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
                Some(db) => trf!(
                    "Item(s) Saved. Linked to {server} / {db}.",
                    server = l.server,
                    db = db
                ),
                None => trf!("Item(s) Saved. Linked to {server}.", server = l.server),
            },
            None => {
                tr("Item(s) Saved. Register this server in the workspace to link the query to it.")
                    .to_string()
            }
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
            self.status = trf!(
                "The query is linked to \"{server}\", which is not a server of this workspace.",
                server = link.server
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
            self.status = trf!("Could not create {path}: {e}", path = path.display(), e = e);
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

    /// As conexões desta máquina que podem ir para o workspace: a árvore
    /// dos Registered Servers e, soltas na raiz, as conexões recentes e a
    /// conexão ativa que não estão registradas. Quem já está no workspace
    /// (mesmo usuário, servidor e porta) não entra de novo como conexão
    /// solta. Devolve a árvore e as chaves das senhas a procurar.
    fn local_connections(&self) -> (registered::ServerGroup, Vec<String>) {
        let mut root = self.registered.servers().root().clone();
        let mut keys: Vec<String> = Vec::new();
        for s in self.registered.servers().all_servers() {
            let key = server_key(&s.settings);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        let in_workspace = |key: &str| {
            self.workspace_panel
                .servers()
                .is_some_and(|p| find_server_by_key(p.servers().root(), key).is_some())
        };
        let loose = self
            .connection
            .iter()
            .map(|c| &c.settings)
            .chain(self.recent.list());
        for settings in loose {
            let key = server_key(settings);
            if keys.contains(&key) || in_workspace(&key) {
                continue;
            }
            // Um servidor de mesmo nome seria substituído ao juntar.
            let ours = self
                .workspace_panel
                .servers()
                .map(|p| p.servers().root().servers.as_slice())
                .unwrap_or_default();
            let taken = |name: &str| {
                root.servers
                    .iter()
                    .chain(ours)
                    .any(|s| s.name.eq_ignore_ascii_case(name))
            };
            let host = settings.server_name();
            let name = [
                host.clone(),
                format!("{host} ({})", settings.user),
                format!("{host} ({}, {})", settings.user, settings.database),
            ]
            .into_iter()
            .find(|n| !taken(n))
            .unwrap_or_else(|| key.clone());
            root.servers.push(registered::RegisteredServer {
                name,
                settings: settings.clone(),
                ..Default::default()
            });
            keys.push(key);
        }
        (root, keys)
    }

    /// Quantas conexões desta máquina ainda não estão no workspace aberto.
    pub fn importable_connections(&self) -> usize {
        let Some(panel) = self.workspace_panel.servers() else {
            return 0;
        };
        let ours = panel.servers().root();
        let (_, keys) = self.local_connections();
        keys.iter()
            .filter(|k| find_server_by_key(ours, k).is_none())
            .count()
    }

    /// A senha em uso na conexão ativa, com a chave dela.
    fn current_password(&self) -> Option<(String, String)> {
        let c = self.connection.as_ref()?;
        (!self.password.is_empty()).then(|| (server_key(&c.settings), self.password.clone()))
    }

    /// Traz as conexões desta máquina para o workspace aberto: os Registered
    /// Servers (grupos de mesmo nome se juntam, um servidor de mesmo nome é
    /// substituído), as conexões recentes e a conexão ativa. As senhas
    /// lembradas delas são copiadas para o cofre do workspace, numa thread;
    /// se ele ainda não existe ou está trancado, a senha mestra é pedida e a
    /// cópia acontece em seguida. O cofre do sistema continua com as dele.
    /// Devolve a mensagem para a status bar.
    pub fn import_registered_into_workspace(&mut self) -> Result<String, String> {
        if self.workspace_panel.workspace().is_none() {
            return Err(tr("No workspace is open.").into());
        }
        let (local, keys) = self.local_connections();
        let environments = self
            .registered
            .servers()
            .custom_environments()
            .map(<[_]>::to_vec);
        if keys.is_empty() {
            return Err(tr("There are no connections on this computer to import. Right-click the workspace under Servers and choose New Server Registration.").into());
        }
        let known = self
            .current_password()
            .into_iter()
            .filter(|(k, _)| keys.contains(k))
            .collect();
        let Some(panel) = self.workspace_panel.servers_mut() else {
            return Err(tr("No workspace is open.").into());
        };
        if let Some(envs) = &environments {
            panel.servers_mut().merge_environments(envs);
        }
        let n = panel.servers_mut().import_tree(&[], local)?;
        let imported = if n == 1 {
            tr("Imported 1 server into the workspace.").to_string()
        } else {
            trf!("Imported {n} servers into the workspace.", n = n)
        };
        Ok(self.bring_passwords(imported, super::panel::PendingCopy { keys, known }))
    }

    /// Registra no workspace o servidor conectado agora, com a senha em uso.
    pub fn add_current_connection_to_workspace(&mut self) -> Result<String, String> {
        let Some(settings) = self.connection.as_ref().map(|c| c.settings.clone()) else {
            return Err(tr("Not connected.").into());
        };
        let key = server_key(&settings);
        let known = self.current_password().into_iter().collect();
        let Some(panel) = self.workspace_panel.servers_mut() else {
            return Err(tr("No workspace is open.").into());
        };
        if find_server_by_key(panel.servers().root(), &key).is_some() {
            return Err(tr("This connection is already in the workspace.").into());
        }
        let taken: Vec<String> = panel
            .servers()
            .root()
            .servers
            .iter()
            .map(|s| s.name.clone())
            .collect();
        let host = settings.server_name();
        let name = [host.clone(), format!("{host} ({})", settings.user)]
            .into_iter()
            .find(|n| !taken.iter().any(|t| t.eq_ignore_ascii_case(n)))
            .unwrap_or_else(|| key.clone());
        let server = registered::RegisteredServer {
            name: name.clone(),
            settings,
            ..Default::default()
        };
        let group = registered::ServerGroup {
            servers: vec![server],
            ..Default::default()
        };
        panel.servers_mut().import_tree(&[], group)?;
        let added = trf!("Added {name} to the workspace.", name = name);
        Ok(self.bring_passwords(
            added,
            super::panel::PendingCopy {
                keys: vec![key],
                known,
            },
        ))
    }

    /// Leva as senhas para o cofre do workspace: copia já, se ele está
    /// destrancado; se não, pede a senha mestra e copia depois. Devolve a
    /// mensagem, começando por `done`.
    fn bring_passwords(&mut self, done: String, pending: super::panel::PendingCopy) -> String {
        let (exists, unlocked) = self
            .workspace_panel
            .passwords()
            .map_or((false, false), |p| (p.exists(), p.is_unlocked()));
        match (&self.secrets, unlocked) {
            (Some(store), true) => {
                self.workspace_panel.copy_passwords(pending, store.clone());
                trf!(
                    "{imported} Copying their saved passwords to the workspace passwords...",
                    imported = done
                )
            }
            _ => {
                self.workspace_panel.copy_when_unlocked(pending);
                if let Some(p) = self.workspace_panel.passwords_mut()
                    && p.dialog().is_none()
                    && !p.is_busy()
                {
                    p.open_dialog(if exists {
                        super::passwords::DialogKind::Unlock
                    } else {
                        super::passwords::DialogKind::Create
                    });
                }
                trf!(
                    "{imported} Turn on or unlock the workspace passwords to bring their saved passwords too.",
                    imported = done
                )
            }
        }
    }

    /// Uma vez por frame: trata os pedidos e os diálogos do Workspace
    /// Explorer.
    pub(crate) fn workspace_step(&mut self, ctx: &eframe::egui::Context, actions: Vec<Action>) {
        if let Some(panel) = self.workspace_panel.servers_mut() {
            panel.current = self.connection.as_ref().map(|c| c.settings.clone());
        }
        self.workspace_panel.importable = self.importable_connections();
        self.workspace_panel.unsaved_current = self.connection.as_ref().and_then(|c| {
            let ours = self.workspace_panel.servers()?.servers().root();
            find_server_by_key(ours, &server_key(&c.settings))
                .is_none()
                .then(|| c.settings.server_name())
        });
        if self.workspace_panel.take_accepted_import() {
            self.status = match self.import_registered_into_workspace() {
                Ok(s) | Err(s) => s,
            };
        }
        if let Some(pending) = self.workspace_panel.take_pending_copy()
            && let Some(store) = self.secrets.clone()
        {
            self.workspace_panel.copy_passwords(pending, store);
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
                    let nothing =
                        self.importable_connections() == 0 && self.local_connections().1.is_empty();
                    self.status = match self.import_registered_into_workspace() {
                        Ok(s) | Err(s) => s,
                    };
                    // Nada a importar: em vez de só uma frase na status
                    // bar, abre o registro de um servidor no workspace.
                    if nothing {
                        self.new_workspace_server();
                    }
                }
                Action::AddCurrent => {
                    self.status = match self.add_current_connection_to_workspace() {
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
                    self.ask_path(Question::Open, move |app, _, path| {
                        let Some(path) = path else {
                            return;
                        };
                        let Some(panel) = app.workspace_panel.servers_mut() else {
                            return;
                        };
                        app.status = match panel.servers_mut().import(&group, &path) {
                            Ok(1) => tr("Imported 1 server into the workspace.").to_string(),
                            Ok(n) => trf!("Imported {n} servers into the workspace.", n = n),
                            Err(e) => e,
                        };
                    });
                }
                Action::Servers(registered::Action::Export(group)) => {
                    let question = Question::save("RegisteredServers.json");
                    self.ask_path(question, move |app, _, path| {
                        let Some(path) = path else {
                            return;
                        };
                        let Some(panel) = app.workspace_panel.servers() else {
                            return;
                        };
                        app.status = match panel.servers().export(&group, &path) {
                            Ok(()) => trf!(
                                "Exported servers to {path} (passwords are not exported).",
                                path = path.display()
                            ),
                            Err(e) => e,
                        };
                    });
                }
            }
        }
        // Só procura a IA quando o diálogo de Commit está aberto.
        let ai = self
            .workspace_panel
            .source_control()
            .is_some_and(|g| g.has_dialog())
            .then(|| self.ai.backend())
            .flatten();
        if let Some(status) =
            self.workspace_panel
                .show_windows(ctx, self.secrets.as_ref(), ai.as_ref())
        {
            self.status = status;
        }
        if let Some(url) = self.workspace_panel.take_clone_request() {
            let question = Question::Folder {
                title: tr("Clone Workspace into").into(),
            };
            self.ask_path(question, move |app, _, parent| {
                if let Some(parent) = parent {
                    app.clone_workspace(&url, &parent);
                }
            });
        }
        if let Some(result) = self.workspace_panel.take_cloned() {
            self.finish_clone(result);
        }
    }
}
