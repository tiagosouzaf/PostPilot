//! Ordenação e filtro das linhas já recebidas, sem rodar a consulta de
//! novo.
//!
//! As páginas do [`ResultStore`](crate::ResultStore) não mudam: a visão é
//! um índice (linha mostrada → linha guardada) montado aos poucos, alguns
//! milissegundos por frame, para a janela não parar com resultados
//! grandes. A ordenação em si roda numa thread, só com as chaves.

use std::cmp::Ordering;

/// Ordenação por uma coluna.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub col: usize,
    pub descending: bool,
}

/// O que a grade mostra do resultado: ordem e filtro.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewSpec {
    pub sort: Option<Sort>,
    /// Texto do filtro, como digitado (veja [`Filter::parse`]).
    pub filter: String,
}

impl ViewSpec {
    /// Sem ordem e sem filtro: o resultado como chegou.
    pub fn is_plain(&self) -> bool {
        self.sort.is_none() && self.filter.trim().is_empty()
    }
}

/// Andamento da montagem da visão.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewProgress {
    /// Ainda montando (chame de novo no próximo frame).
    pub working: bool,
    /// Parte das linhas já lida, de 0 a 1.
    pub fraction: f32,
}

impl ViewProgress {
    pub(crate) const IDLE: ViewProgress = ViewProgress {
        working: false,
        fraction: 1.0,
    };
}

/// Um termo do filtro: o texto (já dobrado) e as colunas onde procurar
/// (`None`: qualquer uma).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    text: String,
    cols: Option<Vec<usize>>,
}

/// Filtro pronto para comparar com as linhas.
///
/// O texto é separado em termos por espaços (aspas juntam um termo com
/// espaços) e a linha passa se tiver todos. Um termo vale para qualquer
/// coluna, a não ser que venha como `coluna:texto`. Maiúsculas e acentos
/// não importam; NULL conta como o texto `NULL`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    terms: Vec<Term>,
}

impl Filter {
    pub fn parse(text: &str, columns: &[String]) -> Filter {
        let names: Vec<String> = columns.iter().map(|c| fold(c)).collect();
        let mut terms = Vec::new();
        for (token, quoted_from) in tokens(text) {
            // `coluna:texto`, com o `:` fora das aspas.
            let colon = token
                .char_indices()
                .find(|&(i, c)| c == ':' && quoted_from.is_none_or(|q| i < q))
                .map(|(i, _)| i);
            if let Some(i) = colon.filter(|&i| i > 0) {
                let prefix = fold(&token[..i]);
                let mut cols: Vec<usize> =
                    (0..names.len()).filter(|&c| names[c] == prefix).collect();
                if cols.is_empty() {
                    cols = (0..names.len())
                        .filter(|&c| names[c].starts_with(&prefix))
                        .collect();
                }
                if !cols.is_empty() {
                    let value = fold(&token[i + 1..]);
                    // `coluna:` ainda sem texto: a pessoa está digitando.
                    if !value.is_empty() {
                        terms.push(Term {
                            text: value,
                            cols: Some(cols),
                        });
                    }
                    continue;
                }
            }
            let text = fold(&token);
            if !text.is_empty() {
                terms.push(Term { text, cols: None });
            }
        }
        Filter { terms }
    }

    /// Nenhum termo: todas as linhas passam.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Se a linha passa. `cell` devolve o valor de uma coluna dela.
    pub(crate) fn matches<'a>(
        &self,
        columns: usize,
        buf: &mut String,
        mut cell: impl FnMut(usize) -> Option<&'a str>,
    ) -> bool {
        self.terms.iter().all(|t| {
            let mut hit = |c: usize| contains(cell(c).unwrap_or("NULL"), &t.text, buf);
            match &t.cols {
                Some(cols) => cols.iter().any(|&c| hit(c)),
                None => (0..columns).any(&mut hit),
            }
        })
    }
}

/// O texto do filtro que acha `value` na coluna `column`.
pub fn filter_for_value(column: &str, value: &str) -> String {
    // Sem escape para aspas: fica o maior pedaço entre elas.
    let value = value
        .split('"')
        .max_by_key(|p| p.len())
        .unwrap_or("")
        .trim();
    let simple =
        !column.is_empty() && !column.contains(|c: char| c.is_whitespace() || c == ':' || c == '"');
    if simple {
        format!("{column}:\"{value}\"")
    } else {
        format!("\"{value}\"")
    }
}

/// Termos do texto, sem as aspas, e onde começa o trecho entre aspas (para
/// não ler um `:` de dentro dele como separador de coluna).
fn tokens(text: &str) -> Vec<(String, Option<usize>)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted_from = None;
    let mut in_quotes = false;
    let mut any = false;
    for c in text.chars() {
        if c == '"' {
            in_quotes = !in_quotes;
            if in_quotes && quoted_from.is_none() {
                quoted_from = Some(current.len());
            }
            any = true;
        } else if c.is_whitespace() && !in_quotes {
            if any {
                out.push((std::mem::take(&mut current), quoted_from.take()));
                any = false;
            }
        } else {
            current.push(c);
            any = true;
        }
    }
    if any {
        out.push((current, quoted_from));
    }
    out
}

/// Minúsculas e sem os acentos mais comuns.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    fold_into(text, &mut out);
    out
}

fn fold_into(text: &str, out: &mut String) {
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
            continue;
        }
        for l in c.to_lowercase() {
            out.push(match l {
                'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
                'ç' => 'c',
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'í' | 'ì' | 'î' | 'ï' => 'i',
                'ñ' => 'n',
                'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
                'ú' | 'ù' | 'û' | 'ü' => 'u',
                'ý' | 'ÿ' => 'y',
                other => other,
            });
        }
    }
}

/// Se `hay` contém `needle` (já dobrado), sem olhar maiúsculas e acentos.
fn contains(hay: &str, needle: &str, buf: &mut String) -> bool {
    if needle.len() > hay.len() {
        return false;
    }
    if hay.is_ascii() {
        // Caminho rápido, sem alocar.
        if !needle.is_ascii() {
            return false;
        }
        let (h, n) = (hay.as_bytes(), needle.as_bytes());
        let first = n[0];
        return (0..=h.len() - n.len()).any(|i| {
            h[i].to_ascii_lowercase() == first
                && h[i..i + n.len()]
                    .iter()
                    .zip(n)
                    .all(|(a, b)| a.to_ascii_lowercase() == *b)
        });
    }
    buf.clear();
    fold_into(hay, buf);
    buf.contains(needle)
}

/// Ordena `rows` pelas chaves (o valor da coluna em cada linha, na mesma
/// posição). Se todos os valores são números, a ordem é numérica; se não,
/// é a do texto sem olhar maiúsculas e acentos. Empates ficam na ordem em
/// que vieram; NULL vai sempre para o fim (quem ordena do maior para o
/// menor quer ver os maiores, não os vazios).
pub fn sort_rows(rows: Vec<u64>, keys: Vec<Option<String>>, descending: bool) -> Vec<u64> {
    let numbers: Option<Vec<Option<f64>>> = keys
        .iter()
        .map(|k| match k {
            None => Some(None),
            Some(v) => v.trim().parse::<f64>().ok().map(Some),
        })
        .collect();
    fn nulls_last<T>(a: &Option<T>, b: &Option<T>, cmp: impl Fn(&T, &T) -> Ordering) -> Ordering {
        match (a, b) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => cmp(a, b),
        }
    }
    let mut order: Vec<u32> = (0..rows.len() as u32).collect();
    let by = |o: Ordering| if descending { o.reverse() } else { o };
    match numbers {
        Some(n) if n.iter().any(Option::is_some) => {
            order.sort_by(|&a, &b| {
                nulls_last(&n[a as usize], &n[b as usize], |x, y| by(x.total_cmp(y)))
            });
        }
        _ => {
            let folded: Vec<Option<String>> = keys.iter().map(|k| k.as_deref().map(fold)).collect();
            order.sort_by(|&a, &b| {
                let (a, b) = (a as usize, b as usize);
                nulls_last(&folded[a], &folded[b], |x, y| by(x.cmp(y)))
                    .then_with(|| nulls_last(&keys[a], &keys[b], |x, y| by(x.cmp(y))))
            });
        }
    }
    order.into_iter().map(|i| rows[i as usize]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols() -> Vec<String> {
        vec!["id".into(), "nome".into(), "cidade".into()]
    }

    fn passes(filter: &str, row: [Option<&str>; 3]) -> bool {
        let f = Filter::parse(filter, &cols());
        f.matches(3, &mut String::new(), |c| row[c])
    }

    #[test]
    fn terms_match_any_column_ignoring_case_and_accents() {
        let row = [Some("7"), Some("João Silva"), Some("São Paulo")];
        assert!(passes("joao", row));
        assert!(passes("SILVA paulo", row));
        assert!(passes("  ", row));
        assert!(!passes("joao rio", row));
        assert!(passes("\"joão silva\"", row));
        assert!(!passes("\"silva joão\"", row));
    }

    #[test]
    fn a_column_prefix_restricts_the_term() {
        let row = [Some("7"), Some("Paulo"), Some("Rio")];
        assert!(passes("nome:paulo", row));
        assert!(!passes("cidade:paulo", row));
        assert!(passes("cid:rio", row), "começo do nome da coluna");
        assert!(passes("nome:\"Paulo\"", row));
        // Coluna que não existe: é texto comum (uma hora, por exemplo).
        assert!(passes("12:30", [Some("12:30"), None, None]));
        assert!(!passes("12:30", row));
        // Ainda digitando o valor: não filtra.
        assert!(passes("nome:", row));
    }

    #[test]
    fn null_is_found_as_text() {
        assert!(passes("cidade:null", [Some("1"), Some("a"), None]));
        assert!(!passes("cidade:null", [Some("1"), Some("a"), Some("b")]));
    }

    #[test]
    fn filter_for_value_round_trips() {
        let text = filter_for_value("nome", "João Silva");
        assert_eq!(text, "nome:\"João Silva\"");
        assert!(passes(&text, [Some("1"), Some("João Silva"), None]));
        assert!(!passes(&text, [Some("João Silva"), Some("x"), None]));
        assert_eq!(filter_for_value("a b", "x"), "\"x\"");
    }

    fn sorted(values: &[Option<&str>], descending: bool) -> Vec<u64> {
        let keys = values.iter().map(|v| v.map(String::from)).collect();
        sort_rows((0..values.len() as u64).collect(), keys, descending)
    }

    #[test]
    fn numbers_sort_as_numbers_with_nulls_last() {
        let v = [Some("10"), Some("9"), None, Some("-1.5"), Some("9")];
        assert_eq!(sorted(&v, false), vec![3, 1, 4, 0, 2]);
        assert_eq!(sorted(&v, true), vec![0, 1, 4, 3, 2]);
    }

    #[test]
    fn text_sorts_without_case_and_accents() {
        let v = [
            Some("banana"),
            Some("Árvore"),
            Some("abacaxi"),
            Some("10"),
            None,
        ];
        assert_eq!(sorted(&v, false), vec![3, 2, 1, 0, 4]);
    }
}
