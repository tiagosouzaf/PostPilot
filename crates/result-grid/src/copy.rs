//! Copiar a seleção da grade no formato que o usuário escolheu, como os
//! "extractors" do DataGrip: o Ctrl+C manda para a área de transferência o
//! texto já no formato de saída em uso.
//!
//! - TSV: valores separados por tab, NULL como `NULL` (o que a grade
//!   sempre copiou; cola direto numa planilha).
//! - CSV (vírgula ou ponto e vírgula): aspas só quando precisa, NULL como
//!   campo vazio. O ponto e vírgula é o que o Excel em português abre.
//! - JSON: uma lista de objetos; números saem como números e células com
//!   um objeto ou array JSON entram como JSON, não como texto.
//! - SQL INSERT: um `INSERT INTO` por linha.
//! - Lista para IN: os valores entre vírgulas, prontos para um
//!   `WHERE coluna IN (...)`; com mais de uma coluna, uma tupla por linha.
//! - Markdown e HTML: uma tabela.

use std::ops::Range;

use crate::export::{self, csv_field, json_string, literal};
use crate::store::ResultStore;
use crate::{json, text, xlsx};

/// Formato do que o Ctrl+C copia.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CopyFormat {
    #[default]
    Tsv,
    Csv,
    CsvSemicolon,
    Json,
    Insert,
    InList,
    Markdown,
    Html,
}

impl CopyFormat {
    /// Todos, na ordem dos menus.
    pub const ALL: [CopyFormat; 8] = [
        CopyFormat::Tsv,
        CopyFormat::Csv,
        CopyFormat::CsvSemicolon,
        CopyFormat::Json,
        CopyFormat::Insert,
        CopyFormat::InList,
        CopyFormat::Markdown,
        CopyFormat::Html,
    ];

    /// Nome nos menus (em inglês; passa por `tr` ao exibir).
    pub fn label(self) -> &'static str {
        match self {
            CopyFormat::Tsv => "Tab-separated (TSV)",
            CopyFormat::Csv => "CSV (comma)",
            CopyFormat::CsvSemicolon => "CSV (semicolon)",
            CopyFormat::Json => "JSON",
            CopyFormat::Insert => "SQL INSERT",
            CopyFormat::InList => "List for IN (...)",
            CopyFormat::Markdown => "Markdown table",
            CopyFormat::Html => "HTML table",
        }
    }

    /// Nome curto, para o seletor embaixo da grade.
    pub fn short_label(self) -> &'static str {
        match self {
            CopyFormat::Tsv => "TSV",
            CopyFormat::Csv => "CSV",
            CopyFormat::CsvSemicolon => "CSV ;",
            CopyFormat::Json => "JSON",
            CopyFormat::Insert => "INSERT",
            CopyFormat::InList => "IN (...)",
            CopyFormat::Markdown => "Markdown",
            CopyFormat::Html => "HTML",
        }
    }

    /// Como o formato é gravado nas preferências.
    pub fn code(self) -> &'static str {
        match self {
            CopyFormat::Tsv => "tsv",
            CopyFormat::Csv => "csv",
            CopyFormat::CsvSemicolon => "csv-semicolon",
            CopyFormat::Json => "json",
            CopyFormat::Insert => "insert",
            CopyFormat::InList => "in-list",
            CopyFormat::Markdown => "markdown",
            CopyFormat::Html => "html",
        }
    }

    /// O formato gravado como `code`; desconhecido, `None`.
    pub fn from_code(code: &str) -> Option<CopyFormat> {
        CopyFormat::ALL.into_iter().find(|f| f.code() == code)
    }

    /// Se a opção "incluir cabeçalho" muda o que sai (nos outros, os nomes
    /// das colunas fazem parte do formato ou não cabem nele).
    pub fn headers_optional(self) -> bool {
        matches!(
            self,
            CopyFormat::Tsv | CopyFormat::Csv | CopyFormat::CsvSemicolon
        )
    }
}

/// As linhas e colunas escolhidas no formato `format`. `headers` vale para
/// TSV e CSV. `table` é a tabela dos INSERTs.
pub fn copy(
    store: &mut ResultStore,
    rows: Range<u64>,
    cols: Range<usize>,
    format: CopyFormat,
    headers: bool,
    table: &str,
) -> String {
    match format {
        CopyFormat::Tsv => text::copy(store, rows, cols, headers),
        CopyFormat::Csv => delimited(store, rows, cols, headers, ','),
        CopyFormat::CsvSemicolon => delimited(store, rows, cols, headers, ';'),
        CopyFormat::Json => json_text(store, rows, cols),
        CopyFormat::Insert => export::insert_text(store, rows, cols, table),
        CopyFormat::InList => in_list(store, rows, cols),
        CopyFormat::Markdown => markdown(store, rows, cols),
        CopyFormat::Html => html(store, rows, cols),
    }
}

fn names(store: &ResultStore, cols: &Range<usize>) -> Vec<String> {
    store.columns()[cols.clone()].to_vec()
}

fn delimited(
    store: &mut ResultStore,
    rows: Range<u64>,
    cols: Range<usize>,
    headers: bool,
    delimiter: char,
) -> String {
    let mut out = String::new();
    let sep = delimiter.to_string();
    if headers {
        let fields: Vec<String> = names(store, &cols)
            .iter()
            .map(|n| csv_field(Some(n), delimiter))
            .collect();
        out.push_str(&fields.join(&sep));
        out.push('\n');
    }
    for r in rows {
        for (i, c) in cols.clone().enumerate() {
            if i > 0 {
                out.push(delimiter);
            }
            out.push_str(&csv_field(store.cell(r, c), delimiter));
        }
        out.push('\n');
    }
    out
}

/// Valor de uma célula em JSON: `null`, número, JSON embutido ou texto.
fn json_value(value: Option<&str>) -> String {
    match value {
        None => "null".to_string(),
        Some(v) if xlsx::is_number(v) && v != "-0" => v.to_string(),
        Some(v) if json::container(v).is_some() => v.trim().to_string(),
        Some(v) => json_string(v),
    }
}

fn json_text(store: &mut ResultStore, rows: Range<u64>, cols: Range<usize>) -> String {
    let keys: Vec<String> = names(store, &cols).iter().map(|n| json_string(n)).collect();
    let mut out = String::from("[");
    let mut first = true;
    for r in rows {
        out.push_str(if first { "\n  {" } else { ",\n  {" });
        first = false;
        for (i, c) in cols.clone().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&keys[i]);
            out.push_str(": ");
            out.push_str(&json_value(store.cell(r, c)));
        }
        out.push('}');
    }
    out.push_str(if first { "]\n" } else { "\n]\n" });
    out
}

/// Literal de SQL para a lista do IN: números sem aspas.
fn in_value(value: Option<&str>) -> String {
    match value {
        Some(v) if xlsx::is_number(v) => v.to_string(),
        v => literal(v),
    }
}

fn in_list(store: &mut ResultStore, rows: Range<u64>, cols: Range<usize>) -> String {
    let single = cols.len() == 1;
    let mut items = Vec::new();
    for r in rows {
        let values: Vec<String> = cols.clone().map(|c| in_value(store.cell(r, c))).collect();
        items.push(if single {
            values.concat()
        } else {
            format!("({})", values.join(", "))
        });
    }
    let mut out = items.join(", ");
    out.push('\n');
    out
}

fn markdown_cell(value: Option<&str>) -> String {
    match value {
        None => "NULL".to_string(),
        Some(v) => v
            .replace('\\', "\\\\")
            .replace('|', "\\|")
            .replace("\r\n", "<br>")
            .replace(['\n', '\r'], "<br>"),
    }
}

fn markdown(store: &mut ResultStore, rows: Range<u64>, cols: Range<usize>) -> String {
    let names = names(store, &cols);
    let mut out = String::new();
    let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
    out.push_str(&line(
        names.iter().map(|n| markdown_cell(Some(n))).collect(),
    ));
    out.push_str(&line(names.iter().map(|_| "---".to_string()).collect()));
    for r in rows {
        out.push_str(&line(
            cols.clone()
                .map(|c| markdown_cell(store.cell(r, c)))
                .collect(),
        ));
    }
    out
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace("\r\n", "<br>")
        .replace(['\n', '\r'], "<br>")
}

fn html(store: &mut ResultStore, rows: Range<u64>, cols: Range<usize>) -> String {
    let mut out = String::from("<table>\n  <thead>\n    <tr>");
    for n in names(store, &cols) {
        out.push_str(&format!("<th>{}</th>", html_escape(&n)));
    }
    out.push_str("</tr>\n  </thead>\n  <tbody>\n");
    for r in rows {
        out.push_str("    <tr>");
        for c in cols.clone() {
            match store.cell(r, c) {
                Some(v) => out.push_str(&format!("<td>{}</td>", html_escape(v))),
                None => out.push_str("<td></td>"),
            }
        }
        out.push_str("</tr>\n");
    }
    out.push_str("  </tbody>\n</table>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ResultStore {
        let s = |v: &str| Some(v.to_string());
        let mut st = ResultStore::new(vec!["id".into(), "nome".into(), "obs".into()]);
        st.push(&[s("1"), s("Ana; \"A\", B"), None]);
        st.push(&[s("007"), s("a|b\nc"), s(r#"{"k": [1, 2]}"#)]);
        st
    }

    fn all(format: CopyFormat, headers: bool) -> String {
        copy(&mut store(), 0..2, 0..3, format, headers, "t")
    }

    #[test]
    fn tsv_is_what_the_grid_always_copied() {
        assert_eq!(
            all(CopyFormat::Tsv, false),
            "1\tAna; \"A\", B\tNULL\n007\ta|b\nc\t{\"k\": [1, 2]}\n"
        );
        assert!(all(CopyFormat::Tsv, true).starts_with("id\tnome\tobs\n1\t"));
    }

    #[test]
    fn csv_quotes_by_its_own_delimiter() {
        assert_eq!(
            all(CopyFormat::Csv, true),
            "id,nome,obs\n1,\"Ana; \"\"A\"\", B\",\n007,\"a|b\nc\",\"{\"\"k\"\": [1, 2]}\"\n"
        );
        // Com ponto e vírgula, a vírgula sozinha não pede aspas.
        let mut s = ResultStore::new(vec!["a".into(), "b".into()]);
        s.push(&[Some("1,5".into()), Some("x;y".into())]);
        assert_eq!(
            copy(&mut s, 0..1, 0..2, CopyFormat::CsvSemicolon, false, "t"),
            "1,5;\"x;y\"\n"
        );
        assert_eq!(
            copy(&mut s, 0..1, 0..2, CopyFormat::Csv, false, "t"),
            "\"1,5\",x;y\n"
        );
    }

    #[test]
    fn json_keeps_numbers_and_embedded_json() {
        assert_eq!(
            all(CopyFormat::Json, false),
            "[\n  {\"id\": 1, \"nome\": \"Ana; \\\"A\\\", B\", \"obs\": null},\n  \
             {\"id\": \"007\", \"nome\": \"a|b\\nc\", \"obs\": {\"k\": [1, 2]}}\n]\n"
        );
        assert_eq!(
            copy(&mut store(), 0..0, 0..3, CopyFormat::Json, false, "t"),
            "[]\n"
        );
    }

    #[test]
    fn insert_and_in_list() {
        assert_eq!(
            copy(&mut store(), 0..1, 0..2, CopyFormat::Insert, false, "t"),
            "INSERT INTO \"t\" (\"id\", \"nome\") VALUES ('1', 'Ana; \"A\", B');\n"
        );
        // Uma coluna: os valores; números sem aspas, o resto como literal.
        assert_eq!(
            copy(&mut store(), 0..2, 0..1, CopyFormat::InList, false, "t"),
            "1, '007'\n"
        );
        let mut s = ResultStore::new(vec!["a".into(), "b".into()]);
        s.push(&[Some("O'Neil".into()), None]);
        s.push(&[Some("x".into()), Some("2.50".into())]);
        assert_eq!(
            copy(&mut s, 0..2, 0..2, CopyFormat::InList, false, "t"),
            "('O''Neil', NULL), ('x', 2.50)\n"
        );
    }

    #[test]
    fn markdown_and_html_are_tables() {
        assert_eq!(
            all(CopyFormat::Markdown, false),
            "| id | nome | obs |\n| --- | --- | --- |\n\
             | 1 | Ana; \"A\", B | NULL |\n| 007 | a\\|b<br>c | {\"k\": [1, 2]} |\n"
        );
        let html = copy(&mut store(), 0..1, 0..3, CopyFormat::Html, false, "t");
        assert_eq!(
            html,
            "<table>\n  <thead>\n    <tr><th>id</th><th>nome</th><th>obs</th></tr>\n  </thead>\n  \
             <tbody>\n    <tr><td>1</td><td>Ana; &quot;A&quot;, B</td><td></td></tr>\n  \
             </tbody>\n</table>\n"
        );
    }

    #[test]
    fn codes_round_trip() {
        for f in CopyFormat::ALL {
            assert_eq!(CopyFormat::from_code(f.code()), Some(f));
        }
        assert_eq!(CopyFormat::from_code("xml"), None);
    }
}
