//! Diálogo "Connect to Server" e a lista de conexões recentes.
//!
//! O diálogo só junta os dados; quem conecta é o [`Connector`] que o
//! aplicativo recebe, numa thread separada, para a interface não esperar a
//! rede. As conexões recentes ficam em `connections.json`, sem senha.
//!
//! Com "Remember password" marcado, a senha vai para o cofre do sistema
//! (veja [`crate::secrets`]) depois que a conexão funciona. Ao abrir ou
//! trocar de servidor, o diálogo procura a senha guardada numa thread
//! separada e preenche o campo; sem cofre, o campo fica vazio e pronto.
//!
//! [`Connector`]: pgstudio_core::connection::Connector

use pgstudio_i18n::{tr, trf};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui;
use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, Engine, SslMode,
};
use serde::{Deserialize, Serialize};

use crate::secrets::{self, Lookup, SecretStore};

/// Quantas conexões recentes guardar.
pub const MAX_RECENT: usize = 10;

/// Versão do formato de `connections.json`.
pub const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    recent: Vec<ConnectionSettings>,
}

/// Conexões usadas por último, a mais recente primeiro.
#[derive(Clone, Debug, Default)]
pub struct RecentConnections {
    path: Option<PathBuf>,
    list: Vec<ConnectionSettings>,
}

impl RecentConnections {
    /// Lê o arquivo. Arquivo ausente ou ruim vira lista vazia, sem erro: as
    /// conexões recentes são só uma conveniência.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let list = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<File>(&text).ok())
            .filter(|f| f.version == VERSION)
            .map(|f| f.recent)
            .unwrap_or_default();
        RecentConnections {
            path: Some(path),
            list,
        }
    }

    /// Lista em memória, sem arquivo (testes).
    pub fn in_memory(list: Vec<ConnectionSettings>) -> Self {
        RecentConnections { path: None, list }
    }

    /// Caminho do arquivo, se houver.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// As conexões, a mais recente primeiro.
    pub fn list(&self) -> &[ConnectionSettings] {
        &self.list
    }

    /// Põe a conexão no topo (sem repetir servidor e usuário) e grava.
    pub fn remember(&mut self, settings: &ConnectionSettings) {
        self.list.retain(|s| {
            !(s.host == settings.host && s.port == settings.port && s.user == settings.user)
        });
        self.list.insert(0, settings.clone());
        self.list.truncate(MAX_RECENT);
        if let Err(e) = self.save()
            && let Some(path) = &self.path
        {
            eprintln!("connections: could not save {}: {e}", path.display());
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
            recent: self.list.clone(),
        };
        let text = serde_json::to_string_pretty(&file).expect("always serializable");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

/// O que o usuário fez no diálogo neste frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogAction {
    /// Nada.
    None,
    /// Fechou sem conectar.
    Cancel,
    /// Pediu para conectar.
    Connect(ConnectionSettings, String),
}

/// Estado do diálogo "Connect to Server".
pub struct ConnectDialog {
    /// "Server type": PostgreSQL ou SQL Server.
    pub engine: Engine,
    pub server: String,
    pub user: String,
    pub password: String,
    pub database: String,
    pub ssl: SslMode,
    /// "Remember password": guardar a senha no cofre ao conectar.
    pub remember: bool,
    error: Option<String>,
    pending: Option<Pending>,
    focus: bool,
    /// Cofre das senhas lembradas, se houver.
    store: Option<Arc<dyn SecretStore>>,
    /// Leitura da senha guardada em andamento.
    lookup: Option<Lookup>,
    /// O cofre tinha senha para o servidor do diálogo.
    saved: bool,
    /// Conectar sozinho se o cofre tiver a senha (Registered Servers).
    auto_connect: bool,
}

struct Pending {
    rx: Receiver<Result<Connected, ConnectError>>,
    server: String,
    /// A senha usada, para as ferramentas externas (pg_dump) depois.
    password: String,
}

impl ConnectDialog {
    /// Abre preenchido com `settings` (a conexão mais recente ou o padrão).
    pub fn new(settings: &ConnectionSettings) -> Self {
        ConnectDialog {
            engine: settings.engine,
            server: settings.server_name(),
            user: settings.user.clone(),
            password: String::new(),
            database: settings.database.clone(),
            // Conexões gravadas antes do S1.3 podem ter um modo que o motor não
            // oferece (Prefer no SQL Server): vale o equivalente.
            ssl: if SslMode::options(settings.engine).contains(&settings.ssl) {
                settings.ssl
            } else {
                SslMode::Require
            },
            remember: false,
            error: None,
            pending: None,
            focus: true,
            store: None,
            lookup: None,
            saved: false,
            auto_connect: false,
        }
    }

    /// Usa o cofre das senhas lembradas e já procura a senha do servidor.
    pub fn with_store(mut self, store: Option<Arc<dyn SecretStore>>) -> Self {
        self.store = store;
        self.start_lookup();
        self
    }

    /// Conecta sozinho quando o cofre tiver a senha; sem senha guardada (ou
    /// sem cofre), o diálogo fica aberto pedindo a senha.
    pub fn connect_when_found(mut self) -> Self {
        self.auto_connect = self.lookup.is_some();
        self
    }

    /// Preenche com uma conexão recente; a senha é apagada e procurada de
    /// novo no cofre.
    pub fn fill(&mut self, settings: &ConnectionSettings) {
        let store = self.store.take();
        *self = ConnectDialog {
            focus: false,
            ..ConnectDialog::new(settings)
        }
        .with_store(store);
    }

    /// Troca o "Server type". Login, banco e criptografia que estavam no
    /// padrão do motor antigo passam para o padrão do novo; a senha
    /// guardada é procurada de novo (a chave do cofre muda com o motor).
    pub fn set_engine(&mut self, engine: Engine) {
        let old = self.engine;
        if old == engine {
            return;
        }
        if self.user.trim().is_empty() || self.user.trim() == old.default_user() {
            self.user = engine.default_user().into();
        }
        if self.database.trim() == old.default_database() {
            self.database = engine.default_database().into();
        }
        if !SslMode::options(engine).contains(&self.ssl) {
            self.ssl = SslMode::Require;
        }
        self.engine = engine;
        if self.saved {
            self.password.clear();
            self.saved = false;
            self.remember = false;
        }
        self.start_lookup();
    }

    fn start_lookup(&mut self) {
        self.lookup = match (&self.store, self.settings()) {
            (Some(store), Ok(s)) => Some(secrets::lookup(store, secrets::password_key(&s))),
            _ => None,
        };
    }

    /// Se ainda está procurando a senha guardada.
    pub fn is_looking_up(&self) -> bool {
        self.lookup.is_some()
    }

    /// Se a senha do campo veio do cofre.
    pub fn password_was_saved(&self) -> bool {
        self.saved
    }

    /// Aplica a senha guardada quando a leitura terminar. Devolve se é
    /// hora de conectar sozinho.
    fn poll_lookup(&mut self) -> bool {
        let Some(found) = self.lookup.as_ref().and_then(Lookup::poll) else {
            return false;
        };
        let key = self.lookup.take().map(|l| l.key);
        let same_server = self.settings().ok().map(|s| secrets::password_key(&s)) == key;
        let auto = std::mem::take(&mut self.auto_connect);
        match found {
            Some(password) if same_server && self.password.is_empty() => {
                self.password = password;
                self.remember = true;
                self.saved = true;
                auto
            }
            _ => false,
        }
    }

    /// O que fazer com o cofre depois de conectar com `password`: guardar,
    /// esquecer a senha que estava lá, ou nada.
    pub fn save_password(&self, settings: &ConnectionSettings, password: &str) {
        let Some(store) = &self.store else {
            return;
        };
        let key = secrets::password_key(settings);
        if self.remember {
            secrets::save_in_background(store, key, Some(password.to_string()));
        } else if self.saved {
            secrets::save_in_background(store, key, None);
        }
    }

    /// Mensagem de erro da última tentativa.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Se está esperando a conexão.
    pub fn is_connecting(&self) -> bool {
        self.pending.is_some()
    }

    /// Junta os campos numa conexão, ou explica o que falta.
    pub fn settings(&self) -> Result<ConnectionSettings, String> {
        let mut s = ConnectionSettings {
            user: self.user.trim().to_string(),
            database: self.database.trim().to_string(),
            ssl: self.ssl,
            ..ConnectionSettings::for_engine(self.engine)
        };
        s.set_server_name(&self.server)?;
        if s.user.is_empty() {
            return Err(tr("Enter a login.").into());
        }
        if s.database.is_empty() {
            s.database = self.engine.default_database().into();
        }
        Ok(s)
    }

    /// Começa a conectar numa thread separada.
    pub fn start(
        &mut self,
        connector: Arc<dyn Connector>,
        settings: ConnectionSettings,
        password: String,
    ) {
        let (tx, rx) = mpsc::channel();
        self.error = None;
        self.pending = Some(Pending {
            rx,
            server: settings.server_name(),
            password: password.clone(),
        });
        std::thread::Builder::new()
            .name("pgstudio-connect".into())
            .spawn(move || {
                let _ = tx.send(connector.connect(&settings, &password));
            })
            .expect("spawn connect thread");
    }

    /// Resultado da conexão, quando chegar. Em caso de erro o diálogo
    /// continua aberto mostrando a mensagem.
    /// Com a conexão vem a senha usada (fica só na memória).
    pub fn poll(&mut self) -> Option<(Connected, String)> {
        let pending = self.pending.as_ref()?;
        match pending.rx.try_recv() {
            Ok(Ok(connected)) => {
                let password = self.pending.take().map(|p| p.password).unwrap_or_default();
                Some((connected, password))
            }
            Ok(Err(e)) => {
                self.pending = None;
                self.error = Some(e.message().to_string());
                None
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.error = Some(tr("The connection attempt stopped unexpectedly.").into());
                None
            }
        }
    }

    /// Desenha o diálogo. `recent` alimenta o menu do campo "Server name".
    pub fn show(&mut self, ctx: &egui::Context, recent: &[ConnectionSettings]) -> DialogAction {
        let mut action = DialogAction::None;
        if self.poll_lookup()
            && self.pending.is_none()
            && let Ok(s) = self.settings()
        {
            action = DialogAction::Connect(s, self.password.clone());
        }
        if self.lookup.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(20));
        }
        let connecting = self.pending.is_some() || action != DialogAction::None;
        let modal = crate::theme::modal(egui::Id::new("connect_to_server")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(tr("Connect to Server"));
            ui.add_space(4.0);
            ui.add_enabled_ui(!connecting, |ui| {
                egui::Grid::new("connect_fields")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| self.fields(ui, recent));
            });
            if let Some(error) = &self.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some(p) = &self.pending {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(trf!("Connecting to {server}…", server = p.server));
                });
            } else if self.auto_connect {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Looking for the saved password…"));
                });
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Cancel")).clicked() {
                    action = DialogAction::Cancel;
                }
                let connect = ui.add_enabled(!connecting, egui::Button::new(tr("Connect")));
                let enter = !connecting && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (connect.clicked() || enter) && action == DialogAction::None {
                    match self.settings() {
                        Ok(s) => action = DialogAction::Connect(s, self.password.clone()),
                        Err(e) => self.error = Some(e),
                    }
                }
            });
        });
        if modal.should_close() && action == DialogAction::None {
            action = DialogAction::Cancel;
        }
        if connecting {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        action
    }

    fn fields(&mut self, ui: &mut egui::Ui, recent: &[ConnectionSettings]) {
        let label = ui.label(tr("Server type:"));
        let mut engine = self.engine;
        egui::ComboBox::from_id_salt("connect_engine")
            .selected_text(engine.label())
            .width(300.0)
            .show_ui(ui, |ui| {
                for e in Engine::ALL {
                    ui.selectable_value(&mut engine, e, e.label());
                }
            })
            .response
            .labelled_by(label.id);
        if engine != self.engine {
            self.set_engine(engine);
        }
        ui.end_row();

        let label = ui.label(tr("Server name:"));
        ui.horizontal(|ui| {
            let edit = ui
                .add(
                    egui::TextEdit::singleline(&mut self.server)
                        .desired_width(270.0)
                        .hint_text(match self.engine {
                            Engine::PostgreSql => tr("host or host,port"),
                            Engine::SqlServer => tr("host, host,port or host\\instance"),
                        }),
                )
                .labelled_by(label.id);
            // O primeiro frame do modal só mede o tamanho; insiste até o
            // foco pegar.
            if self.focus {
                if edit.has_focus() {
                    self.focus = false;
                } else {
                    edit.request_focus();
                }
            }
            let mut chosen = None;
            ui.add_enabled_ui(!recent.is_empty(), |ui| {
                ui.menu_button(egui_phosphor::regular::CARET_DOWN, |ui| {
                    for s in recent {
                        let text = match s.engine {
                            Engine::PostgreSql => format!("{} ({})", s.server_name(), s.user),
                            e => format!("{} ({}, {})", s.server_name(), s.user, e.label()),
                        };
                        if ui.button(text).clicked() {
                            chosen = Some(s.clone());
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text(tr("Recent servers"));
            });
            if let Some(s) = chosen {
                self.fill(&s);
            }
        });
        ui.end_row();

        let label = ui.label(tr("Authentication:"));
        ui.label(tr("Password")).labelled_by(label.id);
        ui.end_row();

        let label = ui.label(tr("Login:"));
        ui.add(egui::TextEdit::singleline(&mut self.user).desired_width(300.0))
            .labelled_by(label.id);
        ui.end_row();

        let label = ui.label(tr("Password:"));
        ui.add(
            egui::TextEdit::singleline(&mut self.password)
                .password(true)
                .desired_width(300.0),
        )
        .labelled_by(label.id);
        ui.end_row();

        ui.label("");
        ui.checkbox(&mut self.remember, tr("Remember password"));
        ui.end_row();

        let label = ui.label(tr("Database:"));
        ui.add(
            egui::TextEdit::singleline(&mut self.database)
                .desired_width(300.0)
                .hint_text(self.engine.default_database()),
        )
        .labelled_by(label.id);
        ui.end_row();

        let label = ui.label(tr("Encryption:"));
        egui::ComboBox::from_id_salt("connect_ssl")
            .selected_text(self.ssl.label_for(self.engine))
            .width(300.0)
            .show_ui(ui, |ui| {
                for &mode in SslMode::options(self.engine) {
                    ui.selectable_value(&mut self.ssl, mode, mode.label_for(self.engine));
                }
            })
            .response
            .labelled_by(label.id);
        ui.end_row();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(host: &str, user: &str) -> ConnectionSettings {
        ConnectionSettings {
            host: host.into(),
            user: user.into(),
            ..Default::default()
        }
    }

    #[test]
    fn recent_keeps_the_latest_first_without_repeats() {
        let mut recent = RecentConnections::in_memory(Vec::new());
        recent.remember(&settings("a", "u"));
        recent.remember(&settings("b", "u"));
        recent.remember(&ConnectionSettings {
            database: "other".into(),
            ..settings("a", "u")
        });
        let hosts: Vec<_> = recent.list().iter().map(|s| s.host.as_str()).collect();
        assert_eq!(hosts, ["a", "b"]);
        assert_eq!(recent.list()[0].database, "other");
        for i in 0..20 {
            recent.remember(&settings(&format!("h{i}"), "u"));
        }
        assert_eq!(recent.list().len(), MAX_RECENT);
    }

    #[test]
    fn recent_survives_a_restart_and_ignores_garbage() {
        let dir = std::env::temp_dir().join(format!("pgstudio-recent-{}", std::process::id()));
        let path = dir.join("connections.json");
        let _ = std::fs::remove_file(&path);
        let mut recent = RecentConnections::open(&path);
        assert!(recent.list().is_empty());
        recent.remember(&settings("db.example.com", "alice"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("password"), "{text}");
        let again = RecentConnections::open(&path);
        assert_eq!(again.list(), recent.list());

        std::fs::write(&path, "{ nope").unwrap();
        assert!(RecentConnections::open(&path).list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dialog_validates_the_fields() {
        let mut d = ConnectDialog::new(&ConnectionSettings::default());
        assert_eq!(d.settings().unwrap(), ConnectionSettings::default());
        d.server = "db,6000".into();
        d.database = "  ".into();
        let s = d.settings().unwrap();
        assert_eq!((s.host.as_str(), s.port), ("db", 6000));
        assert_eq!(s.database, "postgres");
        d.server = "db,x".into();
        assert!(d.settings().is_err());
        d.server = "db".into();
        d.user = " ".into();
        assert_eq!(d.settings().unwrap_err(), "Enter a login.");
    }

    #[test]
    fn server_type_switches_the_defaults() {
        let mut d = ConnectDialog::new(&ConnectionSettings::default());
        d.server = "sql01,14330".into();
        d.set_engine(Engine::SqlServer);
        assert_eq!((d.user.as_str(), d.database.as_str()), ("sa", "master"));
        assert_eq!(d.ssl, SslMode::Require);
        let s = d.settings().unwrap();
        assert_eq!(s.engine, Engine::SqlServer);
        assert_eq!((s.host.as_str(), s.port), ("sql01", 14330));
        d.server = "sql01".into();
        d.database.clear();
        let s = d.settings().unwrap();
        assert_eq!((s.port, s.database.as_str()), (1433, "master"));
        // O que a pessoa digitou fica.
        d.user = "app".into();
        d.database = "vendas".into();
        d.set_engine(Engine::PostgreSql);
        assert_eq!((d.user.as_str(), d.database.as_str()), ("app", "vendas"));
        assert_eq!(d.settings().unwrap().port, 5432);
        // Reabrir com uma conexão recente traz o motor dela.
        let recent = ConnectionSettings::for_engine(Engine::SqlServer);
        assert_eq!(ConnectDialog::new(&recent).engine, Engine::SqlServer);
    }
}
