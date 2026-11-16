//! As respostas do Elasticsearch como grades e mensagens.
//!
//! | Resposta | Como aparece |
//! | --- | --- |
//! | busca (`hits.hits`) | uma linha por documento: `_index`, `_id`, `_score` e um campo do `_source` por coluna; uma grade a mais por agregação |
//! | `_cat` e outras listas de objetos | uma linha por item |
//! | `_sql` (`columns` e `rows`) | a tabela como veio |
//! | `_bulk` (`items`) | uma linha por operação, com o erro quando há |
//! | qualquer outro objeto | uma linha por campo: `field` e `value` |
//! | texto | em Mensagens |

use pgstudio_core::query::{QueryEvent, ServerMessage};
use pgstudio_i18n::{tr, trf};

use crate::http::Reply;
use crate::json::Json;

type Sink<'a> = &'a mut dyn FnMut(QueryEvent);

fn notice(message: String) -> QueryEvent {
    QueryEvent::Notice(ServerMessage {
        // Sem severidade: a mensagem aparece como está em Messages.
        message,
        ..Default::default()
    })
}

/// As colunas de uma lista de objetos: a união dos campos, na ordem em
/// que aparecem.
fn union<'a>(objects: impl IntoIterator<Item = &'a Json>) -> Vec<String> {
    let mut columns: Vec<String> = Vec::new();
    for object in objects {
        for (key, _) in object.as_object().unwrap_or_default() {
            if !columns.iter().any(|c| c == key) {
                columns.push(key.clone());
            }
        }
    }
    columns
}

/// Uma grade de objetos, um por linha.
fn grid(objects: &[&Json], sink: Sink) {
    let columns = union(objects.iter().copied());
    if columns.is_empty() {
        // Lista de valores soltos.
        sink(QueryEvent::Columns(vec!["value".into()]));
        for value in objects {
            sink(QueryEvent::Row(vec![value.cell()]));
        }
    } else {
        sink(QueryEvent::Columns(columns.clone()));
        for object in objects {
            sink(QueryEvent::Row(
                columns
                    .iter()
                    .map(|c| object.get(c).and_then(Json::cell))
                    .collect(),
            ));
        }
    }
    sink(QueryEvent::CommandComplete {
        rows: objects.len() as u64,
    });
}

/// Um objeto como `field` e `value`, uma linha por campo.
fn fields(rows: Vec<(String, Option<String>)>, sink: Sink) {
    sink(QueryEvent::Columns(vec!["field".into(), "value".into()]));
    let n = rows.len() as u64;
    for (field, value) in rows {
        sink(QueryEvent::Row(vec![Some(field), value]));
    }
    sink(QueryEvent::CommandComplete { rows: n });
}

/// Os documentos de uma busca (ou de um `_mget`), um por linha: os campos
/// de fora que existirem e depois os do `_source`.
fn documents(hits: &[Json], sink: Sink) {
    const META: &[&str] = &["_index", "_id", "_score", "found"];
    const EXTRA: &[&str] = &["fields", "highlight", "sort", "inner_hits", "error"];
    let mut columns: Vec<String> = META
        .iter()
        .filter(|m| hits.iter().any(|h| h.get(m).is_some()))
        .map(|m| m.to_string())
        .collect();
    let meta = columns.len();
    let source = union(hits.iter().filter_map(|h| h.get("_source")));
    columns.extend(source.iter().cloned());
    let extra: Vec<&str> = EXTRA
        .iter()
        .copied()
        .filter(|e| hits.iter().any(|h| h.get(e).is_some()))
        .collect();
    columns.extend(extra.iter().map(|e| e.to_string()));
    sink(QueryEvent::Columns(columns.clone()));
    for hit in hits {
        let mut row: Vec<Option<String>> = columns[..meta]
            .iter()
            .map(|c| hit.get(c).and_then(Json::cell))
            .collect();
        let doc = hit.get("_source");
        row.extend(
            source
                .iter()
                .map(|c| doc.and_then(|d| d.get(c)).and_then(Json::cell)),
        );
        row.extend(extra.iter().map(|e| hit.get(e).and_then(Json::cell)));
        sink(QueryEvent::Row(row));
    }
    sink(QueryEvent::CommandComplete {
        rows: hits.len() as u64,
    });
}

/// O valor de um campo de um bucket: o número de uma métrica
/// (`{ "value": 3 }`) ou o que vier.
fn metric(value: &Json) -> Option<String> {
    match value.get("value_as_string").or_else(|| value.get("value")) {
        Some(v) if value.as_object().is_some_and(|o| o.len() <= 2) => v.cell(),
        _ => value.cell(),
    }
}

/// Uma grade por agregação: os buckets (a chave na coluna com o nome da
/// agregação) ou a linha de uma métrica.
fn aggregations(aggs: &Json, sink: Sink) {
    for (name, agg) in aggs.as_object().unwrap_or_default() {
        let buckets: Option<Vec<(Option<String>, &Json)>> = match agg.get("buckets") {
            Some(Json::Array(list)) => Some(
                list.iter()
                    .map(|b| {
                        let key = b.get("key_as_string").or_else(|| b.get("key"));
                        (key.and_then(Json::cell), b)
                    })
                    .collect(),
            ),
            // `filters` com nomes: um objeto de buckets.
            Some(Json::Object(named)) => {
                Some(named.iter().map(|(k, b)| (Some(k.clone()), b)).collect())
            }
            _ => None,
        };
        match buckets {
            Some(buckets) => {
                let mut rest = union(buckets.iter().map(|(_, b)| *b));
                rest.retain(|c| c != "key" && c != "key_as_string");
                let mut columns = vec![name.clone()];
                columns.extend(rest.iter().cloned());
                sink(QueryEvent::Columns(columns));
                for (key, bucket) in &buckets {
                    let mut row = vec![key.clone()];
                    row.extend(rest.iter().map(|c| bucket.get(c).and_then(metric)));
                    sink(QueryEvent::Row(row));
                }
                sink(QueryEvent::CommandComplete {
                    rows: buckets.len() as u64,
                });
            }
            None => {
                let mut columns = vec!["aggregation".to_string()];
                let mut row = vec![Some(name.clone())];
                for (field, value) in agg.as_object().unwrap_or_default() {
                    columns.push(field.clone());
                    row.push(value.cell());
                }
                sink(QueryEvent::Columns(columns));
                sink(QueryEvent::Row(row));
                sink(QueryEvent::CommandComplete { rows: 1 });
            }
        }
    }
}

/// Uma resposta de busca: os documentos, as agregações e uma mensagem
/// com o total.
fn search(reply: &Json, hits: &[Json], sink: Sink) {
    documents(hits, sink);
    if let Some(aggs) = reply.get("aggregations") {
        aggregations(aggs, sink);
    }
    let took = reply.get("took").and_then(Json::as_u64).unwrap_or(0);
    let total = reply.at("hits.total");
    let count = total
        .and_then(|t| t.get("value").or(Some(t)))
        .and_then(Json::as_u64);
    let more = total.and_then(|t| t.get("relation")).and_then(Json::as_str) == Some("gte");
    let message = match count {
        Some(n) if more => trf!(
            "More than {total} document(s) found in {took} ms; showing {shown}.",
            total = n,
            took = took,
            shown = hits.len()
        ),
        Some(n) => trf!(
            "{total} document(s) found in {took} ms; showing {shown}.",
            total = n,
            took = took,
            shown = hits.len()
        ),
        None => trf!(
            "Search done in {took} ms; showing {shown} document(s).",
            took = took,
            shown = hits.len()
        ),
    };
    sink(notice(message));
    if reply.get("timed_out").and_then(Json::as_bool) == Some(true) {
        sink(notice(
            tr("The search timed out: the results are partial.").to_string(),
        ));
    }
    let failed = reply
        .at("_shards.failed")
        .and_then(Json::as_u64)
        .unwrap_or(0);
    if failed > 0 {
        sink(notice(trf!(
            "{n} shard(s) failed: the results are partial.",
            n = failed
        )));
    }
}

/// As operações de um `_bulk`, uma por linha.
fn bulk(items: &[Json], sink: Sink) {
    sink(QueryEvent::Columns(
        ["action", "_index", "_id", "status", "result", "error"]
            .map(String::from)
            .to_vec(),
    ));
    let mut failed = 0;
    for item in items {
        let Some((action, detail)) = item.as_object().and_then(|o| o.first()) else {
            continue;
        };
        let error = detail.get("error").map(|e| {
            match (
                e.get("type").and_then(Json::as_str),
                e.get("reason").and_then(Json::as_str),
            ) {
                (Some(ty), Some(reason)) => format!("{ty}: {reason}"),
                _ => e.compact(),
            }
        });
        failed += usize::from(error.is_some());
        let cell = |field: &str| detail.get(field).and_then(Json::cell);
        sink(QueryEvent::Row(vec![
            Some(action.clone()),
            cell("_index"),
            cell("_id"),
            cell("status"),
            cell("result"),
            error,
        ]));
    }
    sink(QueryEvent::CommandComplete {
        rows: items.len() as u64,
    });
    sink(notice(if failed == 0 {
        trf!("{n} operation(s) done.", n = items.len())
    } else {
        trf!(
            "{n} operation(s), {failed} with an error (see the error column).",
            n = items.len(),
            failed = failed
        )
    }));
}

/// Um objeto qualquer em `field` e `value`. Um objeto com um campo só
/// (`GET /indice` responde `{ "indice": {...} }`) abre um nível, e um
/// documento (`GET /indice/_doc/1`) mostra os campos do `_source`.
fn object(reply: &Json, sink: Sink) {
    let top = reply.as_object().unwrap_or_default();
    let mut rows = Vec::new();
    match top {
        [(name, Json::Object(inner))] if !inner.is_empty() => {
            for (field, value) in inner {
                rows.push((format!("{name}.{field}"), value.cell()));
            }
        }
        _ => {
            for (field, value) in top {
                match (field.as_str(), value) {
                    ("_source", Json::Object(source)) => {
                        for (field, value) in source {
                            rows.push((field.clone(), value.cell()));
                        }
                    }
                    _ => rows.push((field.clone(), value.cell())),
                }
            }
        }
    }
    fields(rows, sink);
}

/// Mostra a resposta `reply`. `Err` com a mensagem e o status quando o
/// servidor recusou.
pub fn show(reply: &Reply, sink: Sink) -> Result<(), (String, String)> {
    if !reply.ok() {
        return Err((reply.error_text(), reply.status.to_string()));
    }
    if reply.body.trim().is_empty() {
        sink(notice(format!("HTTP {}", reply.status)));
        return Ok(());
    }
    let Some(json) = reply.json() else {
        // Texto (um `_cat` com `format=txt`, a ajuda de uma API).
        sink(notice(reply.body.trim_end().to_string()));
        return Ok(());
    };
    if let Some(hits) = json.at("hits.hits").and_then(Json::as_array) {
        search(&json, hits, sink);
    } else if let Some(responses) = json.get("responses").and_then(Json::as_array) {
        // `_msearch`: uma busca por resposta.
        for response in responses {
            match response.at("hits.hits").and_then(Json::as_array) {
                Some(hits) => search(response, hits, sink),
                None => object(response, sink),
            }
        }
    } else if let (Some(columns), Some(rows)) = (
        json.get("columns").and_then(Json::as_array),
        json.get("rows").and_then(Json::as_array),
    ) {
        // `_sql`.
        sink(QueryEvent::Columns(
            columns
                .iter()
                .map(|c| {
                    c.get("name")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect(),
        ));
        for row in rows {
            sink(QueryEvent::Row(
                row.as_array()
                    .unwrap_or_default()
                    .iter()
                    .map(Json::cell)
                    .collect(),
            ));
        }
        sink(QueryEvent::CommandComplete {
            rows: rows.len() as u64,
        });
        if json.get("cursor").is_some() {
            sink(notice(
                tr("There are more rows: add a LIMIT or raise fetch_size to see them.").to_string(),
            ));
        }
    } else if let (Some(items), Some(_)) = (
        json.get("items").and_then(Json::as_array),
        json.get("errors"),
    ) {
        bulk(items, sink);
    } else if let Some(docs) = json.get("docs").and_then(Json::as_array) {
        documents(docs, sink);
    } else if let Some(items) = json.as_array() {
        grid(&items.iter().collect::<Vec<_>>(), sink);
    } else if json.as_object().is_some() {
        object(&json, sink);
    } else {
        fields(vec![("value".into(), json.cell())], sink);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As grades (colunas e linhas) e as mensagens de uma resposta.
    type Grid = (Vec<String>, Vec<Vec<String>>);
    fn shown(body: &str) -> (Vec<Grid>, Vec<String>) {
        let reply = Reply {
            status: 200,
            content_type: "application/json".into(),
            body: body.into(),
        };
        let mut grids: Vec<Grid> = Vec::new();
        let mut notices = Vec::new();
        show(&reply, &mut |event| match event {
            QueryEvent::Columns(c) => grids.push((c, Vec::new())),
            QueryEvent::Row(r) => grids.last_mut().unwrap().1.push(
                r.into_iter()
                    .map(|c| c.unwrap_or_else(|| "NULL".into()))
                    .collect(),
            ),
            QueryEvent::Notice(m) => notices.push(m.message),
            _ => {}
        })
        .unwrap();
        (grids, notices)
    }

    #[test]
    fn a_search_is_one_row_per_document() {
        let (grids, notices) = shown(
            r#"{"took":3,"timed_out":false,"_shards":{"failed":0},
            "hits":{"total":{"value":10000,"relation":"gte"},"hits":[
              {"_index":"pedidos","_id":"1","_score":1.5,"_source":{"numero":1,"cliente":{"nome":"Ana"}}},
              {"_index":"pedidos","_id":"2","_score":null,"_source":{"numero":2,"cupom":"X"},"sort":[2]}]},
            "aggregations":{
              "por_cidade":{"buckets":[{"key":"Recife","doc_count":2,"total":{"value":10.5}}]},
              "media":{"value":7.0}}}"#,
        );
        assert_eq!(grids.len(), 3);
        assert_eq!(
            grids[0].0,
            [
                "_index", "_id", "_score", "numero", "cliente", "cupom", "sort"
            ]
        );
        assert_eq!(
            grids[0].1,
            [
                [
                    "pedidos",
                    "1",
                    "1.5",
                    "1",
                    r#"{"nome":"Ana"}"#,
                    "NULL",
                    "NULL"
                ],
                ["pedidos", "2", "NULL", "2", "NULL", "X", "[2]"],
            ]
        );
        assert_eq!(grids[1].0, ["por_cidade", "doc_count", "total"]);
        assert_eq!(grids[1].1, [["Recife", "2", "10.5"]]);
        assert_eq!(grids[2].0, ["aggregation", "value"]);
        assert_eq!(grids[2].1, [["media", "7.0"]]);
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0].contains("10000") && notices[0].contains("2"),
            "{notices:?}"
        );
    }

    #[test]
    fn lists_tables_and_objects() {
        // `_cat` em JSON: as colunas na ordem do servidor.
        let (grids, _) = shown(
            r#"[{"health":"green","index":"a","docs.count":"3"},{"health":"yellow","index":"b","docs.count":"0"}]"#,
        );
        assert_eq!(grids[0].0, ["health", "index", "docs.count"]);
        assert_eq!(grids[0].1[1], ["yellow", "b", "0"]);
        // `_sql`.
        let (grids, notices) = shown(
            r#"{"columns":[{"name":"nome","type":"text"},{"name":"n","type":"long"}],"rows":[["Ana",1],[null,2]],"cursor":"abc"}"#,
        );
        assert_eq!(grids[0].0, ["nome", "n"]);
        assert_eq!(grids[0].1, [["Ana", "1"], ["NULL", "2"]]);
        assert_eq!(notices.len(), 1);
        // Um objeto: campo e valor; o de um campo só abre um nível.
        let (grids, _) = shown(r#"{"acknowledged":true,"index":"a"}"#);
        assert_eq!(grids[0].0, ["field", "value"]);
        assert_eq!(grids[0].1, [["acknowledged", "true"], ["index", "a"]]);
        let (grids, _) = shown(r#"{"pedidos":{"aliases":{},"mappings":{"properties":{}}}}"#);
        assert_eq!(grids[0].1[0], ["pedidos.aliases", "{}"]);
        assert_eq!(grids[0].1[1][0], "pedidos.mappings");
        // Um documento: os campos do `_source` depois dos de fora.
        let (grids, _) = shown(r#"{"_index":"a","_id":"1","found":true,"_source":{"nome":"Ana"}}"#);
        assert_eq!(grids[0].1.last().unwrap(), &["nome", "Ana"]);
        assert_eq!(grids[0].1.len(), 4);
    }

    #[test]
    fn bulk_shows_each_operation() {
        let (grids, notices) = shown(
            r#"{"errors":true,"items":[
              {"index":{"_index":"a","_id":"1","status":201,"result":"created"}},
              {"delete":{"_index":"a","_id":"9","status":400,"error":{"type":"x_exception","reason":"no"}}}]}"#,
        );
        assert_eq!(grids[0].1[0], ["index", "a", "1", "201", "created", "NULL"]);
        assert_eq!(grids[0].1[1][5], "x_exception: no");
        assert!(notices[0].contains('1'), "{notices:?}");
    }

    #[test]
    fn text_and_errors() {
        let (grids, notices) = shown("=^.^=\n/_cat/indices\n");
        assert!(grids.is_empty());
        assert_eq!(notices, ["=^.^=\n/_cat/indices"]);
        let reply = Reply {
            status: 404,
            content_type: String::new(),
            body: r#"{"error":{"type":"index_not_found_exception","reason":"no such index [x]"}}"#
                .into(),
        };
        assert_eq!(
            show(&reply, &mut |_| {}),
            Err((
                "index_not_found_exception: no such index [x]".into(),
                "404".into()
            ))
        );
    }
}
