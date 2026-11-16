//! Exportar resultados: "Save Results As", "Results to File" e "Copy as
//! INSERT".
//!
//! O [`Writer`] escreve uma linha por vez, então dá para exportar direto
//! do servidor para o arquivo (Results to File) ou do buffer paginado, sem
//! juntar tudo na memória.
//!
//! - CSV: como o `COPY ... (FORMAT csv, HEADER)` lê: vírgula, aspas só
//!   quando precisa, NULL como campo vazio e texto vazio como `""`.
//! - TSV: o formato texto do `COPY` (tab, `\N` para NULL e barra invertida
//!   escapando tab, quebra de linha e a própria barra), com uma linha de
//!   cabeçalho. O `COPY ... (FORMAT text, HEADER)` lê o cabeçalho a partir
//!   do PostgreSQL 15; antes disso, ele precisa sair do arquivo.
//! - JSON: uma lista de objetos, coluna por chave, valores como texto ou
//!   `null`.
//! - INSERT: um `INSERT INTO` por linha, valores como literais que o banco
//!   converte para o tipo da coluna.
//! - Excel: uma pasta de trabalho `.xlsx` (veja [`crate::xlsx`]).

use std::io::{self, Write};
use std::ops::Range;
use std::path::Path;

use pgstudio_i18n::tr;

use crate::ResultStore;
use crate::xlsx::Workbook;

/// O que um "Save Results As" escreveu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    /// Linhas escritas.
    pub rows: u64,
    /// Células cortadas por não caberem no formato (Excel).
    pub truncated: u64,
}

/// Formato do arquivo exportado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Tsv,
    Json,
    /// Comandos `INSERT INTO` numa tabela.
    Insert,
    /// Pasta de trabalho do Excel (`.xlsx`).
    Xlsx,
}

impl Format {
    /// Formato pela extensão do arquivo (`.csv`, `.tsv`/`.txt`, `.json`,
    /// `.sql`, `.xlsx`); sem extensão conhecida, CSV.
    pub fn from_path(path: &Path) -> Format {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("tsv" | "txt" | "tab") => Format::Tsv,
            Some("json") => Format::Json,
            Some("sql") => Format::Insert,
            Some("xlsx") => Format::Xlsx,
            _ => Format::Csv,
        }
    }
}

/// Escreve um resultado, uma linha por vez.
pub struct Writer<W: Write> {
    /// A saída dos formatos de texto.
    out: Option<W>,
    /// A pasta de trabalho do Excel (uma planilha).
    book: Option<Workbook<W>>,
    format: Format,
    columns: Vec<String>,
    /// Tabela dos INSERTs, já entre aspas.
    table: String,
    rows: u64,
}

impl<W: Write> Writer<W> {
    /// Começa um resultado com estas colunas (o CSV e o TSV já escrevem o
    /// cabeçalho). `table` é a tabela dos INSERTs.
    pub fn new(mut out: W, format: Format, columns: Vec<String>, table: &str) -> io::Result<Self> {
        if format == Format::Xlsx {
            let mut book = Workbook::new(out, tr("Results"));
            book.sheet(&columns)?;
            return Ok(Writer {
                out: None,
                book: Some(book),
                format,
                columns,
                table: String::new(),
                rows: 0,
            });
        }
        match format {
            Format::Csv => {
                let names: Vec<String> = columns.iter().map(|c| csv_field(Some(c))).collect();
                writeln!(out, "{}", names.join(","))?;
            }
            Format::Tsv => {
                let names: Vec<String> = columns.iter().map(|c| tsv_field(Some(c))).collect();
                writeln!(out, "{}", names.join("\t"))?;
            }
            Format::Json => out.write_all(b"[")?,
            Format::Insert | Format::Xlsx => {}
        }
        Ok(Writer {
            out: Some(out),
            book: None,
            format,
            columns,
            table: quote_ident(table),
            rows: 0,
        })
    }

    /// Escreve uma linha.
    pub fn row(&mut self, row: &[Option<String>]) -> io::Result<()> {
        let cell = |i: usize| row.get(i).and_then(|v| v.as_deref());
        let n = self.columns.len();
        if let Some(book) = &mut self.book {
            book.row(row)?;
            self.rows += 1;
            return Ok(());
        }
        let out = self.out.as_mut().expect("formato de texto");
        match self.format {
            Format::Csv => {
                let fields: Vec<String> = (0..n).map(|i| csv_field(cell(i))).collect();
                writeln!(out, "{}", fields.join(","))?;
            }
            Format::Tsv => {
                let fields: Vec<String> = (0..n).map(|i| tsv_field(cell(i))).collect();
                writeln!(out, "{}", fields.join("\t"))?;
            }
            Format::Json => {
                let sep = if self.rows == 0 { "\n  {" } else { ",\n  {" };
                out.write_all(sep.as_bytes())?;
                for (i, name) in self.columns.iter().enumerate() {
                    if i > 0 {
                        out.write_all(b", ")?;
                    }
                    let value = cell(i).map_or("null".to_string(), json_string);
                    write!(out, "{}: {value}", json_string(name))?;
                }
                out.write_all(b"}")?;
            }
            Format::Xlsx => {}
            Format::Insert => {
                let names: Vec<String> = self.columns.iter().map(|c| quote_ident(c)).collect();
                let values: Vec<String> = (0..n).map(|i| literal(cell(i))).collect();
                writeln!(
                    out,
                    "INSERT INTO {} ({}) VALUES ({});",
                    self.table,
                    names.join(", "),
                    values.join(", ")
                )?;
            }
        }
        self.rows += 1;
        Ok(())
    }

    /// Linhas escritas.
    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// Células cortadas por não caberem no formato (só o Excel corta:
    /// veja [`crate::xlsx::MAX_CELL_CHARS`]).
    pub fn truncated(&self) -> u64 {
        self.book.as_ref().map_or(0, Workbook::truncated)
    }

    /// Fecha o resultado e devolve a saída.
    pub fn finish(self) -> io::Result<W> {
        if let Some(book) = self.book {
            return book.finish();
        }
        let mut out = self.out.expect("formato de texto");
        if self.format == Format::Json {
            let end = if self.rows == 0 { "]\n" } else { "\n]\n" };
            out.write_all(end.as_bytes())?;
        }
        out.flush()?;
        Ok(out)
    }
}

/// Exporta as linhas `rows` de um buffer travado só linha a linha: quem
/// mostra a grade continua lendo enquanto isso. Devolve o que foi
/// escrito.
pub fn write_rows<W: Write>(
    store: &std::sync::Mutex<ResultStore>,
    rows: Range<u64>,
    format: Format,
    table: &str,
    out: W,
) -> io::Result<Saved> {
    let lock = || store.lock().unwrap_or_else(|e| e.into_inner());
    let columns = lock().columns().to_vec();
    let mut w = Writer::new(out, format, columns, table)?;
    for r in rows {
        let row = lock().row(r);
        w.row(&row)?;
    }
    let saved = Saved {
        rows: w.rows(),
        truncated: w.truncated(),
    };
    w.finish()?;
    Ok(saved)
}

/// As linhas e colunas escolhidas como comandos INSERT (para "Copy as
/// INSERT").
pub fn insert_text(
    store: &mut ResultStore,
    rows: Range<u64>,
    cols: Range<usize>,
    table: &str,
) -> String {
    let columns = store.columns()[cols.clone()].to_vec();
    let mut w = Writer::new(Vec::new(), Format::Insert, columns, table).expect("Vec não falha");
    for r in rows {
        let row: Vec<Option<String>> = cols
            .clone()
            .map(|c| store.cell(r, c).map(str::to_string))
            .collect();
        w.row(&row).expect("Vec não falha");
    }
    String::from_utf8(w.finish().expect("Vec não falha")).expect("só texto")
}

/// Campo de CSV: aspas quando há vírgula, aspas, quebra de linha ou espaço
/// nas pontas, e sempre para o texto vazio (campo vazio é NULL).
fn csv_field(value: Option<&str>) -> String {
    match value {
        None => String::new(),
        Some(v)
            if v.is_empty()
                || v.contains([',', '"', '\n', '\r'])
                || v.starts_with(' ')
                || v.ends_with(' ') =>
        {
            format!("\"{}\"", v.replace('"', "\"\""))
        }
        Some(v) => v.to_string(),
    }
}

/// Campo do formato texto do `COPY`.
fn tsv_field(value: Option<&str>) -> String {
    match value {
        None => "\\N".to_string(),
        Some(v) => {
            let mut out = String::with_capacity(v.len());
            for c in v.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\t' => out.push_str("\\t"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    c => out.push(c),
                }
            }
            out
        }
    }
}

/// Texto como string de JSON.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Identificador entre aspas (sempre: vale para qualquer nome). Um nome
/// com ponto (`schema.tabela`) vira dois.
fn quote_ident(name: &str) -> String {
    name.split('.')
        .map(|p| format!("\"{}\"", p.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(".")
}

/// Valor como literal de SQL (NULL ou texto entre aspas; `E'...'` com
/// barra invertida).
fn literal(value: Option<&str>) -> String {
    match value {
        None => "NULL".to_string(),
        Some(v) if v.contains('\\') => {
            format!("E'{}'", v.replace('\\', "\\\\").replace('\'', "''"))
        }
        Some(v) => format!("'{}'", v.replace('\'', "''")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Vec<Option<String>>> {
        let s = |v: &str| Some(v.to_string());
        vec![
            vec![s("1"), s("Ana, \"A\""), None],
            vec![s("2"), s(""), s("linha\num\ttab \\ fim")],
        ]
    }

    fn export(format: Format) -> String {
        let columns = vec!["id".into(), "nome".into(), "obs".into()];
        let mut w = Writer::new(Vec::new(), format, columns, "public.cliente").unwrap();
        for r in rows() {
            w.row(&r).unwrap();
        }
        String::from_utf8(w.finish().unwrap()).unwrap()
    }

    #[test]
    fn csv_quotes_only_when_needed_and_keeps_null_apart_from_empty() {
        assert_eq!(
            export(Format::Csv),
            "id,nome,obs\n1,\"Ana, \"\"A\"\"\",\n2,\"\",\"linha\num\ttab \\ fim\"\n"
        );
    }

    #[test]
    fn tsv_is_the_copy_text_format() {
        assert_eq!(
            export(Format::Tsv),
            "id\tnome\tobs\n1\tAna, \"A\"\t\\N\n2\t\tlinha\\num\\ttab \\\\ fim\n"
        );
    }

    #[test]
    fn json_is_a_list_of_objects() {
        assert_eq!(
            export(Format::Json),
            "[\n  {\"id\": \"1\", \"nome\": \"Ana, \\\"A\\\"\", \"obs\": null},\n  \
             {\"id\": \"2\", \"nome\": \"\", \"obs\": \"linha\\num\\ttab \\\\ fim\"}\n]\n"
        );
        let empty = Writer::new(Vec::new(), Format::Json, vec!["a".into()], "t").unwrap();
        assert_eq!(empty.finish().unwrap(), b"[]\n");
    }

    #[test]
    fn inserts_quote_names_and_values() {
        assert_eq!(
            export(Format::Insert),
            "INSERT INTO \"public\".\"cliente\" (\"id\", \"nome\", \"obs\") VALUES ('1', 'Ana, \"A\"', NULL);\n\
             INSERT INTO \"public\".\"cliente\" (\"id\", \"nome\", \"obs\") VALUES ('2', '', E'linha\num\ttab \\\\ fim');\n"
        );
    }

    #[test]
    fn the_format_comes_from_the_extension() {
        assert_eq!(Format::from_path(Path::new("a.CSV")), Format::Csv);
        assert_eq!(Format::from_path(Path::new("a.txt")), Format::Tsv);
        assert_eq!(Format::from_path(Path::new("a.json")), Format::Json);
        assert_eq!(Format::from_path(Path::new("a.sql")), Format::Insert);
        assert_eq!(Format::from_path(Path::new("a.XLSX")), Format::Xlsx);
        assert_eq!(Format::from_path(Path::new("a")), Format::Csv);
    }

    #[test]
    fn a_spilled_store_exports_every_row() {
        // Orçamento pequeno: a maior parte das páginas vai para o disco.
        let mut store = ResultStore::with_budget(vec!["n".into(), "t".into()], 64 * 1024);
        for i in 0..50_000u64 {
            store.push(&[Some(i.to_string()), Some(format!("linha {i}"))]);
        }
        assert!(store.is_spilled());
        let store = std::sync::Mutex::new(store);
        let mut out = Vec::new();
        let n = write_rows(&store, 0..50_000, Format::Csv, "t", &mut out).unwrap();
        assert_eq!(n.rows, 50_000);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 50_001);
        assert_eq!(text.lines().last(), Some("49999,linha 49999"));
    }

    #[test]
    fn xlsx_is_a_workbook_with_one_sheet() {
        let mut store = ResultStore::new(vec!["id".into(), "nome".into(), "obs".into()]);
        for r in rows() {
            store.push(&r);
        }
        let store = std::sync::Mutex::new(store);
        let mut out = Vec::new();
        let saved = write_rows(&store, 0..2, Format::Xlsx, "t", &mut out).unwrap();
        assert_eq!(
            saved,
            Saved {
                rows: 2,
                truncated: 0
            }
        );
        let files = crate::xlsx::unzip(&out);
        let sheet = &files[0].1;
        assert!(sheet.contains("<c r=\"A2\"><v>1</v></c>"));
        assert!(sheet.contains(">Ana, &quot;A&quot;<"));
        assert!(sheet.contains("<autoFilter ref=\"A1:C3\"/>"));
    }

    #[test]
    fn copy_as_insert_uses_the_selection() {
        let mut store = ResultStore::new(vec!["id".into(), "nome".into(), "obs".into()]);
        for r in rows() {
            store.push(&r);
        }
        assert_eq!(
            insert_text(&mut store, 0..1, 0..2, "t"),
            "INSERT INTO \"t\" (\"id\", \"nome\") VALUES ('1', 'Ana, \"A\"');\n"
        );
    }
}
