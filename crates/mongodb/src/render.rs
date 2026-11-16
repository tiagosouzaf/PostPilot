//! Documentos do MongoDB como linhas de uma grade.
//!
//! Cada campo do primeiro nível vira uma coluna. Textos, números, datas e
//! `ObjectId` saem como texto simples; documentos e listas de dentro, como
//! JSON (o visualizador de JSON da grade abre).

use mongodb::bson::{Bson, Document};

/// O valor de uma célula (`None` é NULL: campo `null` ou que falta).
pub fn cell(value: &Bson) -> Option<String> {
    Some(match value {
        Bson::Null | Bson::Undefined => return None,
        Bson::String(s) => s.clone(),
        Bson::Boolean(b) => b.to_string(),
        Bson::Int32(n) => n.to_string(),
        Bson::Int64(n) => n.to_string(),
        Bson::Double(n) => number(*n),
        Bson::Decimal128(n) => n.to_string(),
        Bson::ObjectId(id) => id.to_hex(),
        Bson::DateTime(d) => d
            .try_to_rfc3339_string()
            .unwrap_or_else(|_| d.timestamp_millis().to_string()),
        Bson::Symbol(s) => s.clone(),
        Bson::JavaScriptCode(code) => code.clone(),
        Bson::RegularExpression(r) => format!("/{}/{}", r.pattern, r.options),
        Bson::Timestamp(t) => format!("Timestamp({}, {})", t.time, t.increment),
        Bson::MinKey => "MinKey".into(),
        Bson::MaxKey => "MaxKey".into(),
        Bson::Binary(b) => match b.to_uuid() {
            Ok(uuid) => uuid.to_string(),
            Err(_) => json(value),
        },
        Bson::Document(_)
        | Bson::Array(_)
        | Bson::JavaScriptCodeWithScope(_)
        | Bson::DbPointer(_) => json(value),
    })
}

/// Um número de ponto flutuante sem o `.0` que o Rust não escreve para
/// inteiros e com ele quando o valor é inteiro (`2.0`), como o mongosh.
fn number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.1}")
    } else {
        n.to_string()
    }
}

/// O valor em JSON (o "relaxed extended JSON" do MongoDB: datas e ids
/// continuam reconhecíveis, como `{"$oid": "..."}`).
pub fn json(value: &Bson) -> String {
    value.clone().into_relaxed_extjson().to_string()
}

/// As colunas de um conjunto de documentos: os campos do primeiro nível,
/// na ordem em que aparecem.
pub fn columns<'a>(docs: impl IntoIterator<Item = &'a Document>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for doc in docs {
        for key in doc.keys() {
            if !out.iter().any(|c| c == key) {
                out.push(key.clone());
            }
        }
    }
    out
}

/// A linha de `doc` para `columns`, e os campos dele que não têm coluna.
pub fn row<'a>(doc: &'a Document, columns: &[String]) -> (Vec<Option<String>>, Vec<&'a str>) {
    let cells = columns.iter().map(|c| doc.get(c).and_then(cell)).collect();
    let extra = doc
        .keys()
        .filter(|k| !columns.iter().any(|c| c == *k))
        .map(String::as_str)
        .collect();
    (cells, extra)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::{self, doc, oid::ObjectId};

    #[test]
    fn cells_are_plain_text_and_nested_values_are_json() {
        let id = ObjectId::parse_str("507f1f77bcf86cd799439011").unwrap();
        let d = doc! {
            "_id": id,
            "nome": "Ana",
            "idade": 30,
            "saldo": 10.5,
            "inteiro": 2.0,
            "ativo": true,
            "nada": Bson::Null,
            "quando": bson::DateTime::from_millis(1_714_559_400_000),
            "endereco": { "cidade": "Recife", "cep": 50000 },
            "tags": ["a", 1],
            "longo": 9_007_199_254_740_993_i64,
        };
        let cols = columns([&d]);
        let (cells, extra) = row(&d, &cols);
        let cells: Vec<String> = cells
            .into_iter()
            .map(|c| c.unwrap_or_else(|| "NULL".into()))
            .collect();
        assert_eq!(
            cells,
            [
                "507f1f77bcf86cd799439011",
                "Ana",
                "30",
                "10.5",
                "2.0",
                "true",
                "NULL",
                "2024-05-01T10:30:00Z",
                r#"{"cidade":"Recife","cep":50000}"#,
                r#"["a",1]"#,
                "9007199254740993",
            ]
        );
        assert!(extra.is_empty());
    }

    #[test]
    fn columns_are_the_union_in_order_of_appearance() {
        let a = doc! { "_id": 1, "a": 1 };
        let b = doc! { "_id": 2, "b": 2, "a": 3 };
        let cols = columns([&a, &b]);
        assert_eq!(cols, ["_id", "a", "b"]);
        assert_eq!(row(&a, &cols).0, [Some("1".into()), Some("1".into()), None]);
        let (_, extra) = row(&b, &cols[..2]);
        assert_eq!(extra, ["b"]);
    }
}
