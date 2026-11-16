//! Vários servidores no Object Explorer ao mesmo tempo.
//!
//! A casca trabalha sempre com um servidor "ativo": a árvore, a conexão, a
//! senha e os catálogos do IntelliSense dele ficam nos campos de sempre do
//! [`ShellApp`]. Os outros servidores conectados ficam
//! guardados aqui, cada um com o seu estado, e trocam de lugar com o ativo
//! quando o usuário mexe na árvore deles ou volta para uma janela de
//! consulta aberta neles. Assim o resto da casca não precisa saber que há
//! mais de um servidor.

use eframe::egui;
use pgstudio_core::connection::{Connected, ConnectionSettings};
use pgstudio_i18n::{tr, trf};
use pgstudio_object_explorer::{Event, ObjectExplorer};

use crate::intellisense::Catalogs;

/// Identifica um servidor conectado: usuário, servidor e porta (o banco não
/// conta, como no SSMS).
pub fn server_key(settings: &ConnectionSettings) -> String {
    crate::secrets::password_key(settings)
}

/// Um servidor conectado que não é o ativo.
#[derive(Default)]
pub struct Parked {
    /// Posição na lista do Object Explorer (ordem de conexão).
    pub order: u32,
    pub explorer: ObjectExplorer,
    pub connection: Option<Connected>,
    pub password: String,
    pub catalogs: Catalogs,
}

impl Parked {
    /// Chave do servidor, se conectado.
    pub fn key(&self) -> Option<String> {
        self.connection.as_ref().map(|c| server_key(&c.settings))
    }
}

/// Os servidores guardados e a posição do ativo.
#[derive(Default)]
pub struct Servers {
    pub parked: Vec<Parked>,
    /// Posição do servidor ativo na lista.
    pub active_order: u32,
    next_order: u32,
    /// Servidores registrados a conectar um depois do outro.
    queue: std::collections::VecDeque<ConnectionSettings>,
}

impl Servers {
    /// Posição para um servidor novo, no fim da lista.
    pub fn next_order(&mut self) -> u32 {
        self.next_order += 1;
        self.next_order
    }

    /// Índice do servidor guardado com essa chave.
    pub fn find(&self, key: &str) -> Option<usize> {
        self.parked
            .iter()
            .position(|p| p.key().as_deref() == Some(key))
    }

    /// Quantos servidores há além do ativo.
    pub fn len(&self) -> usize {
        self.parked.len()
    }

    /// Se só há o servidor ativo.
    pub fn is_empty(&self) -> bool {
        self.parked.is_empty()
    }

    /// Se ainda há servidores na fila para conectar.
    pub fn has_queue(&self) -> bool {
        !self.queue.is_empty()
    }
}

/// O que aconteceu ao desenhar as árvores.
#[derive(Default)]
pub struct Shown {
    /// Pedidos do menu de contexto (já do servidor que ficará ativo).
    pub events: Vec<Event>,
    /// Índice em [`Servers::parked`] do servidor que o usuário usou e que
    /// deve virar o ativo.
    pub activate: Option<usize>,
    /// Pedidos das linhas dos servidores guardados (conectar, nova consulta).
    pub actions: Vec<Action>,
}

/// Uma linha do Object Explorer: uma pasta de conexões, um servidor
/// guardado ainda não conectado ou uma linha da árvore de objetos de um
/// servidor conectado.
enum Line {
    Group(Vec<usize>, usize),
    Saved(crate::registered::NodePath, usize),
    Object {
        /// `None`: o servidor ativo; `Some(i)`: o guardado `i`.
        slot: Option<usize>,
        row: usize,
        depth: usize,
    },
}

/// Desenha o Object Explorer: as pastas de conexões de `saved` (as do
/// workspace aberto ou as dos Registered Servers), com os servidores
/// conectados dentro da pasta deles, cada um com a árvore de objetos, e os
/// desconectados numa linha que conecta com duplo clique. Os servidores
/// conectados que não estão em pasta nenhuma ficam no fim, na ordem em que
/// foram conectados. Tudo numa lista só, com uma rolagem só.
pub fn show(
    ui: &mut egui::Ui,
    active: &mut ObjectExplorer,
    active_key: Option<&str>,
    servers: &mut Servers,
    saved: &mut RegisteredPanel,
    store: Option<&Arc<dyn SecretStore>>,
) -> Shown {
    let mut shown = Shown::default();
    // "Move To" no nó de cada servidor conectado: as pastas, com a raiz.
    let targets: Vec<String> = saved
        .servers()
        .all_groups()
        .into_iter()
        .map(|(path, depth, name)| {
            if path.is_empty() {
                // A raiz: fora de qualquer pasta.
                return tr("(no folder)").to_string();
            }
            format!(
                "{}{}",
                "    ".repeat(depth - 1),
                crate::registered::display_name(&name)
            )
        })
        .collect();
    let targets = if targets.len() < 2 {
        Vec::new()
    } else {
        targets
    };
    active.begin(ui.ctx());
    active.set_move_targets(targets.clone());
    for p in &mut servers.parked {
        p.explorer.begin(ui.ctx());
        p.explorer.set_move_targets(targets.clone());
    }

    // Os servidores conectados, na ordem de conexão.
    let mut slots: Vec<(u32, Option<usize>, Option<String>)> = servers
        .parked
        .iter()
        .enumerate()
        .map(|(i, p)| (p.order, Some(i), p.key()))
        .collect();
    if active.is_connected() {
        slots.push((servers.active_order, None, active_key.map(str::to_string)));
    }
    slots.sort_by_key(|(o, ..)| *o);

    let tree = saved.tree_rows();
    if slots.is_empty() && tree.is_empty() {
        ui.weak(tr("Not connected. Use File > Connect Object Explorer."));
        return shown;
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut placed = vec![false; slots.len()];
    let objects = |slot: Option<usize>,
                   depth: usize,
                   lines: &mut Vec<Line>,
                   active: &ObjectExplorer,
                   servers: &Servers| {
        let rows = match slot {
            None => active.visible_rows().len(),
            Some(i) => servers.parked[i].explorer.visible_rows().len(),
        };
        lines.extend((0..rows).map(|row| Line::Object { slot, row, depth }));
    };
    for row in tree {
        match row {
            TreeRow::Group { groups, depth } => lines.push(Line::Group(groups, depth)),
            TreeRow::Server { path, depth } => {
                let key = saved
                    .servers()
                    .server(&path)
                    .map(|s| server_key(&s.settings));
                let connected = slots
                    .iter()
                    .enumerate()
                    .position(|(i, s)| !placed[i] && s.2.is_some() && s.2 == key);
                match connected {
                    Some(i) => {
                        placed[i] = true;
                        objects(slots[i].1, depth, &mut lines, active, servers);
                    }
                    None => lines.push(Line::Saved(path, depth)),
                }
            }
        }
    }
    // Os conectados que não estão em pasta nenhuma. Os que estão numa
    // pasta fechada ficam escondidos com ela.
    let filed: Vec<String> = saved
        .servers()
        .all_servers()
        .into_iter()
        .map(|s| server_key(&s.settings))
        .collect();
    for (i, slot) in slots.iter().enumerate() {
        let hidden = slot.2.as_ref().is_some_and(|k| filed.contains(k));
        if !placed[i] && !hidden {
            objects(slot.1, 0, &mut lines, active, servers);
        }
    }

    const ROW: f32 = 20.0;
    let mut menu = TreeMenu::default();
    egui::ScrollArea::both()
        .auto_shrink(false)
        .show_rows(ui, ROW, lines.len(), |ui, range| {
            let mut i = range.start;
            while i < range.end {
                match &lines[i] {
                    Line::Group(groups, depth) => {
                        saved.show_group_row(ui, groups, *depth, &mut menu);
                        i += 1;
                    }
                    Line::Saved(path, depth) => {
                        saved.show_server_row(ui, path, *depth, &mut shown.actions, &mut menu);
                        i += 1;
                    }
                    Line::Object { slot, row, depth } => {
                        // As linhas seguidas do mesmo servidor saem juntas.
                        let (slot, depth) = (*slot, *depth);
                        let mut end = i + 1;
                        while end < range.end
                            && matches!(&lines[end], Line::Object { slot: s, .. } if *s == slot)
                        {
                            end += 1;
                        }
                        let rows = *row..*row + (end - i);
                        match slot {
                            None => shown.events.extend(active.show_range(ui, rows, depth)),
                            Some(p) => {
                                let explorer = &mut servers.parked[p].explorer;
                                let before = explorer.selected().map(|n| n.id.clone());
                                let events = explorer.show_range(ui, rows, depth);
                                let after = explorer.selected().map(|n| n.id.clone());
                                if !events.is_empty() || (after.is_some() && before != after) {
                                    shown.activate = Some(p);
                                    shown.events = events;
                                }
                            }
                        }
                        i = end;
                    }
                }
            }
        });
    saved.apply_tree_menu(menu, store);
    shown
}

use std::sync::Arc;

use crate::ShellApp;
use crate::connect::ConnectDialog;
use crate::panels::Panel;
use crate::registered::{Action, RegisteredPanel, RegisteredServers, TreeMenu, TreeRow};
use crate::secrets::SecretStore;

impl ShellApp {
    /// Usa este cofre para "Remember password" (o aplicativo usa o do
    /// sistema; sem cofre, a senha é sempre pedida).
    pub fn with_secret_store(mut self, store: Arc<dyn SecretStore>) -> Self {
        // O cofre do workspace aberto vem antes deste, para os servidores
        // do workspace.
        self.secrets = Some(Arc::new(crate::repo::passwords::WorkspaceSecrets::new(
            Some(store),
            self.workspace_panel.vault_slot(),
        )));
        if let Some(dialog) = self.connect_dialog.take() {
            let settings = dialog.settings().unwrap_or_default();
            self.connect_dialog =
                Some(ConnectDialog::new(&settings).with_store(self.secrets.clone()));
        }
        self
    }

    /// Usa esta árvore de servidores registrados (o aplicativo lê de
    /// `registered_servers.json`).
    pub fn with_registered_servers(mut self, servers: RegisteredServers) -> Self {
        self.registered = RegisteredPanel::new(servers);
        self
    }

    /// Abre a janela Registered Servers (Ctrl+Alt+G).
    pub fn show_registered_servers(&mut self) {
        self.workspace.open(Panel::RegisteredServers);
    }

    /// Janela Registered Servers.
    pub fn registered(&self) -> &RegisteredPanel {
        &self.registered
    }

    /// Janela Registered Servers, mutável.
    pub fn registered_mut(&mut self) -> &mut RegisteredPanel {
        &mut self.registered
    }

    /// Servidores conectados no Object Explorer, na ordem da árvore, com o
    /// ativo marcado.
    pub fn connected_servers(&self) -> Vec<(String, bool)> {
        let mut list: Vec<(u32, String, bool)> = self
            .servers
            .parked
            .iter()
            .filter_map(|p| {
                Some((
                    p.order,
                    p.connection.as_ref()?.settings.server_name(),
                    false,
                ))
            })
            .collect();
        if let Some(c) = &self.connection {
            list.push((self.servers.active_order, c.settings.server_name(), true));
        }
        list.sort_by_key(|(o, ..)| *o);
        list.into_iter().map(|(_, n, a)| (n, a)).collect()
    }

    /// Conecta a um servidor registrado: com a senha do cofre, direto; sem
    /// ela, abre o "Connect to Server" preenchido pedindo a senha. Com
    /// `new_query`, abre uma janela de consulta ao conectar.
    pub fn connect_registered(&mut self, settings: &crate::ConnectionSettings, new_query: bool) {
        if self.connector.is_none() {
            self.status = tr("Registered servers need a real connection.").to_string();
            return;
        }
        let key = server_key(settings);
        if self.connection.as_ref().map(|c| server_key(&c.settings)) == Some(key.clone())
            || self.servers.find(&key).is_some()
        {
            // Já conectado: só traz o servidor para a frente.
            self.activate_key(&key);
            self.workspace.open(Panel::ObjectExplorer);
            if new_query {
                self.new_query_on_connect = Some(key);
            }
            return;
        }
        if self
            .connect_dialog
            .as_ref()
            .is_some_and(ConnectDialog::is_connecting)
        {
            self.status = tr("Wait for the current connection attempt.").to_string();
            return;
        }
        self.connect_dialog = Some(
            ConnectDialog::new(settings)
                .with_store(self.secrets.clone())
                .connect_when_found(),
        );
        self.new_query_on_connect = new_query.then_some(key);
    }

    /// "Register..." e "Properties" no servidor do Object Explorer: o
    /// registro da conexão ativa (a árvore que recebeu o clique), onde ficam
    /// o nome, a pasta e o ambiente dela. Com um workspace aberto, o registro
    /// é o do workspace, para ir ao repositório; sem um, o dos Registered
    /// Servers. Um servidor já registrado abre para editar; um novo abre o
    /// "New Server Registration" preenchido, com a senha em uso.
    pub fn register_active_server(&mut self) {
        let Some(settings) = self.connection.as_ref().map(|c| c.settings.clone()) else {
            self.status = tr("Only a real connection can be registered.").to_string();
            return;
        };
        let key = server_key(&settings);
        let password = self.password.clone();
        let in_workspace = self.workspace_panel.workspace().is_some();
        let panel = if in_workspace {
            self.workspace.open(Panel::WorkspaceExplorer);
            self.workspace_panel
                .register_or_edit(&settings, self.secrets.as_ref())
        } else {
            self.workspace.open(Panel::RegisteredServers);
            let store = self.secrets.clone();
            self.registered.register_or_edit(&settings, store.as_ref());
            Some(&mut self.registered)
        };
        let Some(panel) = panel else {
            return;
        };
        let editing = panel.servers().path_for_key(&key).is_some();
        if !editing
            && !password.is_empty()
            && let Some(d) = panel.server_dialog_mut()
        {
            d.password = password;
            d.remember = true;
        }
        self.status = match (in_workspace, editing) {
            (true, true) => tr("This server is registered in the workspace: edit its name, environment and description here."),
            (true, false) => tr("Register the server in the workspace: choose its name, folder and environment."),
            (false, true) => tr("This server is registered on this computer: edit its name, environment and description here."),
            (false, false) => tr("Register the server on this computer: choose its name, folder and environment."),
        }
        .to_string();
    }

    /// "Move To" no servidor do Object Explorer: põe o registro da conexão
    /// ativa na pasta de índice `index` (a ordem de `all_groups`). Um
    /// servidor ainda não registrado abre o registro com a pasta escolhida.
    pub fn move_active_server_to_folder(&mut self, index: usize) {
        let Some(settings) = self.connection.as_ref().map(|c| c.settings.clone()) else {
            self.status = tr("Only a real connection can be registered.").to_string();
            return;
        };
        let key = server_key(&settings);
        let password = self.password.clone();
        let secrets = self.secrets.clone();
        let (panel, store) = match self.workspace_panel.saved_servers(secrets.as_ref()) {
            Some(found) => found,
            None => (&mut self.registered, secrets),
        };
        let Some((groups, ..)) = panel.servers().all_groups().into_iter().nth(index) else {
            return;
        };
        match panel.servers().path_for_key(&key) {
            Some(path) => {
                let message = panel.move_node(&path, &groups);
                if !message.is_empty() {
                    self.status = message;
                }
            }
            None => {
                panel.register_in(&settings, &groups, store.as_ref());
                if !password.is_empty()
                    && let Some(d) = panel.server_dialog_mut()
                {
                    d.password = password;
                    d.remember = true;
                }
            }
        }
        self.sync_server_labels();
    }

    /// Tasks > Import... num grupo do Registered Servers.
    fn import_registered(&mut self, group: &[usize]) {
        let group = group.to_vec();
        self.ask_path(crate::file_dialogs::Question::Open, move |app, _, path| {
            let Some(path) = path else {
                return;
            };
            app.status = match app.registered.servers_mut().import(&group, &path) {
                Ok(1) => tr("Imported 1 registered server.").to_string(),
                Ok(n) => trf!("Imported {n} registered servers.", n = n),
                Err(e) => e,
            };
        });
    }

    /// Tasks > Export... num grupo do Registered Servers (sem senhas).
    fn export_registered(&mut self, group: &[usize]) {
        let group = group.to_vec();
        let question = crate::file_dialogs::Question::save("RegisteredServers.json");
        self.ask_path(question, move |app, _, path| {
            let Some(path) = path else {
                return;
            };
            app.status = match app.registered.servers().export(&group, &path) {
                Ok(()) => trf!(
                    "Exported registered servers to {path} (passwords are not exported).",
                    path = path.display()
                ),
                Err(e) => e,
            };
        });
    }

    /// Servidor de cada janela de consulta, como "servidor (usuário)",
    /// para a faixa de status dela. Uma janela cujo servidor já foi
    /// desconectado mostra a chave dele.
    pub fn document_servers(&self) -> std::collections::HashMap<u32, String> {
        let mut labels = std::collections::HashMap::new();
        let connections = self.connection.iter().chain(
            self.servers
                .parked
                .iter()
                .filter_map(|p| p.connection.as_ref()),
        );
        for c in connections {
            labels.insert(
                server_key(&c.settings),
                format!(
                    "{} ({})",
                    c.settings.server_name(),
                    c.settings.login_label()
                ),
            );
        }
        self.doc_servers
            .iter()
            .map(|(doc, key)| {
                (
                    *doc,
                    labels.get(key).cloned().unwrap_or_else(|| key.clone()),
                )
            })
            .collect()
    }

    /// As conexões guardadas, para o "Connect to Server": as do workspace
    /// aberto primeiro, depois as dos Registered Servers que não estão lá.
    pub fn saved_connections(&self) -> Vec<crate::connect::SavedConnection> {
        let mut out: Vec<crate::connect::SavedConnection> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        let workspace = self.workspace_panel.servers().map(|p| p.servers());
        let sources = workspace
            .into_iter()
            .map(|r| (r, true))
            .chain([(self.registered.servers(), false)]);
        for (servers, workspace) in sources {
            for (path, folder, s) in servers.servers_with_folders() {
                let key = server_key(&s.settings);
                if keys.contains(&key) {
                    continue;
                }
                keys.push(key);
                out.push(crate::connect::SavedConnection {
                    name: s.name.clone(),
                    folder,
                    environment: servers
                        .node_tag(&path)
                        .and_then(|t| Some((t.environment?, t.color))),
                    workspace,
                    settings: s.settings.clone(),
                });
            }
        }
        out
    }

    /// Ambiente e cor do servidor com esta chave (`usuario@host:porta`),
    /// vindos do workspace aberto ou, sem ele lá, dos Registered Servers.
    pub fn server_tag(&self, key: &str) -> Option<crate::registered::ServerTag> {
        self.workspace_panel
            .servers()
            .and_then(|p| p.servers().tag_for_key(key))
            .or_else(|| self.registered.servers().tag_for_key(key))
    }

    /// O nome com que o servidor desta chave está registrado: no workspace
    /// aberto ou, se não está lá, nos Registered Servers.
    pub fn registered_name(&self, key: &str) -> Option<String> {
        self.workspace_panel
            .servers()
            .and_then(|p| p.servers().name_for_key(key))
            .or_else(|| self.registered.servers().name_for_key(key))
            .map(str::to_string)
    }

    /// Como um servidor conectado aparece no Object Explorer quando está
    /// registrado: o nome do registro e o ambiente dele.
    pub fn server_label(
        &self,
        settings: &ConnectionSettings,
    ) -> Option<pgstudio_object_explorer::ServerLabel> {
        let key = server_key(settings);
        let name = self.registered_name(&key)?;
        let environment = self
            .server_tag(&key)
            .and_then(|t| Some((t.environment?, t.color.0)));
        Some(pgstudio_object_explorer::ServerLabel { name, environment })
    }

    /// Atualiza o nó do servidor de cada árvore do Object Explorer (a ativa
    /// e as outras conectadas) com o registro dele.
    pub(crate) fn sync_server_labels(&mut self) {
        let active = self
            .connection
            .as_ref()
            .and_then(|c| self.server_label(&c.settings));
        self.explorer.set_server_label(active);
        let labels: Vec<_> = self
            .servers
            .parked
            .iter()
            .map(|p| {
                p.connection
                    .as_ref()
                    .and_then(|c| self.server_label(&c.settings))
            })
            .collect();
        for (p, label) in self.servers.parked.iter_mut().zip(labels) {
            p.explorer.set_server_label(label);
        }
    }

    /// Ambiente e cor de cada janela de consulta, para a faixa de status
    /// e a barra colorida dela.
    pub fn document_tags(&self) -> std::collections::HashMap<u32, crate::registered::ServerTag> {
        self.doc_servers
            .iter()
            .filter_map(|(doc, key)| Some((*doc, self.server_tag(key)?)))
            .collect()
    }

    /// Ambiente e cor de onde o usuário está: o servidor da janela de
    /// consulta ativa ou, sem ela, o servidor ativo do Object Explorer.
    pub fn active_tag(&self) -> Option<crate::registered::ServerTag> {
        let key = self
            .active_doc
            .and_then(|id| self.doc_servers.get(&id).cloned())
            .or_else(|| self.connection.as_ref().map(|c| server_key(&c.settings)))?;
        self.server_tag(&key)
    }

    /// Conecta este servidor quando as conexões em andamento terminarem,
    /// como um duplo clique no Registered Servers (linha de comando e
    /// screenshots).
    pub fn queue_registered_connect(&mut self, settings: ConnectionSettings) {
        self.servers.queue.push_back(settings);
    }

    /// Antes de pôr uma conexão nova como ativa: se o servidor já está
    /// conectado, ele vira o ativo (e a conexão nova o substitui); se não,
    /// o ativo atual vai para a lista dos guardados.
    pub(crate) fn make_room_for(&mut self, settings: &crate::ConnectionSettings) {
        let key = server_key(settings);
        if let Some(i) = self.servers.find(&key) {
            self.activate_parked(i);
            return;
        }
        let Some(active) = &self.connection else {
            return;
        };
        if server_key(&active.settings) == key {
            return;
        }
        let order = self.servers.active_order;
        let parked = Parked {
            order,
            explorer: std::mem::take(&mut self.explorer),
            connection: self.connection.take(),
            password: std::mem::take(&mut self.password),
            catalogs: std::mem::take(&mut self.catalogs),
        };
        self.servers.parked.push(parked);
        self.servers.active_order = self.servers.next_order();
    }

    /// Depois de desconectar o ativo, o primeiro dos guardados assume.
    pub(crate) fn promote_next_server(&mut self) {
        if self.servers.is_empty() {
            return;
        }
        let first = (0..self.servers.len())
            .min_by_key(|&i| self.servers.parked[i].order)
            .expect("not empty");
        let p = self.servers.parked.remove(first);
        self.servers.active_order = p.order;
        self.explorer = p.explorer;
        self.connection = p.connection;
        self.password = p.password;
        self.catalogs = p.catalogs;
    }

    /// Troca o servidor ativo pelo guardado `i`.
    pub(crate) fn activate_parked(&mut self, i: usize) {
        let Some(p) = self.servers.parked.get_mut(i) else {
            return;
        };
        std::mem::swap(&mut p.order, &mut self.servers.active_order);
        std::mem::swap(&mut p.explorer, &mut self.explorer);
        std::mem::swap(&mut p.connection, &mut self.connection);
        std::mem::swap(&mut p.password, &mut self.password);
        std::mem::swap(&mut p.catalogs, &mut self.catalogs);
        if self.connection.is_none() {
            // O ativo não tinha conexão (demonstração): não fica guardado.
            self.servers.parked.remove(i);
        }
    }

    /// Torna ativo o servidor com essa chave, se conectado. Devolve se ele
    /// é o ativo agora.
    pub fn activate_key(&mut self, key: &str) -> bool {
        if self
            .connection
            .as_ref()
            .map(|c| server_key(&c.settings))
            .as_deref()
            == Some(key)
        {
            return true;
        }
        match self.servers.find(key) {
            Some(i) => {
                self.activate_parked(i);
                true
            }
            None => false,
        }
    }

    /// Antes de executar uma janela de consulta: o servidor dela vira o
    /// ativo, se ainda estiver conectado.
    pub(crate) fn activate_server_of(&mut self, doc: u32) {
        if let Some(key) = self.doc_servers.get(&doc).cloned() {
            self.activate_key(&key);
        }
    }

    /// Uma vez por frame: liga as janelas de consulta novas ao servidor
    /// ativo, troca o ativo quando o usuário volta para uma janela de outro
    /// servidor, trata os pedidos e os diálogos do Registered Servers.
    pub(crate) fn servers_step(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        let active_key = self.connection.as_ref().map(|c| server_key(&c.settings));
        let ids = self.documents.ids().to_vec();
        self.doc_servers.retain(|id, _| ids.contains(id));
        if let Some(key) = &active_key {
            for id in ids {
                self.doc_servers.entry(id).or_insert_with(|| key.clone());
            }
        }
        if self.active_doc != self.last_active_doc {
            self.last_active_doc = self.active_doc;
            if let Some(id) = self.active_doc {
                self.activate_server_of(id);
            }
        }
        if let Some(key) = self.new_query_on_connect.clone()
            && self.connect_dialog.is_none()
        {
            self.new_query_on_connect = None;
            if self.activate_key(&key) {
                self.execute(ctx, pgstudio_core::CommandId::NewQuery);
            }
        }

        // A fila espera o cofre do workspace destrancar: é lá que podem
        // estar as senhas dos servidores que a sessão vai reconectar.
        let unlocking = self
            .workspace_panel
            .passwords()
            .is_some_and(|p| p.is_busy());
        if self.connect_dialog.is_none()
            && !unlocking
            && let Some(next) = self.servers.queue.pop_front()
        {
            self.connect_registered(&next, false);
        }
        self.registered.current = self.connection.as_ref().map(|c| c.settings.clone());
        for action in actions {
            match action {
                Action::ConnectExplorer(s) => self.connect_registered(&s, false),
                Action::NewQuery(s) => self.connect_registered(&s, true),
                Action::Import(group) => self.import_registered(&group),
                Action::Export(group) => self.export_registered(&group),
            }
        }
        if let Some(status) = self.registered.show_windows(ctx) {
            self.status = status;
        }
    }
}
