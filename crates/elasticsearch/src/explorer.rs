//! Object Explorer do Elasticsearch: o cluster, os índices (com os campos
//! do mapeamento), os índices ocultos, os aliases e os data streams.
//!
//! | Nó | Id |
//! | --- | --- |
//! | servidor | `server` |
//! | grupos | `indices`, `hidden`, `aliases`, `streams` |
//! | índice | `i/<nome em hexadecimal>` |
//! | campo | `i/<índice>/f/<caminho em hexadecimal>` |
//! | alias | `a/<nome em hexadecimal>` |
//! | data stream | `d/<nome em hexadecimal>` |

use std::sync::Mutex;

use pgstudio_core::catalog::{Catalog, Column, Relation, RelationKind, Schema};
use pgstudio_core::connection::{ConnectionSettings, ELASTICSEARCH_DATABASE, Engine};
use pgstudio_core::metadata::{
    MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode, Property, ScriptAction,
};
use pgstudio_i18n::tr;

use crate::http::EsClient;
use crate::json::Json;

const SERVER: &str = "server";
const INDICES: &str = "indices";
const HIDDEN: &str = "hidden";
const ALIASES: &str = "aliases";
const STREAMS: &str = "streams";

/// Campos mostrados de um índice (um mapeamento pode ter milhares).
const MAX_FIELDS: usize = 2000;
/// Índices que o IntelliSense guarda.
const CATALOG_INDICES: usize = 500;

/// Um nó abaixo dos grupos.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    Index(String),
    Field(String, String),
    Alias(String),
    Stream(String),
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<String> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

fn place(node: &NodeId) -> Option<Place> {
    let parts: Vec<&str> = node.0.split('/').collect();
    match parts.as_slice() {
        ["i", index] => Some(Place::Index(unhex(index)?)),
        ["i", index, "f", field] => Some(Place::Field(unhex(index)?, unhex(field)?)),
        ["a", alias] => Some(Place::Alias(unhex(alias)?)),
        ["d", stream] => Some(Place::Stream(unhex(stream)?)),
        _ => None,
    }
}

/// Se `name` passa no filtro: um pedaço do nome, ou um padrão com `*`.
fn matches(filter: &str, name: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    let name = name.to_lowercase();
    if !filter.contains('*') {
        return name.contains(&filter);
    }
    let mut rest = name.as_str();
    let parts: Vec<&str> = filter.split('*').collect();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

/// Os campos de um mapeamento, com o caminho (`cliente.cidade`) e o tipo.
/// Os subcampos (`nome.keyword`) entram também: é por eles que se
/// ordena e agrega.
pub fn mapping_fields(properties: &Json, path: &str, out: &mut Vec<(String, String)>) {
    for (name, field) in properties.as_object().unwrap_or_default() {
        if out.len() >= MAX_FIELDS {
            return;
        }
        let full = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}.{name}")
        };
        let ty = field
            .get("type")
            .and_then(Json::as_str)
            .unwrap_or("object")
            .to_string();
        out.push((full.clone(), ty));
        if let Some(inner) = field.get("properties") {
            mapping_fields(inner, &full, out);
        }
        if let Some(sub) = field.get("fields") {
            mapping_fields(sub, &full, out);
        }
    }
}

/// Um tamanho em bytes como `1.5 MB`.
fn size(bytes: &str) -> String {
    let Ok(n) = bytes.parse::<f64>() else {
        return bytes.to_string();
    };
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < units.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

/// Lê os metadados pela API REST.
pub struct EsExplorer {
    client: EsClient,
    label: String,
    /// O filtro dos índices (grupo `indices`).
    filter: Mutex<String>,
}

impl EsExplorer {
    pub fn new(client: EsClient, settings: &ConnectionSettings, server_version: &str) -> Self {
        let label = format!(
            "{} (Elasticsearch {} - {})",
            settings.server_name(),
            server_version,
            settings.login_label()
        );
        EsExplorer {
            client,
            label,
            filter: Mutex::new(String::new()),
        }
    }

    fn get(&self, path: &str) -> Result<Json, MetadataError> {
        self.client.get_json(path).map_err(MetadataError)
    }

    fn current_filter(&self) -> String {
        self.filter.lock().map(|f| f.clone()).unwrap_or_default()
    }

    /// Os nomes dos índices: os comuns ou os ocultos (que começam com
    /// `.`), sem os que estão por trás de um data stream.
    fn index_names(&self, hidden: bool) -> Result<Vec<String>, MetadataError> {
        let list = self.get("/_cat/indices?format=json&h=index&expand_wildcards=all")?;
        let mut names: Vec<String> = list
            .as_array()
            .unwrap_or_default()
            .iter()
            .filter_map(|i| i.get("index").and_then(Json::as_str))
            .filter(|name| name.starts_with('.') == hidden)
            .map(str::to_string)
            .collect();
        names.sort();
        Ok(names)
    }

    fn indices(&self, hidden: bool) -> Result<Vec<ObjectNode>, MetadataError> {
        let filter = if hidden {
            String::new()
        } else {
            self.current_filter()
        };
        Ok(self
            .index_names(hidden)?
            .into_iter()
            .filter(|name| filter.trim().is_empty() || matches(&filter, name))
            .map(|name| ObjectNode {
                id: NodeId(format!("i/{}", hex(&name))),
                name,
                kind: NodeKind::Table,
                has_children: true,
            })
            .collect())
    }

    /// Os campos do mapeamento de `index`, em ordem.
    fn fields(&self, index: &str) -> Result<Vec<(String, String)>, MetadataError> {
        let mapping = self.get(&format!("/{index}/_mapping"))?;
        let mut out = Vec::new();
        for (_, body) in mapping.as_object().unwrap_or_default() {
            if let Some(properties) = body.at("mappings.properties") {
                mapping_fields(properties, "", &mut out);
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn aliases(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        let list = self.get("/_cat/aliases?format=json&h=alias")?;
        let mut names: Vec<&str> = list
            .as_array()
            .unwrap_or_default()
            .iter()
            .filter_map(|a| a.get("alias").and_then(Json::as_str))
            // Os aliases internos (`.kibana`, `.security`) não são do
            // usuário.
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort_unstable();
        names.dedup();
        Ok(names
            .into_iter()
            .map(|name| ObjectNode {
                id: NodeId(format!("a/{}", hex(name))),
                name: name.to_string(),
                kind: NodeKind::View,
                has_children: false,
            })
            .collect())
    }

    fn streams(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        // Um cluster sem o recurso (ou sem permissão) só fica sem a lista.
        let Ok(list) = self.get("/_data_stream") else {
            return Ok(Vec::new());
        };
        let mut names: Vec<&str> = list
            .get("data_streams")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|s| s.get("name").and_then(Json::as_str))
            .collect();
        names.sort_unstable();
        Ok(names
            .into_iter()
            .map(|name| ObjectNode {
                id: NodeId(format!("d/{}", hex(name))),
                name: name.to_string(),
                kind: NodeKind::View,
                has_children: false,
            })
            .collect())
    }

    /// O `PUT` que recria `index`: os mapeamentos e as configurações que
    /// o usuário pode escolher (sem o que o servidor preenche).
    fn create_script(&self, index: &str) -> Result<String, MetadataError> {
        let reply = self.get(&format!("/{index}"))?;
        let Some((_, info)) = reply.as_object().and_then(|o| o.first()) else {
            return Err(MetadataError(format!("no such index [{index}]")));
        };
        let mut body = Vec::new();
        if let Some(Json::Object(settings)) = info.at("settings.index") {
            let kept: Vec<(String, Json)> = settings
                .iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "creation_date" | "uuid" | "version" | "provided_name" | "routing"
                    )
                })
                .cloned()
                .collect();
            if !kept.is_empty() {
                body.push((
                    "settings".to_string(),
                    Json::Object(vec![("index".into(), Json::Object(kept))]),
                ));
            }
        }
        if let Some(mappings) = info.get("mappings")
            && mappings.as_object().is_some_and(|m| !m.is_empty())
        {
            body.push(("mappings".to_string(), mappings.clone()));
        }
        if let Some(aliases) = info.get("aliases")
            && aliases.as_object().is_some_and(|a| !a.is_empty())
        {
            body.push(("aliases".to_string(), aliases.clone()));
        }
        Ok(format!("PUT /{index}\n{}", Json::Object(body).pretty()))
    }
}

fn group(id: &str, name: &str) -> ObjectNode {
    ObjectNode {
        id: NodeId(id.into()),
        name: name.to_string(),
        kind: NodeKind::KeyGroup,
        has_children: true,
    }
}

/// A busca que abre os primeiros documentos de um índice.
pub fn search_script(target: &str, limit: u32) -> String {
    format!(
        "GET /{target}/_search\n{{\n  \"size\": {limit},\n  \"query\": {{ \"match_all\": {{}} }}\n}}"
    )
}

impl MetadataProvider for EsExplorer {
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        Ok(vec![ObjectNode {
            id: NodeId(SERVER.into()),
            name: self.label.clone(),
            kind: NodeKind::Server,
            has_children: true,
        }])
    }

    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError> {
        match parent.0.as_str() {
            SERVER => {
                return Ok(vec![
                    group(INDICES, tr("Indices")),
                    group(HIDDEN, tr("Hidden Indices")),
                    group(ALIASES, tr("Aliases")),
                    group(STREAMS, tr("Data Streams")),
                ]);
            }
            INDICES => return self.indices(false),
            HIDDEN => return self.indices(true),
            ALIASES => return self.aliases(),
            STREAMS => return self.streams(),
            _ => {}
        }
        match place(parent) {
            Some(Place::Index(index)) => Ok(self
                .fields(&index)?
                .into_iter()
                .map(|(field, ty)| ObjectNode {
                    id: NodeId(format!("i/{}/f/{}", hex(&index), hex(&field))),
                    name: format!("{field} ({ty})"),
                    kind: NodeKind::Column,
                    has_children: false,
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    fn relational(&self) -> bool {
        false
    }

    fn search_indices(&self) -> bool {
        true
    }

    fn filter(&self, node: &NodeId) -> Option<String> {
        (node.0 == INDICES).then(|| self.current_filter())
    }

    fn set_filter(&self, node: &NodeId, filter: &str) {
        if node.0 == INDICES
            && let Ok(mut f) = self.filter.lock()
        {
            *f = filter.trim().to_string();
        }
    }

    fn database_of(&self, _node: &NodeId) -> Option<String> {
        Some(ELASTICSEARCH_DATABASE.to_string())
    }

    /// Os índices, aliases e data streams para o IntelliSense, com os
    /// campos do mapeamento de cada índice.
    fn catalog(&self, database: &str) -> Result<Catalog, MetadataError> {
        let mut relations = Vec::new();
        let mapping = self.get("/_mapping?expand_wildcards=open")?;
        for (index, body) in mapping.as_object().unwrap_or_default() {
            if index.starts_with('.') || relations.len() >= CATALOG_INDICES {
                continue;
            }
            let mut fields = Vec::new();
            if let Some(properties) = body.at("mappings.properties") {
                mapping_fields(properties, "", &mut fields);
            }
            fields.sort();
            relations.push(Relation {
                name: index.clone(),
                kind: RelationKind::Table,
                columns: fields
                    .into_iter()
                    .map(|(name, ty)| Column { name, ty })
                    .collect(),
            });
        }
        relations.sort_by(|a, b| a.name.cmp(&b.name));
        for node in self.aliases()?.into_iter().chain(self.streams()?) {
            if !relations.iter().any(|r| r.name == node.name) {
                relations.push(Relation {
                    name: node.name,
                    kind: RelationKind::View,
                    columns: Vec::new(),
                });
            }
        }
        Ok(Catalog {
            engine: Engine::Elasticsearch,
            database: database.to_string(),
            search_path: vec![String::new()],
            schemas: vec![Schema {
                name: String::new(),
                relations,
                functions: Vec::new(),
            }],
        })
    }

    /// "Open Documents": a busca com limite. "Drop...": a requisição para
    /// conferir e rodar.
    fn script(&self, node: &NodeId, action: ScriptAction) -> Result<String, MetadataError> {
        let unavailable =
            || MetadataError(tr("Scripting is not available for this server.").to_string());
        match (place(node).ok_or_else(unavailable)?, action) {
            (
                Place::Index(name) | Place::Alias(name) | Place::Stream(name),
                ScriptAction::SelectTop(limit),
            ) => Ok(search_script(&name, limit)),
            (Place::Index(index), ScriptAction::Create) => self.create_script(&index),
            (Place::Index(index), ScriptAction::Drop) => Ok(format!("DELETE /{index}")),
            (Place::Alias(alias), ScriptAction::Create) => {
                let reply = self.get(&format!("/_alias/{alias}"))?;
                let actions: Vec<Json> = reply
                    .as_object()
                    .unwrap_or_default()
                    .iter()
                    .map(|(index, body)| {
                        let mut add = vec![
                            ("index".to_string(), Json::String(index.clone())),
                            ("alias".to_string(), Json::String(alias.clone())),
                        ];
                        // Filtro, roteamento e `is_write_index` do alias.
                        if let Some(Json::Object(options)) = body.at(&format!("aliases.{alias}")) {
                            add.extend(options.iter().cloned());
                        }
                        Json::Object(vec![("add".into(), Json::Object(add))])
                    })
                    .collect();
                Ok(format!(
                    "POST /_aliases\n{}",
                    Json::Object(vec![("actions".into(), Json::Array(actions))]).pretty()
                ))
            }
            (Place::Alias(alias), ScriptAction::Drop) => Ok(format!("DELETE /_all/_alias/{alias}")),
            (Place::Stream(stream), ScriptAction::Create) => {
                Ok(format!("PUT /_data_stream/{stream}"))
            }
            (Place::Stream(stream), ScriptAction::Drop) => {
                Ok(format!("DELETE /_data_stream/{stream}"))
            }
            _ => Err(unavailable()),
        }
    }

    fn properties(&self, node: &NodeId) -> Result<Vec<Property>, MetadataError> {
        let mut out = Vec::new();
        match place(node) {
            Some(Place::Index(index)) => {
                let list = self.get(&format!(
                    "/_cat/indices/{index}?format=json&bytes=b&expand_wildcards=all&h=health,status,pri,rep,docs.count,docs.deleted,store.size,pri.store.size,creation.date.string,uuid"
                ))?;
                let Some(row) = list.as_array().and_then(|l| l.first()) else {
                    return Ok(out);
                };
                let text = |field: &str| row.get(field).and_then(Json::cell);
                let mut push = |name: &str, value: Option<String>| {
                    if let Some(value) = value {
                        out.push(Property::new(name, value));
                    }
                };
                push(tr("Health"), text("health"));
                push(tr("Status"), text("status"));
                push(tr("Documents"), text("docs.count"));
                push(tr("Deleted documents"), text("docs.deleted"));
                push(tr("Storage size"), text("store.size").map(|s| size(&s)));
                push(
                    tr("Primary storage size"),
                    text("pri.store.size").map(|s| size(&s)),
                );
                push(tr("Primary shards"), text("pri"));
                push(tr("Replicas"), text("rep"));
                push(tr("Created"), text("creation.date.string"));
                push("UUID", text("uuid"));
            }
            Some(Place::Field(index, field)) => {
                let ty = self
                    .fields(&index)?
                    .into_iter()
                    .find(|(name, _)| *name == field)
                    .map(|(_, ty)| ty);
                out.push(Property::new(tr("Index"), index));
                if let Some(ty) = ty {
                    out.push(Property::new(tr("Data type"), ty));
                }
            }
            Some(Place::Alias(alias)) => {
                let reply = self.get(&format!("/_alias/{alias}"))?;
                let indices: Vec<&str> = reply
                    .as_object()
                    .unwrap_or_default()
                    .iter()
                    .map(|(index, _)| index.as_str())
                    .collect();
                out.push(Property::new(tr("Indices"), indices.join(", ")));
            }
            Some(Place::Stream(stream)) => {
                let reply = self.get(&format!("/_data_stream/{stream}"))?;
                if let Some(info) = reply
                    .get("data_streams")
                    .and_then(Json::as_array)
                    .and_then(|l| l.first())
                {
                    let text = |field: &str| info.get(field).and_then(Json::cell);
                    if let Some(status) = text("status") {
                        out.push(Property::new(tr("Health"), status.to_lowercase()));
                    }
                    if let Some(template) = text("template") {
                        out.push(Property::new(tr("Index template"), template));
                    }
                    let backing = info
                        .get("indices")
                        .and_then(Json::as_array)
                        .map_or(0, <[Json]>::len);
                    out.push(Property::new(tr("Backing indices"), backing.to_string()));
                }
            }
            None => {}
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_round_trip() {
        assert_eq!(
            place(&NodeId(format!("i/{}", hex("pedidos-2026")))),
            Some(Place::Index("pedidos-2026".into()))
        );
        assert_eq!(
            place(&NodeId(format!("i/{}/f/{}", hex("a"), hex("cliente.nome")))),
            Some(Place::Field("a".into(), "cliente.nome".into()))
        );
        assert_eq!(
            place(&NodeId(format!("a/{}", hex("atual")))),
            Some(Place::Alias("atual".into()))
        );
        assert_eq!(place(&NodeId("indices".into())), None);
        assert_eq!(place(&NodeId("i/zz".into())), None);
    }

    #[test]
    fn filters_match_a_piece_or_a_pattern() {
        assert!(matches("ped", "meus-pedidos"));
        assert!(matches("PED", "pedidos"));
        assert!(!matches("log", "pedidos"));
        assert!(matches("logs-*", "logs-2026.11"));
        assert!(!matches("logs-*", "app-logs-2026"));
        assert!(matches("*-2026", "logs-2026"));
        assert!(matches("logs-*-prod", "logs-2026-prod"));
        assert!(!matches("logs-*-prod", "logs-2026-dev"));
        assert!(matches("*", "x"));
    }

    #[test]
    fn mapping_fields_have_paths_and_types() {
        let properties = Json::parse(
            r#"{"nome":{"type":"text","fields":{"keyword":{"type":"keyword"}}},
                "cliente":{"properties":{"cidade":{"type":"keyword"}}},
                "itens":{"type":"nested","properties":{"sku":{"type":"keyword"}}}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        mapping_fields(&properties, "", &mut out);
        out.sort();
        assert_eq!(
            out,
            [
                ("cliente".to_string(), "object".to_string()),
                ("cliente.cidade".into(), "keyword".into()),
                ("itens".into(), "nested".into()),
                ("itens.sku".into(), "keyword".into()),
                ("nome".into(), "text".into()),
                ("nome.keyword".into(), "keyword".into()),
            ]
        );
    }

    #[test]
    fn sizes_and_scripts() {
        assert_eq!(size("512"), "512 B");
        assert_eq!(size("1536"), "1.5 KB");
        assert_eq!(size("?"), "?");
        assert_eq!(
            search_script("pedidos", 1000),
            "GET /pedidos/_search\n{\n  \"size\": 1000,\n  \"query\": { \"match_all\": {} }\n}"
        );
    }
}
