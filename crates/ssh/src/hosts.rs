//! Confere a chave do servidor SSH: primeiro no arquivo do app, depois no
//! `~/.ssh/known_hosts` do usuário. Servidor que nenhum dos dois conhece é
//! anotado no arquivo do app (confiança no primeiro uso); chave diferente
//! da anotada recusa a conexão, com a explicação em [`Checker::refusal`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pgstudio_i18n::trf;
use russh::client::Handler;
use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::{Error as KeyError, PublicKey, PublicKeyOrCertificate};

pub struct Checker {
    host: String,
    port: u16,
    /// Arquivo do app.
    ours: Option<PathBuf>,
    /// `~/.ssh/known_hosts`.
    user: Option<PathBuf>,
    refusal: Arc<Mutex<Option<String>>>,
}

impl Checker {
    pub fn new(host: &str, port: u16, ours: Option<PathBuf>) -> Self {
        Checker {
            host: host.to_string(),
            port,
            ours,
            user: std::env::home_dir().map(|h| h.join(".ssh").join("known_hosts")),
            refusal: Arc::default(),
        }
    }

    #[cfg(test)]
    fn with_user_file(mut self, user: Option<PathBuf>) -> Self {
        self.user = user;
        self
    }

    /// Por que a chave foi recusada, depois que a conexão falha.
    pub fn refusal(&self) -> Arc<Mutex<Option<String>>> {
        self.refusal.clone()
    }

    /// Se a chave vale. Recusa só quando um dos arquivos tem outra chave
    /// para esse servidor.
    pub fn check(&self, key: &PublicKey) -> bool {
        for file in [&self.ours, &self.user].into_iter().flatten() {
            match check_known_hosts_path(&self.host, self.port, key, file) {
                Ok(true) => return true,
                // Não conhece, não existe ou não deu para ler: segue.
                Ok(false) => {}
                Err(KeyError::KeyChanged { line }) => {
                    *self.refusal.lock().expect("ssh refusal") = Some(trf!(
                        "The key of the SSH server {server} is not the one seen before ({file}, line {line}). If the server really changed, remove that line and connect again.",
                        server = self.host,
                        file = file.display(),
                        line = line
                    ));
                    return false;
                }
                Err(_) => {}
            }
        }
        if let Some(ours) = &self.ours
            && let Err(e) = learn_known_hosts_path(&self.host, self.port, key, ours)
        {
            eprintln!("ssh: could not write {}: {e}", ours.display());
        }
        true
    }
}

impl Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(c) => PublicKey::from(c.public_key().clone()),
        };
        Ok(self.check(&key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XFSqti";
    const B: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("postpilot-ssh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn first_use_is_recorded_and_a_changed_key_is_refused() {
        let d = dir("tofu");
        let ours = d.join("known_hosts");
        let a = PublicKey::from_openssh(A).unwrap();
        let b = PublicKey::from_openssh(B).unwrap();
        let checker =
            || Checker::new("bastion.example", 2222, Some(ours.clone())).with_user_file(None);
        assert!(checker().check(&a));
        let text = std::fs::read_to_string(&ours).unwrap();
        assert!(text.contains("[bastion.example]:2222"), "{text}");
        // De novo com a mesma chave: aceita sem repetir a linha.
        assert!(checker().check(&a));
        assert_eq!(std::fs::read_to_string(&ours).unwrap(), text);
        let c = checker();
        assert!(!c.check(&b));
        let why = c.refusal().lock().unwrap().clone().unwrap();
        assert!(why.contains("bastion.example") && why.contains("known_hosts"), "{why}");
        // Outro servidor não se confunde com esse.
        assert!(Checker::new("other.example", 22, Some(ours.clone()))
            .with_user_file(None)
            .check(&b));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_users_known_hosts_counts_too() {
        let d = dir("user");
        let user = d.join("user_known_hosts");
        std::fs::write(&user, format!("bastion.example {A}\n")).unwrap();
        let ours = d.join("known_hosts");
        let a = PublicKey::from_openssh(A).unwrap();
        let b = PublicKey::from_openssh(B).unwrap();
        let checker = || {
            Checker::new("bastion.example", 22, Some(ours.clone()))
                .with_user_file(Some(user.clone()))
        };
        assert!(checker().check(&a));
        assert!(!ours.exists(), "already known by the user's file");
        assert!(!checker().check(&b));
        let _ = std::fs::remove_dir_all(d);
    }
}
