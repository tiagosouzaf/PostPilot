//! Registered Servers (Ctrl+Alt+G): servidores guardados em grupos, como no
//! SSMS, para conectar com dois cliques.
//!
//! A árvore fica em `registered_servers.json`, sem senha. A senha de um
//! servidor registrado com "Remember password" vai para o cofre do sistema
//! (veja [`crate::secrets`]), na mesma chave que o "Connect to Server" usa;
//! conectar um servidor registrado procura a senha lá e só pede ao usuário
//! quando não acha (ou quando não há cofre).

use pgstudio_i18n::{tr, trc, trf};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::connection::{ConnectionSettings, Engine, SslMode};
use pgstudio_core::connection_string;
use serde::{Deserialize, Serialize};

use crate::repo::search::{fold, terms};
use crate::secrets::{self, Lookup, SecretStore};

/// Versão do formato de `registered_servers.json`.
pub const VERSION: u32 = 1;

/// Nome do grupo raiz, como no SSMS.
pub const ROOT_NAME: &str = "Local Server Groups";

/// Nome de um grupo na tela: o grupo raiz sai traduzido.
pub fn display_name(name: &str) -> &str {
    if name == ROOT_NAME {
        tr(ROOT_NAME)
    } else {
        name
    }
}

/// Uma cor de pasta, gravada no arquivo como `"#rrggbb"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    /// Lê `#rrggbb` (o `#` é opcional).
    pub fn parse(text: &str) -> Option<Rgb> {
        let hex = text.trim().trim_start_matches('#');
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Rgb([byte(0)?, byte(2)?, byte(4)?]))
    }

    /// `#rrggbb`.
    pub fn hex(self) -> String {
        let [r, g, b] = self.0;
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    pub fn color32(self) -> egui::Color32 {
        let [r, g, b] = self.0;
        egui::Color32::from_rgb(r, g, b)
    }
}

impl Serialize for Rgb {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Rgb::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("not a #rrggbb color: {text}")))
    }
}

/// As cores oferecidas para as pastas, como as do DataGrip: vermelho,
/// laranja, amarelo, verde, azul-petróleo, azul, roxo e cinza.
pub const FOLDER_COLORS: [Rgb; 8] = [
    Rgb([0xE5, 0x53, 0x4B]),
    Rgb([0xE8, 0x91, 0x2D]),
    Rgb([0xD4, 0xB1, 0x06]),
    Rgb([0x3F, 0xA3, 0x4D]),
    Rgb([0x2A, 0xA1, 0x98]),
    Rgb([0x3B, 0x82, 0xF6]),
    Rgb([0x8B, 0x5C, 0xF6]),
    Rgb([0x8B, 0x8B, 0x8B]),
];

/// Ambientes que todo arquivo tem enquanto ninguém mexe na lista. Os nomes
/// ficam em inglês no arquivo (ele vai para o git e pode ser aberto em
/// outro idioma) e saem traduzidos na tela.
pub const DEVELOPMENT: &str = "Development";
pub const STAGING: &str = "Staging";
pub const PRODUCTION: &str = "Production";

/// Um ambiente (desenvolvimento, homologação, produção...): um nome e uma
/// cor, que aparece no servidor e nas janelas de consulta abertas nele.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub name: String,
    pub color: Rgb,
    /// Pedir confirmação antes de rodar, numa janela de consulta, SQL que
    /// pode alterar o banco (INSERT, UPDATE, DELETE, DDL...).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm_changes: bool,
    /// O que a IA (Claude e os outros provedores) pode fazer nos servidores
    /// do ambiente. Sem escolha, vale [`Environment::ai_access`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<AiAccess>,
}

impl Environment {
    /// O que a IA pode fazer aqui: a escolha do usuário ou, sem ela,
    /// perguntar antes de tudo nos ambientes que já confirmam alterações
    /// (Produção, por padrão) e a regra de sempre nos outros.
    pub fn ai_access(&self) -> AiAccess {
        self.ai.unwrap_or(if self.confirm_changes {
            AiAccess::Confirm
        } else {
            AiAccess::Free
        })
    }
}

/// O que a IA pode fazer nos servidores de um ambiente.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiAccess {
    /// Ler é livre; alterar pede confirmação.
    #[default]
    Free,
    /// Ler o esquema e o plano estimado é livre; tudo o que roda SQL ou
    /// traz dados do servidor pede confirmação.
    Confirm,
    /// A IA não chega ao servidor.
    Blocked,
}

impl AiAccess {
    pub const ALL: [AiAccess; 3] = [AiAccess::Free, AiAccess::Confirm, AiAccess::Blocked];

    /// Nome na tela.
    pub fn label(self) -> &'static str {
        match self {
            AiAccess::Free => tr("AI: reads freely"),
            AiAccess::Confirm => tr("AI: asks first"),
            AiAccess::Blocked => tr("AI: blocked"),
        }
    }

    /// Como as ferramentas dizem ao modelo.
    pub fn id(self) -> &'static str {
        match self {
            AiAccess::Free => "free",
            AiAccess::Confirm => "confirm",
            AiAccess::Blocked => "blocked",
        }
    }
}

/// Desenvolvimento (verde), homologação (laranja) e produção (vermelho).
pub fn default_environments() -> Vec<Environment> {
    vec![
        Environment {
            name: DEVELOPMENT.into(),
            color: FOLDER_COLORS[3],
            confirm_changes: false,
            ai: None,
        },
        Environment {
            name: STAGING.into(),
            color: FOLDER_COLORS[1],
            confirm_changes: false,
            ai: None,
        },
        Environment {
            name: PRODUCTION.into(),
            color: FOLDER_COLORS[0],
            confirm_changes: true,
            ai: None,
        },
    ]
}

/// Nome de um ambiente na tela: os três padrão saem traduzidos.
pub fn environment_name(name: &str) -> &str {
    match name {
        DEVELOPMENT => trc("environment", DEVELOPMENT),
        STAGING => trc("environment", STAGING),
        PRODUCTION => trc("environment", PRODUCTION),
        other => other,
    }
}

/// Como mostrar um servidor registrado: o ambiente dele (próprio ou da
/// pasta) e a cor, que é a do ambiente ou, sem ambiente, a da pasta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerTag {
    /// Nome do ambiente, já traduzido.
    pub environment: Option<String>,
    pub color: Rgb,
    /// O ambiente pede confirmação antes de alterar o banco.
    pub confirm_changes: bool,
    /// O que a IA pode fazer no servidor.
    pub ai: AiAccess,
}

/// Um servidor registrado.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredServer {
    /// "Registered server name": o nome mostrado na árvore.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Ambiente do servidor. Sem um, vale o da pasta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Como conectar (sem senha).
    pub settings: ConnectionSettings,
}

/// Um grupo de servidores, com subgrupos.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerGroup {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Cor da pasta (o ícone dela e o dos servidores dentro). Subpastas sem
    /// cor usam a da pasta de cima.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    /// Ambiente dos servidores da pasta (e das subpastas) que não têm um.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default)]
    pub groups: Vec<ServerGroup>,
    #[serde(default)]
    pub servers: Vec<RegisteredServer>,
}

/// Onde está um nó da árvore: os índices dos grupos a partir da raiz e,
/// para um servidor, o índice dele no último grupo.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct NodePath {
    pub groups: Vec<usize>,
    pub server: Option<usize>,
}

impl NodePath {
    /// Um grupo (a raiz é `&[]`).
    pub fn group(groups: &[usize]) -> Self {
        NodePath {
            groups: groups.to_vec(),
            server: None,
        }
    }

    /// Um servidor dentro de um grupo.
    pub fn server(groups: &[usize], index: usize) -> Self {
        NodePath {
            groups: groups.to_vec(),
            server: Some(index),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    /// Lista de ambientes. Ausente: os três padrão (arquivos antigos e
    /// quem nunca mexeu na lista).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    environments: Option<Vec<Environment>>,
    root: ServerGroup,
}

/// A árvore de servidores registrados e o arquivo onde ela fica.
#[derive(Clone, Debug)]
pub struct RegisteredServers {
    path: Option<PathBuf>,
    root: ServerGroup,
    /// `None`: os ambientes padrão.
    environments: Option<Vec<Environment>>,
}

impl Default for RegisteredServers {
    fn default() -> Self {
        RegisteredServers::in_memory(ServerGroup::default())
    }
}

impl RegisteredServers {
    /// Lê o arquivo. Arquivo ausente ou ruim vira árvore vazia.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (root, environments) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<File>(&text).ok())
            .filter(|f| f.version == VERSION)
            .map(|f| (f.root, f.environments))
            .unwrap_or_default();
        RegisteredServers {
            path: Some(path),
            root: ServerGroup {
                name: ROOT_NAME.into(),
                ..root
            },
            environments,
        }
    }

    /// Árvore em memória, sem arquivo (testes).
    pub fn in_memory(root: ServerGroup) -> Self {
        RegisteredServers {
            path: None,
            root: ServerGroup {
                name: ROOT_NAME.into(),
                ..root
            },
            environments: None,
        }
    }

    /// Dá outro nome à raiz (a de um workspace leva o nome dele).
    pub fn with_root_name(mut self, name: &str) -> Self {
        self.root.name = name.into();
        self
    }

    /// Caminho do arquivo, se houver.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// O grupo raiz ("Local Server Groups").
    pub fn root(&self) -> &ServerGroup {
        &self.root
    }

    /// Um grupo pelo caminho.
    pub fn group(&self, groups: &[usize]) -> Option<&ServerGroup> {
        groups.iter().try_fold(&self.root, |g, &i| g.groups.get(i))
    }

    fn group_mut(&mut self, groups: &[usize]) -> Option<&mut ServerGroup> {
        groups
            .iter()
            .try_fold(&mut self.root, |g, &i| g.groups.get_mut(i))
    }

    /// Um servidor pelo caminho.
    pub fn server(&self, path: &NodePath) -> Option<&RegisteredServer> {
        self.group(&path.groups)?.servers.get(path.server?)
    }

    /// Os ambientes, na ordem da lista.
    pub fn environments(&self) -> Vec<Environment> {
        self.environments
            .clone()
            .unwrap_or_else(default_environments)
    }

    /// Os ambientes, se a lista foi mudada (`None`: os padrão).
    pub fn custom_environments(&self) -> Option<&[Environment]> {
        self.environments.as_deref()
    }

    /// Troca a lista de ambientes. `renamed` diz o nome antigo de cada
    /// ambiente da lista nova (`None` para um novo): servidores e pastas
    /// de um ambiente renomeado passam para o nome novo, e os de um
    /// ambiente apagado ficam sem ambiente.
    pub fn set_environments(
        &mut self,
        list: Vec<Environment>,
        renamed: &[Option<String>],
    ) -> Result<(), String> {
        let mut list: Vec<Environment> = list
            .into_iter()
            .map(|e| Environment {
                name: e.name.trim().to_string(),
                ..e
            })
            .collect();
        for (i, e) in list.iter().enumerate() {
            if e.name.is_empty() {
                return Err(tr("Enter an environment name.").into());
            }
            if list[..i]
                .iter()
                .any(|o| o.name.eq_ignore_ascii_case(&e.name))
            {
                return Err(trf!(
                    "There are two environments named \"{name}\".",
                    name = e.name
                ));
            }
        }
        let mut map: std::collections::HashMap<String, Option<String>> = self
            .environments()
            .into_iter()
            .map(|e| (e.name, None))
            .collect();
        for (e, old) in list.iter().zip(renamed) {
            if let Some(old) = old {
                map.insert(old.clone(), Some(e.name.clone()));
            }
        }
        fn walk(g: &mut ServerGroup, map: &std::collections::HashMap<String, Option<String>>) {
            let fix = |env: &mut Option<String>| {
                if let Some(name) = env.as_ref()
                    && let Some(new) = map.get(name)
                {
                    *env = new.clone();
                }
            };
            fix(&mut g.environment);
            for s in &mut g.servers {
                fix(&mut s.environment);
            }
            for sub in &mut g.groups {
                walk(sub, map);
            }
        }
        walk(&mut self.root, &map);
        list.truncate(64);
        self.environments = if list == default_environments() {
            None
        } else {
            Some(list)
        };
        self.save_or_log();
        Ok(())
    }

    /// Junta ambientes de outro arquivo aos daqui: os de nome novo entram
    /// no fim da lista; os de mesmo nome ficam como estão.
    pub fn merge_environments(&mut self, other: &[Environment]) {
        let mut list = self.environments();
        let before = list.len();
        for e in other {
            if !list.iter().any(|o| o.name.eq_ignore_ascii_case(&e.name)) {
                list.push(e.clone());
            }
        }
        if list.len() != before {
            self.environments = Some(list);
            self.save_or_log();
        }
    }

    /// Um ambiente pelo nome.
    pub fn environment(&self, name: &str) -> Option<Environment> {
        self.environments().into_iter().find(|e| e.name == name)
    }

    /// O ambiente de um grupo: o dele ou o do grupo de cima mais próximo.
    pub fn group_environment(&self, groups: &[usize]) -> Option<Environment> {
        (0..=groups.len()).rev().find_map(|n| {
            self.group(&groups[..n])
                .and_then(|g| g.environment.as_deref())
                .and_then(|name| self.environment(name))
        })
    }

    /// O ambiente de um nó: o do servidor ou o da pasta.
    pub fn node_environment(&self, path: &NodePath) -> Option<Environment> {
        self.server(path)
            .and_then(|s| s.environment.as_deref())
            .and_then(|name| self.environment(name))
            .or_else(|| self.group_environment(&path.groups))
    }

    /// Como mostrar um nó: ambiente e cor (a do ambiente ou, sem ambiente,
    /// a da pasta). `None`: nem ambiente nem cor.
    pub fn node_tag(&self, path: &NodePath) -> Option<ServerTag> {
        match self.node_environment(path) {
            Some(e) => Some(ServerTag {
                environment: Some(environment_name(&e.name).to_string()),
                color: e.color,
                confirm_changes: e.confirm_changes,
                ai: e.ai_access(),
            }),
            None => self.color(&path.groups).map(|color| ServerTag {
                environment: None,
                color,
                confirm_changes: false,
                ai: AiAccess::Free,
            }),
        }
    }

    /// O que mostrar para uma conexão (`usuario@host:porta`): o primeiro
    /// servidor registrado com essa chave que tenha ambiente ou cor.
    pub fn tag_for_key(&self, key: &str) -> Option<ServerTag> {
        fn walk(
            r: &RegisteredServers,
            g: &ServerGroup,
            path: &mut Vec<usize>,
            key: &str,
        ) -> Option<ServerTag> {
            for (i, s) in g.servers.iter().enumerate() {
                if secrets::password_key(&s.settings) == key
                    && let Some(tag) = r.node_tag(&NodePath::server(path, i))
                {
                    return Some(tag);
                }
            }
            for (i, sub) in g.groups.iter().enumerate() {
                path.push(i);
                let found = walk(r, sub, path, key);
                path.pop();
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        walk(self, &self.root, &mut Vec::new(), key)
    }

    /// A cor de um grupo: a dele ou, sem cor, a do grupo de cima mais
    /// próximo que tenha uma.
    pub fn color(&self, groups: &[usize]) -> Option<Rgb> {
        (0..=groups.len())
            .rev()
            .find_map(|n| self.group(&groups[..n]).and_then(|g| g.color))
    }

    /// Todos os grupos, em ordem da árvore: caminho, profundidade (a raiz é
    /// 0) e nome.
    pub fn all_groups(&self) -> Vec<(Vec<usize>, usize, String)> {
        fn walk(
            g: &ServerGroup,
            path: &mut Vec<usize>,
            out: &mut Vec<(Vec<usize>, usize, String)>,
        ) {
            out.push((path.clone(), path.len(), g.name.clone()));
            for (i, sub) in g.groups.iter().enumerate() {
                path.push(i);
                walk(sub, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut Vec::new(), &mut out);
        out
    }

    /// Onde está o servidor com esta chave (`usuario@host:porta`), se ele
    /// está registrado.
    pub fn path_for_key(&self, key: &str) -> Option<NodePath> {
        fn walk(g: &ServerGroup, path: &mut Vec<usize>, key: &str) -> Option<NodePath> {
            if let Some(i) = g
                .servers
                .iter()
                .position(|s| secrets::password_key(&s.settings) == key)
            {
                return Some(NodePath::server(path, i));
            }
            for (i, sub) in g.groups.iter().enumerate() {
                path.push(i);
                let found = walk(sub, path, key);
                path.pop();
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        walk(&self.root, &mut Vec::new(), key)
    }

    /// O nome com que o servidor desta chave está registrado.
    pub fn name_for_key(&self, key: &str) -> Option<&str> {
        let path = self.path_for_key(key)?;
        self.server(&path).map(|s| s.name.as_str())
    }

    /// Todos os servidores com o caminho do nó e a pasta por extenso
    /// ("Clientes / Acme"; vazio na raiz): primeiro os da pasta, depois os
    /// das subpastas.
    pub fn servers_with_folders(&self) -> Vec<(NodePath, String, &RegisteredServer)> {
        fn walk<'a>(
            g: &'a ServerGroup,
            path: &mut Vec<usize>,
            names: &mut Vec<&'a str>,
            out: &mut Vec<(NodePath, String, &'a RegisteredServer)>,
        ) {
            for (i, s) in g.servers.iter().enumerate() {
                out.push((NodePath::server(path, i), names.join(" / "), s));
            }
            for (i, sub) in g.groups.iter().enumerate() {
                path.push(i);
                names.push(&sub.name);
                walk(sub, path, names, out);
                names.pop();
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut Vec::new(), &mut Vec::new(), &mut out);
        out
    }

    /// Todos os servidores, em ordem da árvore.
    pub fn all_servers(&self) -> Vec<&RegisteredServer> {
        fn walk<'a>(g: &'a ServerGroup, out: &mut Vec<&'a RegisteredServer>) {
            for sub in &g.groups {
                walk(sub, out);
            }
            out.extend(&g.servers);
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    /// Cria um grupo dentro de `parent`. Devolve o caminho dele.
    pub fn add_group(
        &mut self,
        parent: &[usize],
        group: ServerGroup,
    ) -> Result<Vec<usize>, String> {
        let name = group.name.trim().to_string();
        let g = self
            .group_mut(parent)
            .ok_or(tr("The group no longer exists."))?;
        check_name(&name, g.groups.iter().map(|g| g.name.as_str()), "group")?;
        g.groups.push(ServerGroup { name, ..group });
        let mut path = parent.to_vec();
        path.push(g.groups.len() - 1);
        self.save_or_log();
        Ok(path)
    }

    /// Troca nome, descrição e cor de um grupo (a raiz não muda).
    pub fn update_group(
        &mut self,
        groups: &[usize],
        name: &str,
        description: &str,
        color: Option<Rgb>,
        environment: Option<String>,
    ) -> Result<(), String> {
        let Some((&last, parent)) = groups.split_last() else {
            return Err(tr("The root group cannot be renamed.").into());
        };
        let name = name.trim().to_string();
        let p = self
            .group_mut(parent)
            .ok_or(tr("The group no longer exists."))?;
        let others = p
            .groups
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != last)
            .map(|(_, g)| g.name.as_str());
        check_name(&name, others, "group")?;
        let g = p
            .groups
            .get_mut(last)
            .ok_or(tr("The group no longer exists."))?;
        g.name = name;
        g.description = description.trim().to_string();
        g.color = color;
        g.environment = environment;
        self.save_or_log();
        Ok(())
    }

    /// Registra um servidor em `parent`. Devolve o caminho dele.
    pub fn add_server(
        &mut self,
        parent: &[usize],
        server: RegisteredServer,
    ) -> Result<NodePath, String> {
        let server = clean(server);
        let g = self
            .group_mut(parent)
            .ok_or(tr("The group no longer exists."))?;
        check_name(
            &server.name,
            g.servers.iter().map(|s| s.name.as_str()),
            "server",
        )?;
        g.servers.push(server);
        let path = NodePath::server(parent, g.servers.len() - 1);
        self.save_or_log();
        Ok(path)
    }

    /// Troca os dados de um servidor registrado.
    pub fn update_server(
        &mut self,
        path: &NodePath,
        server: RegisteredServer,
    ) -> Result<(), String> {
        let server = clean(server);
        let index = path.server.ok_or(tr("Not a server."))?;
        let g = self
            .group_mut(&path.groups)
            .ok_or(tr("The group no longer exists."))?;
        let others = g
            .servers
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, s)| s.name.as_str());
        check_name(&server.name, others, "server")?;
        *g.servers
            .get_mut(index)
            .ok_or(tr("The server no longer exists."))? = server;
        self.save_or_log();
        Ok(())
    }

    /// Apaga um servidor ou um grupo (com tudo o que há dentro). A raiz
    /// não é apagada.
    pub fn remove(&mut self, path: &NodePath) -> bool {
        let removed = match (path.server, path.groups.split_last()) {
            (Some(i), _) => self.group_mut(&path.groups).is_some_and(|g| {
                i < g.servers.len() && {
                    g.servers.remove(i);
                    true
                }
            }),
            (None, Some((&last, parent))) => self.group_mut(parent).is_some_and(|g| {
                last < g.groups.len() && {
                    g.groups.remove(last);
                    true
                }
            }),
            (None, None) => false,
        };
        if removed {
            self.save_or_log();
        }
        removed
    }

    /// Move um servidor ou um grupo (com tudo o que há dentro) para o grupo
    /// `to`, no fim dele, como arrastar no SSMS. Devolve o caminho novo. A
    /// raiz não se move, e um grupo não vai para dentro dele mesmo.
    pub fn move_node(&mut self, from: &NodePath, to: &[usize]) -> Result<NodePath, String> {
        self.group(to)
            .ok_or(tr("The target group no longer exists."))?;
        if let Some(index) = from.server {
            if from.groups == to {
                return Ok(from.clone());
            }
            let server = self
                .server(from)
                .ok_or(tr("The server no longer exists."))?
                .clone();
            let target = self.group(to).expect("checked above");
            check_name(
                &server.name,
                target.servers.iter().map(|s| s.name.as_str()),
                "server",
            )?;
            // Tirar um servidor não muda os índices dos grupos.
            self.group_mut(&from.groups)
                .expect("checked above")
                .servers
                .remove(index);
            let g = self.group_mut(to).expect("checked above");
            g.servers.push(server);
            let path = NodePath::server(to, g.servers.len() - 1);
            self.save_or_log();
            return Ok(path);
        }
        let Some((&last, parent)) = from.groups.split_last() else {
            return Err(tr("The root group cannot be moved.").into());
        };
        if to.starts_with(&from.groups) {
            return Err(tr("A group cannot be moved into itself.").into());
        }
        if to == parent {
            return Ok(from.clone());
        }
        let group = self
            .group(&from.groups)
            .ok_or(tr("The group no longer exists."))?
            .clone();
        let target = self.group(to).expect("checked above");
        check_name(
            &group.name,
            target.groups.iter().map(|g| g.name.as_str()),
            "group",
        )?;
        // Tirar o grupo desloca os irmãos seguintes: o destino pode passar
        // por um deles.
        let mut to = to.to_vec();
        if to.len() > parent.len() && to.starts_with(parent) && to[parent.len()] > last {
            to[parent.len()] -= 1;
        }
        self.group_mut(parent)
            .expect("checked above")
            .groups
            .remove(last);
        let g = self.group_mut(&to).expect("same group, index adjusted");
        g.groups.push(group);
        to.push(g.groups.len() - 1);
        self.save_or_log();
        Ok(NodePath::group(&to))
    }

    /// Exporta o grupo `groups` (a raiz é `&[]`), com subgrupos e
    /// servidores, para um arquivo no mesmo formato de
    /// `registered_servers.json`. Senhas nunca vão para o arquivo: elas
    /// ficam só no cofre do sistema.
    pub fn export(&self, groups: &[usize], path: &Path) -> Result<(), String> {
        let root = self
            .group(groups)
            .ok_or(tr("The group no longer exists."))?
            .clone();
        let file = File {
            version: VERSION,
            environments: Some(self.environments()),
            root,
        };
        let text = serde_json::to_string_pretty(&file).expect("always serializable");
        std::fs::write(path, text).map_err(|e| {
            trf!(
                "Could not write {path}: {error}",
                path = path.display(),
                error = e
            )
        })
    }

    /// Importa um arquivo exportado para dentro do grupo `parent`. Um
    /// arquivo da árvore inteira junta os grupos e servidores dele aos de
    /// `parent`; o de um grupo entra como subgrupo. Grupos de mesmo nome
    /// se juntam e um servidor de mesmo nome é substituído. Devolve
    /// quantos servidores vieram.
    pub fn import(&mut self, parent: &[usize], path: &Path) -> Result<usize, String> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            trf!(
                "Could not read {path}: {error}",
                path = path.display(),
                error = e
            )
        })?;
        let file: File = serde_json::from_str(&text).map_err(|e| {
            trf!(
                "{path} is not a registered servers file: {error}",
                path = path.display(),
                error = e
            )
        })?;
        if file.version != VERSION {
            return Err(trf!(
                "{path} has an unsupported version ({version}).",
                path = path.display(),
                version = file.version
            ));
        }
        if let Some(envs) = &file.environments {
            self.merge_environments(envs);
        }
        let target = self
            .group_mut(parent)
            .ok_or(tr("The group no longer exists."))?;
        let count = if file.root.name == ROOT_NAME || file.root.name.trim().is_empty() {
            merge(target, file.root)
        } else {
            merge_group(target, file.root)
        };
        self.save_or_log();
        Ok(count)
    }

    /// Junta uma árvore inteira (a raiz de outro Registered Servers) ao
    /// grupo `parent`, como `import` faz com um arquivo. Devolve quantos
    /// servidores vieram.
    pub fn import_tree(&mut self, parent: &[usize], root: ServerGroup) -> Result<usize, String> {
        let target = self
            .group_mut(parent)
            .ok_or(tr("The group no longer exists."))?;
        let count = merge(target, root);
        self.save_or_log();
        Ok(count)
    }

    fn save_or_log(&self) {
        if let Err(e) = self.save()
            && let Some(path) = &self.path
        {
            eprintln!("registered servers: could not save {}: {e}", path.display());
        }
    }

    /// Grava o arquivo agora (as mudanças já gravam sozinhas).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = File {
            version: VERSION,
            environments: self.environments.clone(),
            root: self.root.clone(),
        };
        let text = serde_json::to_string_pretty(&file).expect("always serializable");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

/// Junta os subgrupos e servidores de `from` aos de `into`. Devolve
/// quantos servidores vieram.
fn merge(into: &mut ServerGroup, from: ServerGroup) -> usize {
    let mut count = 0;
    for group in from.groups {
        count += merge_group(into, group);
    }
    for server in from.servers {
        let server = clean(server);
        if server.name.is_empty() {
            continue;
        }
        match into
            .servers
            .iter_mut()
            .find(|s| s.name.eq_ignore_ascii_case(&server.name))
        {
            Some(old) => *old = server,
            None => into.servers.push(server),
        }
        count += 1;
    }
    count
}

/// Põe `group` dentro de `into`, juntando com um subgrupo de mesmo nome.
fn merge_group(into: &mut ServerGroup, group: ServerGroup) -> usize {
    let name = group.name.trim().to_string();
    if name.is_empty() {
        return merge(into, group);
    }
    let i = match into
        .groups
        .iter()
        .position(|g| g.name.eq_ignore_ascii_case(&name))
    {
        Some(i) => i,
        None => {
            into.groups.push(ServerGroup {
                name,
                description: group.description.trim().to_string(),
                color: group.color,
                environment: group.environment.clone(),
                ..Default::default()
            });
            into.groups.len() - 1
        }
    };
    merge(&mut into.groups[i], group)
}

fn clean(server: RegisteredServer) -> RegisteredServer {
    let name = match server.name.trim() {
        "" => server.settings.server_name(),
        n => n.to_string(),
    };
    RegisteredServer {
        name,
        description: server.description.trim().to_string(),
        ..server
    }
}

fn check_name<'a>(
    name: &str,
    mut siblings: impl Iterator<Item = &'a str>,
    what: &str,
) -> Result<(), String> {
    let group = what == "group";
    if name.is_empty() {
        return Err(if group {
            tr("Enter a group name.")
        } else {
            tr("Enter a server name.")
        }
        .to_string());
    }
    if siblings.any(|s| s.eq_ignore_ascii_case(name)) {
        return Err(if group {
            trf!(
                "A group named \"{name}\" already exists in this group.",
                name = name
            )
        } else {
            trf!(
                "A server named \"{name}\" already exists in this group.",
                name = name
            )
        });
    }
    Ok(())
}

/// Um servidor que a pesquisa achou.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerHit {
    /// Onde os termos apareceram fora do que a árvore já mostra (nome,
    /// pasta e ambiente): "servidor: db01 · banco: vendas". Vazio quando
    /// tudo foi achado no que já está na tela.
    pub detail: String,
}

/// O que a pesquisa achou na árvore.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerHits {
    /// Os servidores achados.
    pub servers: HashMap<NodePath, ServerHit>,
    /// As pastas que aparecem: as dos servidores achados, as de cima
    /// delas e as que têm o termo no próprio nome.
    pub groups: HashSet<Vec<usize>>,
    /// Os servidores achados, na ordem da árvore.
    pub order: Vec<NodePath>,
    /// Os termos pesquisados (sem acentos e em minúsculas).
    pub terms: Vec<String>,
}

impl RegisteredServers {
    /// Pesquisa `query` na árvore. Cada palavra precisa aparecer em algum
    /// lugar do servidor: nome, pasta (em qualquer nível), ambiente,
    /// descrição, host, porta, banco, usuário ou tipo do servidor. Sem
    /// diferença de maiúsculas e de acentos. A senha nunca entra: ela nem
    /// fica nesta árvore.
    pub fn search(&self, query: &str) -> ServerHits {
        let mut hits = ServerHits {
            terms: terms(query),
            ..Default::default()
        };
        if hits.terms.is_empty() {
            return hits;
        }
        let mut path = Vec::new();
        self.search_group(&self.root, &mut path, &[], &mut hits);
        hits
    }

    /// Devolve se a pasta aparece (ela ou algo dentro dela foi achado).
    fn search_group(
        &self,
        group: &ServerGroup,
        path: &mut Vec<usize>,
        folders: &[String],
        hits: &mut ServerHits,
    ) -> bool {
        // A pasta com todos os termos no caminho aparece mesmo vazia.
        let folded: Vec<String> = folders.iter().map(|f| fold(f)).collect();
        let mut shown = !path.is_empty()
            && hits
                .terms
                .iter()
                .all(|t| folded.iter().any(|f| f.contains(t.as_str())));
        for (i, sub) in group.groups.iter().enumerate() {
            path.push(i);
            let mut names = folders.to_vec();
            names.push(sub.name.clone());
            shown |= self.search_group(sub, path, &names, hits);
            path.pop();
        }
        for (i, server) in group.servers.iter().enumerate() {
            let node = NodePath::server(path, i);
            let environment = self.node_environment(&node).map(|e| e.name);
            if let Some(hit) = match_server(server, folders, environment.as_deref(), &hits.terms) {
                hits.servers.insert(node.clone(), hit);
                hits.order.push(node);
                shown = true;
            }
        }
        if shown {
            hits.groups.insert(path.clone());
        }
        shown
    }
}

/// Procura `terms` num servidor que está nas pastas `folders` e tem o
/// ambiente `environment` (o dele ou o da pasta).
fn match_server(
    server: &RegisteredServer,
    folders: &[String],
    environment: Option<&str>,
    terms: &[String],
) -> Option<ServerHit> {
    let s = &server.settings;
    // O que a árvore já mostra: achar aqui não precisa de explicação.
    let mut visible = vec![fold(&server.name)];
    visible.extend(folders.iter().map(|f| fold(f)));
    if let Some(e) = environment {
        visible.push(fold(e));
        visible.push(fold(environment_name(e)));
    }
    // O resto, com o nome do campo para mostrar embaixo do servidor.
    let port = s.port.to_string();
    let hidden = [
        (tr("server"), s.server_name()),
        (tr("server"), port),
        (tr("database"), s.database.clone()),
        (tr("user"), s.user.clone()),
        (tr("type"), s.engine.label().to_string()),
        (tr("description"), server.description.clone()),
    ];
    let mut parts: Vec<String> = Vec::new();
    let mut used = HashSet::new();
    for term in terms {
        if visible.iter().any(|v| v.contains(term.as_str())) {
            continue;
        }
        let (i, (label, value)) = hidden
            .iter()
            .enumerate()
            .find(|(_, (_, value))| fold(value).contains(term.as_str()))?;
        // A porta já aparece no nome do servidor quando não é a padrão.
        let (i, value) = if i == 1 {
            (0, &hidden[0].1)
        } else {
            (i, value)
        };
        if used.insert(i) {
            let value: String = value.split_whitespace().collect::<Vec<_>>().join(" ");
            parts.push(format!("{label}: {value}")); // i18n-ignore
        }
    }
    Some(ServerHit {
        detail: parts.join(" · "),
    })
}

/// O texto de `detail` com os termos achados em destaque.
fn detail_job(ui: &egui::Ui, detail: &str, terms: &[String]) -> egui::text::LayoutJob {
    let mut font = egui::TextStyle::Body.resolve(ui.style());
    font.size = (font.size * 0.9).round();
    let weak = egui::TextFormat::simple(font.clone(), ui.visuals().weak_text_color());
    let strong = egui::TextFormat {
        background: ui.visuals().selection.bg_fill.gamma_multiply(0.5),
        ..egui::TextFormat::simple(font, ui.visuals().strong_text_color())
    };
    // `fold` não muda a quantidade de caracteres: as posições valem.
    let chars: Vec<char> = detail.chars().collect();
    let folded: Vec<char> = fold(detail).chars().collect();
    let mut marked = vec![false; chars.len()];
    for term in terms {
        let term: Vec<char> = term.chars().collect();
        if term.is_empty() || term.len() > folded.len() {
            continue;
        }
        for at in 0..=folded.len() - term.len() {
            if folded[at..at + term.len()] == term[..] {
                marked[at..at + term.len()].fill(true);
            }
        }
    }
    let mut job = egui::text::LayoutJob::default();
    let mut from = 0;
    while from < chars.len() {
        let mut to = from + 1;
        while to < chars.len() && marked[to] == marked[from] {
            to += 1;
        }
        let text: String = chars[from..to].iter().collect();
        let format = if marked[from] {
            strong.clone()
        } else {
            weak.clone()
        };
        job.append(&text, 0.0, format);
        from = to;
    }
    job
}

mod explorer_rows;
pub use explorer_rows::{TreeMenu, TreeRow};

/// O que o usuário pediu na janela Registered Servers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Conectar o Object Explorer ao servidor (duplo clique ou Connect >
    /// Object Explorer).
    ConnectExplorer(ConnectionSettings),
    /// Conectar e abrir uma janela de consulta (Connect > New Query).
    NewQuery(ConnectionSettings),
    /// Tasks > Import... num grupo: escolher o arquivo e importar nele.
    Import(Vec<usize>),
    /// Tasks > Export... num grupo: escolher onde salvar o grupo.
    Export(Vec<usize>),
}

/// Janela Registered Servers: a árvore, a seleção e os diálogos.
pub struct RegisteredPanel {
    servers: RegisteredServers,
    selected: Option<NodePath>,
    /// Grupos recolhidos (os demais aparecem abertos).
    collapsed: HashSet<Vec<usize>>,
    server_dialog: Option<ServerDialog>,
    group_dialog: Option<GroupDialog>,
    environments_dialog: Option<EnvironmentsDialog>,
    confirm_delete: Option<NodePath>,
    /// Mensagem para a status bar vinda da árvore (arrastar).
    message: Option<String>,
    /// Conexão ativa, para preencher "New Server Registration".
    pub current: Option<ConnectionSettings>,
    /// O que está escrito no campo de pesquisa.
    pub filter: String,
}

impl Default for RegisteredPanel {
    fn default() -> Self {
        RegisteredPanel::new(RegisteredServers::default())
    }
}

impl RegisteredPanel {
    pub fn new(servers: RegisteredServers) -> Self {
        RegisteredPanel {
            servers,
            selected: None,
            collapsed: HashSet::new(),
            server_dialog: None,
            group_dialog: None,
            environments_dialog: None,
            confirm_delete: None,
            message: None,
            current: None,
            filter: String::new(),
        }
    }

    /// A árvore.
    pub fn servers(&self) -> &RegisteredServers {
        &self.servers
    }

    /// A árvore, para mudar sem os diálogos (testes e scripts).
    pub fn servers_mut(&mut self) -> &mut RegisteredServers {
        &mut self.servers
    }

    /// Nó selecionado.
    pub fn selected(&self) -> Option<&NodePath> {
        self.selected.as_ref()
    }

    /// Seleciona um nó.
    pub fn select(&mut self, path: NodePath) {
        self.selected = Some(path);
    }

    /// Diálogo de servidor aberto, se houver.
    pub fn server_dialog_mut(&mut self) -> Option<&mut ServerDialog> {
        self.server_dialog.as_mut()
    }

    /// Diálogo de grupo aberto, se houver.
    pub fn group_dialog_mut(&mut self) -> Option<&mut GroupDialog> {
        self.group_dialog.as_mut()
    }

    /// Se algum diálogo da janela está aberto.
    pub fn has_dialog(&self) -> bool {
        self.server_dialog.is_some()
            || self.group_dialog.is_some()
            || self.environments_dialog.is_some()
            || self.confirm_delete.is_some()
    }

    /// "New Server Registration..." no grupo `parent`, preenchido com a
    /// conexão ativa (ou o padrão).
    pub fn new_server(&mut self, parent: &[usize], store: Option<&Arc<dyn SecretStore>>) {
        let settings = self.current.clone().unwrap_or_default();
        let server = RegisteredServer {
            settings,
            ..Default::default()
        };
        let mut dialog = ServerDialog::new(parent.to_vec(), None, &server, store);
        dialog.environments = self.servers.environments();
        dialog.folder_environment = self.servers.group_environment(parent);
        dialog.folders = self.folders();
        self.server_dialog = Some(dialog);
    }

    /// As pastas para o campo "Folder" de um registro novo (vazio quando só
    /// existe a raiz).
    fn folders(&self) -> Vec<(Vec<usize>, usize, String, Option<Environment>)> {
        let groups = self.servers.all_groups();
        if groups.len() < 2 {
            return Vec::new();
        }
        groups
            .into_iter()
            .map(|(path, depth, name)| {
                let env = self.servers.group_environment(&path);
                (path, depth, name, env)
            })
            .collect()
    }

    /// "Register..." no servidor do Object Explorer: "New Server
    /// Registration" na raiz desta árvore (o campo "Folder" escolhe outra
    /// pasta), preenchido com a conexão dele.
    pub fn register(
        &mut self,
        settings: &ConnectionSettings,
        store: Option<&Arc<dyn SecretStore>>,
    ) {
        let server = RegisteredServer {
            settings: settings.clone(),
            ..Default::default()
        };
        let mut dialog = ServerDialog::new(Vec::new(), None, &server, store);
        dialog.environments = self.servers.environments();
        dialog.folder_environment = self.servers.group_environment(&[]);
        dialog.folders = self.folders();
        self.server_dialog = Some(dialog);
    }

    /// Como [`RegisteredPanel::register`], já com a pasta `parent`
    /// escolhida ("Move To" de um servidor ainda não registrado).
    pub fn register_in(
        &mut self,
        settings: &ConnectionSettings,
        parent: &[usize],
        store: Option<&Arc<dyn SecretStore>>,
    ) {
        self.register(settings, store);
        if let Some(d) = &mut self.server_dialog {
            d.parent = parent.to_vec();
            d.folder_environment = self.servers.group_environment(parent);
        }
    }

    /// O registro do servidor desta conexão: edita, se ele já está na
    /// árvore; se não, abre o "New Server Registration" preenchido.
    pub fn register_or_edit(
        &mut self,
        settings: &ConnectionSettings,
        store: Option<&Arc<dyn SecretStore>>,
    ) {
        match self.servers.path_for_key(&secrets::password_key(settings)) {
            Some(path) => {
                self.selected = Some(path.clone());
                self.edit_server(&path, store);
            }
            None => self.register(settings, store),
        }
    }

    /// Move um nó para o grupo `to` (o que arrastar faz), selecionando-o
    /// no lugar novo. Devolve a mensagem para a status bar.
    pub fn move_node(&mut self, from: &NodePath, to: &[usize]) -> String {
        let name = match from.server {
            Some(_) => self.servers.server(from).map(|s| s.name.clone()),
            None => self.servers.group(&from.groups).map(|g| g.name.clone()),
        }
        .unwrap_or_default();
        match self.servers.move_node(from, to) {
            Ok(path) if &path == from => String::new(),
            Ok(path) => {
                // Grupos recolhidos guardam caminhos: depois de mover um
                // grupo eles podem apontar para outro lugar.
                if from.server.is_none() {
                    self.collapsed.clear();
                }
                self.collapsed.remove(to);
                self.selected = Some(path);
                let group = self
                    .servers
                    .group(to)
                    .map(|g| g.name.clone())
                    .unwrap_or_default();
                trf!(
                    "Moved \"{name}\" to \"{group}\".",
                    name = name,
                    group = display_name(&group)
                )
            }
            Err(e) => e,
        }
    }

    /// "Properties" de um servidor registrado.
    pub fn edit_server(&mut self, path: &NodePath, store: Option<&Arc<dyn SecretStore>>) {
        if let Some(server) = self.servers.server(path) {
            let mut dialog =
                ServerDialog::new(path.groups.clone(), Some(path.clone()), server, store);
            dialog.environments = self.servers.environments();
            dialog.folder_environment = self.servers.group_environment(&path.groups);
            dialog.folders = self.folders();
            self.server_dialog = Some(dialog);
        }
    }

    /// "Environments..." no grupo raiz: a lista de ambientes.
    pub fn edit_environments(&mut self) {
        self.environments_dialog = Some(EnvironmentsDialog {
            rows: self
                .servers
                .environments()
                .into_iter()
                .map(|e| EnvironmentRow {
                    original: Some(e.name.clone()),
                    name: environment_name(&e.name).to_string(),
                    color: e.color,
                    confirm_changes: e.confirm_changes,
                    ai: e.ai_access(),
                    ai_chosen: e.ai.is_some(),
                    stored: e.name,
                })
                .collect(),
            error: None,
        });
    }

    /// Diálogo "Environments" aberto, se houver.
    pub fn environments_dialog_mut(&mut self) -> Option<&mut EnvironmentsDialog> {
        self.environments_dialog.as_mut()
    }

    /// "New Server Group..." dentro de `parent`.
    pub fn new_group(&mut self, parent: &[usize]) {
        self.group_dialog = Some(GroupDialog {
            parent: parent.to_vec(),
            editing: false,
            name: String::new(),
            description: String::new(),
            color: None,
            environment: None,
            environments: self.servers.environments(),
            error: None,
        });
    }

    fn edit_group(&mut self, groups: &[usize]) {
        let Some(g) = self.servers.group(groups) else {
            return;
        };
        self.group_dialog = Some(GroupDialog {
            parent: groups.to_vec(),
            editing: true,
            name: g.name.clone(),
            description: g.description.clone(),
            color: g.color,
            environment: g.environment.clone(),
            environments: self.servers.environments(),
            error: None,
        });
    }

    /// Desenha a árvore e devolve os pedidos de conexão.
    pub fn show(&mut self, ui: &mut egui::Ui, store: Option<&Arc<dyn SecretStore>>) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut menu: Option<Menu> = None;
        let hits = self.search_ui(ui, &mut actions);
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(icon::DESKTOP_TOWER);
                ui.strong(tr("Database Engine"));
            });
            self.group_rows(ui, &[], 0, hits.as_ref(), &mut actions, &mut menu);
            if hits.is_none()
                && self.servers.all_servers().is_empty()
                && self.servers.root().groups.is_empty()
            {
                ui.add_space(6.0);
                ui.weak(trf!(
                    "No registered servers. Right-click \"{group}\" and choose \"New Server Registration...\".",
                    group = display_name(&self.servers.root().name)
                ));
            }
        });
        match menu {
            Some(Menu::NewServer(parent)) => self.new_server(&parent, store),
            Some(Menu::NewGroup(parent)) => self.new_group(&parent),
            Some(Menu::Edit(path)) if path.server.is_some() => self.edit_server(&path, store),
            Some(Menu::Edit(path)) => self.edit_group(&path.groups),
            Some(Menu::Delete(path)) => self.confirm_delete = Some(path),
            Some(Menu::Environments) => self.edit_environments(),
            Some(Menu::Move(from, to)) => {
                let message = self.move_node(&from, &to);
                if !message.is_empty() {
                    self.message = Some(message);
                }
            }
            None => {}
        }
        actions
    }

    /// O campo de pesquisa e, embaixo, quantos servidores ele achou.
    /// Devolve o que foi achado quando há algo digitado. Enter conecta o
    /// primeiro servidor achado.
    fn search_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> Option<ServerHits> {
        // A largura visível: o painel pode ser mais largo do que aparece.
        let width = ui
            .available_width()
            .min(ui.clip_rect().right() - ui.cursor().left() - 4.0)
            .max(120.0);
        let mut connect_first = false;
        let row = egui::vec2(width, ui.spacing().interact_size.y);
        let layout = egui::Layout::right_to_left(egui::Align::Center);
        ui.allocate_ui_with_layout(row, layout, |ui| {
            ui.set_max_width(width);
            let clear = !self.filter.trim().is_empty()
                && ui
                    .add(egui::Button::new(icon::X).frame(false))
                    .on_hover_text(tr("Clear the search"))
                    .clicked();
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .id_salt("registered_servers_search")
                    .hint_text(format!(
                        "{} {}",
                        icon::MAGNIFYING_GLASS,
                        tr("Search servers by name, host or database")
                    ))
                    .desired_width(f32::INFINITY),
            );
            let field = field.on_hover_text(tr(
                "Searches the name, folder, environment, host, port, database, user and description. Enter connects to the first one found.",
            ));
            if field.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.filter.clear();
            }
            // O Enter é consumido: o diálogo de conexão que abre em seguida
            // não pode tomá-lo como um "Connect" antes de achar a senha.
            if field.lost_focus()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
            {
                connect_first = true;
            }
            if clear {
                self.filter.clear();
                field.request_focus();
            }
        });
        if self.filter.trim().is_empty() {
            return None;
        }
        let hits = self.servers.search(&self.filter);
        let text = match hits.order.len() {
            0 => tr("No servers found.").to_string(),
            1 => tr("1 server found.").to_string(),
            n => trf!("{n} servers found.", n = n),
        };
        ui.label(egui::RichText::new(text).small().weak());
        if connect_first
            && let Some(first) = hits.order.first()
            && let Some(server) = self.servers.server(first)
        {
            actions.push(Action::ConnectExplorer(server.settings.clone()));
            self.selected = Some(first.clone());
        }
        Some(hits)
    }

    fn group_rows(
        &mut self,
        ui: &mut egui::Ui,
        groups: &[usize],
        depth: usize,
        hits: Option<&ServerHits>,
        actions: &mut Vec<Action>,
        menu: &mut Option<Menu>,
    ) {
        let Some(group) = self.servers.group(groups) else {
            return;
        };
        // Na pesquisa só aparecem as pastas com algo achado, todas abertas.
        if let Some(hits) = hits
            && !groups.is_empty()
            && !hits.groups.contains(groups)
        {
            return;
        }
        let expanded = hits.is_some() || !self.collapsed.contains(groups);
        let here = NodePath::group(groups);
        let is_root = groups.is_empty();
        let name = group.name.clone();
        let description = group.description.clone();
        let (subgroups, servers) = (group.groups.len(), group.servers.clone());
        let color = self.servers.color(groups);
        let own_environment = group
            .environment
            .as_deref()
            .and_then(|n| self.servers.environment(n));
        let mut toggle = false;
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 16.0 + 8.0);
            let caret = if expanded {
                icon::CARET_DOWN
            } else {
                icon::CARET_RIGHT
            };
            if ui.add(egui::Button::new(caret).frame(false)).clicked() {
                toggle = true;
            }
            let folder = if expanded {
                icon::FOLDER_OPEN
            } else {
                icon::FOLDER
            };
            let sense = if is_root {
                egui::Sense::click()
            } else {
                egui::Sense::click_and_drag()
            };
            let text = icon_text(ui, folder, color, display_name(&name));
            let label = ui.add(
                egui::Button::selectable(self.selected.as_ref() == Some(&here), text)
                    .frame_when_inactive(false)
                    .sense(sense),
            );
            if !is_root {
                label.dnd_set_drag_payload(here.clone());
            }
            drop_target(ui, &label, &here, groups, menu);
            if let Some(e) = &own_environment {
                environment_badge(ui, environment_name(&e.name), e.color);
            }
            let label = if description.is_empty() {
                label
            } else {
                label.on_hover_text(&description)
            };
            if label.clicked() {
                self.selected = Some(here.clone());
            }
            if label.double_clicked() {
                toggle = true;
            }
            label.context_menu(|ui| {
                self.selected = Some(here.clone());
                if ui.button(tr("New Server Group...")).clicked() {
                    *menu = Some(Menu::NewGroup(groups.to_vec()));
                    ui.close();
                }
                if ui.button(tr("New Server Registration...")).clicked() {
                    *menu = Some(Menu::NewServer(groups.to_vec()));
                    ui.close();
                }
                if is_root && ui.button(tr("Environments...")).clicked() {
                    *menu = Some(Menu::Environments);
                    ui.close();
                }
                ui.separator();
                ui.menu_button(tr("Tasks"), |ui| {
                    if ui.button(tr("Import...")).clicked() {
                        actions.push(Action::Import(groups.to_vec()));
                        ui.close();
                    }
                    if ui.button(tr("Export...")).clicked() {
                        actions.push(Action::Export(groups.to_vec()));
                        ui.close();
                    }
                });
                if !is_root {
                    ui.separator();
                    self.move_to_menu(ui, &here, menu);
                    if ui.button(tr("Delete")).clicked() {
                        *menu = Some(Menu::Delete(here.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(tr("Properties")).clicked() {
                        *menu = Some(Menu::Edit(here.clone()));
                        ui.close();
                    }
                }
            });
        });
        if toggle && hits.is_none() {
            if expanded {
                self.collapsed.insert(groups.to_vec());
            } else {
                self.collapsed.remove(groups);
            }
        }
        if !expanded {
            return;
        }
        for i in 0..subgroups {
            let mut sub = groups.to_vec();
            sub.push(i);
            self.group_rows(ui, &sub, depth + 1, hits, actions, menu);
        }
        for (i, server) in servers.iter().enumerate() {
            let path = NodePath::server(groups, i);
            let hit = hits.and_then(|h| h.servers.get(&path));
            if hits.is_some() && hit.is_none() {
                continue;
            }
            // Onde a pesquisa achou, quando não foi no que a linha mostra.
            let detail = hit.filter(|h| !h.detail.is_empty()).map(|h| {
                let terms = hits.map(|h| h.terms.as_slice()).unwrap_or_default();
                (h.detail.clone(), terms)
            });
            ui.horizontal(|ui| {
                ui.add_space((depth + 1) as f32 * 16.0 + 8.0 + 14.0);
                let text = icon_text(ui, icon::DESKTOP_TOWER, color, &server.name);
                let label = ui.add(
                    egui::Button::selectable(self.selected.as_ref() == Some(&path), text)
                        .frame_when_inactive(false)
                        .sense(egui::Sense::click_and_drag()),
                );
                label.dnd_set_drag_payload(path.clone());
                // Soltar em cima de um servidor põe no grupo dele.
                drop_target(ui, &label, &path, groups, menu);
                // A etiqueta só aparece quando o servidor tem um ambiente
                // diferente do da pasta: o da pasta já está na linha dela.
                if let Some(e) = self.servers.node_environment(&path)
                    && Some(&e) != self.servers.group_environment(groups).as_ref()
                {
                    environment_badge(ui, environment_name(&e.name), e.color);
                }
                let tip = if server.description.is_empty() {
                    format!(
                        "{} ({})",
                        server.settings.server_name(),
                        server.settings.user
                    )
                } else {
                    format!(
                        "{} ({})\n{}",
                        server.settings.server_name(),
                        server.settings.user,
                        server.description
                    )
                };
                let label = label.on_hover_text(tip);
                if label.clicked() {
                    self.selected = Some(path.clone());
                }
                if label.double_clicked() {
                    actions.push(Action::ConnectExplorer(server.settings.clone()));
                }
                label.context_menu(|ui| {
                    self.selected = Some(path.clone());
                    ui.menu_button(tr("Connect"), |ui| {
                        if ui.button(tr("Object Explorer")).clicked() {
                            actions.push(Action::ConnectExplorer(server.settings.clone()));
                            ui.close();
                        }
                        if ui.button(tr("New Query")).clicked() {
                            actions.push(Action::NewQuery(server.settings.clone()));
                            ui.close();
                        }
                    });
                    ui.separator();
                    self.move_to_menu(ui, &path, menu);
                    if ui.button(tr("Delete")).clicked() {
                        *menu = Some(Menu::Delete(path.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(tr("Properties")).clicked() {
                        *menu = Some(Menu::Edit(path.clone()));
                        ui.close();
                    }
                });
            });
            if let Some((detail, terms)) = detail {
                ui.horizontal(|ui| {
                    ui.add_space((depth + 1) as f32 * 16.0 + 8.0 + 14.0 + 22.0);
                    let job = detail_job(ui, &detail, terms);
                    ui.add(egui::Label::new(job).selectable(false));
                });
            }
        }
    }

    /// "Move To" no menu de um servidor ou grupo: os grupos para onde ele
    /// pode ir, em árvore. O grupo onde ele já está e, para um grupo, ele
    /// mesmo e os de dentro ficam de fora.
    fn move_to_menu(&self, ui: &mut egui::Ui, node: &NodePath, menu: &mut Option<Menu>) {
        let current: &[usize] = match node.server {
            Some(_) => &node.groups,
            None => node.groups.split_last().map(|(_, p)| p).unwrap_or(&[]),
        };
        let targets: Vec<_> = self
            .servers
            .all_groups()
            .into_iter()
            .filter(|(g, ..)| {
                g.as_slice() != current && (node.server.is_some() || !g.starts_with(&node.groups))
            })
            .collect();
        ui.add_enabled_ui(!targets.is_empty(), |ui| {
            ui.menu_button(tr("Move To"), |ui| {
                for (groups, depth, name) in targets {
                    let color = self.servers.color(&groups);
                    let text = icon_text(ui, icon::FOLDER, color, display_name(&name));
                    let clicked = ui
                        .horizontal(|ui| {
                            ui.add_space(depth as f32 * 12.0);
                            ui.button(text).clicked()
                        })
                        .inner;
                    if clicked {
                        *menu = Some(Menu::Move(node.clone(), groups));
                        ui.close();
                    }
                }
            });
        });
    }

    /// Desenha os diálogos abertos. Devolve uma mensagem para a status bar
    /// quando algo mudou.
    pub fn show_windows(&mut self, ctx: &egui::Context) -> Option<String> {
        let mut status = self.message.take();
        if let Some(dialog) = &mut self.server_dialog {
            match dialog.show(ctx) {
                Reply::None => {}
                Reply::Cancel => self.server_dialog = None,
                Reply::Ok => {
                    let dialog = self.server_dialog.take().expect("dialog is open");
                    match dialog.apply(&mut self.servers) {
                        Ok(path) => {
                            status = Some(trf!(
                                "Registered server \"{name}\" saved.",
                                name = self
                                    .servers
                                    .server(&path)
                                    .map(|s| s.name.as_str())
                                    .unwrap_or("")
                            ));
                            self.selected = Some(path);
                        }
                        Err(failed) => {
                            let (mut dialog, e) = *failed;
                            dialog.error = Some(e);
                            self.server_dialog = Some(dialog);
                        }
                    }
                }
            }
        }
        if let Some(dialog) = &mut self.group_dialog {
            match dialog.show(ctx) {
                Reply::None => {}
                Reply::Cancel => self.group_dialog = None,
                Reply::Ok => {
                    let result = if dialog.editing {
                        self.servers
                            .update_group(
                                &dialog.parent,
                                &dialog.name,
                                &dialog.description,
                                dialog.color,
                                dialog.environment.clone(),
                            )
                            .map(|()| dialog.parent.clone())
                    } else {
                        self.servers.add_group(
                            &dialog.parent,
                            ServerGroup {
                                name: dialog.name.clone(),
                                description: dialog.description.clone(),
                                color: dialog.color,
                                environment: dialog.environment.clone(),
                                ..Default::default()
                            },
                        )
                    };
                    match result {
                        Ok(path) => {
                            status = Some(trf!(
                                "Server group \"{name}\" saved.",
                                name = dialog.name.trim()
                            ));
                            self.selected = Some(NodePath::group(&path));
                            self.group_dialog = None;
                        }
                        Err(e) => dialog.error = Some(e),
                    }
                }
            }
        }
        if let Some(dialog) = &mut self.environments_dialog {
            match dialog.show(ctx) {
                Reply::None => {}
                Reply::Cancel => self.environments_dialog = None,
                Reply::Ok => {
                    let (list, renamed) = dialog.result();
                    match self.servers.set_environments(list, &renamed) {
                        Ok(()) => {
                            status = Some(tr("Environments saved.").to_string());
                            self.environments_dialog = None;
                        }
                        Err(e) => dialog.error = Some(e),
                    }
                }
            }
        }
        if let Some(path) = self.confirm_delete.clone() {
            let name = match path.server {
                Some(_) => self.servers.server(&path).map(|s| s.name.clone()),
                None => self.servers.group(&path.groups).map(|g| g.name.clone()),
            };
            let Some(name) = name else {
                self.confirm_delete = None;
                return status;
            };
            let question = if path.server.is_some() {
                trf!(
                    "Are you sure you want to delete the server registration \"{name}\"?",
                    name = name
                )
            } else {
                trf!(
                    "Are you sure you want to delete the server group \"{name}\" and everything in it?",
                    name = display_name(&name)
                )
            };
            let mut answer = None;
            let modal = crate::theme::modal(egui::Id::new("registered_delete")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading(tr("Delete"));
                ui.add_space(4.0);
                ui.label(question);
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("No")).clicked() {
                        answer = Some(false);
                    }
                    if ui.button(tr("Yes")).clicked() {
                        answer = Some(true);
                    }
                });
            });
            if modal.should_close() && answer.is_none() {
                answer = Some(false);
            }
            if let Some(yes) = answer {
                self.confirm_delete = None;
                if yes && self.servers.remove(&path) {
                    self.selected = None;
                    status = Some(trf!("Deleted \"{name}\".", name = name));
                }
            }
        }
        status
    }
}

/// Escolha de cor: "Nenhuma", as cores prontas e uma livre.
fn color_choice(ui: &mut egui::Ui, color: &mut Option<Rgb>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if ui
            .selectable_label(color.is_none(), trc("color", "None"))
            .clicked()
        {
            *color = None;
        }
        for c in FOLDER_COLORS {
            let selected = *color == Some(c);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, c.hex())
            });
            ui.painter().rect_filled(rect.shrink(2.0), 3.0, c.color32());
            if selected || response.hovered() {
                ui.painter().rect_stroke(
                    rect,
                    3.0,
                    ui.visuals().selection.stroke,
                    egui::StrokeKind::Inside,
                );
            }
            if response.on_hover_text(c.hex()).clicked() {
                *color = Some(c);
            }
        }
        let mut rgb = color.unwrap_or(FOLDER_COLORS[5]).0;
        if egui::color_picker::color_edit_button_srgb(ui, &mut rgb)
            .on_hover_text(tr("Other color..."))
            .changed()
        {
            *color = Some(Rgb(rgb));
        }
    });
}

/// Combo de ambiente. `None` é "Nenhum" ou, quando a pasta tem um
/// ambiente, "Da pasta (...)".
fn environment_choice(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut Option<String>,
    list: &[Environment],
    inherited: Option<&Environment>,
) -> egui::Response {
    let none = match inherited {
        Some(e) => trf!("From folder ({name})", name = environment_name(&e.name)),
        None => trc("environment", "None").to_string(),
    };
    let selected = match value {
        Some(name) => environment_name(name).to_string(),
        None => none.clone(),
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(260.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, none);
            for e in list {
                let text = icon_text(ui, icon::CIRCLE, Some(e.color), environment_name(&e.name));
                ui.selectable_value(value, Some(e.name.clone()), text);
            }
            // Um ambiente que não está mais na lista continua escolhido até
            // o usuário trocar.
            if let Some(name) = value.clone()
                && !list.iter().any(|e| e.name == name)
            {
                ui.selectable_value(value, Some(name.clone()), name);
            }
        })
        .response
}

/// Etiqueta do ambiente: o nome em letras pequenas, numa caixa da cor dele.
pub fn environment_badge(ui: &mut egui::Ui, name: &str, color: Rgb) -> egui::Response {
    let fill = color.color32();
    // Texto escuro em cor clara, claro em cor escura.
    let [r, g, b] = color.0;
    let light = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > 150.0;
    let text = if light {
        egui::Color32::from_rgb(0x1E, 0x1E, 0x1E)
    } else {
        egui::Color32::WHITE
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(5, 0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(name).strong().color(text))
        })
        .response
}

/// Ícone colorido seguido do nome, para as linhas da árvore.
fn icon_text(ui: &egui::Ui, glyph: &str, color: Option<Rgb>, text: &str) -> egui::WidgetText {
    let mut job = egui::text::LayoutJob::default();
    let mut glyph = egui::RichText::new(glyph);
    if let Some(c) = color {
        glyph = glyph.color(c.color32());
    }
    glyph.append_to(
        &mut job,
        ui.style(),
        egui::FontSelection::Default,
        egui::Align::Center,
    );
    // i18n-ignore
    egui::RichText::new(format!(" {text}")).append_to(
        &mut job,
        ui.style(),
        egui::FontSelection::Default,
        egui::Align::Center,
    );
    job.into()
}

enum Menu {
    Environments,
    NewServer(Vec<usize>),
    NewGroup(Vec<usize>),
    Edit(NodePath),
    Delete(NodePath),
    /// Arrastou o nó para o grupo.
    Move(NodePath, Vec<usize>),
}

/// Uma linha que aceita um nó arrastado: realça enquanto ele está em cima
/// e, ao soltar, pede para movê-lo para o grupo `group`.
fn drop_target(
    ui: &egui::Ui,
    label: &egui::Response,
    row: &NodePath,
    group: &[usize],
    menu: &mut Option<Menu>,
) {
    let Some(dragged) = label.dnd_hover_payload::<NodePath>() else {
        return;
    };
    if *dragged == *row {
        // Ainda em cima de onde saiu.
        return;
    }
    let allowed = dragged.server.is_some() || !group.starts_with(&dragged.groups);
    let stroke = if allowed {
        ui.visuals().selection.stroke
    } else {
        egui::Stroke::new(1.0, ui.visuals().error_fg_color)
    };
    ui.painter()
        .rect_stroke(label.rect, 2.0, stroke, egui::StrokeKind::Inside);
    if allowed && let Some(dragged) = label.dnd_release_payload::<NodePath>() {
        *menu = Some(Menu::Move((*dragged).clone(), group.to_vec()));
    }
}

enum Reply {
    None,
    Cancel,
    Ok,
}

/// "New Server Registration" e "Edit Server Registration Properties".
pub struct ServerDialog {
    parent: Vec<usize>,
    editing: Option<NodePath>,
    /// "Server type": PostgreSQL ou SQL Server.
    pub engine: Engine,
    pub server: String,
    pub user: String,
    pub password: String,
    pub remember: bool,
    pub database: String,
    pub ssl: SslMode,
    pub name: String,
    pub description: String,
    /// Ambiente do servidor (`None`: o da pasta).
    pub environment: Option<String>,
    environments: Vec<Environment>,
    /// Ambiente que a pasta dá ao servidor sem ambiente próprio.
    folder_environment: Option<Environment>,
    /// As pastas onde um registro novo pode ficar: caminho, profundidade,
    /// nome e o ambiente que a pasta dá. Vazio ao editar (aí vale "Move
    /// To") e quando só existe a raiz.
    folders: Vec<(Vec<usize>, usize, String, Option<Environment>)>,
    /// Uma connection string colada: preenche os campos acima e é limpa
    /// (a senha vai para o campo de senha, nunca para o arquivo).
    pub connection_string: String,
    connection_error: Option<String>,
    error: Option<String>,
    store: Option<Arc<dyn SecretStore>>,
    lookup: Option<Lookup>,
    saved: bool,
}

impl ServerDialog {
    fn new(
        parent: Vec<usize>,
        editing: Option<NodePath>,
        server: &RegisteredServer,
        store: Option<&Arc<dyn SecretStore>>,
    ) -> Self {
        let s = &server.settings;
        let lookup = store.map(|st| secrets::lookup(st, secrets::password_key(s)));
        ServerDialog {
            parent,
            editing,
            engine: s.engine,
            server: s.server_name(),
            user: s.user.clone(),
            password: String::new(),
            remember: false,
            database: s.database.clone(),
            // Conexões gravadas antes do S1.3 podem ter um modo que o motor não
            // oferece (Prefer no SQL Server): vale o equivalente.
            ssl: if SslMode::options(s.engine).contains(&s.ssl) {
                s.ssl
            } else {
                SslMode::Require
            },
            name: server.name.clone(),
            description: server.description.clone(),
            environment: server.environment.clone(),
            environments: default_environments(),
            folder_environment: None,
            folders: Vec::new(),
            connection_string: String::new(),
            connection_error: None,
            error: None,
            store: store.cloned(),
            lookup,
            saved: false,
        }
    }

    /// Mensagem de erro da última tentativa de salvar.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Erro da connection string colada, se ela não pôde ser lida.
    pub fn connection_error(&self) -> Option<&str> {
        self.connection_error.as_deref()
    }

    /// Lê `connection_string` e preenche servidor, login, senha, banco e
    /// SSL. A senha lida liga o "Remember password". Depois de ler, o campo
    /// da connection string fica vazio.
    pub fn apply_connection_string(&mut self) -> Result<(), String> {
        let parsed = match connection_string::parse(&self.connection_string) {
            Ok(p) => p,
            Err(e) => {
                self.connection_error = Some(e.clone());
                return Err(e);
            }
        };
        let s = parsed.settings;
        self.engine = s.engine;
        self.server = s.server_name();
        self.user = s.user;
        self.database = s.database;
        self.ssl = s.ssl;
        if let Some(p) = parsed.password {
            // A senha lembrada antes (de outro login) não vale mais.
            self.lookup = None;
            self.password = p;
            self.remember = true;
        }
        self.connection_string.clear();
        self.connection_error = None;
        Ok(())
    }

    /// Espera a leitura da senha guardada (testes).
    pub fn wait_lookup(&mut self) {
        if let Some(l) = self.lookup.take()
            && let Some(p) = l.wait()
            && self.password.is_empty()
        {
            self.password = p;
            self.remember = true;
            self.saved = true;
        }
    }

    fn poll_lookup(&mut self) {
        let Some(found) = self.lookup.as_ref().and_then(Lookup::poll) else {
            return;
        };
        self.lookup = None;
        if let Some(p) = found
            && self.password.is_empty()
        {
            self.password = p;
            self.remember = true;
            self.saved = true;
        }
    }

    fn settings(&self) -> Result<ConnectionSettings, String> {
        let mut s = ConnectionSettings {
            user: self.user.trim().to_string(),
            database: self.database.trim().to_string(),
            ssl: self.ssl,
            ..ConnectionSettings::for_engine(self.engine)
        };
        s.set_server_name(&self.server)?;
        if s.user.is_empty() {
            return Err(tr("Enter a login.").into());
        }
        if s.database.is_empty() {
            s.database = self.engine.default_database().into();
        }
        Ok(s)
    }

    /// Troca o "Server type", como no Connect to Server: o que estava no
    /// padrão do motor antigo vai para o padrão do novo, e a senha guardada
    /// é procurada de novo.
    pub fn set_engine(&mut self, engine: Engine) {
        let old = self.engine;
        if old == engine {
            return;
        }
        if self.user.trim().is_empty() || self.user.trim() == old.default_user() {
            self.user = engine.default_user().into();
        }
        if self.database.trim() == old.default_database() {
            self.database = engine.default_database().into();
        }
        if !SslMode::options(engine).contains(&self.ssl) {
            self.ssl = SslMode::Require;
        }
        self.engine = engine;
        if self.saved {
            self.password.clear();
            self.saved = false;
            self.remember = false;
        }
        self.lookup = match (&self.store, self.settings()) {
            (Some(st), Ok(s)) => Some(secrets::lookup(st, secrets::password_key(&s))),
            _ => None,
        };
    }

    /// Grava na árvore e no cofre. Em caso de erro devolve o diálogo para
    /// continuar aberto.
    fn apply(
        self,
        servers: &mut RegisteredServers,
    ) -> Result<NodePath, Box<(ServerDialog, String)>> {
        let settings = match self.settings() {
            Ok(s) => s,
            Err(e) => return Err(Box::new((self, e))),
        };
        let server = RegisteredServer {
            name: self.name.clone(),
            description: self.description.clone(),
            environment: self.environment.clone(),
            settings: settings.clone(),
        };
        let result = match &self.editing {
            // Editando, o campo "Folder" move o servidor de pasta.
            Some(path) => servers.update_server(path, server).and_then(|()| {
                if self.parent == path.groups {
                    Ok(path.clone())
                } else {
                    servers.move_node(path, &self.parent)
                }
            }),
            None => servers.add_server(&self.parent, server),
        };
        let path = match result {
            Ok(p) => p,
            Err(e) => return Err(Box::new((self, e))),
        };
        if let Some(store) = &self.store {
            let key = secrets::password_key(&settings);
            if self.remember && !self.password.is_empty() {
                secrets::save_in_background(store, key, Some(self.password.clone()));
            } else if !self.remember && self.saved {
                secrets::save_in_background(store, key, None);
            }
        }
        Ok(path)
    }

    fn show(&mut self, ctx: &egui::Context) -> Reply {
        self.poll_lookup();
        if self.lookup.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(20));
        }
        let mut reply = Reply::None;
        let title = if self.editing.is_some() {
            tr("Edit Server Registration Properties")
        } else {
            tr("New Server Registration")
        };
        let modal = crate::theme::modal(egui::Id::new("registered_server")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(title);
            ui.add_space(4.0);
            egui::Grid::new("registered_server_fields")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    let label = ui.label(tr("Connection string:"));
                    let hide = !self.connection_string.is_empty();
                    let changed = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.connection_string)
                                .password(hide)
                                .desired_width(300.0)
                                .hint_text(tr("paste postgresql://... to fill the fields")),
                        )
                        .labelled_by(label.id)
                        .changed();
                    if changed {
                        if self.connection_string.trim().is_empty() {
                            self.connection_error = None;
                        } else {
                            let _ = self.apply_connection_string();
                        }
                    }
                    ui.end_row();
                    if let Some(e) = &self.connection_error {
                        ui.label("");
                        ui.colored_label(ui.visuals().error_fg_color, e);
                        ui.end_row();
                    }

                    let label = ui.label(tr("Server type:"));
                    let mut engine = self.engine;
                    egui::ComboBox::from_id_salt("registered_engine")
                        .selected_text(engine.label())
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            for e in Engine::ALL {
                                ui.selectable_value(&mut engine, e, e.label());
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    if engine != self.engine {
                        self.set_engine(engine);
                    }
                    ui.end_row();

                    let label = ui.label(tr("Server name:"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.server)
                            .desired_width(300.0)
                            .hint_text(match self.engine {
                                Engine::PostgreSql => tr("host or host,port"),
                                Engine::SqlServer => tr("host, host,port or host\\instance"),
                            }),
                    )
                    .labelled_by(label.id);
                    ui.end_row();

                    let label = ui.label(tr("Login:"));
                    ui.add(egui::TextEdit::singleline(&mut self.user).desired_width(300.0))
                        .labelled_by(label.id);
                    ui.end_row();

                    let label = ui.label(tr("Password:"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.password)
                            .password(true)
                            .desired_width(300.0),
                    )
                    .labelled_by(label.id);
                    ui.end_row();

                    ui.label("");
                    ui.checkbox(&mut self.remember, tr("Remember password"));
                    ui.end_row();

                    let label = ui.label(tr("Database:"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.database)
                            .desired_width(300.0)
                            .hint_text(self.engine.default_database()),
                    )
                    .labelled_by(label.id);
                    ui.end_row();

                    let label = ui.label(tr("Encryption:"));
                    egui::ComboBox::from_id_salt("registered_ssl")
                        .selected_text(self.ssl.label_for(self.engine))
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            for &mode in SslMode::options(self.engine) {
                                ui.selectable_value(
                                    &mut self.ssl,
                                    mode,
                                    mode.label_for(self.engine),
                                );
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    ui.end_row();

                    let label = ui.label(tr("Registered server name:"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.name)
                            .desired_width(300.0)
                            .hint_text(tr("same as the server name")),
                    )
                    .labelled_by(label.id);
                    ui.end_row();

                    if !self.folders.is_empty() {
                        let label = ui.label(tr("Folder:"));
                        let name = |n: &str| display_name(n).to_string();
                        let selected = self
                            .folders
                            .iter()
                            .find(|f| f.0 == self.parent)
                            .map(|f| name(&f.2))
                            .unwrap_or_default();
                        let mut chosen = None;
                        egui::ComboBox::from_id_salt("registered_server_folder")
                            .selected_text(selected)
                            .width(260.0)
                            .show_ui(ui, |ui| {
                                for (i, (path, depth, folder, _)) in self.folders.iter().enumerate()
                                {
                                    let text = format!("{}{}", "    ".repeat(*depth), name(folder));
                                    if ui.selectable_label(*path == self.parent, text).clicked() {
                                        chosen = Some(i);
                                    }
                                }
                            })
                            .response
                            .labelled_by(label.id);
                        if let Some(i) = chosen {
                            self.parent = self.folders[i].0.clone();
                            self.folder_environment = self.folders[i].3.clone();
                        }
                        ui.end_row();
                    }

                    let label = ui.label(tr("Environment:"));
                    environment_choice(
                        ui,
                        "registered_server_environment",
                        &mut self.environment,
                        &self.environments,
                        self.folder_environment.as_ref(),
                    )
                    .labelled_by(label.id);
                    ui.end_row();

                    let label = ui.label(tr("Registered server description:"));
                    ui.add(
                        egui::TextEdit::multiline(&mut self.description)
                            .desired_rows(2)
                            .desired_width(300.0),
                    )
                    .labelled_by(label.id);
                    ui.end_row();
                });
            if let Some(error) = &self.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Cancel")).clicked() {
                    reply = Reply::Cancel;
                }
                if ui.button(tr("Save")).clicked() {
                    reply = Reply::Ok;
                }
            });
        });
        if modal.should_close() && matches!(reply, Reply::None) {
            reply = Reply::Cancel;
        }
        reply
    }
}

/// Uma linha do diálogo "Environments".
pub struct EnvironmentRow {
    /// Nome no arquivo antes de abrir o diálogo (`None`: ambiente novo).
    original: Option<String>,
    /// Nome no arquivo, para comparar com o editado: um padrão aparece
    /// traduzido e, sem mudança, continua gravado em inglês.
    stored: String,
    pub name: String,
    pub color: Rgb,
    /// "Confirm changes": pedir confirmação antes de alterar o banco.
    pub confirm_changes: bool,
    /// O que a IA pode fazer nos servidores do ambiente.
    pub ai: AiAccess,
    /// O usuário escolheu `ai` (senão ela acompanha "Confirm changes").
    ai_chosen: bool,
}

/// "Environments": nome e cor de cada ambiente, adicionar e remover.
pub struct EnvironmentsDialog {
    pub rows: Vec<EnvironmentRow>,
    error: Option<String>,
}

impl EnvironmentsDialog {
    /// Mensagem de erro da última tentativa de salvar.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Acrescenta um ambiente novo no fim.
    pub fn add(&mut self, name: &str, color: Rgb) {
        self.rows.push(EnvironmentRow {
            original: None,
            stored: String::new(),
            name: name.into(),
            color,
            confirm_changes: false,
            ai: AiAccess::Free,
            ai_chosen: false,
        });
    }

    /// A lista nova e, para cada ambiente, o nome antigo.
    fn result(&self) -> (Vec<Environment>, Vec<Option<String>>) {
        self.rows
            .iter()
            .map(|r| {
                // Nome padrão que só aparece traduzido: grava em inglês.
                let name = if r.original.is_some() && r.name.trim() == environment_name(&r.stored) {
                    r.stored.clone()
                } else {
                    r.name.trim().to_string()
                };
                (
                    Environment {
                        name,
                        color: r.color,
                        confirm_changes: r.confirm_changes,
                        ai: r.ai_chosen.then_some(r.ai),
                    },
                    r.original.clone(),
                )
            })
            .unzip()
    }

    fn show(&mut self, ctx: &egui::Context) -> Reply {
        let mut reply = Reply::None;
        let modal = crate::theme::modal(egui::Id::new("registered_environments")).show(ctx, |ui| {
            ui.set_width(600.0);
            ui.heading(tr("Environments"));
            ui.add_space(4.0);
            ui.weak(tr(
                "Servers and folders can belong to an environment. Query windows show its color.",
            ));
            ui.add_space(6.0);
            let mut remove = None;
            egui::Grid::new("registered_environments_rows")
                .num_columns(5)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    for (i, row) in self.rows.iter_mut().enumerate() {
                        let mut rgb = row.color.0;
                        if egui::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                            row.color = Rgb(rgb);
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut row.name)
                                .id_salt(("environment_name", i))
                                .desired_width(200.0),
                        );
                        ui.checkbox(&mut row.confirm_changes, tr("Confirm changes"))
                            .on_hover_text(tr(
                                "Ask before running SQL that can change the database (INSERT, UPDATE, DELETE, DDL) in a query window.",
                            ));
                        if !row.ai_chosen {
                            row.ai = if row.confirm_changes {
                                AiAccess::Confirm
                            } else {
                                AiAccess::Free
                            };
                        }
                        let before = row.ai;
                        egui::ComboBox::from_id_salt(("environment_ai", i))
                            .selected_text(row.ai.label())
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for level in AiAccess::ALL {
                                    ui.selectable_value(&mut row.ai, level, level.label());
                                }
                            })
                            .response
                            .on_hover_text(tr(
                                "What Claude and the other AI providers may do on servers of this environment. Reads freely: reading is free and changes ask for confirmation. Asks first: reading the schema is free, and anything that runs SQL or brings data asks for confirmation. Blocked: the AI cannot reach these servers.",
                            ));
                        if row.ai != before {
                            row.ai_chosen = true;
                        }
                        if ui.button(icon::TRASH).on_hover_text(tr("Remove")).clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            if let Some(i) = remove {
                self.rows.remove(i);
            }
            if ui.button(tr("Add Environment")).clicked() {
                let color = FOLDER_COLORS[(self.rows.len() + 5) % FOLDER_COLORS.len()];
                self.add("", color);
            }
            if let Some(error) = &self.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Cancel")).clicked() {
                    reply = Reply::Cancel;
                }
                if ui.button(tr("OK")).clicked() {
                    reply = Reply::Ok;
                }
            });
        });
        if modal.should_close() && matches!(reply, Reply::None) {
            reply = Reply::Cancel;
        }
        reply
    }
}

/// "New Server Group Properties" e "Edit Server Group Properties".
pub struct GroupDialog {
    /// Grupo pai (novo) ou o próprio grupo (editando).
    parent: Vec<usize>,
    editing: bool,
    pub name: String,
    pub description: String,
    /// Cor da pasta (`None`: sem cor própria).
    pub color: Option<Rgb>,
    /// Ambiente da pasta (`None`: o da pasta de cima, se houver).
    pub environment: Option<String>,
    environments: Vec<Environment>,
    error: Option<String>,
}

impl GroupDialog {
    /// Mensagem de erro da última tentativa de salvar.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn show(&mut self, ctx: &egui::Context) -> Reply {
        let mut reply = Reply::None;
        let title = if self.editing {
            tr("Edit Server Group Properties")
        } else {
            tr("New Server Group Properties")
        };
        let modal = crate::theme::modal(egui::Id::new("registered_group")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading(title);
            ui.add_space(4.0);
            egui::Grid::new("registered_group_fields")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    let label = ui.label(tr("Group name:"));
                    ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(260.0))
                        .labelled_by(label.id);
                    ui.end_row();
                    let label = ui.label(tr("Group description:"));
                    ui.add(
                        egui::TextEdit::multiline(&mut self.description)
                            .desired_rows(2)
                            .desired_width(260.0),
                    )
                    .labelled_by(label.id);
                    ui.end_row();
                    ui.label(tr("Color:"));
                    color_choice(ui, &mut self.color);
                    ui.end_row();
                    let label = ui.label(tr("Environment:"));
                    environment_choice(
                        ui,
                        "registered_group_environment",
                        &mut self.environment,
                        &self.environments,
                        None,
                    )
                    .labelled_by(label.id);
                    ui.end_row();
                });
            if let Some(error) = &self.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Cancel")).clicked() {
                    reply = Reply::Cancel;
                }
                if ui.button(tr("OK")).clicked() {
                    reply = Reply::Ok;
                }
            });
        });
        if modal.should_close() && matches!(reply, Reply::None) {
            reply = Reply::Cancel;
        }
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, host: &str) -> RegisteredServer {
        RegisteredServer {
            name: name.into(),
            description: String::new(),
            environment: None,
            settings: ConnectionSettings {
                host: host.into(),
                ..Default::default()
            },
        }
    }

    /// SaaS (produção) > Jucesp, com servidores nos dois níveis.
    fn search_tree() -> RegisteredServers {
        let mut r = RegisteredServers::default();
        let saas = r
            .add_group(
                &[],
                ServerGroup {
                    name: "SaaS".into(),
                    environment: Some(PRODUCTION.into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let jucesp = r
            .add_group(
                &saas,
                ServerGroup {
                    name: "Jucesp".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        r.add_group(
            &[],
            ServerGroup {
                name: "Vazia".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut tiib = server("Tiib@Production", "db-prod.exemplo.com");
        tiib.settings.database = "tiib".into();
        tiib.settings.user = "app_tiib".into();
        tiib.description = "Aplicação\nprincipal".into();
        r.add_server(&saas, tiib).unwrap();
        let mut webged = server("WebGed@Jucesp", "10.0.0.7");
        webged.settings.port = 5433;
        webged.settings.database = "webged".into();
        webged.environment = Some(STAGING.into());
        r.add_server(&jucesp, webged).unwrap();
        let mut local = server("local", "localhost");
        local.settings.engine = Engine::SqlServer;
        r.add_server(&[], local).unwrap();
        r
    }

    fn found(r: &RegisteredServers, query: &str) -> Vec<String> {
        let hits = r.search(query);
        hits.order
            .iter()
            .map(|p| r.server(p).unwrap().name.clone())
            .collect()
    }

    #[test]
    fn search_matches_name_folder_environment_and_connection_fields() {
        let r = search_tree();
        assert!(r.search("   ").servers.is_empty());
        // Nome, sem diferença de maiúsculas.
        assert_eq!(found(&r, "TIIB@prod"), ["Tiib@Production"]);
        // Pasta: tudo o que está nela e nas de dentro.
        assert_eq!(found(&r, "saas"), ["WebGed@Jucesp", "Tiib@Production"]);
        assert_eq!(found(&r, "jucesp"), ["WebGed@Jucesp"]);
        // Ambiente: o do servidor ganha do da pasta.
        assert_eq!(found(&r, "production saas"), ["Tiib@Production"]);
        assert_eq!(found(&r, "staging"), ["WebGed@Jucesp"]);
        // Host, porta, banco, usuário, tipo e descrição.
        assert_eq!(found(&r, "db-prod"), ["Tiib@Production"]);
        assert_eq!(found(&r, "5433"), ["WebGed@Jucesp"]);
        assert_eq!(found(&r, "app_tiib"), ["Tiib@Production"]);
        assert_eq!(found(&r, "sql server"), ["local"]);
        assert_eq!(found(&r, "aplicacao"), ["Tiib@Production"]);
        // Todos os termos, cada um onde estiver.
        assert_eq!(found(&r, "saas webged 10.0"), ["WebGed@Jucesp"]);
        assert!(found(&r, "saas localhost").is_empty());
    }

    #[test]
    fn search_tells_where_it_matched_and_which_folders_to_show() {
        let r = search_tree();
        // Achou no que a árvore mostra: sem explicação.
        let hits = r.search("tiib@");
        assert_eq!(hits.servers[&hits.order[0]].detail, "");
        // Só as pastas do caminho aparecem.
        assert!(hits.groups.contains(&vec![]) && hits.groups.contains(&vec![0]));
        assert!(!hits.groups.contains(&vec![0, 0]) && !hits.groups.contains(&vec![1]));
        // Achou no host e no banco: diz onde, uma vez cada.
        let hits = r.search("10.0 5433 webged jucesp");
        assert_eq!(hits.servers[&hits.order[0]].detail, "server: 10.0.0.7,5433");
        let hits = r.search("exemplo app_ principal");
        assert_eq!(
            hits.servers[&hits.order[0]].detail,
            "server: db-prod.exemplo.com · user: app_tiib · description: Aplicação principal"
        );
        // Pasta vazia com o termo no nome aparece, sem servidores.
        let hits = r.search("vazia");
        assert!(hits.servers.is_empty());
        assert!(hits.groups.contains(&vec![1]) && !hits.groups.contains(&vec![0]));
    }

    #[test]
    fn folder_colors_are_saved_as_hex_and_inherited() {
        assert_eq!(Rgb::parse("#E5534b"), Some(Rgb([0xE5, 0x53, 0x4B])));
        assert_eq!(Rgb::parse("3fa34d").unwrap().hex(), "#3fa34d");
        assert_eq!(Rgb::parse("#12345"), None);
        assert_eq!(Rgb::parse("#gg0000"), None);

        let dir = std::env::temp_dir().join(format!("pgstudio-colors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("registered_servers.json");
        let mut r = RegisteredServers::open(&file);
        let prod = r
            .add_group(
                &[],
                ServerGroup {
                    name: "Production".into(),
                    color: Some(FOLDER_COLORS[0]),
                    ..Default::default()
                },
            )
            .unwrap();
        let eu = r
            .add_group(
                &prod,
                ServerGroup {
                    name: "EU".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let dev = r
            .add_group(
                &[],
                ServerGroup {
                    name: "Dev".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        // Subpasta sem cor usa a da pasta de cima; a raiz não tem cor.
        assert_eq!(r.color(&eu), Some(FOLDER_COLORS[0]));
        assert_eq!(r.color(&dev), None);
        assert_eq!(r.color(&[]), None);
        r.update_group(&dev, "Dev", "", Some(FOLDER_COLORS[3]), None)
            .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"color\": \"#e5534b\""), "{text}");
        assert!(text.contains("\"color\": \"#3fa34d\""), "{text}");
        // Pasta sem cor não grava a chave: arquivos antigos não mudam.
        assert_eq!(text.matches("\"color\"").count(), 2, "{text}");
        let again = RegisteredServers::open(&file);
        assert_eq!(again.root(), r.root());
        let groups: Vec<_> = again.all_groups().into_iter().collect();
        assert_eq!(
            groups,
            vec![
                (vec![], 0, ROOT_NAME.to_string()),
                (vec![0], 1, "Production".to_string()),
                (vec![0, 0], 2, "EU".to_string()),
                (vec![1], 1, "Dev".to_string()),
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ai_access_follows_confirm_changes_until_chosen() {
        // Production pergunta antes; os outros seguem a regra de sempre.
        let levels: Vec<_> = default_environments()
            .iter()
            .map(Environment::ai_access)
            .collect();
        assert_eq!(levels, [AiAccess::Free, AiAccess::Free, AiAccess::Confirm]);
        // Um arquivo antigo, sem a escolha, cai no mesmo padrão.
        let old: Environment =
            serde_json::from_str(r##"{"name":"Prod","color":"#c80000","confirm_changes":true}"##)
                .unwrap();
        assert_eq!(old.ai, None);
        assert_eq!(old.ai_access(), AiAccess::Confirm);
        assert!(!serde_json::to_string(&old).unwrap().contains("\"ai\""));
        // A escolha do usuário vale e fica gravada.
        let chosen = Environment {
            ai: Some(AiAccess::Blocked),
            ..old
        };
        let text = serde_json::to_string(&chosen).unwrap();
        assert!(text.contains(r#""ai":"blocked""#), "{text}");
        let back: Environment = serde_json::from_str(&text).unwrap();
        assert_eq!(back.ai_access(), AiAccess::Blocked);

        // O diálogo só grava a escolha quando o usuário mexe nela.
        let mut dialog = EnvironmentsDialog {
            rows: Vec::new(),
            error: None,
        };
        dialog.add("QA", FOLDER_COLORS[2]);
        assert_eq!(dialog.result().0[0].ai, None);
        dialog.rows[0].ai = AiAccess::Blocked;
        dialog.rows[0].ai_chosen = true;
        assert_eq!(dialog.result().0[0].ai, Some(AiAccess::Blocked));
    }

    #[test]
    fn environments_are_inherited_renamed_and_saved_in_english() {
        let dir = std::env::temp_dir().join(format!("pgstudio-envs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("servers.json");
        let mut r = RegisteredServers::open(&file);
        assert_eq!(r.environments(), default_environments());
        // Só Production pede confirmação, de início.
        let confirm: Vec<_> = r
            .environments()
            .into_iter()
            .filter(|e| e.confirm_changes)
            .map(|e| e.name)
            .collect();
        assert_eq!(confirm, [PRODUCTION]);
        let prod = r
            .add_group(
                &[],
                ServerGroup {
                    name: "Clientes".into(),
                    environment: Some(PRODUCTION.into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let eu = r
            .add_group(
                &prod,
                ServerGroup {
                    name: "EU".into(),
                    color: Some(FOLDER_COLORS[5]),
                    ..Default::default()
                },
            )
            .unwrap();
        let a = r.add_server(&eu, server("a", "db-a")).unwrap();
        let b = r
            .add_server(
                &eu,
                RegisteredServer {
                    environment: Some(STAGING.into()),
                    ..server("b", "db-b")
                },
            )
            .unwrap();
        let loose = r.add_server(&[], server("c", "db-c")).unwrap();
        // "a" herda Production da pasta de cima; "b" tem o dele.
        assert_eq!(r.node_environment(&a).unwrap().name, PRODUCTION);
        assert_eq!(r.node_environment(&b).unwrap().name, STAGING);
        assert_eq!(r.node_environment(&loose), None);
        let tag = r.node_tag(&a).unwrap();
        assert_eq!(tag.environment.as_deref(), Some("Production"));
        assert_eq!(tag.color, FOLDER_COLORS[0], "environment color wins");
        assert_eq!(r.node_tag(&loose), None);
        assert_eq!(
            r.tag_for_key(&secrets::password_key(&r.server(&b).unwrap().settings))
                .unwrap()
                .color,
            FOLDER_COLORS[1]
        );
        // Sem ambiente, a cor da pasta.
        r.update_group(&prod, "Clientes", "", None, None).unwrap();
        assert_eq!(
            r.node_tag(&a),
            Some(ServerTag {
                environment: None,
                color: FOLDER_COLORS[5],
                confirm_changes: false,
                ai: AiAccess::Free,
            })
        );
        r.update_group(&prod, "Clientes", "", None, Some(PRODUCTION.into()))
            .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"environment\": \"Production\""), "{text}");
        // Com a lista padrão, o arquivo não grava a lista.
        assert!(!text.contains("\"environments\""), "{text}");

        // Renomear Production para "Prod" e apagar Staging.
        let mut list = r.environments();
        list[2].name = "Prod".into();
        list[2].color = FOLDER_COLORS[6];
        list.remove(1);
        r.set_environments(list, &[Some(DEVELOPMENT.into()), Some(PRODUCTION.into())])
            .unwrap();
        assert_eq!(r.node_environment(&a).unwrap().name, "Prod");
        assert_eq!(
            r.node_environment(&b).unwrap().name,
            "Prod",
            "Staging is gone"
        );
        assert_eq!(r.server(&b).unwrap().environment, None);
        let again = RegisteredServers::open(&file);
        assert_eq!(again.environments().len(), 2);
        assert_eq!(again.node_environment(&a).unwrap().color, FOLDER_COLORS[6]);
        assert!(
            r.set_environments(
                vec![
                    Environment {
                        name: "QA".into(),
                        color: FOLDER_COLORS[0],
                        confirm_changes: false,
                        ai: None,
                    },
                    Environment {
                        name: "qa".into(),
                        color: FOLDER_COLORS[1],
                        confirm_changes: false,
                        ai: None,
                    }
                ],
                &[None, None]
            )
            .is_err()
        );

        // Exportar leva a lista; importar junta os ambientes novos.
        let exported = dir.join("export.json");
        r.export(&[], &exported).unwrap();
        let mut other = RegisteredServers::in_memory(ServerGroup::default());
        other.import(&[], &exported).unwrap();
        let names: Vec<_> = other.environments().into_iter().map(|e| e.name).collect();
        assert_eq!(names, [DEVELOPMENT, STAGING, PRODUCTION, "Prod"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn environments_dialog_keeps_default_names_in_english() {
        let mut panel = RegisteredPanel::default();
        panel.edit_environments();
        let dialog = panel.environments_dialog.as_mut().unwrap();
        // O nome aparece traduzido; sem mudança, volta em inglês.
        dialog.rows[0].name = environment_name(DEVELOPMENT).to_string();
        dialog.add("QA", FOLDER_COLORS[5]);
        let (list, renamed) = dialog.result();
        assert_eq!(list[0].name, DEVELOPMENT);
        assert_eq!(list[3].name, "QA");
        assert_eq!(renamed[3], None);
        assert_eq!(renamed[0].as_deref(), Some(DEVELOPMENT));
    }

    #[test]
    fn files_without_colors_still_open() {
        // Um servers.json de antes das cores.
        let text = r#"{"version":1,"root":{"name":"x","groups":[{"name":"Prod","servers":[]}],"servers":[]}}"#;
        let file: File = serde_json::from_str(text).unwrap();
        assert_eq!(file.root.groups[0].color, None);
        // Uma cor ruim não perde a árvore inteira em silêncio: o arquivo
        // não abre (e o erro aparece no import).
        let bad = r#"{"version":1,"root":{"name":"x","groups":[{"name":"Prod","color":"red"}]}}"#;
        assert!(serde_json::from_str::<File>(bad).is_err());
    }

    #[test]
    fn groups_and_servers_nest_and_names_are_unique_per_group() {
        let mut r = RegisteredServers::default();
        let prod = r
            .add_group(
                &[],
                ServerGroup {
                    name: " Production ".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(r.group(&prod).unwrap().name, "Production");
        assert!(
            r.add_group(
                &[],
                ServerGroup {
                    name: "production".into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(r.add_group(&[], ServerGroup::default()).is_err());
        let eu = r
            .add_group(
                &prod,
                ServerGroup {
                    name: "EU".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(eu, vec![0, 0]);
        let a = r.add_server(&eu, server("", "db1.example.com")).unwrap();
        assert_eq!(r.server(&a).unwrap().name, "db1.example.com");
        assert!(
            r.add_server(&eu, server("DB1.example.com", "other"))
                .is_err()
        );
        r.add_server(&[], server("local", "localhost")).unwrap();
        let names: Vec<_> = r.all_servers().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["db1.example.com", "local"]);

        r.update_group(&eu, "Europe", "west", None, None).unwrap();
        assert_eq!(r.group(&eu).unwrap().description, "west");
        assert!(r.update_group(&[], "x", "", None, None).is_err());
        r.update_server(&a, server("primary", "db1.example.com"))
            .unwrap();
        assert_eq!(r.server(&a).unwrap().name, "primary");

        assert!(r.remove(&NodePath::group(&prod)));
        assert_eq!(r.all_servers().len(), 1);
        assert!(!r.remove(&NodePath::group(&[])));
    }

    fn group(name: &str) -> ServerGroup {
        ServerGroup {
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn moving_servers_and_groups_between_groups() {
        let mut r = RegisteredServers::default();
        let prod = r.add_group(&[], group("Production")).unwrap();
        let dev = r.add_group(&[], group("Dev")).unwrap();
        let eu = r.add_group(&prod, group("EU")).unwrap();
        let a = r.add_server(&prod, server("a", "a")).unwrap();
        r.add_server(&dev, server("b", "b")).unwrap();

        // Servidor para outro grupo.
        let moved = r.move_node(&a, &dev).unwrap();
        assert_eq!(moved, NodePath::server(&dev, 1));
        assert!(r.group(&prod).unwrap().servers.is_empty());
        // Nome repetido no destino é recusado e nada muda.
        r.add_server(&prod, server("b", "other")).unwrap();
        assert!(r.move_node(&NodePath::server(&prod, 0), &dev).is_err());
        assert_eq!(r.group(&prod).unwrap().servers.len(), 1);

        // Grupo não entra nele mesmo nem num filho; a raiz não se move.
        assert!(r.move_node(&NodePath::group(&prod), &prod).is_err());
        assert!(r.move_node(&NodePath::group(&prod), &eu).is_err());
        assert!(r.move_node(&NodePath::group(&[]), &dev).is_err());

        // "Production" (índice 0) para dentro de "Dev" (índice 1): o
        // caminho de "Dev" muda ao tirar "Production".
        let moved = r.move_node(&NodePath::group(&prod), &dev).unwrap();
        assert_eq!(moved, NodePath::group(&[0, 0]));
        assert_eq!(r.root().groups.len(), 1);
        assert_eq!(r.group(&[0]).unwrap().name, "Dev");
        assert_eq!(r.group(&[0, 0]).unwrap().name, "Production");
        assert_eq!(r.group(&[0, 0, 0]).unwrap().name, "EU");
        // E de volta para a raiz.
        let back = r.move_node(&moved, &[]).unwrap();
        assert_eq!(back, NodePath::group(&[1]));
        assert_eq!(r.group(&[1]).unwrap().servers[0].name, "b");
    }

    #[test]
    fn export_and_import_round_trip_without_passwords() {
        let dir = std::env::temp_dir().join(format!("pgstudio-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut r = RegisteredServers::default();
        let prod = r.add_group(&[], group("Production")).unwrap();
        let eu = r.add_group(&prod, group("EU")).unwrap();
        r.add_server(&eu, server("eu-db", "eu.example.com"))
            .unwrap();
        r.add_server(&prod, server("main", "main.example.com"))
            .unwrap();
        r.add_server(&[], server("local", "localhost")).unwrap();

        // A árvore inteira: importar numa árvore vazia devolve a mesma.
        let all = dir.join("all.json");
        r.export(&[], &all).unwrap();
        let text = std::fs::read_to_string(&all).unwrap();
        assert!(!text.to_lowercase().contains("password"), "{text}");
        let mut other = RegisteredServers::default();
        assert_eq!(other.import(&[], &all).unwrap(), 3);
        assert_eq!(other.root(), r.root());
        // De novo: junta os grupos e substitui os servidores, sem duplicar.
        assert_eq!(other.import(&[], &all).unwrap(), 3);
        assert_eq!(other.root(), r.root());

        // Um grupo só entra como subgrupo do grupo escolhido.
        let one = dir.join("prod.json");
        r.export(&prod, &one).unwrap();
        let mut other = RegisteredServers::default();
        let dev = other.add_group(&[], group("Dev")).unwrap();
        assert_eq!(other.import(&dev, &one).unwrap(), 2);
        assert_eq!(other.group(&[0, 0]).unwrap(), r.group(&prod).unwrap());

        // Uma senha num arquivo editado à mão é ignorada.
        let hand = dir.join("hand.json");
        std::fs::write(
            &hand,
            r#"{"version":1,"root":{"name":"Local Server Groups","servers":[
                {"name":"x","password":"pw","settings":{"host":"x","port":5432,
                 "database":"postgres","user":"u","password":"pw"}}]}}"#,
        )
        .unwrap();
        let mut other = RegisteredServers::default();
        assert_eq!(other.import(&[], &hand).unwrap(), 1);
        let dump = serde_json::to_string(other.root()).unwrap();
        assert!(!dump.contains("pw"), "{dump}");

        std::fs::write(&hand, "not json").unwrap();
        assert!(other.import(&[], &hand).is_err());
        assert!(other.import(&[], &dir.join("missing.json")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn survives_a_restart_without_passwords_and_ignores_garbage() {
        let dir = std::env::temp_dir().join(format!("pgstudio-registered-{}", std::process::id()));
        let path = dir.join("registered_servers.json");
        let _ = std::fs::remove_file(&path);
        let mut r = RegisteredServers::open(&path);
        assert_eq!(r.root().name, ROOT_NAME);
        let g = r
            .add_group(
                &[],
                ServerGroup {
                    name: "Dev".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        r.add_server(&g, server("dev db", "dev.example.com"))
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("password"), "{text}");
        let again = RegisteredServers::open(&path);
        assert_eq!(again.root(), r.root());

        std::fs::write(&path, "[1, 2").unwrap();
        assert!(RegisteredServers::open(&path).all_servers().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_a_registration_remembers_the_password() {
        let store: Arc<dyn SecretStore> = Arc::new(crate::secrets::MemoryStore::default());
        let mut panel = RegisteredPanel {
            current: Some(ConnectionSettings {
                host: "db".into(),
                user: "alice".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        panel.new_server(&[], Some(&store));
        let mut dialog = panel.server_dialog.take().unwrap();
        dialog.wait_lookup();
        assert_eq!(dialog.server, "db");
        dialog.password = "pw".into();
        dialog.remember = true;
        let path = dialog.apply(&mut panel.servers).ok().unwrap();
        assert_eq!(panel.servers.server(&path).unwrap().name, "db");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while store.get("alice@db:5432").unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        // Editar mostra a senha guardada; desmarcar esquece.
        panel.edit_server(&path, Some(&store));
        let mut dialog = panel.server_dialog.take().unwrap();
        dialog.wait_lookup();
        assert_eq!((dialog.password.as_str(), dialog.remember), ("pw", true));
        dialog.remember = false;
        dialog.apply(&mut panel.servers).ok().unwrap();
        while store.get("alice@db:5432").unwrap().is_some() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}
