//! Resultados como texto: "Results to Text" (Ctrl+T) e o que vai para a
//! área de transferência ao copiar da grade.

use std::ops::Range;

use crate::store::ResultStore;

/// Largura máxima de uma coluna em "Results to Text", como o padrão do SSMS.
pub const MAX_TEXT_WIDTH: usize = 256;

/// Linhas lidas para calcular as larguras das colunas.
pub const WIDTH_SAMPLE: u64 = 1000;

/// Larguras (em caracteres) das colunas no modo texto: o maior entre o nome
/// e os valores das primeiras [`WIDTH_SAMPLE`] linhas, até
/// [`MAX_TEXT_WIDTH`].
pub fn widths(store: &mut ResultStore) -> Vec<usize> {
    let rows = store.len().min(WIDTH_SAMPLE);
    (0..store.columns().len())
        .map(|c| {
            let mut w = store.columns()[c].chars().count();
            for r in 0..rows {
                let len = store.cell(r, c).map_or(4, |v| v.chars().count());
                w = w.max(len);
            }
            w.clamp(1, MAX_TEXT_WIDTH)
        })
        .collect()
}

fn pad(out: &mut String, value: &str, width: usize) {
    let mut n = 0;
    for ch in value.chars() {
        if n == width {
            break;
        }
        out.push(if ch == '\n' || ch == '\r' || ch == '\t' {
            ' '
        } else {
            ch
        });
        n += 1;
    }
    out.extend(std::iter::repeat_n(' ', width - n));
}

/// Cabeçalho do modo texto.
pub fn header_line(store: &ResultStore, widths: &[usize]) -> String {
    let mut out = String::new();
    for (i, (name, w)) in store.columns().iter().zip(widths).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        pad(&mut out, name, *w);
    }
    out.trim_end().to_string()
}

/// Linha de traços embaixo do cabeçalho.
pub fn rule_line(widths: &[usize]) -> String {
    widths
        .iter()
        .map(|w| "-".repeat(*w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Uma linha do resultado no modo texto.
pub fn row_line(store: &mut ResultStore, row: u64, widths: &[usize]) -> String {
    let mut out = String::new();
    for (c, w) in widths.iter().enumerate() {
        if c > 0 {
            out.push(' ');
        }
        let value = store.cell(row, c).unwrap_or("NULL").to_string();
        pad(&mut out, &value, *w);
    }
    out.trim_end().to_string()
}

/// Texto copiado da grade: valores separados por tab, uma linha por linha,
/// NULL como `NULL` (como o SSMS). Com `headers`, a primeira linha tem os
/// nomes das colunas.
pub fn copy(
    store: &mut ResultStore,
    rows: Range<u64>,
    cols: Range<usize>,
    headers: bool,
) -> String {
    let mut out = String::new();
    if headers {
        let names: Vec<&str> = store.columns()[cols.clone()]
            .iter()
            .map(String::as_str)
            .collect();
        out.push_str(&names.join("\t"));
        out.push('\n');
    }
    for r in rows {
        for (i, c) in cols.clone().enumerate() {
            if i > 0 {
                out.push('\t');
            }
            out.push_str(store.cell(r, c).unwrap_or("NULL"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ResultStore {
        let mut s = ResultStore::new(vec!["id".into(), "nome".into(), "email".into()]);
        s.push(&[Some("1".into()), Some("Ana".into()), None]);
        s.push(&[
            Some("22".into()),
            Some("Bruno\nLima".into()),
            Some("b@x".into()),
        ]);
        s
    }

    #[test]
    fn text_mode_pads_columns_like_ssms() {
        let mut s = store();
        let w = widths(&mut s);
        assert_eq!(w, vec![2, 10, 5]);
        assert_eq!(header_line(&s, &w), "id nome       email");
        assert_eq!(rule_line(&w), "-- ---------- -----");
        assert_eq!(row_line(&mut s, 0, &w), "1  Ana        NULL");
        assert_eq!(row_line(&mut s, 1, &w), "22 Bruno Lima b@x");
    }

    #[test]
    fn copy_with_and_without_headers() {
        let mut s = store();
        assert_eq!(
            copy(&mut s, 0..2, 1..3, false),
            "Ana\tNULL\nBruno\nLima\tb@x\n"
        );
        assert_eq!(copy(&mut s, 0..1, 0..2, true), "id\tnome\n1\tAna\n");
    }
}
