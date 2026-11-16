//! Autenticação no servidor SSH: arquivo de chave, senha ou agente.

use std::path::PathBuf;
use std::sync::Arc;

use pgstudio_core::connection::{ConnectError, SshAuth, SshSettings};
use pgstudio_i18n::{tr, trf};
use russh::MethodKind;
use russh::client::{AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::{Error as KeyError, PrivateKeyWithHashAlg, load_secret_key};

use crate::hosts::Checker;

/// Chaves tentadas quando o campo do arquivo fica vazio, como o `ssh`.
const DEFAULT_KEYS: [&str; 3] = ["~/.ssh/id_ed25519", "~/.ssh/id_ecdsa", "~/.ssh/id_rsa"];

/// Troca o `~` do começo pela pasta do usuário neste computador.
pub fn expand_home(path: &str) -> PathBuf {
    let path = path.trim();
    if let Some(rest) = path.strip_prefix('~')
        && (rest.is_empty() || rest.starts_with(['/', '\\']))
        && let Some(home) = std::env::home_dir()
    {
        return home.join(rest.trim_start_matches(['/', '\\']));
    }
    PathBuf::from(path)
}

fn network(ssh: &SshSettings, e: &dyn std::fmt::Display) -> ConnectError {
    ConnectError::Network(trf!(
        "Could not connect to the SSH server {server}: {e}",
        server = ssh.server_name(),
        e = e
    ))
}

/// Com o que o usuário tentou entrar.
enum Tried<'a> {
    Password,
    Key(&'a std::path::Path),
    Agent,
}

fn refused(ssh: &SshSettings, tried: Tried) -> ConnectError {
    let server = ssh.server_name();
    ConnectError::Authentication(match tried {
        Tried::Password => trf!(
            "The SSH server {server} refused the password of user {user}.",
            server = server,
            user = ssh.user
        ),
        Tried::Key(file) => trf!(
            "The SSH server {server} refused the key {file} for user {user}.",
            server = server,
            file = file.display(),
            user = ssh.user
        ),
        Tried::Agent => trf!(
            "The SSH server {server} refused the keys of the SSH agent for user {user}.",
            server = server,
            user = ssh.user
        ),
    })
}

pub async fn authenticate(
    session: &mut Handle<Checker>,
    ssh: &SshSettings,
    secret: Option<&str>,
) -> Result<(), ConnectError> {
    match ssh.auth {
        SshAuth::Password => password(session, ssh, secret.unwrap_or_default()).await,
        SshAuth::KeyFile => key_file(session, ssh, secret).await,
        SshAuth::Agent => agent(session, ssh).await,
    }
}

async fn password(
    session: &mut Handle<Checker>,
    ssh: &SshSettings,
    password: &str,
) -> Result<(), ConnectError> {
    if password.is_empty() {
        return Err(ConnectError::Authentication(trf!(
            "Enter the password of {user} on the SSH server {server}.",
            user = ssh.user,
            server = ssh.server_name()
        )));
    }
    let result = session
        .authenticate_password(ssh.user.as_str(), password)
        .await
        .map_err(|e| network(ssh, &e))?;
    let remaining = match result {
        AuthResult::Success => return Ok(()),
        AuthResult::Failure {
            remaining_methods, ..
        } => remaining_methods,
    };
    // Servidores com PAM pedem a senha pelo "keyboard-interactive".
    if remaining.contains(&MethodKind::KeyboardInteractive) {
        let mut reply = session
            .authenticate_keyboard_interactive_start(ssh.user.as_str(), None::<String>)
            .await
            .map_err(|e| network(ssh, &e))?;
        for _ in 0..3 {
            match reply {
                KeyboardInteractiveAuthResponse::Success => return Ok(()),
                KeyboardInteractiveAuthResponse::Failure { .. } => break,
                KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                    let answers = prompts.iter().map(|_| password.to_string()).collect();
                    reply = session
                        .authenticate_keyboard_interactive_respond(answers)
                        .await
                        .map_err(|e| network(ssh, &e))?;
                }
            }
        }
    }
    Err(refused(ssh, Tried::Password))
}

async fn key_file(
    session: &mut Handle<Checker>,
    ssh: &SshSettings,
    passphrase: Option<&str>,
) -> Result<(), ConnectError> {
    let chosen = !ssh.key_file.trim().is_empty();
    let files: Vec<PathBuf> = if chosen {
        vec![expand_home(&ssh.key_file)]
    } else {
        DEFAULT_KEYS
            .iter()
            .map(|p| expand_home(p))
            .filter(|p| p.is_file())
            .collect()
    };
    if files.is_empty() {
        return Err(ConnectError::Authentication(
            tr("Choose the private key file of the SSH tunnel (none was found in ~/.ssh).").into(),
        ));
    }
    let mut last = None;
    for file in files {
        let key = match load_secret_key(&file, passphrase.filter(|p| !p.is_empty())) {
            Ok(k) => k,
            Err(e) => {
                let e = key_error(&file, &e, passphrase.is_some_and(|p| !p.is_empty()));
                if chosen {
                    return Err(e);
                }
                last = Some(e);
                continue;
            }
        };
        // Chave RSA: a assinatura mais forte que o servidor aceita.
        let hash = session
            .best_supported_rsa_hash()
            .await
            .ok()
            .flatten()
            .flatten();
        let result = session
            .authenticate_publickey(
                ssh.user.as_str(),
                PrivateKeyWithHashAlg::new(Arc::new(key), hash),
            )
            .await
            .map_err(|e| network(ssh, &e))?;
        if result.success() {
            return Ok(());
        }
        last = Some(refused(ssh, Tried::Key(&file)));
    }
    Err(last.expect("at least one key was tried"))
}

fn key_error(file: &std::path::Path, e: &KeyError, had_passphrase: bool) -> ConnectError {
    ConnectError::Authentication(match e {
        KeyError::KeyIsEncrypted if !had_passphrase => trf!(
            "The key {file} is protected by a passphrase. Enter it in the SSH tunnel fields.",
            file = file.display()
        ),
        KeyError::IO(e) => trf!(
            "Could not read the SSH key {file}: {e}",
            file = file.display(),
            e = e
        ),
        _ if had_passphrase => trf!(
            "Could not open the SSH key {file}: the passphrase is wrong or the file is not a private key.",
            file = file.display()
        ),
        other => trf!(
            "Could not read the SSH key {file}: {e}",
            file = file.display(),
            e = other
        ),
    })
}

async fn agent(session: &mut Handle<Checker>, ssh: &SshSettings) -> Result<(), ConnectError> {
    let no_agent = |e: &dyn std::fmt::Display| {
        ConnectError::Authentication(trf!("The SSH agent is not available: {e}", e = e))
    };
    #[cfg(unix)]
    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|e| no_agent(&e))?
        .dynamic();
    // No Windows: o agente do OpenSSH e, sem ele, o Pageant.
    #[cfg(windows)]
    let mut agent = match AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
        Ok(a) => a.dynamic(),
        Err(_) => AgentClient::connect_pageant()
            .await
            .map_err(|e| no_agent(&e))?
            .dynamic(),
    };
    let identities = agent.request_identities().await.map_err(|e| no_agent(&e))?;
    if identities.is_empty() {
        return Err(ConnectError::Authentication(
            tr("The SSH agent has no keys. Add one with ssh-add.").into(),
        ));
    }
    for identity in identities {
        let AgentIdentity::PublicKey { key, .. } = identity else {
            continue;
        };
        let hash = session
            .best_supported_rsa_hash()
            .await
            .ok()
            .flatten()
            .flatten();
        // Chave que o agente não quis assinar: tenta a próxima.
        if let Ok(result) = session
            .authenticate_publickey_with(ssh.user.as_str(), key, hash, &mut agent)
            .await
            && result.success()
        {
            return Ok(());
        }
        if session.is_closed() {
            return Err(network(ssh, &tr("the server closed the connection")));
        }
    }
    Err(refused(ssh, Tried::Agent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_is_the_home_folder_of_this_computer() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(
            expand_home("~/.ssh/id_ed25519"),
            home.join(".ssh/id_ed25519")
        );
        assert_eq!(expand_home("~\\.ssh\\id_rsa"), home.join(".ssh\\id_rsa"));
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("/keys/~x"), PathBuf::from("/keys/~x"));
        assert_eq!(expand_home("~other/key"), PathBuf::from("~other/key"));
    }
}
