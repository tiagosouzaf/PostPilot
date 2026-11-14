//! HTTP: a consulta do Release e o download conferido.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};
use ureq::Agent;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

use crate::release::{Asset, Release, parse_release, parse_sha256sums};

/// Maior arquivo aceito no download (os pacotes têm uns 13 MB).
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
/// Maior resposta aceita da API e do `SHA256SUMS`.
const MAX_SMALL: u64 = 1024 * 1024;

/// Bytes baixados até agora e o total esperado (0 enquanto não se sabe).
#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
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
    let mut resp = agent(user_agent)
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not reach GitHub: {e}"))?;
    match resp.status().as_u16() {
        200 => {}
        404 => return Ok(None),
        403 | 429 => return Err("GitHub is limiting requests right now; try again later.".into()),
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

/// Baixa `asset` para `dest` e confere o SHA-256 contra o `SHA256SUMS` do
/// mesmo Release. Sem `SHA256SUMS`, ou sem a linha do arquivo, não baixa.
/// O arquivo é escrito em `dest` + `.part` e só vira `dest` depois de
/// conferido; com erro, nada fica no disco.
pub fn download_verified(
    release: &Release,
    asset: &Asset,
    dest: &Path,
    user_agent: &str,
    progress: &Progress,
) -> Result<PathBuf, String> {
    let sums = release
        .checksums()
        .ok_or("This release has no SHA256SUMS file, so the download cannot be verified.")?;
    let agent = agent(user_agent);
    let text = get(&agent, &sums.url)?
        .body_mut()
        .with_config()
        .limit(MAX_SMALL)
        .read_to_string()
        .map_err(|e| format!("could not read SHA256SUMS: {e}"))?;
    let expected = parse_sha256sums(&text, &asset.name)
        .ok_or_else(|| format!("SHA256SUMS has no line for {}.", asset.name))?;

    let part = with_suffix(dest, ".part");
    let result = download_to(&agent, asset, &part, progress).and_then(|actual| {
        if actual == expected {
            std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
        } else {
            Err(format!(
                "The downloaded {} does not match its SHA-256 in SHA256SUMS; it was deleted.",
                asset.name
            ))
        }
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result.map(|()| dest.to_path_buf())
}

fn get(agent: &Agent, url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    if resp.status().is_success() {
        Ok(resp)
    } else {
        Err(format!("download failed: HTTP {}", resp.status().as_u16()))
    }
}

/// Grava o corpo em `path` e devolve o SHA-256 do que gravou.
fn download_to(
    agent: &Agent,
    asset: &Asset,
    path: &Path,
    progress: &Progress,
) -> Result<[u8; 32], String> {
    let io = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut resp = get(agent, &asset.url)?;
    progress.done.store(0, Ordering::Relaxed);
    progress.total.store(asset.size, Ordering::Relaxed);
    let mut reader = resp.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut file = std::fs::File::create(path).map_err(io)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(io)?;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.sync_all().map_err(io)?;
    Ok(hasher.finalize().into())
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
        let done = crate::install(&kind, &r, "Test/1", &Progress::default()).unwrap();
        assert_eq!(done, crate::Installed::Restart(image.clone()));
        assert_eq!(std::fs::read(&image).unwrap(), b"hello");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
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
