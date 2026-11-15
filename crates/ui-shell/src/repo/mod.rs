//! Workspace: uma pasta (normalmente um repositório git) com o trabalho do
//! PostPilot, para clonar em outra máquina e ter tudo de volta. É o
//! equivalente às Solutions do SSMS (Solution Explorer com Connections e
//! Queries).
//!
//! ```text
//! meu-workspace/
//!   postpilot-workspace.json   nome e versão do formato
//!   servers.json              servidores, no formato do Registered Servers, sem senha
//!   queries/                  consultas .sql em pastas livres
//!   .gitignore                o que é local da máquina
//! ```
//!
//! Tudo é texto, um arquivo por consulta, para o diff e o merge do git
//! funcionarem. A primeira linha de uma consulta pode dizer em que servidor
//! e banco ela roda (veja [`QueryLink`]); o servidor é procurado pelo nome
//! em `servers.json`.

mod app;
pub mod git;
pub mod panel;
pub mod passwords;
pub mod search;
pub mod source_control;
pub mod vault;

use pgstudio_i18n::{tr, trf};
use std::path::{Path, PathBuf};

use pgstudio_core::connection::ConnectionSettings;
use serde::{Deserialize, Serialize};

use crate::registered::{RegisteredServer, RegisteredServers, ServerGroup};

/// Arquivo que marca a pasta como workspace.
pub const MANIFEST: &str = "postpilot-workspace.json";
/// Servidores do workspace.
pub const SERVERS: &str = "servers.json";
/// Pasta das consultas.
pub const QUERIES: &str = "queries";
/// Versão do formato do workspace.
pub const FORMAT: u32 = 1;

/// O que nunca vai para o repositório. Arquivos temporários do PostPilot e
/// a senha mestra lembrada não ficam na pasta, mas o `.gitignore` protege
/// também cópias de segurança de editores.
const GITIGNORE: &str = "\
# PostPilot: arquivos locais desta máquina
*.tmp
*.json.tmp
*~
.DS_Store
";

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format: u32,
    name: String,
}

/// Um workspace aberto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
    name: String,
}

impl Workspace {
    /// Cria o workspace em `root` (a pasta pode já existir, por exemplo um
    /// clone vazio). Não sobrescreve um workspace que já está lá.
    pub fn create(root: &Path, name: &str) -> Result<Workspace, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(tr("The workspace needs a name.").into());
        }
        if root.join(MANIFEST).exists() {
            return Err(trf!(
                "{folder} already has a PostPilot workspace. Use Open Workspace.",
                folder = root.display()
            ));
        }
        let io = |e: std::io::Error| trf!("Could not create the workspace: {e}", e = e);
        std::fs::create_dir_all(root.join(QUERIES)).map_err(io)?;
        let manifest = Manifest {
            format: FORMAT,
            name: name.into(),
        };
        let text = serde_json::to_string_pretty(&manifest).expect("always serializable");
        std::fs::write(root.join(MANIFEST), text + "\n").map_err(io)?;
        let gitignore = root.join(".gitignore");
        if !gitignore.exists() {
            std::fs::write(&gitignore, GITIGNORE).map_err(io)?;
        }
        // Um servers.json vazio desde o início: o primeiro commit já tem o
        // formato completo.
        let servers = RegisteredServers::open(root.join(SERVERS));
        servers.save().map_err(io)?;
        // O git não guarda pasta vazia.
        let keep = root.join(QUERIES).join(".gitkeep");
        if !keep.exists() {
            std::fs::write(&keep, "").map_err(io)?;
        }
        Ok(Workspace {
            root: root.to_path_buf(),
            name: name.into(),
        })
    }

    /// Abre o workspace da pasta `root`.
    pub fn open(root: &Path) -> Result<Workspace, String> {
        let text = std::fs::read_to_string(root.join(MANIFEST)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                trf!(
                    "{folder} is not a PostPilot workspace (no {file}). Use New Workspace to create one.",
                    folder = root.display(),
                    file = MANIFEST
                )
            } else {
                trf!("Could not read {file}: {e}", file = MANIFEST, e = e)
            }
        })?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|e| trf!("{file} is invalid: {e}", file = MANIFEST, e = e))?;
        if manifest.format > FORMAT {
            return Err(trf!(
                "This workspace was saved by a newer PostPilot (format {format}). Update PostPilot to open it.",
                format = manifest.format
            ));
        }
        std::fs::create_dir_all(root.join(QUERIES))
            .map_err(|e| trf!("Could not create {folder}: {e}", folder = QUERIES, e = e))?;
        Ok(Workspace {
            root: root.to_path_buf(),
            name: manifest.name,
        })
    }

    /// Pasta do workspace.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Nome do workspace.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Pasta das consultas.
    pub fn queries_dir(&self) -> PathBuf {
        self.root.join(QUERIES)
    }

    /// Lê os servidores do workspace. A raiz da árvore leva o nome do
    /// workspace.
    pub fn servers(&self) -> RegisteredServers {
        RegisteredServers::open(self.root.join(SERVERS)).with_root_name(&self.name)
    }

    /// Se `path` fica dentro da pasta do workspace.
    pub fn contains(&self, path: &Path) -> bool {
        let root = std::fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        path.starts_with(root)
    }

    /// As consultas, em árvore, com pastas antes e ordem alfabética: as de
    /// `queries/` e, para um repositório que já tinha consultas antes de
    /// virar workspace, as outras pastas da raiz que têm `.sql` e os `.sql`
    /// soltos na raiz.
    pub fn queries(&self) -> Folder {
        let dir = self.queries_dir();
        let mut tree = Folder::read(&dir, String::new());
        let same = |p: &Path| match (std::fs::canonicalize(p), std::fs::canonicalize(&dir)) {
            (Ok(a), Ok(b)) => a == b,
            _ => p == dir,
        };
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return tree;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || same(&p) {
                continue;
            }
            if p.is_dir() {
                let folder = Folder::read(&p, name);
                if folder.count() > 0 {
                    tree.folders.push(folder);
                }
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sql")) {
                tree.files.push(p);
            }
        }
        tree.folders.sort_by_key(|f| f.name.to_lowercase());
        tree.files.sort_by_key(|f| {
            f.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        });
        tree
    }

    /// Caminho livre `SQLQueryN.sql` em `folder`.
    pub fn new_query_path(folder: &Path) -> PathBuf {
        (1..)
            .map(|n| folder.join(format!("SQLQuery{n}.sql")))
            .find(|p| !p.exists())
            .expect("there is always a free name")
    }
}

/// Uma pasta de consultas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folder {
    /// Nome da pasta (vazio na raiz).
    pub name: String,
    pub path: PathBuf,
    pub folders: Vec<Folder>,
    /// Arquivos `.sql`, por caminho.
    pub files: Vec<PathBuf>,
}

impl Folder {
    fn read(path: &Path, name: String) -> Folder {
        let mut folder = Folder {
            name,
            path: path.to_path_buf(),
            ..Default::default()
        };
        let Ok(entries) = std::fs::read_dir(path) else {
            return folder;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if file_name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                folder.folders.push(Folder::read(&p, file_name));
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sql")) {
                folder.files.push(p);
            }
        }
        folder.folders.sort_by_key(|f| f.name.to_lowercase());
        folder.files.sort_by_key(|f| {
            f.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        });
        folder
    }

    /// Quantas consultas há na pasta e nas subpastas.
    pub fn count(&self) -> usize {
        self.files.len() + self.folders.iter().map(Folder::count).sum::<usize>()
    }
}

/// Primeira linha de uma consulta do workspace: em que servidor (pelo nome
/// em `servers.json`) e em que banco ela roda.
///
/// ```text
/// -- postpilot: server=Produção; database=vendas
/// ```
///
/// `;` e `\` dentro dos valores vão escapados com `\`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryLink {
    pub server: String,
    pub database: Option<String>,
}

const LINK_PREFIX: &str = "-- postpilot:";

impl QueryLink {
    /// Lê a ligação da primeira linha do texto, se houver.
    pub fn read(text: &str) -> Option<QueryLink> {
        let first = text.lines().next()?;
        let rest = first.trim_end().strip_prefix(LINK_PREFIX)?;
        let mut link = QueryLink::default();
        for part in split_unescaped(rest) {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let value = unescape(value.trim());
            match key.trim() {
                "server" => link.server = value,
                "database" if !value.is_empty() => link.database = Some(value),
                _ => {}
            }
        }
        (!link.server.is_empty()).then_some(link)
    }

    /// A linha, sem a quebra.
    pub fn line(&self) -> String {
        match &self.database {
            Some(db) => format!(
                "{LINK_PREFIX} server={}; database={}",
                escape(&self.server),
                escape(db)
            ),
            None => format!("{LINK_PREFIX} server={}", escape(&self.server)),
        }
    }

    /// O texto com esta ligação na primeira linha (trocando a que houver).
    pub fn apply(&self, text: &str) -> String {
        let body = match text.lines().next() {
            Some(first) if first.trim_end().starts_with(LINK_PREFIX) => {
                let rest = &text[first.len()..];
                rest.strip_prefix("\r\n")
                    .or_else(|| rest.strip_prefix('\n'))
                    .unwrap_or(rest)
            }
            _ => text,
        };
        format!("{}\n{body}", self.line())
    }
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(';', "\\;")
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Divide em `;` que não estão escapados.
fn split_unescaped(text: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let last = parts.last_mut().expect("never empty");
                last.push('\\');
                if let Some(next) = chars.next() {
                    last.push(next);
                }
            }
            ';' => parts.push(String::new()),
            c => parts.last_mut().expect("never empty").push(c),
        }
    }
    parts
}

/// Procura um servidor pelo nome na árvore.
pub fn find_server<'a>(group: &'a ServerGroup, name: &str) -> Option<&'a RegisteredServer> {
    group
        .servers
        .iter()
        .find(|s| s.name == name)
        .or_else(|| group.groups.iter().find_map(|g| find_server(g, name)))
}

/// Procura o servidor com esta chave (usuário, servidor e porta).
pub fn find_server_by_key<'a>(group: &'a ServerGroup, key: &str) -> Option<&'a RegisteredServer> {
    group
        .servers
        .iter()
        .find(|s| crate::secrets::password_key(&s.settings) == key)
        .or_else(|| group.groups.iter().find_map(|g| find_server_by_key(g, key)))
}

/// Como conectar a consulta: o servidor do workspace, no banco da ligação.
pub fn link_settings(servers: &ServerGroup, link: &QueryLink) -> Option<ConnectionSettings> {
    let server = find_server(servers, &link.server)?;
    let mut settings = server.settings.clone();
    if let Some(db) = &link.database {
        settings.database = db.clone();
    }
    Some(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pgstudio-repo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn create_then_open() {
        let dir = temp_dir("create");
        let ws = Workspace::create(&dir, " Vendas ").unwrap();
        assert_eq!(ws.name(), "Vendas");
        assert!(dir.join(MANIFEST).exists());
        assert!(dir.join(SERVERS).exists());
        assert!(dir.join(".gitignore").exists());
        assert!(dir.join(QUERIES).is_dir());
        assert!(Workspace::create(&dir, "Outro").is_err());
        let again = Workspace::open(&dir).unwrap();
        assert_eq!(again, ws);
        assert_eq!(again.servers().root().name, "Vendas");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_needs_a_manifest() {
        let dir = temp_dir("nomanifest");
        std::fs::create_dir_all(&dir).unwrap();
        let e = Workspace::open(&dir).unwrap_err();
        assert!(e.contains("not a PostPilot workspace"), "{e}");
        std::fs::write(dir.join(MANIFEST), r#"{"format": 99, "name": "x"}"#).unwrap();
        assert!(Workspace::open(&dir).unwrap_err().contains("newer"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn queries_tree_sorted_folders_first() {
        let dir = temp_dir("tree");
        let ws = Workspace::create(&dir, "T").unwrap();
        let q = ws.queries_dir();
        std::fs::create_dir_all(q.join("relatorios").join("mensal")).unwrap();
        std::fs::write(q.join("b.sql"), "").unwrap();
        std::fs::write(q.join("A.sql"), "").unwrap();
        std::fs::write(q.join("notas.txt"), "").unwrap();
        std::fs::write(q.join("relatorios").join("vendas.sql"), "").unwrap();
        std::fs::write(q.join("relatorios").join("mensal").join("x.SQL"), "").unwrap();
        let tree = ws.queries();
        let names: Vec<_> = tree
            .files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["A.sql", "b.sql"]);
        assert_eq!(tree.folders.len(), 1);
        assert_eq!(tree.folders[0].name, "relatorios");
        assert_eq!(tree.count(), 4);
        assert!(ws.contains(&q.join("b.sql")));
        assert!(!ws.contains(&std::env::temp_dir()));
        let next = Workspace::new_query_path(&q);
        assert_eq!(next.file_name().unwrap(), "SQLQuery1.sql");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn link_round_trips_with_escapes() {
        let link = QueryLink {
            server: r"Prod; A\B".into(),
            database: Some("vendas".into()),
        };
        let text = link.apply("select 1;\n");
        assert_eq!(
            text.lines().next().unwrap(),
            r"-- postpilot: server=Prod\; A\\B; database=vendas"
        );
        assert_eq!(QueryLink::read(&text), Some(link.clone()));
        // Trocar a ligação não duplica a linha.
        let other = QueryLink {
            server: "Dev".into(),
            database: None,
        };
        let again = other.apply(&text);
        assert_eq!(again, "-- postpilot: server=Dev\nselect 1;\n");
        assert_eq!(QueryLink::read(&again), Some(other));
        assert_eq!(QueryLink::read("select 1"), None);
        assert_eq!(QueryLink::read("-- postpilot: database=x"), None);
        assert_eq!(
            QueryLink::read("-- postpilot: server=X\r\nselect 1")
                .unwrap()
                .server,
            "X"
        );
    }

    #[test]
    fn link_finds_the_server_by_name() {
        let root = ServerGroup {
            groups: vec![ServerGroup {
                name: "g".into(),
                servers: vec![RegisteredServer {
                    name: "Prod".into(),
                    description: String::new(),
                    environment: None,
                    settings: ConnectionSettings {
                        host: "db".into(),
                        ..Default::default()
                    },
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let link = QueryLink {
            server: "Prod".into(),
            database: Some("vendas".into()),
        };
        let s = link_settings(&root, &link).unwrap();
        assert_eq!((s.host.as_str(), s.database.as_str()), ("db", "vendas"));
        assert!(find_server_by_key(&root, "postgres@db:5432").is_some());
        assert!(
            link_settings(
                &root,
                &QueryLink {
                    server: "Nope".into(),
                    database: None
                }
            )
            .is_none()
        );
    }
}
