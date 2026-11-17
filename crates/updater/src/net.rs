//! HTTP: a consulta do Release e o download conferido.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};
use ureq::Agent;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

use crate::release::{Asset, Release, parse_release, parse_sha256sums, release_from_sums};

/// Maior arquivo aceito no download (os pacotes têm uns 13 MB).
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
/// Maior resposta aceita da API e do `SHA256SUMS`.
const MAX_SMALL: u64 = 1024 * 1024;

/// Bytes baixados até agora e o total esperado (0 enquanto não se sabe).
#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    /// O download acabou e o pacote está sendo instalado (a janela de senha
    /// do sistema pode estar aberta).
    pub installing: AtomicBool,
    /// Em qual nova tentativa o download está (0: nenhuma falha até agora),
    /// de [`MAX_ATTEMPTS`].
    pub attempt: AtomicU32,
}

impl Progress {
    /// Fração de 0 a 1, quando o total é conhecido.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| self.done.load(Ordering::Relaxed) as f32 / total as f32)
    }
}

fn agent(user_agent: &str) -> Agent {
    // As raízes do sistema, como no TLS do PostgreSQL: assim um proxy
    // corporativo com autoridade própria instalada no sistema funciona.
    let found = rustls_native_certs::load_native_certs();
    let certs: Vec<Certificate<'static>> = found
        .certs
        .iter()
        .map(|c| Certificate::from_der(c.as_ref()).to_owned())
        .collect();
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::new_with_certs(&certs))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    Agent::config_builder()
        .tls_config(tls)
        .user_agent(user_agent)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// O Release mais recente de `repo` (`dono/nome`). `Ok(None)` quando o
/// repositório ainda não tem Release (ou não é público). `user_agent` é o
/// único dado enviado, por exemplo `PostPilot/0.1.0`.
pub fn fetch_latest(repo: &str, user_agent: &str) -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    // Só em build de desenvolvimento: aponta a consulta para um servidor de
    // teste. Fora do build de release de propósito, para ninguém redirecionar
    // a atualização de um app instalado por variável de ambiente.
    #[cfg(debug_assertions)]
    let url = std::env::var("PGSTUDIO_UPDATE_API").unwrap_or(url);
    let mut resp = agent(user_agent)
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not reach GitHub: {e}"))?;
    match resp.status().as_u16() {
        200 => {}
        404 => return Ok(None),
        // Limite de pedidos sem login (60 por hora por IP, que numa rede
        // compartilhada acaba rápido): lê o Release pelo `SHA256SUMS`, que
        // não passa pela API.
        403 | 429 => {
            return fetch_latest_from_sums(repo, user_agent)
                .map(Some)
                .map_err(|e| {
                    format!("GitHub is limiting requests right now, and the fallback failed: {e}")
                });
        }
        s => return Err(format!("GitHub answered with HTTP {s}")),
    }
    let body = resp
        .body_mut()
        .with_config()
        .limit(MAX_SMALL)
        .read_to_string()
        .map_err(|e| format!("could not read the answer from GitHub: {e}"))?;
    parse_release(&body).map_err(|e| e.to_string())
}

/// O Release mais recente sem a API: `releases/latest/download/SHA256SUMS`
/// redireciona para o arquivo do último Release.
pub fn fetch_latest_from_sums(repo: &str, user_agent: &str) -> Result<Release, String> {
    let url = format!("https://github.com/{repo}/releases/latest/download/SHA256SUMS");
    let text = get(&agent(user_agent), &url)?
        .body_mut()
        .with_config()
        .limit(MAX_SMALL)
        .read_to_string()
        .map_err(|e| format!("could not read SHA256SUMS: {e}"))?;
    release_from_sums(repo, &text)
        .ok_or_else(|| "the latest release has no usable SHA256SUMS".to_string())
}

/// Começo do erro quando o servidor de download respondeu com uma falha
/// temporária (HTTP 5xx, 408, 429) em todas as tentativas. O código HTTP vem
/// depois, entre parênteses.
pub const SERVER_UNAVAILABLE: &str = "GitHub's download server is temporarily unavailable";
/// Começo do erro quando a conexão caiu ou não respondeu em todas as
/// tentativas. O detalhe técnico vem depois, entre parênteses.
pub const CONNECTION_FAILED: &str = "The connection failed during the download";

/// Quantas vezes seguidas o download pode falhar sem avançar nada.
pub const MAX_ATTEMPTS: u32 = 5;
/// Limite de tentativas no total, mesmo avançando um pouco a cada uma.
const MAX_TOTAL_ATTEMPTS: u32 = 20;
/// Espera antes de cada nova tentativa (a última vale para as seguintes).
const RETRY_DELAYS: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
];

/// Por que um pedido falhou: `Temporary` vale tentar de novo.
#[derive(Debug)]
enum Failure {
    /// Servidor fora do ar por um instante: 5xx, 408, 429.
    Server(u16),
    /// Conexão recusada, caída, sem resposta, DNS.
    Connection(String),
    /// Não adianta repetir: 404, disco, resposta inesperada.
    Permanent(String),
}

impl Failure {
    fn temporary(&self) -> bool {
        !matches!(self, Failure::Permanent(_))
    }

    fn message(self) -> String {
        match self {
            Failure::Server(status) => format!("{SERVER_UNAVAILABLE} (HTTP {status})"),
            Failure::Connection(detail) => format!("{CONNECTION_FAILED} ({detail})"),
            Failure::Permanent(text) => text,
        }
    }
}

/// As esperas entre tentativas; os testes usam zero.
#[derive(Clone, Copy)]
struct Retry {
    delays: &'static [Duration],
}

impl Retry {
    const DEFAULT: Retry = Retry {
        delays: &RETRY_DELAYS,
    };

    /// Espera antes da tentativa `attempt` (a segunda é 1).
    fn wait(self, attempt: u32) {
        let index = (attempt as usize).saturating_sub(1);
        if let Some(delay) = self.delays.get(index).or(self.delays.last()) {
            std::thread::sleep(*delay);
        }
    }
}

/// Baixa `asset` para `dest` e confere o SHA-256 contra o `SHA256SUMS` do
/// mesmo Release. Sem `SHA256SUMS`, ou sem a linha do arquivo, não baixa.
/// O arquivo é escrito em `dest` + `.part` e só vira `dest` depois de
/// conferido; com erro, nada fica no disco.
///
/// Falhas temporárias (servidor com 5xx, conexão caída, sem resposta) são
/// repetidas sozinhas, com espera, continuando de onde o download parou.
pub fn download_verified(
    release: &Release,
    asset: &Asset,
    dest: &Path,
    user_agent: &str,
    progress: &Progress,
) -> Result<PathBuf, String> {
    download_verified_with(release, asset, dest, user_agent, progress, Retry::DEFAULT)
}

fn download_verified_with(
    release: &Release,
    asset: &Asset,
    dest: &Path,
    user_agent: &str,
    progress: &Progress,
    retry: Retry,
) -> Result<PathBuf, String> {
    let sums = release
        .checksums()
        .ok_or("This release has no SHA256SUMS file, so the download cannot be verified.")?;
    let agent = agent(user_agent);
    let text = get_text(&agent, &sums.url, progress, retry)?;
    let expected = parse_sha256sums(&text, &asset.name)
        .ok_or_else(|| format!("SHA256SUMS has no line for {}.", asset.name))?;

    let part = with_suffix(dest, ".part");
    let result = download_to(&agent, asset, &part, progress, retry).and_then(|actual| {
        if actual == expected {
            std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
        } else {
            Err(format!(
                "The downloaded {} does not match its SHA-256 in SHA256SUMS; it was deleted.",
                asset.name
            ))
        }
    });
    progress.attempt.store(0, Ordering::Relaxed);
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result.map(|()| dest.to_path_buf())
}

/// Um pedido, sem repetir. `from` > 0 pede só o resto do arquivo.
fn request(
    agent: &Agent,
    url: &str,
    from: u64,
) -> Result<ureq::http::Response<ureq::Body>, Failure> {
    let mut req = agent.get(url);
    if from > 0 {
        req = req.header("Range", format!("bytes={from}-"));
    }
    let resp = req.call().map_err(|e| Failure::Connection(e.to_string()))?;
    let status = resp.status().as_u16();
    match status {
        200..=299 => Ok(resp),
        // O pedaço pedido não existe (o arquivo mudou no servidor): quem
        // chamou recomeça do zero.
        416 if from > 0 => Ok(resp),
        408 | 429 | 500..=599 => Err(Failure::Server(status)),
        _ => Err(Failure::Permanent(format!(
            "download failed: HTTP {status}"
        ))),
    }
}

/// Um arquivo pequeno de texto, repetindo nas falhas temporárias.
fn get_text(agent: &Agent, url: &str, progress: &Progress, retry: Retry) -> Result<String, String> {
    let mut attempt = 0;
    loop {
        let result = request(agent, url, 0).and_then(|mut resp| {
            resp.body_mut()
                .with_config()
                .limit(MAX_SMALL)
                .read_to_string()
                .map_err(|e| Failure::Connection(e.to_string()))
        });
        match result {
            Ok(text) => return Ok(text),
            Err(failure) if failure.temporary() && attempt + 1 < MAX_ATTEMPTS => {
                attempt += 1;
                progress.attempt.store(attempt, Ordering::Relaxed);
                retry.wait(attempt);
            }
            Err(failure) => return Err(failure.message()),
        }
    }
}

fn get(agent: &Agent, url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    request(agent, url, 0).map_err(Failure::message)
}

/// Grava o corpo em `path` e devolve o SHA-256 do que gravou. Numa falha
/// temporária tenta de novo pedindo só o que falta (`Range`); se o servidor
/// mandar o arquivo inteiro, recomeça do zero.
fn download_to(
    agent: &Agent,
    asset: &Asset,
    path: &Path,
    progress: &Progress,
    retry: Retry,
) -> Result<[u8; 32], String> {
    let io = |e: std::io::Error| format!("{}: {e}", path.display());
    progress.done.store(0, Ordering::Relaxed);
    progress.total.store(asset.size, Ordering::Relaxed);
    let mut file = std::fs::File::create(path).map_err(io)?;
    let mut hasher = Sha256::new();
    let mut done: u64 = 0;
    // Falhas seguidas sem avançar, e tentativas no total.
    let (mut stuck, mut total) = (0, 0);
    loop {
        let before = done;
        let failure =
            match download_piece(agent, asset, &mut file, &mut hasher, &mut done, progress) {
                Ok(()) => break,
                Err(Piece::Disk(e)) => return Err(io(e)),
                Err(Piece::Failed(failure)) => failure,
            };
        stuck = if done > before { 1 } else { stuck + 1 };
        total += 1;
        if !failure.temporary() || stuck >= MAX_ATTEMPTS || total >= MAX_TOTAL_ATTEMPTS {
            return Err(failure.message());
        }
        progress.attempt.store(stuck, Ordering::Relaxed);
        retry.wait(stuck);
    }
    file.sync_all().map_err(io)?;
    Ok(hasher.finalize().into())
}

enum Piece {
    Failed(Failure),
    Disk(std::io::Error),
}

/// Uma tentativa: pede o arquivo a partir de `done` e grava o que chegar.
fn download_piece(
    agent: &Agent,
    asset: &Asset,
    file: &mut std::fs::File,
    hasher: &mut Sha256,
    done: &mut u64,
    progress: &Progress,
) -> Result<(), Piece> {
    use std::io::{Seek, SeekFrom};
    let mut resp = request(agent, &asset.url, *done).map_err(Piece::Failed)?;
    let status = resp.status().as_u16();
    if *done > 0 && status != 206 {
        // O servidor não continuou de onde parou: recomeça o arquivo.
        file.set_len(0).map_err(Piece::Disk)?;
        file.seek(SeekFrom::Start(0)).map_err(Piece::Disk)?;
        *hasher = Sha256::new();
        *done = 0;
        progress.done.store(0, Ordering::Relaxed);
        if status == 416 {
            return Err(Piece::Failed(Failure::Connection(
                "the server refused to resume the download".into(),
            )));
        }
    }
    let mut reader = resp.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| Piece::Failed(Failure::Connection(e.to_string())))?;
        if n == 0 {
            return Ok(());
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(Piece::Disk)?;
        *done += n as u64;
        progress.done.store(*done, Ordering::Relaxed);
        // Chegou dado: a tentativa deu certo até aqui.
        progress.attempt.store(0, Ordering::Relaxed);
    }
}

/// `arquivo.AppImage` + `.part` = `arquivo.AppImage.part`.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;

    /// Servidor HTTP mínimo: `/file` e `/sums`, uma resposta por conexão.
    fn serve(file: &'static [u8], sums: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut line = String::new();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                reader.read_line(&mut line).unwrap();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h == "\r\n" || h.is_empty() {
                        break;
                    }
                }
                let body: &[u8] = if line.contains("/sums") {
                    sums.as_bytes()
                } else {
                    file
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
            }
        });
        format!("http://{addr}")
    }

    fn release(base: &str) -> Release {
        Release {
            tag: "v0.2.0".into(),
            version: semver::Version::new(0, 2, 0),
            page: String::new(),
            notes: String::new(),
            assets: vec![
                Asset {
                    name: "app.AppImage".into(),
                    url: format!("{base}/file"),
                    size: 5,
                },
                Asset {
                    name: "SHA256SUMS".into(),
                    url: format!("{base}/sums"),
                    size: 0,
                },
            ],
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("updater-net-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn downloads_and_verifies() {
        let hash = hex(&Sha256::digest(b"hello"));
        let base = serve(b"hello", format!("{hash}  app.AppImage\n"));
        let r = release(&base);
        let dir = temp("ok");
        let progress = Progress::default();
        let out =
            download_verified(&r, &r.assets[0], &dir.join("app"), "Test/1", &progress).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        assert_eq!(progress.fraction(), Some(1.0));
        assert!(!dir.join("app.part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn deletes_a_download_that_does_not_match() {
        let hash = hex(&Sha256::digest(b"other"));
        let base = serve(b"hello", format!("{hash}  app.AppImage\n"));
        let r = release(&base);
        let dir = temp("bad");
        let err = download_verified(
            &r,
            &r.assets[0],
            &dir.join("app"),
            "Test/1",
            &Progress::default(),
        )
        .unwrap_err();
        assert!(err.contains("does not match"), "{err}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_without_checksums() {
        let base = serve(b"hello", "abc  other-file\n".into());
        let mut r = release(&base);
        let dir = temp("nosums");
        let err = download_verified(
            &r,
            &r.assets[0],
            &dir.join("app"),
            "Test/1",
            &Progress::default(),
        )
        .unwrap_err();
        assert!(err.contains("no line"), "{err}");
        r.assets.pop();
        let err = download_verified(
            &r,
            &r.assets[0],
            &dir.join("app"),
            "Test/1",
            &Progress::default(),
        )
        .unwrap_err();
        assert!(err.contains("SHA256SUMS"), "{err}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const NO_WAIT: Retry = Retry { delays: &[] };

    /// Servidor com roteiro: `answer(pedido, n)` recebe o pedido inteiro
    /// (linha e cabeçalhos) e quantos pedidos de `/file` já vieram, e devolve
    /// a resposta crua. `/sums` sempre responde certo. Devolve o endereço e
    /// os pedidos de `/file` recebidos.
    fn scripted(
        sums: String,
        answer: impl Fn(&str, usize) -> Vec<u8> + Send + 'static,
    ) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h == "\r\n" || h.is_empty() {
                        break;
                    }
                    request.push_str(&h);
                }
                let response = if request.starts_with("GET /sums") {
                    ok(sums.as_bytes())
                } else {
                    let n = log.lock().unwrap().len();
                    log.lock().unwrap().push(request.clone());
                    answer(&request, n)
                };
                let _ = stream.write_all(&response);
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn ok(body: &[u8]) -> Vec<u8> {
        let mut r = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        r.extend_from_slice(body);
        r
    }

    fn status(code: u16) -> Vec<u8> {
        format!("HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
    }

    const FILE: &[u8] = b"0123456789abcdefghij";

    fn sums_for(file: &[u8]) -> String {
        format!("{}  app.AppImage\n", hex(&Sha256::digest(file)))
    }

    fn fetch(base: &str, dir: &Path, progress: &Progress) -> Result<PathBuf, String> {
        let r = release(base);
        download_verified_with(
            &r,
            &r.assets[0],
            &dir.join("app"),
            "Test/1",
            progress,
            NO_WAIT,
        )
    }

    #[test]
    fn retries_a_server_that_is_out_for_a_moment() {
        let (base, seen) = scripted(sums_for(FILE), |_, n| match n {
            0 => status(504),
            1 => status(502),
            _ => ok(FILE),
        });
        let dir = temp("retry-5xx");
        let progress = Progress::default();
        let out = fetch(&base, &dir, &progress).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), FILE);
        assert_eq!(seen.lock().unwrap().len(), 3);
        assert_eq!(progress.attempt.load(Ordering::Relaxed), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resumes_a_download_cut_in_the_middle() {
        let (base, seen) = scripted(sums_for(FILE), |request, n| {
            if n == 0 {
                // Promete o arquivo inteiro e fecha depois de 8 bytes.
                let mut r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    FILE.len()
                )
                .into_bytes();
                r.extend_from_slice(&FILE[..8]);
                return r;
            }
            assert!(request.contains("bytes=8-"), "{request}");
            let rest = &FILE[8..];
            let mut r = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\n\
                 Content-Range: bytes 8-{}/{}\r\nConnection: close\r\n\r\n",
                rest.len(),
                FILE.len() - 1,
                FILE.len()
            )
            .into_bytes();
            r.extend_from_slice(rest);
            r
        });
        let dir = temp("resume");
        let out = fetch(&base, &dir, &Progress::default()).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), FILE);
        assert_eq!(seen.lock().unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn starts_over_when_the_server_does_not_resume() {
        let (base, seen) = scripted(sums_for(FILE), |_, n| {
            if n == 0 {
                let mut r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    FILE.len()
                )
                .into_bytes();
                r.extend_from_slice(&FILE[..8]);
                r
            } else if n == 1 {
                status(416)
            } else {
                // Ignora o Range e manda tudo de novo.
                ok(FILE)
            }
        });
        let dir = temp("restart");
        let out = fetch(&base, &dir, &Progress::default()).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), FILE);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen[1].contains("bytes=8-") && !seen[2].contains("Range"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn gives_up_after_a_few_tries_with_a_clear_error() {
        let (base, seen) = scripted(sums_for(FILE), |_, _| status(504));
        let dir = temp("give-up");
        let err = fetch(&base, &dir, &Progress::default()).unwrap_err();
        assert_eq!(err, format!("{SERVER_UNAVAILABLE} (HTTP 504)"));
        assert_eq!(seen.lock().unwrap().len(), MAX_ATTEMPTS as usize);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn does_not_retry_what_will_not_change() {
        let (base, seen) = scripted(sums_for(FILE), |_, _| status(404));
        let dir = temp("no-retry");
        let err = fetch(&base, &dir, &Progress::default()).unwrap_err();
        assert_eq!(err, "download failed: HTTP 404");
        assert_eq!(seen.lock().unwrap().len(), 1);
        // Porta fechada: erro de conexão, depois das tentativas.
        let closed = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let err = fetch(&closed, &dir, &Progress::default()).unwrap_err();
        assert!(err.starts_with(CONNECTION_FAILED), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn updates_an_appimage_end_to_end() {
        let hash = hex(&Sha256::digest(b"hello"));
        let base = serve(b"hello", format!("{hash}  app.AppImage\n"));
        let mut r = release(&base);
        r.assets[0].name = "PostPilot-0.2.0-x86_64.AppImage".into();
        let base_sums = serve(b"", format!("{hash}  PostPilot-0.2.0-x86_64.AppImage\n"));
        r.assets[1].url = format!("{base_sums}/sums");
        let dir = temp("e2e");
        let image = dir.join("PostPilot-0.1.0-x86_64.AppImage");
        std::fs::write(&image, b"old").unwrap();
        let kind = crate::InstallKind::AppImage(image.clone());
        let done = crate::install(&kind, &r, None, "Test/1", &Progress::default()).unwrap();
        assert_eq!(done, crate::Installed::Restart(image.clone()));
        assert_eq!(std::fs::read(&image).unwrap(), b"hello");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// O caminho sem a API, de verdade: `cargo test -p pgstudio-updater -- --ignored`.
    #[test]
    #[ignore = "precisa de rede"]
    fn fetches_the_real_release_without_the_api() {
        let r = fetch_latest_from_sums(crate::REPOSITORY, "PostPilot-test/0").unwrap();
        assert!(r.is_newer_than("0.2.2"));
        assert!(r.asset_ending_with("-x86_64.pkg.tar.zst").is_some());
        assert!(r.checksums().is_some());
    }

    /// Consulta de verdade no GitHub: `cargo test -p pgstudio-updater -- --ignored`.
    #[test]
    #[ignore = "precisa de rede"]
    fn fetches_a_real_release() {
        let r = fetch_latest("BurntSushi/ripgrep", "PostPilot-test/0")
            .unwrap()
            .unwrap();
        assert!(r.is_newer_than("0.0.1"));
        assert!(!r.assets.is_empty());
    }
}
