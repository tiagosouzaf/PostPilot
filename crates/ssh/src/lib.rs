//! Túnel SSH das conexões: o banco que só o bastion alcança chega ao app
//! por uma porta de `127.0.0.1`.
//!
//! [`open`] entra no servidor SSH de [`ConnectionSettings::ssh`] e abre uma
//! porta local; cada conexão que chega nela vira um canal `direct-tcpip` até
//! o banco. Os drivers (PostgreSQL e SQL Server) conectam em
//! [`Tunnel::local_addr`] e guardam o [`Tunnel`]: quando a última conexão
//! que o usa é solta, o túnel fecha. Conexões para o mesmo banco pelo mesmo
//! bastion dividem um túnel só, e as ferramentas de fora (`pg_dump`) acham
//! a porta dele com [`local_addr`].
//!
//! Os segredos (senha do SSH ou frase secreta da chave) não ficam nas
//! configurações: a interface entrega o que o usuário digitou com
//! [`set_secret`], e o que está guardado no cofre vem pela função dada a
//! [`set_secret_source`].
//!
//! A chave do servidor SSH é conferida com o `~/.ssh/known_hosts` do
//! usuário e com o arquivo do app ([`set_known_hosts`]). Servidor novo é
//! anotado no arquivo do app na primeira vez; chave diferente da anotada
//! recusa a conexão.

mod auth;
mod hosts;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::Duration;

use pgstudio_core::connection::{ConnectError, ConnectionSettings, SshSettings};
use pgstudio_i18n::trf;
use russh::client;
use tokio::net::{TcpListener, TcpStream};

/// Tempo máximo para entrar no servidor SSH.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// De quanto em quanto tempo o túnel parado manda um sinal de vida.
const KEEPALIVE: Duration = Duration::from_secs(30);

/// De onde vêm os segredos guardados (o cofre), pela chave
/// [`SshSettings::secret_key`]. Pode esperar o cofre: só é chamada fora da
/// thread da interface.
pub type SecretSource = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Default)]
struct Globals {
    /// Segredos digitados nesta execução, por [`SshSettings::secret_key`].
    typed: Mutex<HashMap<String, String>>,
    source: RwLock<Option<SecretSource>>,
    known_hosts: RwLock<Option<PathBuf>>,
    /// Túneis abertos, por bastion e destino.
    tunnels: Mutex<HashMap<String, Weak<Tunnel>>>,
    /// Um túnel por vez é aberto: duas conexões simultâneas para o mesmo
    /// banco esperam a primeira e dividem o túnel.
    opening: tokio::sync::Mutex<()>,
}

fn globals() -> &'static Globals {
    static G: OnceLock<Globals> = OnceLock::new();
    G.get_or_init(Globals::default)
}

/// Guarda, só na memória, o segredo que o usuário digitou para esse
/// servidor SSH. Vazio apaga (volta a valer o do cofre).
pub fn set_secret(ssh: &SshSettings, secret: &str) {
    let mut typed = globals().typed.lock().expect("ssh secrets");
    if secret.is_empty() {
        typed.remove(&ssh.secret_key());
    } else {
        typed.insert(ssh.secret_key(), secret.to_string());
    }
}

/// O segredo em uso para esse servidor SSH: o digitado ou, sem ele, o do
/// cofre. Chame fora da thread da interface.
pub fn secret(ssh: &SshSettings) -> Option<String> {
    let key = ssh.secret_key();
    if let Some(s) = globals().typed.lock().expect("ssh secrets").get(&key) {
        return Some(s.clone());
    }
    let source = globals().source.read().expect("ssh source").clone();
    source.and_then(|f| f(&key)).filter(|s| !s.is_empty())
}

/// Liga o cofre: `source` devolve o segredo guardado para uma chave.
pub fn set_secret_source(source: SecretSource) {
    *globals().source.write().expect("ssh source") = Some(source);
}

/// Arquivo onde o app anota a chave dos servidores SSH que conheceu.
pub fn set_known_hosts(path: impl Into<PathBuf>) {
    *globals().known_hosts.write().expect("ssh hosts") = Some(path.into());
}

/// Um túnel aberto. Soltar o último `Arc` fecha a porta local e a sessão
/// SSH.
pub struct Tunnel {
    local: SocketAddr,
    alive: Arc<AtomicBool>,
    accept: tokio::task::JoinHandle<()>,
    link: Arc<Link>,
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel")
            .field("local", &self.local)
            .field("via", &self.link.ssh.endpoint())
            .field("to", &format_args!("{}:{}", self.link.host, self.link.port))
            .finish()
    }
}

impl Tunnel {
    /// Onde os drivers conectam: `127.0.0.1` e a porta do túnel.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// Se a porta local ainda atende.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
        self.accept.abort();
    }
}

/// A sessão SSH de um túnel e para onde ele leva. Se a sessão cair, a
/// próxima conexão entra de novo.
struct Link {
    ssh: SshSettings,
    host: String,
    port: u16,
    secret: Option<String>,
    session: tokio::sync::Mutex<Arc<client::Handle<hosts::Checker>>>,
}

impl Link {
    async fn session(&self) -> Result<Arc<client::Handle<hosts::Checker>>, ConnectError> {
        let mut session = self.session.lock().await;
        if session.is_closed() {
            *session = Arc::new(login(&self.ssh, self.secret.as_deref()).await?);
        }
        Ok(session.clone())
    }

    /// Liga `socket` (uma conexão na porta local) ao banco.
    async fn forward(&self, mut socket: TcpStream, from: SocketAddr) -> Result<(), String> {
        let session = self.session().await.map_err(|e| e.to_string())?;
        let channel = session
            .channel_open_direct_tcpip(
                self.host.clone(),
                u32::from(self.port),
                from.ip().to_string(),
                u32::from(from.port()),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut stream = channel.into_stream();
        tokio::io::copy_bidirectional(&mut socket, &mut stream)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn tunnel_key(ssh: &SshSettings, host: &str, port: u16) -> String {
    format!(
        "{}|{:?}|{}|{host}:{port}",
        ssh.endpoint(),
        ssh.auth,
        ssh.key_file
    )
}

/// O host do banco como o bastion o vê: sem a instância nomeada do SQL
/// Server (pelo túnel vale a porta).
fn target_host(host: &str) -> &str {
    host.split_once('\\').map_or(host, |(h, _)| h)
}

/// O túnel de `settings`, se a conexão passa por um: abre (ou reaproveita)
/// e devolve. Sem túnel nas configurações, `None`. Chame de dentro da
/// runtime Tokio que vai manter as conexões.
pub async fn open(settings: &ConnectionSettings) -> Result<Option<Arc<Tunnel>>, ConnectError> {
    match &settings.ssh {
        None => Ok(None),
        Some(ssh) => open_to(ssh, target_host(&settings.host), settings.port)
            .await
            .map(Some),
    }
}

/// Abre (ou reaproveita) o túnel por `ssh` até `host:port`.
pub async fn open_to(
    ssh: &SshSettings,
    host: &str,
    port: u16,
) -> Result<Arc<Tunnel>, ConnectError> {
    let g = globals();
    let key = tunnel_key(ssh, host, port);
    let _one_at_a_time = g.opening.lock().await;
    if let Some(t) = live(&key) {
        return Ok(t);
    }
    // O cofre pode esperar o D-Bus: fora das threads da runtime.
    let secret = {
        let ssh = ssh.clone();
        tokio::task::spawn_blocking(move || secret(&ssh))
            .await
            .ok()
            .flatten()
    };
    let session = login(ssh, secret.as_deref()).await?;
    // Uma ida ao banco já agora: se o servidor SSH não o alcança (ou não
    // deixa encaminhar portas), o erro diz isso, em vez de o driver só ver
    // a conexão fechar.
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        session.channel_open_direct_tcpip(host.to_string(), u32::from(port), "127.0.0.1", 0),
    )
    .await
    {
        Ok(Ok(probe)) => {
            let _ = probe.close().await;
        }
        Ok(Err(e)) => {
            return Err(ConnectError::Network(trf!(
                "The SSH server {server} could not reach {host}:{port}: {e}",
                server = ssh.server_name(),
                host = host,
                port = port,
                e = e
            )));
        }
        Err(_) => {
            return Err(ConnectError::Network(trf!(
                "The SSH server {server} did not answer when asked to reach {host}:{port}.",
                server = ssh.server_name(),
                host = host,
                port = port
            )));
        }
    }
    let network = |e: std::io::Error| {
        ConnectError::Network(trf!(
            "Could not open the local port of the SSH tunnel: {e}",
            e = e
        ))
    };
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(network)?;
    let local = listener.local_addr().map_err(network)?;
    let link = Arc::new(Link {
        ssh: ssh.clone(),
        host: host.to_string(),
        port,
        secret,
        session: tokio::sync::Mutex::new(Arc::new(session)),
    });
    let alive = Arc::new(AtomicBool::new(true));
    let accept = tokio::spawn(accept_loop(listener, link.clone(), Flag(alive.clone())));
    let tunnel = Arc::new(Tunnel {
        local,
        alive,
        accept,
        link,
    });
    let mut tunnels = g.tunnels.lock().expect("ssh tunnels");
    tunnels.retain(|_, t| t.strong_count() > 0);
    tunnels.insert(key, Arc::downgrade(&tunnel));
    Ok(tunnel)
}

/// Baixa a bandeira quando a tarefa da porta local termina (inclusive
/// quando a runtime dela é solta).
struct Flag(Arc<AtomicBool>);

impl Drop for Flag {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

async fn accept_loop(listener: TcpListener, link: Arc<Link>, _flag: Flag) {
    while let Ok((socket, from)) = listener.accept().await {
        let _ = socket.set_nodelay(true);
        let link = link.clone();
        tokio::spawn(async move {
            // Falha aqui aparece no driver como conexão fechada.
            let _ = link.forward(socket, from).await;
        });
    }
}

fn live(key: &str) -> Option<Arc<Tunnel>> {
    let tunnels = globals().tunnels.lock().expect("ssh tunnels");
    tunnels
        .get(key)
        .and_then(Weak::upgrade)
        .filter(|t| t.is_alive())
}

/// A porta local do túnel já aberto para `settings`, para as ferramentas
/// de fora (`pg_dump`, `pg_restore`). `None` sem túnel nas configurações ou
/// sem conexão aberta por ele.
pub fn local_addr(settings: &ConnectionSettings) -> Option<SocketAddr> {
    let ssh = settings.ssh.as_ref()?;
    live(&tunnel_key(
        ssh,
        target_host(&settings.host),
        settings.port,
    ))
    .map(|t| t.local_addr())
}

/// Entra no servidor SSH: confere a chave dele e autentica.
async fn login(
    ssh: &SshSettings,
    secret: Option<&str>,
) -> Result<client::Handle<hosts::Checker>, ConnectError> {
    let timed_out = || {
        ConnectError::Network(trf!(
            "Timed out connecting to the SSH server {server}.",
            server = ssh.server_name()
        ))
    };
    tokio::time::timeout(CONNECT_TIMEOUT, login_inner(ssh, secret))
        .await
        .map_err(|_| timed_out())?
}

async fn login_inner(
    ssh: &SshSettings,
    secret: Option<&str>,
) -> Result<client::Handle<hosts::Checker>, ConnectError> {
    if ssh.host.trim().is_empty() {
        return Err(ConnectError::Other(
            pgstudio_i18n::tr("Enter the SSH server of the tunnel.").into(),
        ));
    }
    if ssh.user.trim().is_empty() {
        return Err(ConnectError::Other(
            pgstudio_i18n::tr("Enter the SSH user of the tunnel.").into(),
        ));
    }
    let config = Arc::new(client::Config {
        keepalive_interval: Some(KEEPALIVE),
        keepalive_max: 3,
        nodelay: true,
        ..Default::default()
    });
    let checker = hosts::Checker::new(
        &ssh.host,
        ssh.port,
        globals().known_hosts.read().expect("ssh hosts").clone(),
    );
    let refused = checker.refusal();
    let mut session = client::connect(config, (ssh.host.as_str(), ssh.port), checker)
        .await
        .map_err(|e| {
            // A recusa da chave do servidor tem a explicação pronta.
            if let Some(why) = refused.lock().expect("ssh refusal").take() {
                return ConnectError::Tls(why);
            }
            ConnectError::Network(trf!(
                "Could not connect to the SSH server {server}: {e}",
                server = ssh.server_name(),
                e = e
            ))
        })?;
    auth::authenticate(&mut session, ssh, secret).await?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_secret_wins_over_the_vault_and_empty_clears_it() {
        let ssh = SshSettings {
            host: "typed.example".into(),
            user: "u".into(),
            ..Default::default()
        };
        set_secret_source(Arc::new(|key| {
            (key == "ssh:u@typed.example:22").then(|| "from-vault".to_string())
        }));
        assert_eq!(secret(&ssh).as_deref(), Some("from-vault"));
        set_secret(&ssh, "typed");
        assert_eq!(secret(&ssh).as_deref(), Some("typed"));
        set_secret(&ssh, "");
        assert_eq!(secret(&ssh).as_deref(), Some("from-vault"));
    }

    #[test]
    fn named_instance_is_dropped_for_the_tunnel() {
        assert_eq!(target_host("db\\SQLEXPRESS"), "db");
        assert_eq!(target_host("10.0.0.5"), "10.0.0.5");
    }

    #[test]
    fn no_tunnel_without_ssh_settings() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let s = ConnectionSettings::default();
        assert!(rt.block_on(open(&s)).unwrap().is_none());
        assert!(local_addr(&s).is_none());
    }
}
