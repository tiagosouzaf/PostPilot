//! Operações de texto do editor, sem interface: comentar linhas, achar
//! linhas e buscar. Posições são índices de caractere (como o cursor do
//! egui), não bytes.

use std::ops::Range;

/// Byte onde começa o caractere `char_idx` (ou o fim do texto).
pub fn byte_of(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map_or(text.len(), |(b, _)| b)
}

/// Índice de caractere do byte `byte` (que deve estar numa fronteira).
pub fn char_of(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].chars().count()
}

/// Linha (a partir de 0) onde está o caractere `char_idx`.
pub fn line_of(text: &str, char_idx: usize) -> usize {
    let byte = byte_of(text, char_idx);
    text[..byte].matches('\n').count()
}

/// Número de linhas (um texto vazio tem uma linha).
pub fn line_count(text: &str) -> usize {
    text.matches('\n').count() + 1
}

/// Caractere onde começa a linha `line` (a partir de 0), limitada à última.
pub fn line_start(text: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    let mut seen = 0;
    for (i, c) in text.chars().enumerate() {
        if c == '\n' {
            seen += 1;
            if seen == line {
                return i + 1;
            }
        }
    }
    // Além do fim: começo da última linha.
    text.rfind('\n').map_or(0, |b| char_of(text, b + 1))
}

/// Linhas tocadas pela seleção `[a, b]` (em caracteres, em qualquer ordem).
/// Uma seleção que termina no começo de uma linha não inclui essa linha,
/// como no Visual Studio.
pub fn selected_lines(text: &str, a: usize, b: usize) -> Range<usize> {
    let (start, end) = (a.min(b), a.max(b));
    let first = line_of(text, start);
    let mut last = line_of(text, end);
    if end > start && last > first && line_start(text, last) == end {
        last -= 1;
    }
    first..last + 1
}

/// Resultado de uma edição: o texto novo e a seleção a manter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edited {
    pub text: String,
    /// Seleção nova, em caracteres: (âncora, cursor).
    pub selection: (usize, usize),
}

/// Põe `-- ` no começo de cada linha selecionada, como Ctrl+K, Ctrl+C no
/// SSMS. Linhas vazias ficam como estão. O comentário fica na menor
/// indentação do bloco, para manter o alinhamento.
pub fn comment_lines(text: &str, a: usize, b: usize) -> Edited {
    let lines = selected_lines(text, a, b);
    let parts: Vec<&str> = text.split('\n').collect();
    let indent = parts[lines.clone()]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = Vec::with_capacity(parts.len());
    for (i, line) in parts.iter().enumerate() {
        if lines.contains(&i) && !line.trim().is_empty() && line.is_char_boundary(indent) {
            out.push(format!("{}-- {}", &line[..indent], &line[indent..]));
        } else {
            out.push((*line).to_string());
        }
    }
    reselect(text, out.join("\n"), lines)
}

/// Tira o `--` (e um espaço depois dele) do começo de cada linha
/// selecionada, como Ctrl+K, Ctrl+U. Linhas sem comentário ficam como estão.
pub fn uncomment_lines(text: &str, a: usize, b: usize) -> Edited {
    let lines = selected_lines(text, a, b);
    let out: Vec<String> = text
        .split('\n')
        .enumerate()
        .map(|(i, line)| {
            if !lines.contains(&i) {
                return line.to_string();
            }
            let trimmed = line.trim_start();
            let indent = &line[..line.len() - trimmed.len()];
            match trimmed.strip_prefix("--") {
                Some(rest) => {
                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                    format!("{indent}{rest}")
                }
                None => line.to_string(),
            }
        })
        .collect();
    reselect(text, out.join("\n"), lines)
}

/// Seleciona as linhas editadas inteiras no texto novo.
fn reselect(_old: &str, text: String, lines: Range<usize>) -> Edited {
    let start = line_start(&text, lines.start);
    let last = lines.end - 1;
    let line_begin = line_start(&text, last);
    let line_len = text.split('\n').nth(last).map_or(0, |l| l.chars().count());
    Edited {
        selection: (start, line_begin + line_len),
        text,
    }
}

/// Próxima ocorrência de `needle` a partir do caractere `from`, voltando ao
/// começo se preciso. Devolve o intervalo em caracteres.
pub fn find_next(text: &str, needle: &str, from: usize, match_case: bool) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let (hay, needle) = if match_case {
        (text.to_string(), needle.to_string())
    } else {
        (text.to_lowercase(), needle.to_lowercase())
    };
    // `to_lowercase` pode mudar o tamanho de alguns caracteres; nesse caso
    // a busca sem diferenciar maiúsculas compara caractere a caractere.
    if hay.len() != text.len() {
        return find_by_chars(text, &needle, from);
    }
    let from_byte = byte_of(text, from);
    let found = hay[from_byte..]
        .find(&needle)
        .map(|b| b + from_byte)
        .or_else(|| hay.find(&needle))?;
    let start = char_of(text, found);
    Some(start..start + needle.chars().count())
}

fn find_by_chars(text: &str, needle_lower: &str, from: usize) -> Option<Range<usize>> {
    let chars: Vec<char> = text.chars().collect();
    let needle: Vec<char> = needle_lower.chars().collect();
    let matches_at = |i: usize| {
        chars.len() >= i + needle.len()
            && chars[i..i + needle.len()]
                .iter()
                .zip(&needle)
                .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
    };
    (from..chars.len())
        .chain(0..from.min(chars.len()))
        .find(|&i| matches_at(i))
        .map(|i| i..i + needle.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_helpers() {
        let t = "ab\ncd\n\nção";
        assert_eq!(line_count(t), 4);
        assert_eq!(line_start(t, 0), 0);
        assert_eq!(line_start(t, 1), 3);
        assert_eq!(line_start(t, 3), 7);
        assert_eq!(line_start(t, 99), 7);
        assert_eq!(line_of(t, 4), 1);
        assert_eq!(line_of(t, 9), 3);
        assert_eq!(byte_of(t, 8), 9);
        assert_eq!(char_of(t, 9), 8);
    }

    #[test]
    fn selection_ending_at_line_start_skips_that_line() {
        let t = "a\nb\nc";
        assert_eq!(selected_lines(t, 0, 4), 0..2);
        assert_eq!(selected_lines(t, 4, 0), 0..2);
        assert_eq!(selected_lines(t, 0, 5), 0..3);
        assert_eq!(selected_lines(t, 2, 2), 1..2);
    }

    #[test]
    fn comment_and_uncomment_round_trip() {
        let t = "select 1\n  from t\n\n  where x";
        let e = comment_lines(t, 0, t.chars().count());
        assert_eq!(e.text, "-- select 1\n--   from t\n\n--   where x");
        let back = uncomment_lines(&e.text, 0, e.text.chars().count());
        assert_eq!(back.text, t);

        let indented = "  a\n    b";
        let e = comment_lines(indented, 0, 9);
        assert_eq!(e.text, "  -- a\n  --   b");
        assert_eq!(uncomment_lines(&e.text, 0, 99).text, "  a\n    b");
    }

    #[test]
    fn comment_touches_only_selected_lines_and_reselects_them() {
        let t = "a\nb\nc";
        let e = comment_lines(t, 2, 2);
        assert_eq!(e.text, "a\n-- b\nc");
        assert_eq!(e.selection, (2, 6));
        assert_eq!(uncomment_lines("x\n--y\nz", 3, 3).text, "x\ny\nz");
        assert_eq!(uncomment_lines("not a comment", 0, 3).text, "not a comment");
    }

    #[test]
    fn find_wraps_and_ignores_case() {
        let t = "Select a FROM b; select c";
        assert_eq!(find_next(t, "select", 0, false), Some(0..6));
        assert_eq!(find_next(t, "select", 1, false), Some(17..23));
        assert_eq!(find_next(t, "select", 18, false), Some(0..6));
        assert_eq!(find_next(t, "Select", 1, true), Some(0..6));
        assert_eq!(find_next(t, "nope", 0, false), None);
        assert_eq!(find_next(t, "", 0, false), None);
        assert_eq!(find_next("ação AÇÃO", "ação", 1, false), Some(5..9));
        // Caractere que muda de tamanho ao passar para minúscula.
        assert_eq!(find_next("İx ix", "ix", 1, false), Some(3..5));
    }
}
