//! Diálogo "Connect to Server" e a lista de conexões recentes.
//!
//! O diálogo só junta os dados; quem conecta é o [`Connector`] que o
//! aplicativo recebe, numa thread separada, para a interface não esperar a
//! rede. As conexões recentes ficam em `connections.json`, sem senha.
//!
//! [`Connector`]: pgstudio_core::connection::Connector

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector, SslMode};
use serde::{Deserialize, Serialize};

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
        if let Err(e) = self.save() {
            if let Some(path) = &self.path {
                eprintln!("connections: could not save {}: {e}", path.display());
            }
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
    pub server: String,
    pub user: String,
    pub password: String,
    pub database: String,
    pub ssl: SslMode,
    error: Option<String>,
    pending: Option<Pending>,
    focus: bool,
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
            server: settings.server_name(),
            user: settings.user.clone(),
            password: String::new(),
            database: settings.database.clone(),
            ssl: settings.ssl,
            error: None,
            pending: None,
            focus: true,
        }
    }

    /// Preenche com uma conexão recente; a senha é apagada.
    pub fn fill(&mut self, settings: &ConnectionSettings) {
        *self = ConnectDialog {
            focus: false,
            ..ConnectDialog::new(settings)
        };
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
            ..Default::default()
        };
        s.set_server_name(&self.server)?;
        if s.user.is_empty() {
            return Err("Enter a login.".into());
        }
        if s.database.is_empty() {
            s.database = pgstudio_core::connection::DEFAULT_DATABASE.into();
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
                self.error = Some("The connection attempt stopped unexpectedly.".into());
                None
            }
        }
    }

    /// Desenha o diálogo. `recent` alimenta o menu do campo "Server name".
    pub fn show(&mut self, ctx: &egui::Context, recent: &[ConnectionSettings]) -> DialogAction {
        let mut action = DialogAction::None;
        let connecting = self.pending.is_some();
        let modal = egui::Modal::new(egui::Id::new("connect_to_server")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading("Connect to Server");
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
                    ui.label(format!("Connecting to {}…", p.server));
                });
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    action = DialogAction::Cancel;
                }
                let connect = ui.add_enabled(!connecting, egui::Button::new("Connect"));
                let enter = !connecting && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if connect.clicked() || enter {
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
        let label = ui.label("Server type:");
        ui.label("PostgreSQL").labelled_by(label.id);
        ui.end_row();

        let label = ui.label("Server name:");
        ui.horizontal(|ui| {
            let edit = ui
                .add(
                    egui::TextEdit::singleline(&mut self.server)
                        .desired_width(270.0)
                        .hint_text("host or host,port"),
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
                        let text = format!("{} ({})", s.server_name(), s.user);
                        if ui.button(text).clicked() {
                            chosen = Some(s.clone());
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("Recent servers");
            });
            if let Some(s) = chosen {
                self.fill(&s);
            }
        });
        ui.end_row();

        let label = ui.label("Authentication:");
        ui.label("Password").labelled_by(label.id);
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

        let label = ui.label("Database:");
        ui.add(
            egui::TextEdit::singleline(&mut self.database)
                .desired_width(300.0)
                .hint_text(pgstudio_core::connection::DEFAULT_DATABASE),
        )
        .labelled_by(label.id);
        ui.end_row();

        let label = ui.label("Encryption:");
        egui::ComboBox::from_id_salt("connect_ssl")
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
}
