//! Object Explorer do MongoDB: o servidor, os bancos, as coleções (e
//! views) de cada um e os índices de cada coleção.
//!
//! | Nó | Id |
//! | --- | --- |
//! | servidor | `server` |
//! | banco | `db/<nome em hexadecimal>` |
//! | coleção ou view | `db/<banco>/c/<nome em hexadecimal>` |
//! | índice | `db/<banco>/c/<coleção>/i/<nome em hexadecimal>` |

use mongodb::Client;
use mongodb::bson::{Bson, Document, doc};
use pgstudio_core::catalog::{Catalog, Column, Relation, RelationKind, Schema};
use pgstudio_core::connection::{ConnectionSettings, Engine, mongodb_collection, mongodb_string};
use pgstudio_core::metadata::{
    MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode, Property, ScriptAction,
};
use pgstudio_i18n::tr;
use pgstudio_pg_client::DbRuntime;

use crate::query::database_names;
use crate::render;

const SERVER: &str = "server";

/// Um nó abaixo do servidor.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    Database(String),
    Collection(String, String),
    Index(String, String, String),
}

/// Lê os metadados pelo `Client` da conexão.
pub struct MongoExplorer {
    client: Client,
    runtime: DbRuntime,
    label: String,
    /// O banco da conexão: aparece mesmo vazio (um banco do MongoDB só
    /// existe depois da primeira coleção).
    database: String,
}

impl MongoExplorer {
    pub fn new(
        client: Client,
        settings: &ConnectionSettings,
        server_version: &str,
        runtime: DbRuntime,
    ) -> Self {
        let label = format!(
            "{} (MongoDB {} - {})",
            settings.server_name(),
            server_version,
            settings.login_label()
        );
        MongoExplorer {
            client,
            runtime,
            label,
            database: settings.database.clone(),
        }
    }

    fn command(&self, database: &str, command: Document) -> Result<Document, MetadataError> {
        self.runtime
            .block_on(async { self.client.database(database).run_command(command).await })
            .map_err(|e| MetadataError(e.to_string()))
    }

    /// Todos os documentos de um comando com cursor.
    fn list(&self, database: &str, command: Document) -> Result<Vec<Document>, MetadataError> {
        self.runtime
            .block_on(async {
                let mut cursor = self
                    .client
                    .database(database)
                    .run_cursor_command(command)
                    .await?;
                let mut out = Vec::new();
                while cursor.advance().await? {
                    out.push(cursor.deserialize_current()?);
                }
                Ok::<_, mongodb::error::Error>(out)
            })
            .map_err(|e| MetadataError(e.to_string()))
    }

    fn databases(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        // Sem permissão para listar os bancos, só o da conexão aparece.
        let mut names = self
            .runtime
            .block_on(database_names(&self.client))
            .unwrap_or_default();
        if !names.contains(&self.database) {
            names.push(self.database.clone());
            names.sort();
        }
        Ok(names
            .into_iter()
            .map(|name| ObjectNode {
                id: NodeId(format!("db/{}", hex(&name))),
                name,
                kind: NodeKind::Database,
                has_children: true,
            })
            .collect())
    }

    fn collections(&self, database: &str) -> Result<Vec<ObjectNode>, MetadataError> {
        let mut found: Vec<(String, bool)> = self
            .list(database, doc! { "listCollections": 1, "nameOnly": true })?
            .iter()
            .filter_map(|c| {
                let name = c.get_str("name").ok()?.to_string();
                Some((name, c.get_str("type") == Ok("view")))
            })
            // As coleções internas das views não são do usuário.
            .filter(|(name, _)| name != "system.views")
            .collect();
        found.sort();
        Ok(found
            .into_iter()
            .map(|(name, view)| ObjectNode {
                id: NodeId(format!("db/{}/c/{}", hex(database), hex(&name))),
                name,
                kind: if view {
                    NodeKind::View
                } else {
                    NodeKind::Table
                },
                has_children: !view,
            })
            .collect())
    }

    fn index_list(&self, database: &str, collection: &str) -> Result<Vec<Document>, MetadataError> {
        self.list(database, doc! { "listIndexes": collection })
    }

    fn indexes(&self, database: &str, collection: &str) -> Result<Vec<ObjectNode>, MetadataError> {
        Ok(self
            .index_list(database, collection)?
            .iter()
            .filter_map(|index| {
                let name = index.get_str("name").ok()?;
                let keys = index
                    .get_document("key")
                    .map(|k| render::json(&Bson::Document(k.clone())))
                    .unwrap_or_default();
                Some(ObjectNode {
                    id: NodeId(format!(
                        "db/{}/c/{}/i/{}",
                        hex(database),
                        hex(collection),
                        hex(name)
                    )),
                    name: format!("{name} {keys}"),
                    kind: NodeKind::Index,
                    has_children: false,
                })
            })
            .collect())
    }
}

/// Coleções que o IntelliSense guarda de cada banco.
const CATALOG_COLLECTIONS: usize = 300;
/// Documentos lidos de cada coleção para saber os campos.
const CATALOG_SAMPLE: i32 = 50;
/// Campos guardados de cada coleção.
const CATALOG_FIELDS: usize = 500;
/// Até onde desce nos documentos de dentro.
const CATALOG_DEPTH: usize = 3;

/// O tipo de um valor como o `$type` chama.
fn type_name(value: &Bson) -> &'static str {
    match value {
        Bson::Double(_) => "double",
        Bson::String(_) => "string",
        Bson::Array(_) => "array",
        Bson::Document(_) => "object",
        Bson::Boolean(_) => "bool",
        Bson::Null => "null",
        Bson::RegularExpression(_) => "regex",
        Bson::Int32(_) => "int",
        Bson::Int64(_) => "long",
        Bson::Timestamp(_) => "timestamp",
        Bson::Binary(_) => "binData",
        Bson::ObjectId(_) => "objectId",
        Bson::DateTime(_) => "date",
        Bson::Decimal128(_) => "decimal",
        _ => "",
    }
}

/// Junta em `fields` os campos de `document` (nome com o caminho e tipo
/// da primeira vez que apareceu com valor).
fn collect_fields(
    document: &Document,
    path: &str,
    depth: usize,
    fields: &mut Vec<(String, String)>,
) {
    for (key, value) in document {
        let name = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        match fields.iter_mut().find(|(n, _)| *n == name) {
            Some((_, ty)) if ty == "null" => *ty = type_name(value).to_string(),
            Some(_) => {}
            None => fields.push((name.clone(), type_name(value).to_string())),
        }
        if let (Bson::Document(inner), true) = (value, depth + 1 < CATALOG_DEPTH) {
            collect_fields(inner, &name, depth + 1, fields);
        }
    }
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<String> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

fn place(node: &NodeId) -> Option<Place> {
    let mut parts = node.0.strip_prefix("db/")?.split('/');
    let database = unhex(parts.next()?)?;
    Some(
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (None, ..) => Place::Database(database),
            (Some("c"), Some(c), None, _) => Place::Collection(database, unhex(c)?),
            (Some("c"), Some(c), Some("i"), Some(i)) => {
                Place::Index(database, unhex(c)?, unhex(i)?)
            }
            _ => return None,
        },
    )
}

/// Um tamanho em bytes como `1.5 MB`.
fn size(reply: &Document, field: &str) -> Option<String> {
    let bytes = match reply.get(field)? {
        Bson::Int32(n) => *n as f64,
        Bson::Int64(n) => *n as f64,
        Bson::Double(n) => *n,
        _ => return None,
    };
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    Some(if unit == 0 {
        format!("{value:.0} B")
    } else {
        format!("{value:.1} {}", units[unit])
    })
}

fn number(reply: &Document, field: &str) -> Option<String> {
    reply.get(field).and_then(render::cell).map(|n| {
        // Contagens vêm como inteiro ou como `12.0`.
        n.strip_suffix(".0").map(str::to_string).unwrap_or(n)
    })
}

impl MetadataProvider for MongoExplorer {
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
            Some(Place::Database(db)) => self.collections(&db),
            Some(Place::Collection(db, collection)) => self.indexes(&db, &collection),
            _ => Ok(Vec::new()),
        }
    }

    fn relational(&self) -> bool {
        false
    }

    /// As coleções do banco para o IntelliSense, com os campos de uma
    /// amostra de documentos de cada uma (`cliente.cidade` para os de
    /// dentro) e o tipo de cada campo.
    fn catalog(&self, database: &str) -> Result<Catalog, MetadataError> {
        let mut relations = Vec::new();
        for node in self
            .collections(database)?
            .into_iter()
            .take(CATALOG_COLLECTIONS)
        {
            // Uma coleção que não dá para ler entra sem campos.
            let sample = self
                .list(
                    database,
                    doc! { "find": &node.name, "limit": CATALOG_SAMPLE, "maxTimeMS": 3000 },
                )
                .unwrap_or_default();
            let mut fields: Vec<(String, String)> = Vec::new();
            for document in &sample {
                collect_fields(document, "", 0, &mut fields);
            }
            fields.sort();
            fields.truncate(CATALOG_FIELDS);
            relations.push(Relation {
                name: node.name,
                kind: if node.kind == NodeKind::View {
                    RelationKind::View
                } else {
                    RelationKind::Table
                },
                columns: fields
                    .into_iter()
                    .map(|(name, ty)| Column { name, ty })
                    .collect(),
            });
        }
        Ok(Catalog {
            engine: Engine::MongoDb,
            database: database.to_string(),
            search_path: vec![String::new()],
            schemas: vec![Schema {
                name: String::new(),
                relations,
                functions: Vec::new(),
            }],
        })
    }

    fn database_of(&self, node: &NodeId) -> Option<String> {
        match place(node)? {
            Place::Database(db) | Place::Collection(db, _) | Place::Index(db, ..) => Some(db),
        }
    }

    /// "Open Documents": o `find` com limite. "Drop...": o comando para
    /// conferir e rodar.
    fn script(&self, node: &NodeId, action: ScriptAction) -> Result<String, MetadataError> {
        let unavailable =
            || MetadataError(tr("Scripting is not available for this server.").to_string());
        match (place(node).ok_or_else(unavailable)?, action) {
            (Place::Collection(_, c), ScriptAction::SelectTop(limit)) => Ok(format!(
                "{}.find({{}}).limit({limit})",
                mongodb_collection(&c)
            )),
            (Place::Collection(_, c), ScriptAction::Drop) => {
                Ok(format!("{}.drop()", mongodb_collection(&c)))
            }
            (Place::Collection(db, c), ScriptAction::Create) => {
                let mut out = format!("db.createCollection({})", mongodb_string(&c));
                for index in self.index_list(&db, &c)? {
                    let (Ok(name), Ok(keys)) = (index.get_str("name"), index.get_document("key"))
                    else {
                        continue;
                    };
                    if name == "_id_" {
                        continue;
                    }
                    let mut options = doc! { "name": name };
                    for (k, v) in &index {
                        if !matches!(k.as_str(), "v" | "key" | "name" | "ns") {
                            options.insert(k, v.clone());
                        }
                    }
                    out.push_str(&format!(
                        "\n{}.createIndex({}, {})",
                        mongodb_collection(&c),
                        render::json(&Bson::Document(keys.clone())),
                        render::json(&Bson::Document(options)),
                    ));
                }
                Ok(out)
            }
            (Place::Index(_, c, index), ScriptAction::Drop) => Ok(format!(
                "{}.dropIndex({})",
                mongodb_collection(&c),
                mongodb_string(&index)
            )),
            (Place::Database(_), ScriptAction::Drop) => Ok("db.dropDatabase()".to_string()),
            _ => Err(unavailable()),
        }
    }

    fn properties(&self, node: &NodeId) -> Result<Vec<Property>, MetadataError> {
        let mut out = Vec::new();
        let mut push = |name: &str, value: Option<String>| {
            if let Some(value) = value {
                out.push(Property::new(name, value));
            }
        };
        match place(node) {
            Some(Place::Database(db)) => {
                let stats = self.command(&db, doc! { "dbStats": 1 })?;
                push(tr("Collections"), number(&stats, "collections"));
                push(tr("Views"), number(&stats, "views"));
                push(tr("Documents"), number(&stats, "objects"));
                push(tr("Data size"), size(&stats, "dataSize"));
                push(tr("Storage size"), size(&stats, "storageSize"));
                push(tr("Indexes"), number(&stats, "indexes"));
                push(tr("Index size"), size(&stats, "indexSize"));
            }
            Some(Place::Collection(db, collection)) => {
                // Numa view o `collStats` é erro: sem propriedades.
                let Ok(stats) = self.command(&db, doc! { "collStats": &collection }) else {
                    return Ok(out);
                };
                push(tr("Documents"), number(&stats, "count"));
                push(tr("Data size"), size(&stats, "size"));
                push(tr("Average document size"), size(&stats, "avgObjSize"));
                push(tr("Storage size"), size(&stats, "storageSize"));
                push(tr("Indexes"), number(&stats, "nindexes"));
                push(tr("Index size"), size(&stats, "totalIndexSize"));
                push(tr("Capped"), number(&stats, "capped"));
            }
            Some(Place::Index(db, collection, name)) => {
                let index = self
                    .index_list(&db, &collection)?
                    .into_iter()
                    .find(|i| i.get_str("name") == Ok(name.as_str()));
                if let Some(index) = index {
                    for (field, value) in &index {
                        if !matches!(field.as_str(), "v" | "name" | "ns") {
                            push(field, render::cell(value));
                        }
                    }
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
        let id = NodeId(format!("db/{}", hex("loja")));
        assert_eq!(place(&id), Some(Place::Database("loja".into())));
        let id = NodeId(format!("db/{}/c/{}", hex("loja"), hex("meus/pedidos")));
        assert_eq!(
            place(&id),
            Some(Place::Collection("loja".into(), "meus/pedidos".into()))
        );
        let id = NodeId(format!("db/{}/c/{}/i/{}", hex("l"), hex("c"), hex("a_1")));
        assert_eq!(
            place(&id),
            Some(Place::Index("l".into(), "c".into(), "a_1".into()))
        );
        assert_eq!(place(&NodeId("server".into())), None);
        assert_eq!(place(&NodeId("db/zz".into())), None);
    }

    #[test]
    fn sizes_are_readable() {
        let d = doc! { "a": 512, "b": 1536_i64, "c": 5_000_000.0 };
        assert_eq!(size(&d, "a").unwrap(), "512 B");
        assert_eq!(size(&d, "b").unwrap(), "1.5 KB");
        assert_eq!(size(&d, "c").unwrap(), "4.8 MB");
        assert_eq!(size(&d, "x"), None);
    }
}
