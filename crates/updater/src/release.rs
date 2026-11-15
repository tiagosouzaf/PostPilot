//! O Release do GitHub: versão, arquivos e o `SHA256SUMS`.

use semver::Version;
use serde::Deserialize;

/// Um arquivo anexado ao Release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    /// Endereço de download direto (`browser_download_url`).
    pub url: String,
    pub size: u64,
}

/// O Release mais recente publicado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// A tag, por exemplo `v0.2.0`.
    pub tag: String,
    /// A versão lida da tag, sem o `v`.
    pub version: Version,
    /// Página do Release no GitHub.
    pub page: String,
    /// Notas do Release (Markdown), como foram escritas.
    pub notes: String,
    pub assets: Vec<Asset>,
}

/// Por que um Release não serve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseError {
    /// A resposta não é o JSON de um Release.
    Json(String),
    /// A tag não é uma versão (`v1.2.3` ou `1.2.3`).
    Tag(String),
}

impl std::fmt::Display for ReleaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReleaseError::Json(e) => write!(f, "unexpected answer from GitHub: {e}"),
            ReleaseError::Tag(t) => write!(f, "the release tag {t:?} is not a version number"),
        }
    }
}

impl std::error::Error for ReleaseError {}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Lê a resposta de `GET /repos/{repo}/releases/latest`. Rascunhos e pré-
/// lançamentos voltam `None` (o `latest` do GitHub já os exclui; a checagem
/// é só uma garantia).
pub fn parse_release(json: &str) -> Result<Option<Release>, ReleaseError> {
    let raw: RawRelease =
        serde_json::from_str(json).map_err(|e| ReleaseError::Json(e.to_string()))?;
    if raw.draft || raw.prerelease {
        return Ok(None);
    }
    let version =
        parse_tag(&raw.tag_name).ok_or_else(|| ReleaseError::Tag(raw.tag_name.clone()))?;
    if !version.pre.is_empty() {
        return Ok(None);
    }
    Ok(Some(Release {
        tag: raw.tag_name,
        version,
        page: raw.html_url,
        notes: raw.body.unwrap_or_default(),
        assets: raw
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
                size: a.size,
            })
            .collect(),
    }))
}

/// Monta o Release a partir do `SHA256SUMS` dele, para quando a API do GitHub
/// recusa a consulta (limite de pedidos por IP): a versão sai do nome dos
/// arquivos (`postpilot-0.2.3-linux-x86_64.tar.gz`) e os endereços, do padrão
/// de download do GitHub. Fica sem as notas e sem os tamanhos.
pub fn release_from_sums(repo: &str, sums: &str) -> Option<Release> {
    let names: Vec<&str> = sums
        .lines()
        .filter_map(|line| {
            let (hash, name) = line.split_once(char::is_whitespace)?;
            let name = name.trim().trim_start_matches('*');
            (hash.len() == 64 && !name.is_empty() && !name.contains('/')).then_some(name)
        })
        .collect();
    let version = names.iter().find_map(|name| {
        name.split(['-', '_'])
            .find_map(|part| Version::parse(part).ok())
    })?;
    if !version.pre.is_empty() {
        return None;
    }
    let tag = format!("v{version}");
    let base = format!("https://github.com/{repo}/releases/download/{tag}");
    let asset = |name: &str| Asset {
        name: name.to_string(),
        url: format!("{base}/{name}"),
        size: 0,
    };
    let mut assets: Vec<Asset> = names.iter().map(|n| asset(n)).collect();
    assets.push(asset("SHA256SUMS"));
    Some(Release {
        page: format!("https://github.com/{repo}/releases/tag/{tag}"),
        tag,
        version,
        notes: String::new(),
        assets,
    })
}

/// `v1.2.3` ou `1.2.3`.
fn parse_tag(tag: &str) -> Option<Version> {
    let t = tag.trim();
    let t = t.strip_prefix(['v', 'V']).unwrap_or(t);
    Version::parse(t).ok()
}

impl Release {
    /// Se este Release é mais novo que `current` (a versão em execução).
    /// Uma versão atual que não dá para ler nunca é considerada antiga.
    pub fn is_newer_than(&self, current: &str) -> bool {
        Version::parse(current).is_ok_and(|c| self.version > c)
    }

    /// O arquivo cujo nome termina com `suffix`, sem diferenciar maiúsculas
    /// (por exemplo `-x86_64.AppImage`). Assim o nome do produto no começo
    /// do arquivo pode mudar sem quebrar a atualização.
    pub fn asset_ending_with(&self, suffix: &str) -> Option<&Asset> {
        let suffix = suffix.to_ascii_lowercase();
        self.assets
            .iter()
            .find(|a| a.name.to_ascii_lowercase().ends_with(&suffix))
    }

    /// O `SHA256SUMS` do Release.
    pub fn checksums(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == "SHA256SUMS")
    }
}

/// O SHA-256 de `file` num `SHA256SUMS` no formato do `sha256sum`
/// (`<hex>  <nome>`, ou `<hex> *<nome>` no modo binário).
pub fn parse_sha256sums(text: &str, file: &str) -> Option<[u8; 32]> {
    text.lines().find_map(|line| {
        let (hash, name) = line.trim_end().split_once(char::is_whitespace)?;
        let name = name.trim_start();
        let name = name.strip_prefix('*').unwrap_or(name);
        (name == file).then(|| decode_hex(hash)).flatten()
    })
}

fn decode_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATEST: &str = r#"{
        "tag_name": "v0.2.0",
        "html_url": "https://github.com/tiagosouzaf/postpilot/releases/tag/v0.2.0",
        "draft": false,
        "prerelease": false,
        "body": "Notas",
        "assets": [
            {"name": "PostPilot-0.2.0-x86_64.AppImage", "size": 10, "browser_download_url": "https://example.test/a"},
            {"name": "PostPilot-0.2.0-x86_64.msi", "size": 11, "browser_download_url": "https://example.test/m"},
            {"name": "PostPilot-0.2.0-windows-x86_64.zip", "size": 12, "browser_download_url": "https://example.test/z"},
            {"name": "SHA256SUMS", "size": 13, "browser_download_url": "https://example.test/s"}
        ]
    }"#;

    #[test]
    fn reads_the_latest_release() {
        let r = parse_release(LATEST).unwrap().unwrap();
        assert_eq!(r.tag, "v0.2.0");
        assert_eq!(r.version, Version::new(0, 2, 0));
        assert_eq!(r.notes, "Notas");
        assert!(r.is_newer_than("0.1.0"));
        assert!(!r.is_newer_than("0.2.0"));
        assert!(!r.is_newer_than("0.10.0"));
        assert!(!r.is_newer_than("not a version"));
        assert_eq!(
            r.asset_ending_with("-x86_64.appimage").unwrap().url,
            "https://example.test/a"
        );
        assert_eq!(r.asset_ending_with("-windows-x86_64.zip").unwrap().size, 12);
        assert_eq!(r.checksums().unwrap().url, "https://example.test/s");
    }

    #[test]
    fn builds_the_release_from_its_checksums_file() {
        let h = "a".repeat(64);
        let sums = format!(
            "{h}  postpilot-0.2.5-linux-x86_64.tar.gz\n{h}  postpilot-0.2.5-1-x86_64.pkg.tar.zst\n\
             {h} *postpilot_0.2.5_amd64.deb\n{h}  PostPilot-0.2.5-x86_64.AppImage\n"
        );
        let r = release_from_sums("o/r", &sums).unwrap();
        assert_eq!(r.tag, "v0.2.5");
        assert!(r.is_newer_than("0.2.3") && !r.is_newer_than("0.2.5"));
        assert_eq!(r.page, "https://github.com/o/r/releases/tag/v0.2.5");
        assert_eq!(
            r.asset_ending_with("-x86_64.pkg.tar.zst").unwrap().url,
            "https://github.com/o/r/releases/download/v0.2.5/postpilot-0.2.5-1-x86_64.pkg.tar.zst"
        );
        assert_eq!(
            r.asset_ending_with("_amd64.deb").unwrap().name,
            "postpilot_0.2.5_amd64.deb"
        );
        assert_eq!(
            r.checksums().unwrap().url,
            "https://github.com/o/r/releases/download/v0.2.5/SHA256SUMS"
        );
        assert_eq!(release_from_sums("o/r", "<html>Not Found</html>"), None);
        assert_eq!(release_from_sums("o/r", ""), None);
    }

    #[test]
    fn ignores_prereleases_and_drafts() {
        let pre = LATEST.replace("\"prerelease\": false", "\"prerelease\": true");
        assert_eq!(parse_release(&pre).unwrap(), None);
        let draft = LATEST.replace("\"draft\": false", "\"draft\": true");
        assert_eq!(parse_release(&draft).unwrap(), None);
        let tag = LATEST.replace("\"v0.2.0\"", "\"v0.3.0-rc.1\"");
        assert_eq!(parse_release(&tag).unwrap(), None);
    }

    #[test]
    fn rejects_bad_answers() {
        assert!(matches!(parse_release("{}"), Err(ReleaseError::Json(_))));
        let tag = LATEST.replace("\"v0.2.0\"", "\"nightly\"");
        assert_eq!(
            parse_release(&tag),
            Err(ReleaseError::Tag("nightly".into()))
        );
    }

    #[test]
    fn reads_sha256sums() {
        let a = "ab".repeat(32);
        let b = "0F".repeat(32);
        let text =
            format!("{a}  PostPilot-0.2.0-x86_64.AppImage\n{b} *PostPilot-0.2.0-x86_64.msi\n");
        assert_eq!(
            parse_sha256sums(&text, "PostPilot-0.2.0-x86_64.AppImage"),
            Some([0xab; 32])
        );
        assert_eq!(
            parse_sha256sums(&text, "PostPilot-0.2.0-x86_64.msi"),
            Some([0x0f; 32])
        );
        assert_eq!(parse_sha256sums(&text, "PostPilot-0.2.0-x86_64"), None);
        assert_eq!(parse_sha256sums("zz  f\n", "f"), None);
    }
}
