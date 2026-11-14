//! Erros de sintaxe sublinhados no editor.
//!
//! Sem um parser completo, o editor acusa o que dá para ver pelos tokens:
//! string, nome entre aspas, `$$` ou comentário de bloco sem fim, e
//! parênteses sem par. O erro que o servidor devolve na execução também é
//! sublinhado (ver `EditorState::mark_error`).

use std::ops::Range;

use crate::lexer::{Token, TokenKind, tokenize};

/// Um trecho sublinhado, em bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub message: String,
}

/// Se o token, que vai até o fim do texto, ficou sem fechar: relido com
/// algo depois, ele engoliria o que vem depois.
fn unterminated(piece: &str) -> bool {
    let probe = format!("{piece}\n x");
    tokenize(&probe)
        .first()
        .is_some_and(|t| t.range.end > piece.len())
}

/// Erros visíveis pelos tokens.
pub fn check(text: &str, tokens: &[Token]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if let Some(last) = tokens.last()
        && last.range.end == text.len()
    {
        let piece = &text[last.range.clone()];
        let message = match last.kind {
            TokenKind::String if piece.starts_with('$') => {
                Some("Unterminated dollar-quoted string.")
            }
            TokenKind::String => Some("Unclosed quotation mark after the character string."),
            TokenKind::QuotedIdentifier => Some("Unclosed quotation mark after the identifier."),
            TokenKind::Comment if piece.starts_with("/*") => Some("Missing end comment mark '*/'."),
            _ => None,
        };
        // Sem o fecho no fim já é erro; com ele, ainda pode ser escape (`''`)
        // ou comentário aninhado, e aí relê.
        let closes = match last.kind {
            TokenKind::String if piece.starts_with('$') => {
                let tag_end = piece[1..].find('$').map_or(1, |i| i + 2);
                piece.len() >= 2 * tag_end && piece.ends_with(&piece[..tag_end])
            }
            TokenKind::String => piece.len() >= 2 && piece.ends_with('\''),
            TokenKind::QuotedIdentifier => piece.len() >= 2 && piece.ends_with('"'),
            _ => piece.len() >= 4 && piece.ends_with("*/"),
        };
        if let Some(message) = message
            && (!closes || unterminated(piece))
        {
            out.push(Diagnostic {
                range: last.range.clone(),
                message: message.to_string(),
            });
        }
    }
    // Parênteses, comando a comando.
    let mut open: Vec<Range<usize>> = Vec::new();
    let flush = |open: &mut Vec<Range<usize>>, out: &mut Vec<Diagnostic>| {
        for r in open.drain(..) {
            out.push(Diagnostic {
                range: r,
                message: "Missing ')'.".to_string(),
            });
        }
    };
    for t in tokens.iter().filter(|t| t.kind == TokenKind::Punctuation) {
        match &text[t.range.clone()] {
            "(" => open.push(t.range.clone()),
            ")" => {
                if open.pop().is_none() {
                    out.push(Diagnostic {
                        range: t.range.clone(),
                        message: "Incorrect syntax near ')'.".to_string(),
                    });
                }
            }
            ";" => flush(&mut open, &mut out),
            _ => {}
        }
    }
    flush(&mut open, &mut out);
    out.sort_by_key(|d| d.range.start);
    out
}

/// O trecho a sublinhar para um erro do servidor na posição `at` (bytes):
/// o token ali, ou o anterior se `at` cair em espaço ou no fim.
pub fn server_range(text: &str, tokens: &[Token], at: usize) -> Range<usize> {
    let i = tokens.partition_point(|t| t.range.end <= at);
    let pick = |t: &Token| !matches!(t.kind, TokenKind::Whitespace);
    match tokens.get(i).filter(|t| pick(t)) {
        Some(t) => t.range.clone(),
        None => tokens[..i]
            .iter()
            .rev()
            .find(|t| pick(t))
            .map_or(at..text.len().min(at + 1), |t| t.range.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(sql: &str) -> Vec<(String, String)> {
        check(sql, &tokenize(sql))
            .into_iter()
            .map(|d| (sql[d.range].to_string(), d.message))
            .collect()
    }

    #[test]
    fn clean_sql_has_no_errors() {
        assert!(errors("select f(a, (b)) from t; select 'x''y', \"q\" /* c */ $$ $$").is_empty());
    }

    #[test]
    fn unterminated_pieces_are_flagged() {
        assert_eq!(
            errors("select 'abc"),
            [(
                "'abc".into(),
                "Unclosed quotation mark after the character string.".into()
            )]
        );
        assert_eq!(errors("select 'it''")[0].0, "'it''");
        assert_eq!(
            errors("select \"Nome")[0].1,
            "Unclosed quotation mark after the identifier."
        );
        assert_eq!(
            errors("select 1 /* x")[0].1,
            "Missing end comment mark '*/'."
        );
        assert_eq!(
            errors("select $f$ body")[0].1,
            "Unterminated dollar-quoted string."
        );
        // Terminados no fim do texto não são erro.
        assert!(errors("select 'abc'").is_empty());
        assert!(errors("select 1 -- fim").is_empty());
    }

    #[test]
    fn parentheses_must_pair_within_a_statement() {
        assert_eq!(
            errors("select f(a; select 1)"),
            [
                ("(".into(), "Missing ')'.".into()),
                (")".into(), "Incorrect syntax near ')'.".into())
            ]
        );
        assert!(errors("select '(' from t").is_empty());
    }

    #[test]
    fn server_errors_underline_the_token() {
        let sql = "select bad from t";
        let tokens = tokenize(sql);
        assert_eq!(&sql[server_range(sql, &tokens, 7)], "bad");
        assert_eq!(&sql[server_range(sql, &tokens, 10)], "bad");
        assert_eq!(&sql[server_range(sql, &tokens, sql.len())], "t");
    }
}
