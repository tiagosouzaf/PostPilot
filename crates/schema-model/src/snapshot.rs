//! O schema inteiro de um banco ([`Schema`]) e o arquivo `.pgsschema`
//! ([`Snapshot`]) em que o Schema Compare o guarda, para comparar depois
//! com um banco de qualquer servidor.
//!
//! O arquivo é JSON: um cabeçalho com a versão do formato, de onde veio e
//! quando, e o schema. Os ids das tabelas são os oids do banco de origem;
//! a comparação alinha tudo pelo nome, então não importa que não batam.

use serde::{Deserialize, Serialize};

use crate::Model;
use crate::objects::Object;

/// Extensão do arquivo de snapshot.
pub const EXTENSION: &str = "pgsschema";

/// Versão do formato que este PGStudio escreve e lê.
pub const FORMAT: u32 = 1;

/// O schema de um banco: as tabelas e o resto.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    pub model: Model,
    pub objects: Vec<Object>,
}

/// Um `.pgsschema`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub format: u32,
    /// O servidor (`host,porta`) e o banco de onde veio.
    pub server: String,
    pub database: String,
    /// Quando foi tirado, em segundos desde 1970 (UTC).
    pub taken: u64,
    pub schema: Schema,
}

impl Snapshot {
    pub fn new(server: &str, database: &str, taken: u64, schema: Schema) -> Self {
        Snapshot {
            format: FORMAT,
            server: server.into(),
            database: database.into(),
            taken,
            schema,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a snapshot always serializes")
    }

    /// Lê um `.pgsschema`, com a mensagem de erro para o usuário.
    pub fn from_json(text: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Header {
            format: u32,
        }
        let header: Header = serde_json::from_str(text)
            .map_err(|e| pgstudio_i18n::trf!("This is not a schema snapshot ({e}).", e = e))?;
        if header.format > FORMAT {
            return Err(pgstudio_i18n::trf!(
                "The snapshot was saved by a newer PostPilot (format {format}).",
                format = header.format
            ));
        }
        serde_json::from_str(text)
            .map_err(|e| pgstudio_i18n::trf!("The snapshot is damaged ({e}).", e = e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::Sequence;
    use crate::{Column, Identity, Key, Table};

    #[test]
    fn snapshots_round_trip() {
        let table = Table {
            id: 16384,
            schema: "public".into(),
            name: "pedido".into(),
            columns: vec![Column {
                id: 1,
                name: "id".into(),
                ty: "integer".into(),
                identity: Some(Identity::Always),
                ..Default::default()
            }],
            primary_key: Some(Key {
                name: "pedido_pkey".into(),
                columns: vec!["id".into()],
            }),
            ..Default::default()
        };
        let schema = Schema {
            model: Model::new(vec![table]),
            objects: vec![
                Object::view("public", "v", "SELECT 1"),
                Object::enum_type("public", "status", vec!["a".into(), "b".into()]),
                Object::sequence(
                    "public",
                    "s",
                    Sequence {
                        ty: "bigint".into(),
                        start: "1".into(),
                        increment: "1".into(),
                        min: "1".into(),
                        max: "9223372036854775807".into(),
                        cache: "1".into(),
                        cycle: false,
                        owned_by: None,
                    },
                ),
            ],
        };
        let snap = Snapshot::new("db,5433", "loja", 1_763_000_000, schema);
        let back = Snapshot::from_json(&snap.to_json()).unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn bad_files_say_why() {
        assert!(
            Snapshot::from_json("{}")
                .unwrap_err()
                .starts_with("This is not a schema snapshot")
        );
        assert!(
            Snapshot::from_json("{\"format\": 99}")
                .unwrap_err()
                .contains("newer PostPilot")
        );
        assert!(
            Snapshot::from_json("{\"format\": 1}")
                .unwrap_err()
                .starts_with("The snapshot is damaged")
        );
    }
}
