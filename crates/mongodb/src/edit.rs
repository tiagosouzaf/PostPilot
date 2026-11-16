//! Edição de documentos direto na grade de resultados.
//!
//! O resultado de um `find` numa coleção é editável: cada linha é um
//! documento, achado pelo `_id`. Mudar uma célula grava só aquele campo
//! (`$set`); Delete exclui os documentos selecionados. A grade só tem o
//! texto de cada valor, então o documento é lido de novo pelo `_id` antes
//! de gravar: o `_id` é procurado nos tipos que escrevem aquele texto e o
//! valor novo fica do tipo que o campo já tinha.

use std::collections::HashMap;

use mongodb::bson::{self, Bson, Document, doc, oid::ObjectId};
use mongodb::{Client, Collection};
use pgstudio_core::query::{DocumentEdits, EditDone};
use pgstudio_i18n::{tr, trf};
use pgstudio_pg_client::DbRuntime;

use crate::query::error_text;
use crate::render;
use crate::shell::{self, Action};

/// A coluna da chave.
const KEY: &str = "_id";

/// A coleção que o resultado de `text` edita, ou por que ele é só leitura.
pub fn collection_of(text: &str) -> Result<String, String> {
    let statements = shell::parse(text).map_err(|e| {
        trf!(
            "The results are read-only: {error}",
            error = e.message.as_str()
        )
    })?;
    let [statement] = statements.as_slice() else {
        return Err(tr(
            "The results are read-only: the batch has more than one command. \
             Run the find by itself to edit its results.",
        )
        .to_string());
    };
    let not_a_find = || {
        tr("The results are read-only: only the documents of a find can be edited here.")
            .to_string()
    };
    let Action::Cursor {
        command,
        admin: false,
    } = &statement.action
    else {
        return Err(not_a_find());
    };
    let collection = command.get_str("find").map_err(|_| not_a_find())?;
    let hidden = command
        .get_document("projection")
        .ok()
        .and_then(|p| p.get(KEY))
        .is_some_and(|v| matches!(v, Bson::Boolean(false)) || number(v) == Some(0.0));
    if hidden {
        return Err(tr(
            "The results are read-only: the projection leaves out _id, so the documents \
             cannot be identified.",
        )
        .to_string());
    }
    Ok(collection.to_string())
}

fn number(v: &Bson) -> Option<f64> {
    match v {
        Bson::Int32(n) => Some(f64::from(*n)),
        Bson::Int64(n) => Some(*n as f64),
        Bson::Double(n) => Some(*n),
        _ => None,
    }
}

/// Os valores de `_id` que a grade mostra como `text`. Um número acha os
/// três tipos de número: o servidor compara pelo valor.
fn key_candidates(text: Option<&str>) -> Vec<Bson> {
    let Some(text) = text else {
        return vec![Bson::Null];
    };
    let mut out = vec![Bson::String(text.to_string())];
    if let Ok(id) = ObjectId::parse_str(text) {
        out.push(Bson::ObjectId(id));
    }
    if let Ok(n) = text.parse::<i64>() {
        out.push(Bson::Int64(n));
    } else if let Ok(n) = text.parse::<f64>() {
        out.push(Bson::Double(n));
    }
    if let Ok(b) = text.parse::<bool>() {
        out.push(Bson::Boolean(b));
    }
    if text.contains('T')
        && let Ok(d) = bson::DateTime::parse_rfc3339_str(text)
    {
        out.push(Bson::DateTime(d));
    }
    if let Ok(uuid) = bson::Uuid::parse_str(text) {
        out.push(Bson::from(uuid));
    }
    if text.starts_with(['{', '['])
        && let Some(value) = from_json(text)
    {
        out.push(value);
    }
    out
}

/// Um valor escrito em JSON (o "extended JSON" do MongoDB, como a grade
/// mostra documentos e listas).
fn from_json(text: &str) -> Option<Bson> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    Bson::try_from(value).ok()
}

fn type_name(value: &Bson) -> String {
    match value {
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Decimal128(_) => tr("number"),
        Bson::Boolean(_) => tr("boolean"),
        Bson::ObjectId(_) => "ObjectId".into(),
        Bson::DateTime(_) => tr("date"),
        Bson::Document(_) => tr("document"),
        Bson::Array(_) => tr("array"),
        _ => tr("value"),
    }
    .to_string()
}

/// O valor novo de um campo que hoje vale `old`, a partir do texto
/// digitado: fica do tipo que o campo já tinha. Num campo vazio (ou que
/// falta) vale a sintaxe do shell (`42`, `true`, `ISODate("...")`,
/// `{ a: 1 }`) e, se não for nada disso, o texto.
pub fn convert(field: &str, old: Option<&Bson>, text: Option<&str>) -> Result<Bson, String> {
    let Some(text) = text else {
        return Ok(Bson::Null);
    };
    let trimmed = text.trim();
    let converted = match old {
        Some(Bson::String(_) | Bson::Symbol(_)) => Some(Bson::String(text.to_string())),
        Some(Bson::Int32(_)) => trimmed
            .parse::<i32>()
            .map(Bson::Int32)
            .or_else(|_| trimmed.parse::<i64>().map(Bson::Int64))
            .or_else(|_| trimmed.parse::<f64>().map(Bson::Double))
            .ok(),
        Some(Bson::Int64(_)) => trimmed
            .parse::<i64>()
            .map(Bson::Int64)
            .or_else(|_| trimmed.parse::<f64>().map(Bson::Double))
            .ok(),
        Some(Bson::Double(_)) => trimmed.parse::<f64>().map(Bson::Double).ok(),
        Some(Bson::Decimal128(_)) => trimmed
            .parse::<bson::Decimal128>()
            .map(Bson::Decimal128)
            .ok(),
        Some(Bson::Boolean(_)) => trimmed.parse::<bool>().map(Bson::Boolean).ok(),
        Some(Bson::ObjectId(_)) => ObjectId::parse_str(trimmed).map(Bson::ObjectId).ok(),
        Some(Bson::DateTime(_)) => shell::parse_date(trimmed).map(Bson::DateTime),
        Some(Bson::Document(_)) => from_json(trimmed)
            .or_else(|| shell::parse_value(trimmed).ok())
            .filter(|v| matches!(v, Bson::Document(_))),
        Some(Bson::Array(_)) => from_json(trimmed)
            .or_else(|| shell::parse_value(trimmed).ok())
            .filter(|v| matches!(v, Bson::Array(_))),
        _ => {
            return Ok(shell::parse_value(trimmed)
                .ok()
                .or_else(|| {
                    Some(trimmed)
                        .filter(|t| t.starts_with(['{', '[']))
                        .and_then(from_json)
                })
                .unwrap_or_else(|| Bson::String(text.to_string())));
        }
    };
    // Trocar o tipo ainda dá, escrevendo o valor como no shell.
    converted
        .or_else(|| {
            shell::parse_value(trimmed)
                .ok()
                .filter(|v| !matches!(v, Bson::String(_)) || trimmed.starts_with(['"', '\'', '`']))
        })
        .ok_or_else(|| {
            trf!(
                "{field} holds a {kind} and \"{value}\" is not one. To store a text there, \
                 type it in quotes.",
                field = field,
                kind = type_name(old.unwrap_or(&Bson::Null)),
                value = text
            )
        })
}

/// Edita os documentos de uma coleção.
pub struct MongoEdits {
    client: Client,
    runtime: DbRuntime,
    database: String,
    collection: String,
}

impl MongoEdits {
    pub fn new(client: Client, runtime: DbRuntime, database: String, collection: String) -> Self {
        MongoEdits {
            client,
            runtime,
            database,
            collection,
        }
    }

    fn collection(&self) -> Collection<Document> {
        self.client
            .database(&self.database)
            .collection(&self.collection)
    }
}

fn failure(e: mongodb::error::Error) -> String {
    error_text(&e).0
}

/// Os documentos cujo `_id` é um dos `candidates`, só com os campos de
/// `projection`.
async fn find(
    coll: &Collection<Document>,
    candidates: Vec<Bson>,
    projection: Document,
) -> Result<Vec<Document>, String> {
    let mut cursor = coll
        .find(doc! { KEY: { "$in": candidates } })
        .projection(projection)
        .await
        .map_err(failure)?;
    let mut out = Vec::new();
    while cursor.advance().await.map_err(failure)? {
        out.push(cursor.deserialize_current().map_err(failure)?);
    }
    Ok(out)
}

fn shown(doc: &Document) -> Option<String> {
    doc.get(KEY).and_then(render::cell)
}

fn ambiguous(key: Option<&str>) -> String {
    trf!(
        "More than one document has an _id shown as {key} (of different types). \
         Use updateOne or deleteOne with the exact _id.",
        key = key.unwrap_or("NULL")
    )
}

async fn update(
    coll: Collection<Document>,
    key: Option<String>,
    field: String,
    value: Option<String>,
) -> Result<Option<String>, String> {
    if field == KEY {
        return Err(tr("The _id of a document cannot be changed.").to_string());
    }
    if field.contains('.') || field.starts_with('$') {
        return Err(trf!(
            "The field {field} cannot be edited here because of its name. Use updateOne.",
            field = field.as_str()
        ));
    }
    let found = find(
        &coll,
        key_candidates(key.as_deref()),
        doc! { KEY: 1, field.as_str(): 1 },
    )
    .await?;
    let mut same = found.iter().filter(|d| shown(d) == key);
    let Some(current) = same.next() else {
        return Err(tr(
            "The document was not found: it may have been changed or deleted by another user.",
        )
        .to_string());
    };
    if same.next().is_some() {
        return Err(ambiguous(key.as_deref()));
    }
    let id = current.get(KEY).cloned().unwrap_or(Bson::Null);
    let new = convert(&field, current.get(&field), value.as_deref())?;
    let result = coll
        .update_one(
            doc! { KEY: id },
            doc! { "$set": { field.as_str(): new.clone() } },
        )
        .await
        .map_err(failure)?;
    if result.matched_count == 0 {
        return Err(tr(
            "The document was not found: it may have been changed or deleted by another user.",
        )
        .to_string());
    }
    Ok(render::cell(&new))
}

async fn delete(
    coll: Collection<Document>,
    keys: Vec<Option<String>>,
) -> Result<Vec<Option<String>>, String> {
    let candidates: Vec<Bson> = keys
        .iter()
        .flat_map(|k| key_candidates(k.as_deref()))
        .collect();
    let found = find(&coll, candidates, doc! { KEY: 1 }).await?;
    let mut by_text: HashMap<Option<String>, Vec<Bson>> = HashMap::new();
    for doc in &found {
        by_text
            .entry(shown(doc))
            .or_default()
            .push(doc.get(KEY).cloned().unwrap_or(Bson::Null));
    }
    let mut ids = Vec::new();
    let mut deleted = Vec::new();
    for key in keys {
        match by_text.get(&key).map(Vec::as_slice) {
            Some([id]) => {
                ids.push(id.clone());
                deleted.push(key);
            }
            Some([_, _, ..]) => return Err(ambiguous(key.as_deref())),
            _ => {}
        }
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    coll.delete_many(doc! { KEY: { "$in": ids } })
        .await
        .map_err(failure)?;
    Ok(deleted)
}

impl DocumentEdits for MongoEdits {
    fn display(&self) -> String {
        format!("{}.{}", self.database, self.collection)
    }

    fn key(&self) -> &str {
        KEY
    }

    fn update(
        &self,
        key: Option<String>,
        field: String,
        value: Option<String>,
        done: EditDone<Option<String>>,
    ) {
        let coll = self.collection();
        self.runtime
            .handle()
            .spawn(async move { done(update(coll, key, field, value).await) });
    }

    fn delete(&self, keys: Vec<Option<String>>, done: EditDone<Vec<Option<String>>>) {
        let coll = self.collection();
        self.runtime
            .handle()
            .spawn(async move { done(delete(coll, keys).await) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_find_with_its_id_is_editable() {
        assert_eq!(
            collection_of("db.pedidos.find({ total: { $gt: 1 } }).sort({ total: -1 })").unwrap(),
            "pedidos"
        );
        assert_eq!(
            collection_of("db.getCollection('meus pedidos').findOne()").unwrap(),
            "meus pedidos"
        );
        assert_eq!(
            collection_of("db.pedidos.find({}, { total: 1 })").unwrap(),
            "pedidos"
        );
        for text in [
            "db.pedidos.find({}, { _id: 0, total: 1 })",
            "db.pedidos.aggregate([])",
            "db.pedidos.find(); db.pedidos.find()",
            "show dbs",
            "db.pedidos.countDocuments()",
            "db.pedidos.find(",
        ] {
            let reason = collection_of(text).unwrap_err();
            assert!(reason.contains("read-only"), "{text}: {reason}");
        }
    }

    #[test]
    fn the_key_is_looked_up_in_the_types_that_show_that_text() {
        let hex = "65f000000000000000000001";
        assert_eq!(
            key_candidates(Some(hex)),
            vec![
                Bson::String(hex.into()),
                Bson::ObjectId(ObjectId::parse_str(hex).unwrap())
            ]
        );
        assert_eq!(
            key_candidates(Some("42")),
            vec![Bson::String("42".into()), Bson::Int64(42)]
        );
        assert_eq!(
            key_candidates(Some("1.5")),
            vec![Bson::String("1.5".into()), Bson::Double(1.5)]
        );
        assert_eq!(key_candidates(None), vec![Bson::Null]);
        assert_eq!(
            key_candidates(Some("ana")),
            vec![Bson::String("ana".into())]
        );
        assert_eq!(
            key_candidates(Some(r#"{"loja":1,"n":2}"#))[1],
            Bson::Document(doc! { "loja": 1, "n": 2 })
        );
    }

    #[test]
    fn the_new_value_keeps_the_type_of_the_field() {
        let c = |old: Bson, text: &str| convert("campo", Some(&old), Some(text));
        assert_eq!(
            c(Bson::String("a".into()), "42"),
            Ok(Bson::String("42".into()))
        );
        assert_eq!(c(Bson::Int32(1), "42"), Ok(Bson::Int32(42)));
        assert_eq!(
            c(Bson::Int32(1), "5000000000"),
            Ok(Bson::Int64(5_000_000_000))
        );
        assert_eq!(c(Bson::Int32(1), "1.5"), Ok(Bson::Double(1.5)));
        assert_eq!(c(Bson::Int64(1), "42"), Ok(Bson::Int64(42)));
        assert_eq!(c(Bson::Double(1.0), "42"), Ok(Bson::Double(42.0)));
        assert_eq!(c(Bson::Boolean(true), "false"), Ok(Bson::Boolean(false)));
        let date = c(
            Bson::DateTime(bson::DateTime::now()),
            "2026-05-01T10:00:00Z",
        )
        .unwrap();
        assert_eq!(render::cell(&date).as_deref(), Some("2026-05-01T10:00:00Z"));
        assert_eq!(
            c(
                Bson::Document(doc! {}),
                r#"{"a":{"$oid":"65f000000000000000000001"}}"#
            ),
            Ok(Bson::Document(
                doc! { "a": ObjectId::parse_str("65f000000000000000000001").unwrap() }
            ))
        );
        assert_eq!(
            c(Bson::Array(vec![]), "[1, 'a']"),
            Ok(Bson::Array(vec![Bson::Int32(1), Bson::String("a".into())]))
        );
        // Outro tipo só escrito como no shell; texto solto num número é
        // engano.
        let error = c(Bson::Int32(1), "abc").unwrap_err();
        assert!(error.contains("campo holds a number"), "{error}");
        assert_eq!(c(Bson::Int32(1), "\"abc\""), Ok(Bson::String("abc".into())));
        assert_eq!(c(Bson::Int32(1), "true"), Ok(Bson::Boolean(true)));
        // NULL e campo que falta: a sintaxe do shell, ou o texto.
        assert_eq!(convert("c", None, Some("42")), Ok(Bson::Int32(42)));
        assert_eq!(
            convert("c", None, Some("ana maria")),
            Ok(Bson::String("ana maria".into()))
        );
        assert_eq!(
            convert("c", Some(&Bson::Null), Some("true")),
            Ok(Bson::Boolean(true))
        );
        assert_eq!(
            convert("c", Some(&Bson::String("a".into())), None),
            Ok(Bson::Null)
        );
    }
}
