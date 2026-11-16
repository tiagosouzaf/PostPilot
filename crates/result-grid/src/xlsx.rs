//! Pasta de trabalho do Excel (`.xlsx`) escrita uma linha por vez.
//!
//! Um `.xlsx` é um ZIP com alguns XML. Aqui o ZIP sai em sequência, sem
//! voltar no arquivo (cada entrada leva o tamanho e o CRC depois dos
//! dados), e os textos vão dentro da própria célula em vez de numa tabela
//! de textos compartilhados. Assim um resultado de milhões de linhas vai
//! para o arquivo sem ficar na memória.
//!
//! - Cada resultado é uma planilha, com o cabeçalho em negrito, congelado
//!   e com filtro.
//! - Uma planilha do Excel tem no máximo [`MAX_ROWS`] linhas: o que passa
//!   continua na planilha seguinte, com o cabeçalho de novo.
//! - Um valor vira número quando o texto é um número que o Excel mostra
//!   igual (veja [`is_number`]); o resto fica como texto, do jeito que o
//!   banco mandou. NULL e texto vazio ficam como célula vazia.
//! - Uma célula do Excel guarda no máximo [`MAX_CELL_CHARS`] caracteres:
//!   o que passa é cortado e contado em [`Workbook::truncated`].

use std::fmt::Write as _;
use std::io::{self, Write};

use flate2::Compression;
use flate2::write::DeflateEncoder;

/// Linhas de uma planilha do Excel (com o cabeçalho).
pub const MAX_ROWS: u32 = 1_048_576;
/// Caracteres de uma célula do Excel.
pub const MAX_CELL_CHARS: usize = 32_767;
/// Colunas de uma planilha do Excel.
pub const MAX_COLUMNS: usize = 16_384;
/// Uma entrada do ZIP sem ZIP64 tem menos de 4 GB: a planilha troca
/// antes disso.
const MAX_SHEET_BYTES: u64 = 3_500_000_000;

const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// Saída que conta os bytes escritos (as posições do ZIP).
struct Counting<W: Write> {
    inner: W,
    written: u64,
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Uma entrada já escrita, para o diretório do fim do ZIP.
struct Entry {
    name: String,
    crc: u32,
    compressed: u64,
    size: u64,
    offset: u64,
}

/// A entrada sendo escrita.
struct Open<W: Write> {
    name: String,
    encoder: DeflateEncoder<Counting<W>>,
    crc: crc32fast::Hasher,
    size: u64,
    offset: u64,
    data_start: u64,
}

/// ZIP em sequência: uma entrada por vez, comprimida.
struct Zip<W: Write> {
    idle: Option<Counting<W>>,
    open: Option<Open<W>>,
    entries: Vec<Entry>,
}

fn too_big() -> io::Error {
    io::Error::other("the workbook is larger than 4 GB, the limit of an .xlsx written here")
}

fn u32_of(n: u64) -> io::Result<u32> {
    u32::try_from(n).map_err(|_| too_big())
}

impl<W: Write> Zip<W> {
    fn new(out: W) -> Self {
        Zip {
            idle: Some(Counting {
                inner: out,
                written: 0,
            }),
            open: None,
            entries: Vec::new(),
        }
    }

    /// Começa a entrada `name` (fechando a anterior).
    fn begin(&mut self, name: &str) -> io::Result<()> {
        self.end()?;
        let mut out = self.idle.take().expect("sem entrada aberta");
        let offset = out.written;
        let mut header = Vec::with_capacity(30 + name.len());
        header.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        header.extend_from_slice(&20u16.to_le_bytes()); // versão para ler
        header.extend_from_slice(&0x0008u16.to_le_bytes()); // tamanhos depois dos dados
        header.extend_from_slice(&8u16.to_le_bytes()); // deflate
        header.extend_from_slice(&0u16.to_le_bytes()); // hora
        header.extend_from_slice(&0x0021u16.to_le_bytes()); // 1980-01-01
        header.extend_from_slice(&[0; 12]); // CRC e tamanhos: depois
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes());
        header.extend_from_slice(name.as_bytes());
        out.write_all(&header)?;
        let data_start = out.written;
        self.open = Some(Open {
            name: name.to_string(),
            encoder: DeflateEncoder::new(out, Compression::fast()),
            crc: crc32fast::Hasher::new(),
            size: 0,
            offset,
            data_start,
        });
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let open = self.open.as_mut().expect("entrada aberta");
        open.encoder.write_all(bytes)?;
        open.crc.update(bytes);
        open.size += bytes.len() as u64;
        Ok(())
    }

    /// Bytes (sem comprimir) da entrada aberta.
    fn open_size(&self) -> u64 {
        self.open.as_ref().map_or(0, |o| o.size)
    }

    /// Fecha a entrada aberta, se houver.
    fn end(&mut self) -> io::Result<()> {
        let Some(open) = self.open.take() else {
            return Ok(());
        };
        let mut out = open.encoder.finish()?;
        let entry = Entry {
            name: open.name,
            crc: open.crc.finalize(),
            compressed: out.written - open.data_start,
            size: open.size,
            offset: open.offset,
        };
        let mut tail = Vec::with_capacity(16);
        tail.extend_from_slice(&0x0807_4b50u32.to_le_bytes());
        tail.extend_from_slice(&entry.crc.to_le_bytes());
        tail.extend_from_slice(&u32_of(entry.compressed)?.to_le_bytes());
        tail.extend_from_slice(&u32_of(entry.size)?.to_le_bytes());
        out.write_all(&tail)?;
        self.entries.push(entry);
        self.idle = Some(out);
        Ok(())
    }

    fn file(&mut self, name: &str, content: &str) -> io::Result<()> {
        self.begin(name)?;
        self.write(content.as_bytes())?;
        self.end()
    }

    /// Escreve o diretório e devolve a saída.
    fn finish(mut self) -> io::Result<W> {
        self.end()?;
        let mut out = self.idle.take().expect("sem entrada aberta");
        let start = out.written;
        for e in &self.entries {
            let mut h = Vec::with_capacity(46 + e.name.len());
            h.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            h.extend_from_slice(&20u16.to_le_bytes()); // feito por
            h.extend_from_slice(&20u16.to_le_bytes()); // versão para ler
            h.extend_from_slice(&0x0008u16.to_le_bytes());
            h.extend_from_slice(&8u16.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&0x0021u16.to_le_bytes());
            h.extend_from_slice(&e.crc.to_le_bytes());
            h.extend_from_slice(&u32_of(e.compressed)?.to_le_bytes());
            h.extend_from_slice(&u32_of(e.size)?.to_le_bytes());
            h.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            h.extend_from_slice(&[0; 12]); // extra, comentário, disco, atributos
            h.extend_from_slice(&u32_of(e.offset)?.to_le_bytes());
            h.extend_from_slice(e.name.as_bytes());
            out.write_all(&h)?;
        }
        let size = out.written - start;
        let count = u16::try_from(self.entries.len()).map_err(|_| too_big())?;
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        end.extend_from_slice(&[0; 4]); // discos
        end.extend_from_slice(&count.to_le_bytes());
        end.extend_from_slice(&count.to_le_bytes());
        end.extend_from_slice(&u32_of(size)?.to_le_bytes());
        end.extend_from_slice(&u32_of(start)?.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        out.write_all(&end)?;
        out.flush()?;
        Ok(out.inner)
    }
}

/// Uma pasta de trabalho sendo escrita.
pub struct Workbook<W: Write> {
    zip: Zip<W>,
    /// Nome base das planilhas.
    title: String,
    /// Planilhas começadas.
    sheets: u32,
    /// Colunas da planilha aberta.
    columns: Vec<String>,
    /// Linhas já escritas na planilha aberta (com o cabeçalho).
    sheet_rows: u32,
    truncated: u64,
    /// Linha sendo montada (reaproveitada).
    line: String,
}

impl<W: Write> Workbook<W> {
    /// Começa uma pasta de trabalho em `out`; `title` é o nome das
    /// planilhas ("Resultados", "Resultados 2"...).
    pub fn new(out: W, title: &str) -> Self {
        let mut title: String = title
            .chars()
            .filter(|c| !matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\' | '\''))
            .take(24)
            .collect();
        if title.trim().is_empty() {
            title = "Sheet".to_string();
        }
        Workbook {
            zip: Zip::new(out),
            title,
            sheets: 0,
            columns: Vec::new(),
            sheet_rows: 0,
            truncated: 0,
            line: String::new(),
        }
    }

    /// Começa uma planilha com estas colunas (fechando a anterior).
    pub fn sheet(&mut self, columns: &[String]) -> io::Result<()> {
        if columns.len() > MAX_COLUMNS {
            return Err(io::Error::other(format!(
                "an Excel sheet holds at most {MAX_COLUMNS} columns"
            )));
        }
        self.end_sheet()?;
        self.columns = columns.to_vec();
        self.sheets += 1;
        self.zip
            .begin(&format!("xl/worksheets/sheet{}.xml", self.sheets))?;
        let mut head = format!("{XML}<worksheet xmlns=\"{MAIN_NS}\"><sheetViews><sheetView");
        if self.sheets == 1 {
            head.push_str(" tabSelected=\"1\"");
        }
        head.push_str(
            " workbookViewId=\"0\"><pane ySplit=\"1\" topLeftCell=\"A2\" \
             activePane=\"bottomLeft\" state=\"frozen\"/></sheetView></sheetViews>",
        );
        if !columns.is_empty() {
            head.push_str("<cols>");
            for (i, name) in columns.iter().enumerate() {
                let width = (name.chars().count() + 4).clamp(12, 60);
                let _ = write!(
                    head,
                    "<col min=\"{n}\" max=\"{n}\" width=\"{width}\" customWidth=\"1\"/>",
                    n = i + 1
                );
            }
            head.push_str("</cols>");
        }
        head.push_str("<sheetData>");
        self.zip.write(head.as_bytes())?;
        self.sheet_rows = 0;
        if columns.is_empty() {
            return Ok(());
        }
        let header: Vec<Option<String>> = columns.iter().map(|c| Some(c.clone())).collect();
        self.write_row(&header, true)
    }

    /// Escreve uma linha na planilha aberta.
    pub fn row(&mut self, row: &[Option<String>]) -> io::Result<()> {
        if self.sheets == 0 {
            self.sheet(&[])?;
        }
        if self.sheet_rows >= MAX_ROWS || self.zip.open_size() >= MAX_SHEET_BYTES {
            let columns = std::mem::take(&mut self.columns);
            self.sheet(&columns)?;
        }
        self.write_row(row, false)
    }

    fn write_row(&mut self, row: &[Option<String>], header: bool) -> io::Result<()> {
        self.sheet_rows += 1;
        let r = self.sheet_rows;
        let mut line = std::mem::take(&mut self.line);
        line.clear();
        let _ = write!(line, "<row r=\"{r}\">");
        let n = if header {
            row.len()
        } else {
            self.columns.len().min(row.len())
        };
        for (i, value) in row.iter().take(n).enumerate() {
            let Some(value) = value.as_deref().filter(|v| !v.is_empty()) else {
                continue;
            };
            line.push_str("<c r=\"");
            column_letters(i, &mut line);
            let _ = write!(line, "{r}\"");
            if !header && is_number(value) {
                let _ = write!(line, "><v>{value}</v></c>");
                continue;
            }
            if header {
                line.push_str(" s=\"1\"");
            }
            line.push_str(" t=\"inlineStr\"><is><t xml:space=\"preserve\">");
            if !push_text(value, &mut line) {
                self.truncated += 1;
            }
            line.push_str("</t></is></c>");
        }
        line.push_str("</row>");
        let result = self.zip.write(line.as_bytes());
        self.line = line;
        result
    }

    fn end_sheet(&mut self) -> io::Result<()> {
        if self.zip.open.is_none() {
            return Ok(());
        }
        let mut tail = String::from("</sheetData>");
        if !self.columns.is_empty() {
            tail.push_str("<autoFilter ref=\"A1:");
            column_letters(self.columns.len() - 1, &mut tail);
            let _ = write!(tail, "{}\"/>", self.sheet_rows.max(1));
        }
        tail.push_str("</worksheet>");
        self.zip.write(tail.as_bytes())?;
        self.zip.end()
    }

    /// Células cortadas por passarem de [`MAX_CELL_CHARS`].
    pub fn truncated(&self) -> u64 {
        self.truncated
    }

    /// Planilhas começadas.
    pub fn sheets(&self) -> u32 {
        self.sheets
    }

    /// Fecha a pasta de trabalho e devolve a saída.
    pub fn finish(mut self) -> io::Result<W> {
        if self.sheets == 0 {
            self.sheet(&[])?;
        }
        self.end_sheet()?;
        let n = self.sheets;
        let mut types = format!(
            "{XML}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
             <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>"
        );
        let mut book = format!("{XML}<workbook xmlns=\"{MAIN_NS}\" xmlns:r=\"{REL_NS}\"><sheets>");
        let mut rels = format!(
            "{XML}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">"
        );
        for i in 1..=n {
            let _ = write!(
                types,
                "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
            );
            let mut name = String::new();
            push_text(&self.title, &mut name);
            if n > 1 {
                let _ = write!(name, " {i}");
            }
            let _ = write!(
                book,
                "<sheet name=\"{name}\" sheetId=\"{i}\" r:id=\"rId{i}\"/>"
            );
            let _ = write!(
                rels,
                "<Relationship Id=\"rId{i}\" Type=\"{REL_NS}/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"
            );
        }
        types.push_str("</Types>");
        book.push_str("</sheets></workbook>");
        let _ = write!(
            rels,
            "<Relationship Id=\"rId{}\" Type=\"{REL_NS}/styles\" Target=\"styles.xml\"/></Relationships>",
            n + 1
        );
        self.zip.file("xl/workbook.xml", &book)?;
        self.zip.file("xl/_rels/workbook.xml.rels", &rels)?;
        self.zip.file("xl/styles.xml", &styles())?;
        self.zip.file(
            "_rels/.rels",
            &format!(
                "{XML}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
                 <Relationship Id=\"rId1\" Type=\"{REL_NS}/officeDocument\" Target=\"xl/workbook.xml\"/>\
                 </Relationships>"
            ),
        )?;
        self.zip.file("[Content_Types].xml", &types)?;
        self.zip.finish()
    }
}

/// Estilos: o normal (0) e o do cabeçalho, em negrito (1).
fn styles() -> String {
    format!(
        "{XML}<styleSheet xmlns=\"{MAIN_NS}\">\
         <fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font>\
         <font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
         <fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill>\
         <fill><patternFill patternType=\"gray125\"/></fill></fills>\
         <borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
         <cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
         <cellXfs count=\"2\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
         <xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"/></cellXfs>\
         <cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>\
         </styleSheet>"
    )
}

/// Letras da coluna `index` (0 é `A`, 26 é `AA`).
fn column_letters(index: usize, out: &mut String) {
    let mut n = index + 1;
    let mut letters = [0u8; 4];
    let mut len = 0;
    while n > 0 {
        let rem = (n - 1) % 26;
        letters[len] = b'A' + rem as u8;
        len += 1;
        n = (n - 1) / 26;
    }
    for b in letters[..len].iter().rev() {
        out.push(*b as char);
    }
}

/// Se o texto é um número que o Excel mostra igual: dígitos com sinal e
/// casas decimais opcionais, sem zero à esquerda (um código como `00123`
/// continua texto), inteiro de até 11 dígitos (acima disso o Excel mostra
/// em notação científica) e até 15 dígitos ao todo (a precisão do Excel).
pub fn is_number(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let (int, frac) = match digits.split_once('.') {
        Some((int, frac)) => (int, Some(frac)),
        None => (digits, None),
    };
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(int) || (int.len() > 1 && int.starts_with('0')) || int.len() > 11 {
        return false;
    }
    match frac {
        None => true,
        Some(frac) => all_digits(frac) && int.len() + frac.len() <= 15,
    }
}

/// Texto como conteúdo de XML, até [`MAX_CELL_CHARS`]. Devolve `false`
/// se cortou. Caracteres de controle que o XML não aceita viram
/// `_xHHHH_`, como o Excel escreve.
fn push_text(value: &str, out: &mut String) -> bool {
    for (i, c) in value.chars().enumerate() {
        if i == MAX_CELL_CHARS {
            return false;
        }
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\r' => out.push_str("&#13;"),
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 || matches!(c, '\u{FFFE}' | '\u{FFFF}') => {
                let _ = write!(out, "_x{:04X}_", c as u32);
            }
            c => out.push(c),
        }
    }
    true
}

/// Leitura mínima de um `.xlsx` para os testes: as entradas do ZIP pelo
/// diretório, descomprimidas e com o CRC conferido.
#[cfg(test)]
pub(crate) fn unzip(bytes: &[u8]) -> Vec<(String, String)> {
    use std::io::Read;
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]) as usize;
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
    let end = bytes.len() - 22;
    assert_eq!(u32_at(end), 0x0605_4b50, "fim do diretório");
    let count = u16_at(end + 10);
    let mut at = u32_at(end + 16);
    assert_eq!(at + u32_at(end + 12), end, "o diretório termina no fim");
    let mut files = Vec::new();
    for _ in 0..count {
        assert_eq!(u32_at(at), 0x0201_4b50, "entrada do diretório");
        let crc = u32_at(at + 16) as u32;
        let compressed = u32_at(at + 20);
        let size = u32_at(at + 24);
        let name_len = u16_at(at + 28);
        let offset = u32_at(at + 42);
        let name = String::from_utf8(bytes[at + 46..at + 46 + name_len].to_vec()).unwrap();
        assert_eq!(u32_at(offset), 0x0403_4b50, "cabeçalho de {name}");
        assert_eq!(u16_at(offset + 26), name_len);
        let data = offset + 30 + name_len + u16_at(offset + 28);
        let mut text = String::new();
        flate2::read::DeflateDecoder::new(&bytes[data..data + compressed])
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text.len(), size, "tamanho de {name}");
        assert_eq!(crc32fast::hash(text.as_bytes()), crc, "CRC de {name}");
        // Depois dos dados: assinatura, CRC e tamanhos.
        let tail = data + compressed;
        assert_eq!(u32_at(tail), 0x0807_4b50);
        assert_eq!(u32_at(tail + 4) as u32, crc);
        assert_eq!(u32_at(tail + 8), compressed);
        assert_eq!(u32_at(tail + 12), size);
        files.push((name, text));
        at += 46 + name_len;
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part<'a>(files: &'a [(String, String)], name: &str) -> &'a str {
        &files
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("sem {name}"))
            .1
    }

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn a_workbook_has_the_parts_excel_needs() {
        let mut w = Workbook::new(Vec::new(), "Resultados");
        w.sheet(&["id".into(), "nome".into()]).unwrap();
        w.row(&[s("1"), s("Ana & <Bia>")]).unwrap();
        w.row(&[s("2"), None]).unwrap();
        let files = unzip(&w.finish().unwrap());
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "xl/worksheets/sheet1.xml",
                "xl/workbook.xml",
                "xl/_rels/workbook.xml.rels",
                "xl/styles.xml",
                "_rels/.rels",
                "[Content_Types].xml",
            ]
        );
        let sheet = part(&files, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains(
            "<row r=\"1\"><c r=\"A1\" s=\"1\" t=\"inlineStr\"><is><t xml:space=\"preserve\">id</t></is></c>"
        ));
        assert!(sheet.contains(
            "<row r=\"2\"><c r=\"A2\"><v>1</v></c>\
             <c r=\"B2\" t=\"inlineStr\"><is><t xml:space=\"preserve\">Ana &amp; &lt;Bia&gt;</t></is></c></row>"
        ));
        // NULL é célula vazia.
        assert!(sheet.contains("<row r=\"3\"><c r=\"A3\"><v>2</v></c></row>"));
        assert!(sheet.ends_with("</sheetData><autoFilter ref=\"A1:B3\"/></worksheet>"));
        assert!(
            part(&files, "xl/workbook.xml")
                .contains("<sheet name=\"Resultados\" sheetId=\"1\" r:id=\"rId1\"/>")
        );
        assert!(part(&files, "[Content_Types].xml").contains("/xl/worksheets/sheet1.xml"));
    }

    #[test]
    fn each_result_is_a_sheet() {
        let mut w = Workbook::new(Vec::new(), "Resultados");
        w.sheet(&["a".into()]).unwrap();
        w.row(&[s("x")]).unwrap();
        w.sheet(&["b".into(), "c".into()]).unwrap();
        w.row(&[s("y"), s("z")]).unwrap();
        assert_eq!(w.sheets(), 2);
        let files = unzip(&w.finish().unwrap());
        assert!(part(&files, "xl/worksheets/sheet2.xml").contains(">y<"));
        let book = part(&files, "xl/workbook.xml");
        assert!(book.contains("<sheet name=\"Resultados 1\" sheetId=\"1\" r:id=\"rId1\"/>"));
        assert!(book.contains("<sheet name=\"Resultados 2\" sheetId=\"2\" r:id=\"rId2\"/>"));
        assert!(
            part(&files, "xl/_rels/workbook.xml.rels")
                .contains("Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\"")
        );
    }

    #[test]
    fn rows_past_the_excel_limit_continue_on_the_next_sheet() {
        let mut w = Workbook::new(std::io::sink(), "R");
        w.sheet(&["n".into()]).unwrap();
        let row = [s("1")];
        for _ in 0..MAX_ROWS {
            w.row(&row).unwrap();
        }
        // O cabeçalho ocupa uma linha: a última foi para a planilha 2.
        assert_eq!(w.sheets(), 2);
        assert_eq!(w.sheet_rows, 2);
        w.finish().unwrap();
    }

    #[test]
    fn only_numbers_excel_shows_the_same_become_numbers() {
        for v in ["0", "7", "-12", "3.50", "0.001", "12345678901", "-0.5"] {
            assert!(is_number(v), "{v}");
        }
        for v in [
            "",
            "-",
            "00123",
            "01",
            "1e5",
            "1.",
            ".5",
            "+1",
            " 1",
            "1,5",
            "NaN",
            "abc",
            "123456789012",
            "1234567890.123456",
            "2026-11-16",
            "t",
        ] {
            assert!(!is_number(v), "{v}");
        }
    }

    #[test]
    fn long_and_odd_text_fits_a_cell() {
        let mut w = Workbook::new(Vec::new(), "R");
        w.sheet(&["t".into()]).unwrap();
        w.row(&[Some("x".repeat(MAX_CELL_CHARS + 10))]).unwrap();
        w.row(&[s("a\u{1}b\r\nc\td \"q\"")]).unwrap();
        assert_eq!(w.truncated(), 1);
        let files = unzip(&w.finish().unwrap());
        let sheet = part(&files, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains(&format!(">{}<", "x".repeat(MAX_CELL_CHARS))));
        assert!(sheet.contains(">a_x0001_b&#13;\nc\td &quot;q&quot;<"));
    }

    #[test]
    fn column_letters_follow_excel() {
        let name = |i| {
            let mut out = String::new();
            column_letters(i, &mut out);
            out
        };
        assert_eq!(name(0), "A");
        assert_eq!(name(25), "Z");
        assert_eq!(name(26), "AA");
        assert_eq!(name(701), "ZZ");
        assert_eq!(name(702), "AAA");
        assert_eq!(name(MAX_COLUMNS - 1), "XFD");
    }

    #[test]
    fn an_empty_workbook_still_opens() {
        let files = unzip(&Workbook::new(Vec::new(), "").finish().unwrap());
        assert!(part(&files, "xl/workbook.xml").contains("<sheet name=\"Sheet\""));
        assert!(
            part(&files, "xl/worksheets/sheet1.xml")
                .ends_with("<sheetData></sheetData></worksheet>")
        );
    }
}
