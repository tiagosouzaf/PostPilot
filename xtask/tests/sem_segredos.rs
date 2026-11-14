//! Checagem de segredos no repositório, que roda com `cargo test`.
//!
//! O repositório é público: nenhum arquivo versionado pode ter chave
//! privada, token de serviço, senha real numa connection string, arquivo
//! `.env` ou e-mail pessoal. Senhas de teste do PostgreSQL local
//! (`postgres`, `secret`...) são aceitas. Se o teste acusar algo, troque a
//! credencial vazada (apagar o arquivo não basta, ela fica no histórico) e
//! tire o valor do arquivo.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Prefixos de tokens conhecidos; acusa quando vêm seguidos de pelo menos
/// 16 caracteres de token.
const TOKEN_PREFIXES: &[&str] = &[
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "sk-ant-",
    "AKIA",
    "ASIA",
    "xoxb-",
    "xoxp-",
    "glpat-",
    "npm_",
    "AIza",
];

/// Senhas que só existem em testes e exemplos (PostgreSQL local em docker,
/// testes de cofre). Qualquer outra numa URI `postgres://` é acusada.
const TEST_PASSWORDS: &[&str] = &[
    "postgres", "secret", "s3cr3t", "s3cr3t!", "senha", "s3nha", "pw", "password", "...", "xxx",
];

/// Domínios de e-mail pessoal: um endereço desses no repositório é dado
/// pessoal de alguém.
const PERSONAL_MAIL: &[&str] = &[
    "gmail.com",
    "hotmail.com",
    "outlook.com",
    "live.com",
    "yahoo.com",
    "icloud.com",
    "proton.me",
    "protonmail.com",
    "uol.com.br",
    "bol.com.br",
];

/// Arquivos que não devem ser versionados, pelo nome.
fn forbidden_name(name: &str) -> bool {
    name == ".env"
        || name.starts_with(".env.") && name != ".env.example"
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name == ".pgpass"
        || name == "pgpass.conf"
        || [".pem", ".key", ".p12", ".pfx", ".keystore"]
            .iter()
            .any(|ext| name.ends_with(ext))
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn tracked_files(root: &Path) -> Option<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn scan_text(text: &str) -> Vec<String> {
    scan(text, false)
}

/// `test_file`: arquivo de teste (em `tests/`). Em código de teste, e depois
/// de um `#[cfg(test)]`, senhas em connection strings são exemplos dos
/// testes do parser e não são acusadas; chaves, tokens e e-mails continuam.
fn scan(text: &str, test_file: bool) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_tests = test_file;
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        if line.trim_start().starts_with("#[cfg(test)]") {
            in_tests = true;
        }
        if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
            found.push(format!("linha {n}: chave privada"));
        }
        for prefix in TOKEN_PREFIXES {
            for (i, _) in line.match_indices(prefix) {
                let before = line[..i].chars().next_back();
                if before.is_some_and(token_char) {
                    continue;
                }
                let tail = line[i + prefix.len()..]
                    .chars()
                    .take_while(|c| token_char(*c))
                    .count();
                if tail >= 16 {
                    found.push(format!("linha {n}: token `{prefix}...`"));
                }
            }
        }
        for scheme in ["postgres://", "postgresql://"]
            .into_iter()
            .filter(|_| !in_tests)
        {
            for (i, _) in line.match_indices(scheme) {
                let rest = &line[i + scheme.len()..];
                let end = rest
                    .find(|c: char| c.is_whitespace() || "\"'`)>".contains(c))
                    .unwrap_or(rest.len());
                let authority = rest[..end].split('/').next().unwrap_or("");
                if let Some((userinfo, _host)) = authority.rsplit_once('@')
                    && let Some((_user, password)) = userinfo.split_once(':')
                    && !password.is_empty()
                    && !password.starts_with('{')
                    && !password.starts_with('$')
                    && !password.starts_with('<')
                    && !TEST_PASSWORDS.contains(&password)
                {
                    found.push(format!("linha {n}: senha numa connection string"));
                }
            }
        }
        for domain in PERSONAL_MAIL {
            for (i, _) in line.match_indices(&format!("@{domain}")) {
                let user_ok = line[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c));
                if user_ok {
                    found.push(format!("linha {n}: e-mail pessoal (@{domain})"));
                }
            }
        }
    }
    found
}

#[test]
fn repositorio_sem_segredos() {
    let root = root();
    let Some(files) = tracked_files(&root) else {
        eprintln!("git indisponível ou fora de um repositório: checagem pulada");
        return;
    };
    let me = "xtask/tests/sem_segredos.rs";
    let mut problems = Vec::new();
    for file in &files {
        let name = file.rsplit('/').next().unwrap_or(file);
        if forbidden_name(name) {
            problems.push(format!("{file}: arquivo que não deve ser versionado"));
        }
        if file == me {
            continue;
        }
        let Ok(bytes) = std::fs::read(root.join(file)) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue; // binário (ícones)
        };
        let test_file = file.split('/').any(|part| part == "tests");
        problems.extend(
            scan(text, test_file)
                .into_iter()
                .map(|p| format!("{file}: {p}")),
        );
    }
    assert!(
        problems.is_empty(),
        "possíveis segredos ou dados pessoais versionados:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn checagem_acusa_os_padroes() {
    let token = format!("let t = \"{}{}\";", "gh", "p_0123456789abcdefghijABCD");
    assert_eq!(scan_text(&token).len(), 1);
    let key = format!("-----BEGIN {} PRIVATE KEY-----", "OPENSSH");
    assert_eq!(scan_text(&key).len(), 1);
    assert_eq!(
        scan_text("postgresql://app:Hunter2x@db.prod:5432/app").len(),
        1
    );
    assert_eq!(scan_text(&format!("fulano@{}", "gmail.com")).len(), 1);
    assert!(forbidden_name(".env"));
    assert!(forbidden_name("server.pem"));
    assert!(!forbidden_name(".env.example"));
    assert!(!forbidden_name("keymap.rs"));

    // Exemplos e testes do PostgreSQL local passam.
    assert!(scan_text("postgres://postgres:postgres@localhost:5432/postgres").is_empty());
    assert!(scan_text("postgresql://alice@db.local/sales").is_empty());
    assert!(scan_text("postgres://{user}:{password}@{host}").is_empty());
    assert!(scan_text("git@github.com:tiagosouzaf/postpilot.git").is_empty());
    assert!(scan_text("tokio_postgres::Config::new()").is_empty());

    // Exemplos nos testes do parser de connection string passam; tokens não.
    let uri = "postgresql://app:p%40ss%3Aw0rd@db.vendas.com:6432/vendas";
    assert!(scan(uri, true).is_empty());
    assert!(scan(&format!("#[cfg(test)]\nmod tests {{\n\"{uri}\"\n}}"), false).is_empty());
    assert_eq!(scan(&format!("\"{uri}\"\n#[cfg(test)]"), false).len(), 1);
    assert_eq!(scan(&token, true).len(), 1);
}
