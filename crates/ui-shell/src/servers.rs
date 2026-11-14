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
}

/// O que aconteceu ao desenhar as árvores.
#[derive(Default)]
pub struct Shown {
    /// Pedidos do menu de contexto (já do servidor que ficará ativo).
    pub events: Vec<Event>,
    /// Índice em [`Servers::parked`] do servidor que o usuário usou e que
    /// deve virar o ativo.
    pub activate: Option<usize>,
}

/// Desenha a árvore do servidor ativo e as dos guardados, uma embaixo da
/// outra na ordem em que foram conectados, dividindo a altura do painel.
pub fn show(ui: &mut egui::Ui, active: &mut ObjectExplorer, servers: &mut Servers) -> Shown {
    if servers.parked.is_empty() {
        return Shown {
            events: active.show(ui),
            activate: None,
        };
    }
    let mut order: Vec<(u32, Option<usize>)> = servers
        .parked
        .iter()
        .enumerate()
        .map(|(i, p)| (p.order, Some(i)))
        .collect();
    order.push((servers.active_order, None));
    order.sort_by_key(|(o, _)| *o);

    // Cada árvore ocupa a altura das linhas dela, como uma árvore só; se
    // não couber, a altura é dividida na proporção das linhas.
    const ROW: f32 = 20.0;
    let available = ui.available_height();
    let gap = ui.spacing().item_spacing.y;
    let wanted: Vec<f32> = order
        .iter()
        .map(|(_, slot)| {
            let rows = match slot {
                None => active.visible_rows().len(),
                Some(i) => servers.parked[*i].explorer.visible_rows().len(),
            };
            rows.max(1) as f32 * ROW + 4.0
        })
        .collect();
    let total: f32 = wanted.iter().sum::<f32>() + gap * (wanted.len() - 1) as f32;
    let heights: Vec<f32> = if total <= available {
        // Sobra espaço: a última árvore fica com ele.
        let mut h = wanted.clone();
        if let Some(last) = h.last_mut() {
            *last += available - total;
        }
        h
    } else {
        let room = available - gap * (wanted.len() - 1) as f32;
        let sum: f32 = wanted.iter().sum();
        wanted
            .iter()
            .map(|w| (w / sum * room).max(3.0 * ROW))
            .collect()
    };
    let width = ui.available_width();
    let mut shown = Shown::default();
    for ((o, slot), height) in order.into_iter().zip(heights) {
        // Cada árvore desenha num retângulo próprio, recortado: a área de
        // rolagem do Object Explorer tem altura mínima e não pode empurrar a
        // árvore seguinte.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("explorer_server", o))
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        match slot {
            None => shown.events.extend(active.show(&mut child)),
            Some(i) => {
                let explorer = &mut servers.parked[i].explorer;
                let before = explorer.selected().map(|n| n.id.clone());
                let events = explorer.show(&mut child);
                let after = explorer.selected().map(|n| n.id.clone());
                if !events.is_empty() || (after.is_some() && before != after) {
                    shown.activate = Some(i);
                    shown.events = events;
                }
            }
        }
    }
    shown
}

use std::sync::Arc;

use crate::ShellApp;
use crate::connect::ConnectDialog;
use crate::panels::Panel;
use crate::registered::{Action, RegisteredPanel, RegisteredServers};
use crate::secrets::SecretStore;

impl ShellApp {
    /// Usa este cofre para "Remember password" (o aplicativo usa o do
    /// sistema; sem cofre, a senha é sempre pedida).
    pub fn with_secret_store(mut self, store: Arc<dyn SecretStore>) -> Self {
        self.secrets = Some(store);
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
            self.status = "Registered servers need a real connection.".to_string();
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
            self.status = "Wait for the current connection attempt.".to_string();
            return;
        }
        self.connect_dialog = Some(
            ConnectDialog::new(settings)
                .with_store(self.secrets.clone())
                .connect_when_found(),
        );
        self.new_query_on_connect = new_query.then_some(key);
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

        if self.connect_dialog.is_none()
            && let Some(next) = self.servers.queue.pop_front()
        {
            self.connect_registered(&next, false);
        }
        self.registered.current = self.connection.as_ref().map(|c| c.settings.clone());
        for action in actions {
            match action {
                Action::ConnectExplorer(s) => self.connect_registered(&s, false),
                Action::NewQuery(s) => self.connect_registered(&s, true),
            }
        }
        if let Some(status) = self.registered.show_windows(ctx) {
            self.status = status;
        }
    }
}
