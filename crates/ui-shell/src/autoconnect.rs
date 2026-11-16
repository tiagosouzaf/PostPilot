//! Conexões em segundo plano.
//!
//! Ao abrir o aplicativo (e ao abrir um workspace), os servidores guardados
//! que têm a senha lembrada conectam sozinhos, sem diálogo e sem segurar a
//! janela: a senha é lida do cofre e a conexão é feita em threads, poucas de
//! cada vez ([`MAX_PARALLEL`]). Os servidores da sessão anterior passam pelo
//! mesmo caminho. O Object Explorer mostra o estado de cada um numa bolinha
//! ([`LinkState`]): quem conectou entra na árvore; quem falhou fica com o
//! erro na dica e tenta de novo com um duplo clique.
//!
//! Conectar só abre a sessão: nada é executado, então as regras de
//! confirmação dos ambientes (Produção) continuam valendo como sempre.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings};
use pgstudio_i18n::{tr, trf};
use pgstudio_object_explorer::ObjectExplorer;

use crate::ShellApp;
use crate::panels::Panel;
use crate::secrets::{self, Lookup};
use crate::servers::{Parked, server_key};

/// Quantos servidores conectam ao mesmo tempo.
pub const MAX_PARALLEL: usize = 4;

/// Quanto esperar por um servidor antes de desistir dele. Os drivers já
/// desistem antes (15 s, mais o túnel SSH); este limite é a garantia de que
/// um servidor mudo não segura a vez dos outros.
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(60);

/// Estado de um servidor guardado que não está conectado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// Na fila para conectar.
    Queued,
    /// Lendo a senha ou conectando.
    Connecting,
    /// Não conectou: a mensagem do erro.
    Failed(String),
    /// Não há senha guardada: conecta com um duplo clique, pedindo a senha.
    NoPassword,
}

enum Stage {
    Waiting,
    Lookup(Lookup),
    Connecting {
        rx: Receiver<Result<Connected, ConnectError>>,
        password: String,
        since: Instant,
    },
}

struct Job {
    key: String,
    settings: ConnectionSettings,
    /// Veio da sessão anterior: sem senha guardada, o diálogo pede.
    session: bool,
    stage: Stage,
}

/// O que uma tentativa deu.
enum Outcome {
    Connected(Box<Connected>, String),
    Failed(String),
    NoPassword,
}

/// As conexões em segundo plano e o estado de cada servidor guardado.
#[derive(Default)]
pub struct Background {
    /// Conectar os servidores guardados ao abrir (o aplicativo liga; os
    /// testes escolhem).
    pub(crate) scan: bool,
    jobs: VecDeque<Job>,
    states: HashMap<String, LinkState>,
    /// Árvores de servidores já percorridas (a pasta do workspace, ou vazio
    /// para os Registered Servers).
    scanned: HashSet<String>,
    /// Quantos cofres de workspace estavam destrancados.
    unlocked: usize,
}

impl Background {
    /// Estado do servidor com esta chave, se ele não está conectado e já
    /// teve (ou espera) a vez.
    pub fn state(&self, key: &str) -> Option<&LinkState> {
        self.states.get(key)
    }

    /// Se há servidores na fila ou conectando.
    pub fn is_busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// Se há servidores da sessão anterior na fila ou conectando.
    pub(crate) fn is_restoring(&self) -> bool {
        self.jobs.iter().any(|j| j.session)
    }

    /// Põe o servidor na fila. Devolve se entrou (não estava nela).
    pub(crate) fn enqueue(&mut self, settings: ConnectionSettings, session: bool) -> bool {
        let key = server_key(&settings);
        if let Some(job) = self.jobs.iter_mut().find(|j| j.key == key) {
            job.session |= session;
            return false;
        }
        self.states.insert(key.clone(), LinkState::Queued);
        self.jobs.push_back(Job {
            key,
            settings,
            session,
            stage: Stage::Waiting,
        });
        true
    }
}

/// Cor e dica da bolinha de um servidor guardado.
pub fn dot(state: Option<&LinkState>, visuals: &egui::Visuals) -> (Dot, String) {
    match state {
        None | Some(LinkState::NoPassword) => (
            Dot::Hollow(visuals.weak_text_color()),
            tr("Not connected").to_string(),
        ),
        Some(LinkState::Queued) => (
            Dot::Filled(AMBER),
            tr("Waiting to connect in the background…").to_string(),
        ),
        Some(LinkState::Connecting) => (
            Dot::Filled(AMBER),
            tr("Connecting in the background…").to_string(),
        ),
        Some(LinkState::Failed(e)) => (
            Dot::Filled(RED),
            trf!("Could not connect: {error}", error = e),
        ),
    }
}

/// Verde de "conectado".
pub const GREEN: egui::Color32 = pgstudio_object_explorer::CONNECTED_GREEN;
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xD2, 0x99, 0x22);
const RED: egui::Color32 = egui::Color32::from_rgb(0xE5, 0x53, 0x4B);

/// Como a bolinha é desenhada.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dot {
    Filled(egui::Color32),
    Hollow(egui::Color32),
}

/// Desenha a bolinha de estado numa linha de servidor.
pub fn show_dot(ui: &mut egui::Ui, dot: Dot, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(
            pgstudio_object_explorer::STATUS_DOT_WIDTH,
            ui.available_height(),
        ),
        egui::Sense::hover(),
    );
    let radius = pgstudio_object_explorer::STATUS_DOT_RADIUS;
    match dot {
        Dot::Filled(color) => {
            ui.painter().circle_filled(rect.center(), radius, color);
        }
        Dot::Hollow(color) => {
            ui.painter()
                .circle_stroke(rect.center(), radius - 0.5, egui::Stroke::new(1.0, color));
        }
    }
    response.on_hover_text(tip)
}

impl ShellApp {
    /// Liga a conexão em segundo plano dos servidores guardados ao abrir
    /// (o aplicativo de verdade; a preferência em Tools > Options desliga).
    pub fn with_background_connect(mut self) -> Self {
        self.servers.background.scan = true;
        self
    }

    /// As conexões em segundo plano.
    pub fn background(&self) -> &Background {
        &self.servers.background
    }

    /// Chave de todos os servidores conectados.
    fn connected_keys(&self) -> HashSet<String> {
        self.connection
            .iter()
            .map(|c| server_key(&c.settings))
            .chain(self.servers.parked.iter().filter_map(Parked::key))
            .collect()
    }

    /// A árvore de servidores que o Object Explorer mostra (a do workspace
    /// ativo ou a dos Registered Servers): o nome dela e as conexões.
    fn shown_servers(&self) -> (String, Vec<ConnectionSettings>) {
        let (source, servers) = match (
            self.workspace_panel.workspace(),
            self.workspace_panel.servers(),
        ) {
            (Some(w), Some(panel)) => (w.root().display().to_string(), panel.servers()),
            _ => (String::new(), self.registered.servers()),
        };
        let list = servers
            .all_servers()
            .into_iter()
            .map(|s| s.settings.clone())
            .collect();
        (source, list)
    }

    /// Põe na fila os servidores guardados que ainda não tiveram a vez.
    fn background_scan(&mut self) {
        // Os cofres de workspace que acabaram de destrancar podem ter as
        // senhas dos que ficaram sem: eles ganham outra vez.
        let unlocked = self.workspace_panel.unlocked_count();
        let more_unlocked = unlocked > self.servers.background.unlocked;
        self.servers.background.unlocked = unlocked;
        if !self.servers.background.scan || !self.preferences.auto_connect {
            return;
        }
        let (source, list) = self.shown_servers();
        let first = self.servers.background.scanned.is_empty();
        let new_source = !self.servers.background.scanned.contains(&source);
        if !new_source && !more_unlocked {
            return;
        }
        self.servers.background.scanned.insert(source);
        let connected = self.connected_keys();
        let mut queued = 0;
        for settings in list {
            let key = server_key(&settings);
            let again = more_unlocked
                && self.servers.background.state(&key) == Some(&LinkState::NoPassword);
            if connected.contains(&key) || !(new_source || again) {
                continue;
            }
            if self.servers.background.enqueue(settings, false) {
                queued += 1;
            }
        }
        // O "Connect to Server" vazio de toda abertura sai da frente: os
        // servidores já estão conectando.
        if first
            && queued > 0
            && self
                .connect_dialog
                .as_ref()
                .is_some_and(|d| !d.is_connecting())
            && !self.servers.has_queue()
        {
            self.connect_dialog = None;
        }
    }

    /// Uma vez por frame: anda a fila das conexões em segundo plano.
    pub(crate) fn background_step(&mut self, ctx: &egui::Context) {
        let Some(connector) = self.connector.clone() else {
            return;
        };
        // As senhas podem estar no cofre de um workspace que ainda está
        // conferindo a senha mestra.
        if self.workspace_panel.is_unlocking() {
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }
        self.background_scan();
        let connected = self.connected_keys();
        let background = &mut self.servers.background;
        background.states.retain(|k, _| !connected.contains(k));
        background.jobs.retain(|j| !connected.contains(&j.key));
        if background.jobs.is_empty() {
            return;
        }

        let mut running = background
            .jobs
            .iter()
            .filter(|j| !matches!(j.stage, Stage::Waiting))
            .count();
        let mut finished: Vec<(usize, Outcome)> = Vec::new();
        for (i, job) in background.jobs.iter_mut().enumerate() {
            if matches!(job.stage, Stage::Waiting) && running < MAX_PARALLEL {
                running += 1;
                background
                    .states
                    .insert(job.key.clone(), LinkState::Connecting);
                match &self.secrets {
                    Some(store) => {
                        job.stage = Stage::Lookup(secrets::lookup(store, job.key.clone()));
                    }
                    None => {
                        finished.push((i, Outcome::NoPassword));
                        continue;
                    }
                }
            }
            let next = match &job.stage {
                Stage::Waiting => None,
                Stage::Lookup(lookup) => match lookup.poll() {
                    None => None,
                    Some(None) => {
                        finished.push((i, Outcome::NoPassword));
                        None
                    }
                    Some(Some(password)) => {
                        let (tx, rx) = mpsc::channel();
                        let (connector, settings, pw, ctx) = (
                            connector.clone(),
                            job.settings.clone(),
                            password.clone(),
                            ctx.clone(),
                        );
                        std::thread::Builder::new()
                            .name("pgstudio-connect-bg".into())
                            .spawn(move || {
                                let _ = tx.send(connector.connect(&settings, &pw));
                                ctx.request_repaint();
                            })
                            .expect("spawn connect thread");
                        Some(Stage::Connecting {
                            rx,
                            password,
                            since: Instant::now(),
                        })
                    }
                },
                Stage::Connecting {
                    rx,
                    password,
                    since,
                } => {
                    match rx.try_recv() {
                        Ok(Ok(c)) => {
                            finished.push((i, Outcome::Connected(Box::new(c), password.clone())));
                        }
                        Ok(Err(e)) => finished.push((i, Outcome::Failed(e.message().to_string()))),
                        Err(TryRecvError::Disconnected) => finished.push((
                            i,
                            Outcome::Failed(
                                tr("The connection attempt stopped unexpectedly.").to_string(),
                            ),
                        )),
                        Err(TryRecvError::Empty) if since.elapsed() > GIVE_UP_AFTER => finished
                            .push((
                                i,
                                Outcome::Failed(tr("The server did not answer in time.").into()),
                            )),
                        Err(TryRecvError::Empty) => {}
                    }
                    None
                }
            };
            if let Some(stage) = next {
                job.stage = stage;
            }
        }
        // De trás para a frente, para os índices valerem.
        let mut done: Vec<(Job, Outcome)> = Vec::new();
        for (i, outcome) in finished.into_iter().rev() {
            if let Some(job) = background.jobs.remove(i) {
                done.push((job, outcome));
            }
        }
        done.reverse();
        let moved = !done.is_empty();
        let shown: HashSet<String> = if done.iter().any(|(j, _)| j.session) {
            self.shown_servers().1.iter().map(server_key).collect()
        } else {
            HashSet::new()
        };
        for (job, outcome) in done {
            match outcome {
                Outcome::Connected(connected, password) => {
                    self.servers.background.states.remove(&job.key);
                    self.background_connected(*connected, password);
                }
                Outcome::NoPassword if job.session => {
                    // Estava aberto na sessão anterior: o diálogo pede a
                    // senha, como sempre.
                    self.servers.background.states.remove(&job.key);
                    self.queue_registered_connect(job.settings);
                }
                Outcome::NoPassword => {
                    self.servers
                        .background
                        .states
                        .insert(job.key, LinkState::NoPassword);
                }
                Outcome::Failed(_) if job.session && !shown.contains(&job.key) => {
                    // Não tem linha no Object Explorer para mostrar o erro:
                    // o diálogo mostra e deixa tentar de novo.
                    self.servers.background.states.remove(&job.key);
                    self.queue_registered_connect(job.settings);
                }
                Outcome::Failed(error) => {
                    let name = self
                        .registered_name(&job.key)
                        .unwrap_or_else(|| job.settings.server_name());
                    self.output.push(trf!(
                        "Could not connect to {name} in the background: {error}",
                        name = name,
                        error = error
                    ));
                    crate::applog::line(&format!(
                        "background connect: {} failed: {error}",
                        job.settings.server_name()
                    ));
                    self.servers
                        .background
                        .states
                        .insert(job.key, LinkState::Failed(error));
                }
            }
        }
        if moved || self.servers.background.is_busy() {
            // As respostas chegam por outras threads, que pedem um frame;
            // este é a garantia (e o limite de tempo).
            ctx.request_repaint_after(Duration::from_millis(if moved { 0 } else { 200 }));
        }
    }

    /// Um servidor conectou em segundo plano: entra no Object Explorer sem
    /// tirar o usuário de onde ele está.
    fn background_connected(&mut self, connected: Connected, password: String) {
        let key = server_key(&connected.settings);
        if self.connected_keys().contains(&key) {
            // O usuário conectou este servidor pelo diálogo nesse meio tempo.
            return;
        }
        let name = connected.settings.server_name();
        self.output.push(trf!(
            "Connected to {name} ({engine} {version}) as {user}.",
            name = name,
            engine = connected.settings.engine.label(),
            version = connected.server_version,
            user = connected.settings.login_label()
        ));
        if self.connection.is_none() {
            // O primeiro vira o servidor ativo.
            self.password = password;
            self.explorer.connect(connected.metadata.clone());
            self.workspace.open(Panel::ObjectExplorer);
            self.status = trf!("Connected to {name}.", name = name);
            let settings = connected.settings.clone();
            self.connection = Some(connected);
            self.catalogs.clear();
            self.assist.reset();
            self.session_connected(&settings);
            return;
        }
        let mut explorer = ObjectExplorer::default();
        explorer.connect(connected.metadata.clone());
        explorer.set_server_label(self.server_label(&connected.settings));
        self.session_restore_tree(&mut explorer, &connected);
        let order = self.servers.next_order();
        self.servers.parked.push(Parked {
            order,
            explorer,
            connection: Some(connected),
            password,
            catalogs: Default::default(),
        });
    }
}
