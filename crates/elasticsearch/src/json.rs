//! JSON com as chaves na ordem em que vieram. O `serde_json::Value` do
//! workspace ordena as chaves, e a ordem importa aqui: as colunas de um
//! `_cat` e os campos de um documento aparecem como o servidor mandou.

use std::fmt;

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(text: &str) -> Result<Json, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    /// O campo `key` de um objeto.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// O valor no caminho `a.b.c`.
    pub fn at(&self, path: &str) -> Option<&Json> {
        path.split('.').try_fold(self, |value, key| value.get(key))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(fields) => Some(fields),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Number(n) => n.as_u64(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Numa linha só.
    pub fn compact(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Com recuo de dois espaços.
    pub fn pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Como uma célula da grade: texto e números como estão, `null` como
    /// NULL, objetos e listas em JSON.
    pub fn cell(&self) -> Option<String> {
        match self {
            Json::Null => None,
            Json::Bool(b) => Some(b.to_string()),
            Json::Number(n) => Some(n.to_string()),
            Json::String(s) => Some(s.clone()),
            Json::Array(_) | Json::Object(_) => Some(self.compact()),
        }
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => s.serialize_unit(),
            Json::Bool(b) => s.serialize_bool(*b),
            Json::Number(n) => n.serialize(s),
            Json::String(text) => s.serialize_str(text),
            Json::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Json::Object(fields) => {
                let mut map = s.serialize_map(Some(fields.len()))?;
                for (key, value) in fields {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Json, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Json;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_none<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_bool<E>(self, v: bool) -> Result<Json, E> {
                Ok(Json::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Json, E> {
                Ok(Json::Number(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Json, E> {
                Ok(Json::Number(v.into()))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Json, E> {
                Ok(serde_json::Number::from_f64(v).map_or(Json::Null, Json::Number))
            }
            fn visit_str<E>(self, v: &str) -> Result<Json, E> {
                Ok(Json::String(v.to_string()))
            }
            fn visit_string<E>(self, v: String) -> Result<Json, E> {
                Ok(Json::String(v))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Json::Array(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut fields = Vec::new();
                while let Some(entry) = map.next_entry::<String, Json>()? {
                    fields.push(entry);
                }
                Ok(Json::Object(fields))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_keep_the_order_they_came_in() {
        let text = r#"{"zeta":1,"alfa":{"y":[1,2.5,null,true],"b":"x"}}"#;
        let json = Json::parse(text).unwrap();
        assert_eq!(json.compact(), text);
        assert_eq!(json.at("alfa.b").and_then(Json::as_str), Some("x"));
        assert_eq!(json.get("zeta").and_then(Json::as_u64), Some(1));
        assert_eq!(
            json.at("alfa.y").unwrap().cell().unwrap(),
            "[1,2.5,null,true]"
        );
        assert_eq!(Json::Null.cell(), None);
        assert!(json.pretty().contains("\n  \"alfa\": {"));
        assert!(Json::parse("{").is_err());
    }
}
