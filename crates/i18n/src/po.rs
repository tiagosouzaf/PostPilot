//! Leitor do subconjunto do formato `.po` (gettext) que o catálogo usa:
//! comentários `#`, `msgctxt`, `msgid` e `msgstr`, cada um com uma ou mais
//! linhas entre aspas, e os escapes `\"`, `\\`, `\n` e `\t`.
//!
//! O formato foi escolhido para que o catálogo abra em editores de tradução
//! comuns (Poedit, Lokalize). Plurais e entradas obsoletas não são aceitos.

/// Uma entrada do catálogo.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub msgctxt: Option<String>,
    pub msgid: String,
    pub msgstr: String,
}

impl Entry {
    /// Chave de busca: o texto, ou contexto + `\u{4}` + texto.
    pub fn key(&self) -> String {
        key(self.msgctxt.as_deref(), &self.msgid)
    }
}

/// Chave de busca de um texto, como no gettext.
pub fn key(context: Option<&str>, text: &str) -> String {
    match context {
        None => text.to_string(),
        Some(c) => format!("{c}\u{4}{text}"),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Ctxt,
    Id,
    Str,
}

/// Lê o catálogo. O cabeçalho (entrada de `msgid ""`) é descartado.
///
/// Erros dizem a linha (contando de 1).
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let mut cur = Entry::default();
    let mut field: Option<Field> = None;
    let mut has_id = false;

    let mut finish = |cur: &mut Entry, has_id: &mut bool, line: usize| {
        if !*has_id {
            if cur.msgctxt.is_some() {
                return Err(format!("linha {line}: msgctxt sem msgid"));
            }
            return Ok(());
        }
        let e = std::mem::take(cur);
        *has_id = false;
        if !e.msgid.is_empty() {
            entries.push(e);
        }
        Ok(())
    };

    for (n, raw) in text.lines().enumerate() {
        let line_no = n + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (kw, rest) = match line.split_once(char::is_whitespace) {
            Some((kw, rest)) if kw.starts_with("msg") => (Some(kw), rest.trim()),
            _ => (None, line),
        };
        let value = unquote(rest).map_err(|e| format!("linha {line_no}: {e}"))?;
        match kw {
            Some("msgctxt") => {
                finish(&mut cur, &mut has_id, line_no)?;
                cur.msgctxt = Some(value);
                field = Some(Field::Ctxt);
            }
            Some("msgid") => {
                if has_id {
                    finish(&mut cur, &mut has_id, line_no)?;
                }
                cur.msgid = value;
                has_id = true;
                field = Some(Field::Id);
            }
            Some("msgstr") => {
                if !has_id {
                    return Err(format!("linha {line_no}: msgstr sem msgid"));
                }
                cur.msgstr = value;
                field = Some(Field::Str);
            }
            Some(other) => return Err(format!("linha {line_no}: palavra desconhecida {other}")),
            None => match field {
                Some(Field::Ctxt) => cur.msgctxt.get_or_insert_default().push_str(&value),
                Some(Field::Id) => cur.msgid.push_str(&value),
                Some(Field::Str) => cur.msgstr.push_str(&value),
                None => return Err(format!("linha {line_no}: texto fora de entrada")),
            },
        }
    }
    finish(&mut cur, &mut has_id, text.lines().count())?;
    Ok(entries)
}

fn unquote(s: &str) -> Result<String, String> {
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .filter(|_| s.len() >= 2)
        .ok_or_else(|| format!("esperava texto entre aspas: {s}"))?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                other => return Err(format!("escape inválido: \\{}", other.unwrap_or(' '))),
            },
            '"' => return Err("aspas sem escape no meio do texto".to_string()),
            c => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries() {
        let text = r#"
# cabeçalho
msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"

#: menus
msgid "Open"
msgstr "Abrir"

msgctxt "state"
msgid "Open"
msgstr "Aberto"

msgid "Say \"hi\"\n"
"twice"
msgstr "Diga \"oi\"\n"
"duas vezes"
"#;
        let entries = parse(text).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].key(), "Open");
        assert_eq!(entries[1].key(), "state\u{4}Open");
        assert_eq!(entries[1].msgstr, "Aberto");
        assert_eq!(entries[2].msgid, "Say \"hi\"\ntwice");
        assert_eq!(entries[2].msgstr, "Diga \"oi\"\nduas vezes");
    }

    #[test]
    fn reports_errors() {
        assert!(parse("msgstr \"x\"").unwrap_err().contains("linha 1"));
        assert!(parse("msgid \"a\nmsgstr \"b\"").is_err());
        assert!(parse("msgid \"a\\q\"\nmsgstr \"b\"").is_err());
    }
}
