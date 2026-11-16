//! Object Explorer do Redis: o servidor, os bancos (`db0`, `db1`...) com
//! a contagem de chaves e, dentro de cada banco, as chaves agrupadas pelo
//! prefixo até o `:`.
//!
//! | Nó | Id |
//! | --- | --- |
//! | servidor | `server` |
//! | banco | `db/<n>` |
//! | grupo de chaves | `db/<n>/g/<prefixo em hexadecimal>` |
//! | chave | `db/<n>/k/<chave em hexadecimal>` |
//! | aviso (filtro, lista cortada) | `db/<n>/note/...` |
//!
//! As chaves vêm do `SCAN`, que não trava o servidor como o `KEYS`. Cada
//! leitura para em [`SCAN_LIMIT`] chaves; o filtro do banco (menu
//! "Filter Keys...") estreita a busca quando há mais.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use pgstudio_core::catalog::{Catalog, Column, Relation, RelationKind, Schema};
use pgstudio_core::connection::{ConnectionSettings, Engine, redis_argument, redis_database_index};
use pgstudio_core::metadata::{
    MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode, Property, ScriptAction,
};
use pgstudio_i18n::{tr, trf};
use pgstudio_pg_client::DbRuntime;

use crate::Connection;
use crate::resp::{Value, bytes_to_text};

const SERVER: &str = "server";
/// Chaves lidas do servidor a cada vez que um nó abre.
pub const SCAN_LIMIT: usize = 20_000;
/// Chaves pedidas a cada `SCAN`.
const SCAN_COUNT: &str = "1000";
/// Chaves que o IntelliSense guarda de cada banco.
pub const CATALOG_KEYS: usize = 5_000;
/// Tipos pedidos de uma vez (um `TYPE` por chave, em sequência).
const TYPE_BATCH: usize = 500;

/// Um nó dentro de um banco.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    Database,
    Group(Vec<u8>),
    Key(Vec<u8>),
}

/// Lê os metadados com a conexão aberta pelo Connect to Server.
pub struct RedisExplorer {
    runtime: DbRuntime,
    connection: tokio::sync::Mutex<Connection>,
    label: String,
    /// Filtro das chaves de cada banco, como o usuário escreveu.
    filters: std::sync::Mutex<HashMap<u32, String>>,
}

impl RedisExplorer {
    pub fn new(connection: Connection, settings: &ConnectionSettings, runtime: DbRuntime) -> Self {
        let user = if settings.user.trim().is_empty() {
            "default"
        } else {
            settings.user.trim()
        };
        let label = format!(
            "{} (Redis {} - {})",
            settings.server_name(),
            connection.server_version(),
            user
        );
        RedisExplorer {
            runtime,
            connection: tokio::sync::Mutex::new(connection),
            label,
            filters: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Nome do servidor na árvore: `host (Redis 7.2.4 - default)`.
    pub fn server_label(&self) -> &str {
        &self.label
    }

    fn query(&self, args: &[&str]) -> Result<Value, MetadataError> {
        self.runtime
            .block_on(async { self.connection.lock().await.query(args).await })
            .map_err(MetadataError)
    }

    /// Chaves por banco, do `INFO keyspace` (só os bancos com chaves
    /// aparecem lá).
    fn key_counts(&self) -> Result<Vec<(u32, u64)>, MetadataError> {
        let info = match self.query(&["INFO", "keyspace"])? {
            Value::Bulk(b) => bytes_to_text(&b),
            _ => String::new(),
        };
        Ok(parse_keyspace(&info))
    }

    fn databases(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        let count = self
            .runtime
            .block_on(async { self.connection.lock().await.database_count().await })
            .map_err(MetadataError)?;
        let keys = self.key_counts()?;
        Ok((0..count)
            .map(|i| {
                let n = keys.iter().find(|(db, _)| *db == i).map_or(0, |(_, n)| *n);
                ObjectNode {
                    id: NodeId(format!("db/{i}")),
                    name: format!("db{i} ({n})"),
                    kind: NodeKind::Database,
                    has_children: n > 0,
                }
            })
            .collect())
    }

    fn filter_of(&self, db: u32) -> String {
        self.filters
            .lock()
            .map(|f| f.get(&db).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    /// Roda os comandos em `db`, um depois do outro, e devolve as
    /// respostas (um erro do servidor ou da rede interrompe).
    fn run(&self, db: u32, commands: &[Vec<Vec<u8>>]) -> Result<Vec<Value>, MetadataError> {
        self.runtime
            .block_on(async {
                let mut c = self.connection.lock().await;
                c.query(&["SELECT", &db.to_string()]).await?;
                let mut out = Vec::with_capacity(commands.len());
                for chunk in commands.chunks(TYPE_BATCH) {
                    for command in chunk {
                        c.send(command).await.map_err(|e| e.to_string())?;
                    }
                    for _ in chunk {
                        out.push(c.read().await.map_err(|e| e.to_string())?);
                    }
                }
                Ok(out)
            })
            .map_err(MetadataError)
    }

    /// As chaves de `db` que casam com `pattern`, em ordem, e se a
    /// leitura parou antes do fim por causa do [`SCAN_LIMIT`].
    fn scan(&self, db: u32, pattern: &[u8]) -> Result<(BTreeSet<Vec<u8>>, bool), MetadataError> {
        self.runtime
            .block_on(async {
                let mut c = self.connection.lock().await;
                c.query(&["SELECT", &db.to_string()]).await?;
                let mut keys = BTreeSet::new();
                let mut cursor = b"0".to_vec();
                loop {
                    let reply = c
                        .call(&[
                            b"SCAN".to_vec(),
                            cursor,
                            b"MATCH".to_vec(),
                            pattern.to_vec(),
                            b"COUNT".to_vec(),
                            SCAN_COUNT.as_bytes().to_vec(),
                        ])
                        .await
                        .map_err(|e| e.to_string())?;
                    if let Value::Error(m) = reply {
                        return Err(m);
                    }
                    let mut parts = reply.into_items().into_iter();
                    cursor = match parts.next() {
                        Some(Value::Bulk(b)) => b,
                        Some(other) => other.as_text().unwrap_or_default().into_bytes(),
                        None => b"0".to_vec(),
                    };
                    for key in parts.next().map(Value::into_items).unwrap_or_default() {
                        if let Value::Bulk(k) = key {
                            keys.insert(k);
                        }
                    }
                    if cursor == b"0" {
                        return Ok((keys, false));
                    }
                    if keys.len() >= SCAN_LIMIT {
                        return Ok((keys, true));
                    }
                }
            })
            .map_err(MetadataError)
    }

    /// Os filhos de um banco (`prefix` vazio) ou de um grupo: os grupos do
    /// nível seguinte e as chaves soltas, com o tipo de cada uma.
    fn keys(&self, db: u32, prefix: &[u8]) -> Result<Vec<ObjectNode>, MetadataError> {
        let filter = self.filter_of(db);
        let pattern = match (filter.trim(), prefix) {
            ("", []) => b"*".to_vec(),
            ("", p) => {
                let mut pattern = glob_escape(p);
                pattern.push(b'*');
                pattern
            }
            (f, _) => filter_pattern(f),
        };
        let (keys, cut) = self.scan(db, &pattern)?;
        let level = group(keys.iter().filter(|k| k.starts_with(prefix)), prefix.len());
        let leaves: Vec<&Vec<u8>> = level
            .iter()
            .filter_map(|e| match e {
                Entry::Key(k) => Some(*k),
                Entry::Group(..) => None,
            })
            .collect();
        let types = self.run(
            db,
            &leaves
                .iter()
                .map(|k| vec![b"TYPE".to_vec(), (*k).clone()])
                .collect::<Vec<_>>(),
        )?;
        let mut types = types.into_iter();
        let mut nodes = Vec::with_capacity(level.len() + 2);
        if prefix.is_empty() && !filter.trim().is_empty() {
            nodes.push(note(
                db,
                "filter",
                trf!("Filter: {filter}", filter = filter.trim()),
            ));
        }
        for entry in level {
            nodes.push(match entry {
                Entry::Group(p, n) => ObjectNode {
                    id: NodeId(format!("db/{db}/g/{}", hex(&p))),
                    name: format!("{}* ({n}{})", bytes_to_text(&p), if cut { "+" } else { "" }),
                    kind: NodeKind::KeyGroup,
                    has_children: true,
                },
                Entry::Key(k) => {
                    let kind = types.next().and_then(|t| t.as_text()).unwrap_or_default();
                    ObjectNode {
                        id: NodeId(format!("db/{db}/k/{}", hex(k))),
                        name: format!("{} ({kind})", bytes_to_text(k)),
                        kind: NodeKind::Key,
                        has_children: false,
                    }
                }
            });
        }
        if cut {
            nodes.push(note(
                db,
                &format!("more/{}", hex(prefix)),
                trf!(
                    "Only the first {n} keys were read. Use Filter Keys to narrow.",
                    n = SCAN_LIMIT
                ),
            ));
        }
        Ok(nodes)
    }

    fn key_type(&self, db: u32, key: &[u8]) -> Result<String, MetadataError> {
        let reply = self.run(db, &[vec![b"TYPE".to_vec(), key.to_vec()]])?;
        Ok(reply
            .first()
            .and_then(Value::as_text)
            .unwrap_or_else(|| "none".into()))
    }
}

fn note(db: u32, id: &str, text: String) -> ObjectNode {
    ObjectNode {
        id: NodeId(format!("db/{db}/note/{id}")),
        name: text,
        kind: NodeKind::Note,
        has_children: false,
    }
}

/// Uma linha de um nível da árvore de chaves.
#[derive(Debug, PartialEq, Eq)]
enum Entry<'a> {
    /// Prefixo (com o `:` do fim) e quantas chaves começam com ele.
    Group(Vec<u8>, usize),
    Key(&'a Vec<u8>),
}

/// Agrupa as chaves (em ordem) pelo trecho até o próximo `:` depois dos
/// `skip` bytes do nível de cima. Um prefixo com uma chave só não vira
/// grupo: a chave aparece direto.
fn group<'a>(keys: impl Iterator<Item = &'a Vec<u8>>, skip: usize) -> Vec<Entry<'a>> {
    let mut groups: BTreeMap<&'a [u8], (usize, &'a Vec<u8>)> = BTreeMap::new();
    let mut loose: Vec<&'a Vec<u8>> = Vec::new();
    for key in keys {
        let rest = &key[skip.min(key.len())..];
        match rest.iter().position(|b| *b == b':') {
            Some(i) if i + 1 < rest.len() => {
                let entry = groups.entry(&key[..skip + i + 1]).or_insert((0, key));
                entry.0 += 1;
            }
            _ => loose.push(key),
        }
    }
    let mut folders: Vec<(&[u8], usize)> = Vec::with_capacity(groups.len());
    for (prefix, (n, first)) in groups {
        if n == 1 {
            loose.push(first);
        } else {
            folders.push((prefix, n));
        }
    }
    folders.sort_by(|a, b| natural(a.0, b.0));
    loose.sort_by(|a, b| natural(a, b));
    let mut out: Vec<Entry<'a>> = Vec::with_capacity(folders.len() + loose.len());
    out.extend(
        folders
            .into_iter()
            .map(|(p, n)| Entry::Group(p.to_vec(), n)),
    );
    out.extend(loose.into_iter().map(Entry::Key));
    out
}

/// Ordem em que os números dentro do nome contam como números:
/// `sessao:2` antes de `sessao:10`.
fn natural(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let run = |s: &[u8], from: usize| {
                let end = from + s[from..].iter().take_while(|c| c.is_ascii_digit()).count();
                let start = from + s[from..end].iter().take_while(|c| **c == b'0').count();
                (start, end)
            };
            let (sa, ea) = run(a, i);
            let (sb, eb) = run(b, j);
            let order = (ea - sa)
                .cmp(&(eb - sb))
                .then_with(|| a[sa..ea].cmp(&b[sb..eb]));
            if order != Ordering::Equal {
                return order;
            }
            (i, j) = (ea, eb);
        } else if a[i] != b[j] {
            return a[i].cmp(&b[j]);
        } else {
            (i, j) = (i + 1, j + 1);
        }
    }
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| a.cmp(b))
}

/// O padrão do `SCAN` para o filtro que o usuário escreveu: como está
/// quando já é um padrão (`user:*`, `sess?on`); senão, "contém o texto".
fn filter_pattern(filter: &str) -> Vec<u8> {
    let filter = filter.trim();
    if filter.contains(['*', '?', '[']) {
        return filter.as_bytes().to_vec();
    }
    let mut out = vec![b'*'];
    out.extend(glob_escape(filter.as_bytes()));
    out.push(b'*');
    out
}

/// `bytes` como texto literal num padrão do `SCAN`.
fn glob_escape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        if matches!(b, b'*' | b'?' | b'[' | b']' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// A chave como argumento de um comando no editor: o texto dela, ou com
/// `\xNN` quando não é UTF-8.
fn key_argument(key: &[u8]) -> String {
    match std::str::from_utf8(key) {
        Ok(text) => redis_argument(text),
        Err(_) => {
            let mut out = String::from("\"");
            for &b in key {
                match b {
                    b'"' => out.push_str("\\\""),
                    b'\\' => out.push_str("\\\\"),
                    0x20..=0x7e => out.push(b as char),
                    _ => out.push_str(&format!("\\x{b:02x}")),
                }
            }
            out.push('"');
            out
        }
    }
}

/// O comando que mostra o valor de uma chave do tipo `kind`, com até
/// `limit` itens nas coleções.
fn read_command(kind: &str, key: &str, limit: u32) -> String {
    let last = limit.saturating_sub(1);
    match kind {
        "string" => format!("GET {key}"),
        "hash" => format!("HGETALL {key}"),
        "list" => format!("LRANGE {key} 0 {last}"),
        "set" => format!("SMEMBERS {key}"),
        "zset" => format!("ZRANGE {key} 0 {last} WITHSCORES"),
        "stream" => format!("XRANGE {key} - + COUNT {limit}"),
        "ReJSON-RL" => format!("JSON.GET {key}"),
        _ => format!("TYPE {key}"),
    }
}

/// O comando que conta os itens de uma chave do tipo `kind`.
fn length_command(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "string" => "STRLEN",
        "hash" => "HLEN",
        "list" => "LLEN",
        "set" => "SCARD",
        "zset" => "ZCARD",
        "stream" => "XLEN",
        _ => return None,
    })
}

/// `db0:keys=2,expires=0,avg_ttl=0` de cada banco com chaves.
fn parse_keyspace(info: &str) -> Vec<(u32, u64)> {
    info.lines()
        .filter_map(|line| {
            let (db, rest) = line.trim().split_once(':')?;
            let db = redis_database_index(db).filter(|_| db.starts_with("db"))?;
            let keys = rest
                .split(',')
                .find_map(|p| p.strip_prefix("keys="))?
                .parse()
                .ok()?;
            Some((db, keys))
        })
        .collect()
}

/// O número do banco de um nó `db/<n>...`.
fn database_index(node: &NodeId) -> Option<u32> {
    node.0.strip_prefix("db/")?.split('/').next()?.parse().ok()
}

/// O banco e o lugar de um nó dentro dele.
fn place(node: &NodeId) -> Option<(u32, Place)> {
    let mut parts = node.0.strip_prefix("db/")?.split('/');
    let db = parts.next()?.parse().ok()?;
    let place = match (parts.next(), parts.next()) {
        (None, _) => Place::Database,
        (Some("g"), Some(p)) => Place::Group(unhex(p)?),
        (Some("k"), Some(k)) => Place::Key(unhex(k)?),
        _ => return None,
    };
    Some((db, place))
}

impl MetadataProvider for RedisExplorer {
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        Ok(vec![ObjectNode {
            id: NodeId(SERVER.into()),
            name: self.label.clone(),
            kind: NodeKind::Server,
            has_children: true,
        }])
    }

    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError> {
        if parent.0 == SERVER {
            return self.databases();
        }
        match place(parent) {
            Some((db, Place::Database)) => self.keys(db, &[]),
            Some((db, Place::Group(prefix))) => self.keys(db, &prefix),
            _ => Ok(Vec::new()),
        }
    }

    fn relational(&self) -> bool {
        false
    }

    fn filter(&self, node: &NodeId) -> Option<String> {
        match place(node)? {
            (db, Place::Database) => Some(self.filter_of(db)),
            _ => None,
        }
    }

    fn set_filter(&self, node: &NodeId, filter: &str) {
        let (Some((db, Place::Database)), Ok(mut filters)) = (place(node), self.filters.lock())
        else {
            return;
        };
        match filter.trim() {
            "" => filters.remove(&db),
            f => filters.insert(db, f.to_string()),
        };
    }

    /// As chaves do banco para o IntelliSense: cada uma é uma "tabela"
    /// do schema sem nome, com o tipo dela como única coluna.
    fn catalog(&self, database: &str) -> Result<Catalog, MetadataError> {
        let db = redis_database_index(database).unwrap_or(0);
        let (keys, _) = self.scan(db, b"*")?;
        let mut keys: Vec<Vec<u8>> = keys
            .into_iter()
            .filter(|k| std::str::from_utf8(k).is_ok())
            .collect();
        keys.sort_by(|a, b| natural(a, b));
        keys.truncate(CATALOG_KEYS);
        let types = self.run(
            db,
            &keys
                .iter()
                .map(|k| vec![b"TYPE".to_vec(), k.clone()])
                .collect::<Vec<_>>(),
        )?;
        let relations = keys
            .iter()
            .zip(types)
            .map(|(key, ty)| Relation {
                name: bytes_to_text(key),
                kind: RelationKind::Table,
                columns: vec![Column {
                    name: "value".into(),
                    ty: ty.as_text().unwrap_or_default(),
                }],
            })
            .collect();
        Ok(Catalog {
            engine: Engine::Redis,
            database: format!("db{db}"),
            search_path: vec![String::new()],
            schemas: vec![Schema {
                name: String::new(),
                relations,
                functions: Vec::new(),
            }],
        })
    }

    fn database_of(&self, node: &NodeId) -> Option<String> {
        database_index(node).map(|n| format!("db{n}"))
    }

    /// "Open Value" de uma chave: o comando de leitura do tipo dela.
    /// "Delete Key": o `DEL`, para o usuário conferir e rodar.
    fn script(&self, node: &NodeId, action: ScriptAction) -> Result<String, MetadataError> {
        let Some((db, Place::Key(key))) = place(node) else {
            return Err(MetadataError(
                tr("Scripting is not available for this server.").to_string(),
            ));
        };
        let name = key_argument(&key);
        match action {
            ScriptAction::SelectTop(limit) => {
                let kind = self.key_type(db, &key)?;
                if kind == "none" {
                    return Err(MetadataError(
                        tr("The key no longer exists. Refresh the list.").to_string(),
                    ));
                }
                Ok(read_command(&kind, &name, limit))
            }
            ScriptAction::Drop => Ok(format!("DEL {name}")),
            ScriptAction::Create => Err(MetadataError(
                tr("Scripting is not available for this server.").to_string(),
            )),
        }
    }

    fn properties(&self, node: &NodeId) -> Result<Vec<Property>, MetadataError> {
        match place(node) {
            Some((db, Place::Database)) => {
                let keys = self
                    .key_counts()?
                    .into_iter()
                    .find(|(d, _)| *d == db)
                    .map_or(0, |(_, n)| n);
                Ok(vec![Property::new(tr("Keys"), keys.to_string())])
            }
            Some((db, Place::Key(key))) => {
                let kind = self.key_type(db, &key)?;
                let mut commands = vec![
                    vec![b"TTL".to_vec(), key.clone()],
                    vec![b"OBJECT".to_vec(), b"ENCODING".to_vec(), key.clone()],
                    vec![b"MEMORY".to_vec(), b"USAGE".to_vec(), key.clone()],
                ];
                if let Some(command) = length_command(&kind) {
                    commands.push(vec![command.as_bytes().to_vec(), key.clone()]);
                }
                let replies = self.run(db, &commands)?;
                // Um comando que o servidor não tem (ou proíbe) só deixa
                // a linha dele de fora.
                let text = |i: usize| {
                    replies
                        .get(i)
                        .filter(|v| !matches!(v, Value::Error(_) | Value::Null))
                        .and_then(Value::as_text)
                };
                let mut out = vec![
                    Property::new(tr("Key"), bytes_to_text(&key)),
                    Property::new(tr("Data type"), kind),
                ];
                if let Some(ttl) = text(0).and_then(|t| t.parse::<i64>().ok()) {
                    let value = match ttl {
                        -1 => tr("No expiry").to_string(),
                        -2 => tr("The key no longer exists. Refresh the list.").to_string(),
                        n => trf!("{n} s", n = n),
                    };
                    out.push(Property::new(tr("Expires in (TTL)"), value));
                }
                if let Some(length) = text(3) {
                    out.push(Property::new(tr("Length"), length));
                }
                if let Some(encoding) = text(1) {
                    out.push(Property::new(tr("Encoding"), encoding));
                }
                if let Some(bytes) = text(2) {
                    out.push(Property::new(tr("Memory (bytes)"), bytes));
                }
                Ok(out)
            }
            _ => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyspace_lines_are_read() {
        let info = "# Keyspace\r\ndb0:keys=12,expires=1,avg_ttl=0\r\ndb3:keys=1,expires=0\r\n";
        assert_eq!(parse_keyspace(info), vec![(0, 12), (3, 1)]);
        assert_eq!(parse_keyspace(""), vec![]);
        assert_eq!(database_index(&NodeId("db/3".into())), Some(3));
        assert_eq!(database_index(&NodeId("server".into())), None);
    }

    fn keys(names: &[&str]) -> Vec<Vec<u8>> {
        let mut keys: Vec<Vec<u8>> = names.iter().map(|n| n.as_bytes().to_vec()).collect();
        keys.sort();
        keys
    }

    #[test]
    fn keys_are_grouped_by_prefix() {
        let all = keys(&[
            "user:1",
            "user:2:name",
            "user:2:mail",
            "cart:9",
            "plain",
            "odd:",
        ]);
        let top = group(all.iter(), 0);
        assert_eq!(
            top,
            vec![
                Entry::Group(b"user:".to_vec(), 3),
                // Prefixo de uma chave só e chave que termina em `:`: soltas.
                Entry::Key(&b"cart:9".to_vec()),
                Entry::Key(&b"odd:".to_vec()),
                Entry::Key(&b"plain".to_vec()),
            ]
        );
        let users = group(all.iter().filter(|k| k.starts_with(b"user:")), 5);
        assert_eq!(
            users,
            vec![
                Entry::Group(b"user:2:".to_vec(), 2),
                Entry::Key(&b"user:1".to_vec()),
            ]
        );
    }

    #[test]
    fn numbers_in_names_sort_as_numbers() {
        let all = keys(&["s:10", "s:2", "s:1", "s:02", "s:a", "s:2:x", "s:2:y"]);
        let names: Vec<String> = group(all.iter(), 2)
            .into_iter()
            .map(|e| match e {
                Entry::Group(p, n) => format!("{}*{n}", bytes_to_text(&p)),
                Entry::Key(k) => bytes_to_text(k),
            })
            .collect();
        assert_eq!(names, ["s:2:*2", "s:1", "s:02", "s:2", "s:10", "s:a"]);
    }

    #[test]
    fn node_ids_round_trip() {
        assert_eq!(place(&NodeId("db/2".into())), Some((2, Place::Database)));
        let id = NodeId(format!("db/1/k/{}", hex(b"a/b:\xff")));
        assert_eq!(place(&id), Some((1, Place::Key(b"a/b:\xff".to_vec()))));
        let id = NodeId(format!("db/0/g/{}", hex(b"user:")));
        assert_eq!(place(&id), Some((0, Place::Group(b"user:".to_vec()))));
        assert_eq!(place(&NodeId("db/0/note/filter".into())), None);
        assert_eq!(place(&NodeId("db/0/k/zz".into())), None);
    }

    #[test]
    fn filters_and_commands() {
        assert_eq!(filter_pattern(" user:* "), b"user:*");
        assert_eq!(filter_pattern("sess"), b"*sess*");
        assert_eq!(glob_escape(b"a*b[c]"), b"a\\*b\\[c\\]");
        assert_eq!(key_argument(b"user:1"), "user:1");
        assert_eq!(key_argument(b"my key"), "\"my key\"");
        assert_eq!(key_argument(b"k\xff\""), "\"k\\xff\\\"\"");
        assert_eq!(read_command("list", "k", 1000), "LRANGE k 0 999");
        assert_eq!(read_command("zset", "k", 10), "ZRANGE k 0 9 WITHSCORES");
        assert_eq!(read_command("stream", "k", 10), "XRANGE k - + COUNT 10");
        assert_eq!(read_command("weird", "k", 10), "TYPE k");
    }
}
