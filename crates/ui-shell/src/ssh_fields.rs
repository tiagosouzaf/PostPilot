//! Os campos do túnel SSH, iguais no "Connect to Server" e no registro de
//! um servidor: servidor SSH, usuário, como autenticar, arquivo da chave e
//! o segredo (senha do SSH ou frase secreta da chave).
//!
//! As configurações ([`SshSettings`]) vão junto com a conexão, sem segredo
//! e com a chave privada só pelo caminho. O segredo digitado é entregue ao
//! túnel na hora de conectar ([`SshFields::hand_over`]) e, com "Remember
//! password", vai para o cofre ([`SshFields::save_secret`]): o do workspace
//! (cifrado pela senha mestra) para os servidores dele, o do sistema para
//! os outros. Com o campo vazio, o túnel usa o segredo guardado.

use std::path::Path;
use std::sync::Arc;

use eframe::egui;
use pgstudio_core::connection::{SSH_PORT, SshAuth, SshSettings, parse_server_name_for};
use pgstudio_i18n::tr;

use crate::secrets::{self, SecretStore};

/// Para o aplicativo: o arquivo onde ficam as chaves dos servidores SSH já
/// vistos, e o segredo de um túnel que não veio de um diálogo.
pub use pgstudio_ssh::{set_known_hosts, set_secret};

/// Estado dos campos do túnel num diálogo.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SshFields {
    /// "Connect through an SSH tunnel".
    pub enabled: bool,
    /// Servidor SSH: `host` ou `host:porta`.
    pub server: String,
    pub user: String,
    pub auth: SshAuth,
    /// Caminho da chave privada.
    pub key_file: String,
    /// Senha do SSH ou frase secreta da chave, se o usuário digitou.
    pub secret: String,
    /// O botão de procurar a chave foi clicado.
    browse: bool,
}

impl SshFields {
    /// Preenchido com o túnel de uma conexão (ou desligado).
    pub fn new(ssh: Option<&SshSettings>) -> Self {
        match ssh {
            None => SshFields {
                auth: SshAuth::KeyFile,
                ..Default::default()
            },
            Some(s) => SshFields {
                enabled: true,
                server: s.server_name(),
                user: s.user.clone(),
                auth: s.auth,
                key_file: s.key_file.clone(),
                secret: String::new(),
                browse: false,
            },
        }
    }

    /// O túnel como vai nas configurações (`None` desligado), ou o que
    /// falta preencher.
    pub fn settings(&self) -> Result<Option<SshSettings>, String> {
        if !self.enabled {
            return Ok(None);
        }
        if self.server.trim().is_empty() {
            return Err(tr("Enter the SSH server of the tunnel.").into());
        }
        let (host, port) = parse_server_name_for(&self.server, SSH_PORT)?;
        let user = self.user.trim().to_string();
        if user.is_empty() {
            return Err(tr("Enter the SSH user of the tunnel.").into());
        }
        Ok(Some(SshSettings {
            host,
            port,
            user,
            auth: self.auth,
            key_file: match self.auth {
                SshAuth::KeyFile => self.key_file.trim().to_string(),
                _ => String::new(),
            },
        }))
    }

    /// Se o botão de procurar a chave foi clicado desde a última chamada.
    pub fn take_browse(&mut self) -> bool {
        std::mem::take(&mut self.browse)
    }

    /// Usa a chave escolhida no diálogo de arquivo. Dentro da pasta do
    /// usuário o caminho fica com `~`, para valer também no outro
    /// computador (o workspace leva o caminho, não a chave).
    pub fn set_key_file(&mut self, path: &Path) {
        self.key_file = portable_path(path, std::env::home_dir().as_deref());
    }

    /// Entrega ao túnel o segredo digitado, antes de conectar. Com o campo
    /// vazio vale o que já estava (o digitado antes ou o do cofre).
    pub fn hand_over(&self, ssh: &SshSettings) {
        if !self.secret.is_empty() {
            pgstudio_ssh::set_secret(ssh, &self.secret);
        }
    }

    /// Guarda no cofre o segredo digitado ("Remember password" marcado).
    pub fn save_secret(&self, store: &Arc<dyn SecretStore>, ssh: &SshSettings) {
        if !self.secret.is_empty() && ssh.auth != SshAuth::Agent {
            secrets::save_in_background(store, ssh.secret_key(), Some(self.secret.clone()));
        }
    }

    /// As linhas dos campos, dentro da grade de duas colunas do diálogo.
    /// `id` separa os campos de um diálogo dos de outro.
    pub fn show(&mut self, ui: &mut egui::Ui, id: &str) {
        ui.label("");
        ui.checkbox(&mut self.enabled, tr("Connect through an SSH tunnel"))
            .on_hover_text(tr(
                "For a database that only an SSH server (a bastion) can reach.",
            ));
        ui.end_row();
        if !self.enabled {
            return;
        }

        let label = ui.label(tr("SSH server:"));
        ui.add(
            egui::TextEdit::singleline(&mut self.server)
                .desired_width(300.0)
                .hint_text(tr("host or host:port")),
        )
        .labelled_by(label.id);
        ui.end_row();

        let label = ui.label(tr("SSH user:"));
        ui.add(egui::TextEdit::singleline(&mut self.user).desired_width(300.0))
            .labelled_by(label.id);
        ui.end_row();

        let label = ui.label(tr("SSH authentication:"));
        egui::ComboBox::from_id_salt((id, "ssh_auth"))
            .selected_text(self.auth.label())
            .width(300.0)
            .show_ui(ui, |ui| {
                for auth in SshAuth::ALL {
                    ui.selectable_value(&mut self.auth, auth, auth.label());
                }
            })
            .response
            .labelled_by(label.id);
        ui.end_row();

        match self.auth {
            SshAuth::KeyFile => {
                let label = ui.label(tr("Key file:"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.key_file)
                            .desired_width(270.0)
                            // i18n-ignore
                            .hint_text("~/.ssh/id_ed25519"),
                    )
                    .labelled_by(label.id)
                    .on_hover_text(tr(
                        "Only the path is saved, never the key. Empty tries the keys in ~/.ssh.",
                    ));
                    if ui
                        .button(egui_phosphor::regular::FOLDER_OPEN)
                        .on_hover_text(tr("Choose the key file"))
                        .clicked()
                    {
                        self.browse = true;
                    }
                });
                ui.end_row();

                let label = ui.label(tr("Passphrase:"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.secret)
                        .password(true)
                        .desired_width(300.0)
                        .hint_text(tr("empty: none, or the saved one")),
                )
                .labelled_by(label.id);
                ui.end_row();
            }
            SshAuth::Password => {
                let label = ui.label(tr("SSH password:"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.secret)
                        .password(true)
                        .desired_width(300.0)
                        .hint_text(tr("empty: the saved one")),
                )
                .labelled_by(label.id);
                ui.end_row();
            }
            SshAuth::Agent => {
                ui.label("");
                ui.weak(tr(
                    "Uses the keys loaded in the SSH agent of this computer.",
                ));
                ui.end_row();
            }
        }
    }
}

/// `path` como texto, com `~` no lugar da pasta do usuário e `/` entre as
/// pastas (o Windows também aceita).
fn portable_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => {
            let rest: Vec<_> = rest
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            format!("~/{}", rest.join("/"))
        }
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip_the_settings() {
        let off = SshFields::new(None);
        assert_eq!(off.settings().unwrap(), None);
        let ssh = SshSettings {
            host: "bastion.example.com".into(),
            port: 2222,
            user: "deploy".into(),
            auth: SshAuth::KeyFile,
            key_file: "~/.ssh/id_ed25519".into(),
        };
        let fields = SshFields::new(Some(&ssh));
        assert_eq!(fields.server, "bastion.example.com:2222");
        assert_eq!(fields.settings().unwrap(), Some(ssh.clone()));
        // A senha não leva o arquivo de chave junto.
        let mut by_password = fields.clone();
        by_password.auth = SshAuth::Password;
        assert_eq!(by_password.settings().unwrap().unwrap().key_file, "");
    }

    #[test]
    fn missing_fields_are_explained() {
        let mut f = SshFields {
            enabled: true,
            ..SshFields::new(None)
        };
        assert_eq!(
            f.settings().unwrap_err(),
            "Enter the SSH server of the tunnel."
        );
        f.server = "bastion:22x".into();
        assert!(f.settings().unwrap_err().contains("not a valid port"));
        f.server = "bastion".into();
        assert_eq!(
            f.settings().unwrap_err(),
            "Enter the SSH user of the tunnel."
        );
        f.user = " deploy ".into();
        let s = f.settings().unwrap().unwrap();
        assert_eq!(
            (s.host.as_str(), s.port, s.user.as_str()),
            ("bastion", 22, "deploy")
        );
    }

    #[test]
    fn chosen_key_is_saved_relative_to_the_home_folder() {
        let home = Path::new("/home/tiago");
        assert_eq!(
            portable_path(Path::new("/home/tiago/.ssh/id_ed25519"), Some(home)),
            "~/.ssh/id_ed25519"
        );
        assert_eq!(
            portable_path(Path::new("/keys/cliente.pem"), Some(home)),
            "/keys/cliente.pem"
        );
    }
}
