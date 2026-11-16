//! Testes contra um servidor SSH de verdade, com um banco que só ele
//! alcança. Sem `SSH_TEST_HOST` eles só avisam. Para rodar, veja
//! `docs/ssh-tunnel.md` (`scripts/ssh-tunnel-test.sh` sobe os contêineres).
//!
//! Variáveis: `SSH_TEST_HOST`, `SSH_TEST_PORT`, `SSH_TEST_USER`,
//! `SSH_TEST_PASSWORD`, `SSH_TEST_KEYS` (pasta com `id_plain`, `id_rsa`,
//! `id_locked` e `id_stranger`), `SSH_TEST_KEY_PASSPHRASE` (a do
//! `id_locked`) e `SSH_TEST_PG` (`host:porta` do PostgreSQL visto do
//! servidor SSH).

use std::sync::Arc;
use std::time::Duration;

use pgstudio_core::connection::{ConnectError, SshAuth, SshSettings};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Env {
    ssh: SshSettings,
    password: String,
    keys: String,
    passphrase: String,
    target: (String, u16),
}

fn env() -> Option<Env> {
    let Ok(host) = std::env::var("SSH_TEST_HOST") else {
        eprintln!("SSH_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let target = var("SSH_TEST_PG", "pgdb:5432");
    let (target_host, target_port) = target.rsplit_once(':').expect("host:port");
    Some(Env {
        ssh: SshSettings {
            host,
            port: var("SSH_TEST_PORT", "22").parse().unwrap(),
            user: var("SSH_TEST_USER", "tunnel"),
            auth: SshAuth::Password,
            key_file: String::new(),
        },
        password: var("SSH_TEST_PASSWORD", ""),
        keys: var("SSH_TEST_KEYS", "."),
        passphrase: var("SSH_TEST_KEY_PASSPHRASE", ""),
        target: (target_host.to_string(), target_port.parse().unwrap()),
    })
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// Cada teste usa um usuário "diferente" só no nome da chave do segredo:
/// o segredo digitado é global, e os testes rodam em paralelo.
fn with_key(e: &Env, file: &str) -> SshSettings {
    SshSettings {
        auth: SshAuth::KeyFile,
        key_file: format!("{}/{file}", e.keys),
        ..e.ssh.clone()
    }
}

/// Manda o `SSLRequest` do PostgreSQL pela porta local e espera o `S` ou o
/// `N` do servidor: prova que os bytes vão e voltam do banco.
async fn postgres_answers(addr: std::net::SocketAddr) -> bool {
    let mut socket = TcpStream::connect(addr).await.expect("local port");
    socket
        .write_all(&[0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f])
        .await
        .unwrap();
    let mut reply = [0u8; 1];
    match tokio::time::timeout(Duration::from_secs(10), socket.read_exact(&mut reply)).await {
        Ok(Ok(_)) => matches!(reply[0], b'S' | b'N'),
        _ => false,
    }
}

#[test]
fn key_file_opens_a_tunnel_to_the_database() {
    let Some(e) = env() else { return };
    let rt = runtime();
    for file in ["id_plain", "id_rsa"] {
        let ssh = with_key(&e, file);
        let tunnel = rt
            .block_on(pgstudio_ssh::open_to(&ssh, &e.target.0, e.target.1))
            .unwrap_or_else(|err| panic!("{file}: {err}"));
        assert!(tunnel.local_addr().ip().is_loopback());
        assert!(rt.block_on(postgres_answers(tunnel.local_addr())), "{file}");
    }
}

#[test]
fn connections_share_the_tunnel_and_the_last_one_closes_it() {
    let Some(e) = env() else { return };
    let rt = runtime();
    let ssh = with_key(&e, "id_plain");
    // Outro destino (mesma porta por outro nome), para não dividir o túnel
    // com os outros testes.
    let host = format!("{}.", e.target.0);
    let a = rt
        .block_on(pgstudio_ssh::open_to(&ssh, &host, e.target.1))
        .unwrap();
    let b = rt
        .block_on(pgstudio_ssh::open_to(&ssh, &host, e.target.1))
        .unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    let addr = a.local_addr();
    // Várias conexões ao mesmo tempo pelo mesmo túnel.
    let all = rt.block_on(async {
        let tasks: Vec<_> = (0..8)
            .map(|_| tokio::spawn(postgres_answers(addr)))
            .collect();
        let mut ok = true;
        for t in tasks {
            ok &= t.await.unwrap();
        }
        ok
    });
    assert!(all);
    drop(a);
    assert!(b.is_alive());
    drop(b);
    let closed = rt.block_on(async {
        for _ in 0..50 {
            if TcpStream::connect(addr).await.is_err() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    });
    assert!(closed, "the local port stays open after the last user");
    // E abre de novo quando alguém precisa.
    let c = rt
        .block_on(pgstudio_ssh::open_to(&ssh, &host, e.target.1))
        .unwrap();
    assert!(rt.block_on(postgres_answers(c.local_addr())));
}

/// O mesmo servidor por outro endereço de loopback (`127.0.0.N`): o
/// segredo digitado é guardado por servidor, e os testes rodam em paralelo.
fn alias(e: &Env, n: u8) -> SshSettings {
    SshSettings {
        host: if e.ssh.host == "127.0.0.1" {
            format!("127.0.0.{n}")
        } else {
            e.ssh.host.clone()
        },
        ..e.ssh.clone()
    }
}

#[test]
fn password_opens_a_tunnel_and_a_wrong_one_is_refused() {
    let Some(e) = env() else { return };
    let rt = runtime();
    let ssh = alias(&e, 2);
    // Sem senha nenhuma: pede a senha, sem tentar.
    match rt.block_on(pgstudio_ssh::open_to(&ssh, &e.target.0, e.target.1)) {
        Err(ConnectError::Authentication(m)) => assert!(m.contains("Enter the password"), "{m}"),
        other => panic!("{other:?}"),
    }
    pgstudio_ssh::set_secret(&ssh, "errada");
    match rt.block_on(pgstudio_ssh::open_to(&ssh, &e.target.0, e.target.1)) {
        Err(ConnectError::Authentication(m)) => {
            assert!(m.contains("refused the password"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    pgstudio_ssh::set_secret(&ssh, &e.password);
    let tunnel = rt
        .block_on(pgstudio_ssh::open_to(&ssh, &e.target.0, e.target.1))
        .expect("password");
    assert!(rt.block_on(postgres_answers(tunnel.local_addr())));
}

#[test]
fn locked_key_needs_its_passphrase() {
    let Some(e) = env() else { return };
    let rt = runtime();
    let ssh = SshSettings {
        auth: SshAuth::KeyFile,
        key_file: format!("{}/id_locked", e.keys),
        ..alias(&e, 3)
    };
    match rt.block_on(pgstudio_ssh::open_to(&ssh, &e.target.0, e.target.1)) {
        Err(ConnectError::Authentication(m)) => {
            assert!(m.contains("protected by a passphrase"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    pgstudio_ssh::set_secret(&ssh, "frase errada");
    match rt.block_on(pgstudio_ssh::open_to(&ssh, &e.target.0, e.target.1)) {
        Err(ConnectError::Authentication(m)) => {
            assert!(m.contains("the passphrase is wrong"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    pgstudio_ssh::set_secret(&ssh, &e.passphrase);
    let tunnel = rt
        .block_on(pgstudio_ssh::open_to(&ssh, &e.target.0, e.target.1))
        .expect("passphrase");
    assert!(rt.block_on(postgres_answers(tunnel.local_addr())));
}

#[test]
fn unknown_key_and_missing_file_are_explained() {
    let Some(e) = env() else { return };
    let rt = runtime();
    match rt.block_on(pgstudio_ssh::open_to(
        &with_key(&e, "id_stranger"),
        &e.target.0,
        e.target.1,
    )) {
        Err(ConnectError::Authentication(m)) => assert!(m.contains("refused the key"), "{m}"),
        other => panic!("{other:?}"),
    }
    match rt.block_on(pgstudio_ssh::open_to(
        &with_key(&e, "no-such-key"),
        &e.target.0,
        e.target.1,
    )) {
        Err(ConnectError::Authentication(m)) => {
            assert!(m.contains("Could not read the SSH key"), "{m}")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn server_that_does_not_answer_is_a_network_error() {
    if env().is_none() {
        return;
    }
    let rt = runtime();
    let ssh = SshSettings {
        host: "127.0.0.1".into(),
        port: 1,
        user: "u".into(),
        auth: SshAuth::Agent,
        key_file: String::new(),
    };
    match rt.block_on(pgstudio_ssh::open_to(&ssh, "db", 5432)) {
        Err(ConnectError::Network(m)) => assert!(m.contains("SSH server 127.0.0.1:1"), "{m}"),
        other => panic!("{other:?}"),
    }
}

/// Só com `SSH_TEST_AGENT=1` e um agente (`SSH_AUTH_SOCK`) com a chave
/// `id_plain` carregada.
#[test]
fn agent_keys_open_a_tunnel() {
    let Some(e) = env() else { return };
    if std::env::var("SSH_TEST_AGENT").is_err() {
        eprintln!("SSH_TEST_AGENT not set; skipping");
        return;
    }
    let rt = runtime();
    let ssh = SshSettings {
        auth: SshAuth::Agent,
        ..e.ssh.clone()
    };
    let tunnel = rt
        .block_on(pgstudio_ssh::open_to(&ssh, &e.target.0, e.target.1))
        .expect("agent");
    assert!(rt.block_on(postgres_answers(tunnel.local_addr())));
}
