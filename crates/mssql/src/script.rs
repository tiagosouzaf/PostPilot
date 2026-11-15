//! Divide um script T-SQL em lotes no `GO`, como o SSMS e o sqlcmd.
//!
//! `GO` não é T-SQL: é um separador do cliente, sozinho na linha (com
//! espaços, um número de repetições e um comentário `--` opcionais). Um
//! `GO` dentro de comentário de bloco ou de texto entre aspas não conta.

/// Um lote do script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    /// O texto do lote, sem a linha do `GO`.
    pub sql: String,
    /// Linha do script (a partir de 0) em que o lote começa.
    pub first_line: usize,
    /// Posição, em caracteres a partir de 0, do começo do lote no script.
    pub start_char: usize,
    /// Quantas vezes rodar (`GO 5`).
    pub repeat: u32,
}

impl Batch {
    /// Posição (em caracteres, a partir de 1) do começo da linha `line`
    /// (a partir de 1) do lote, no script inteiro. É o que a interface usa
    /// para marcar o erro. Linha fora do lote vai para o começo dele.
    pub fn line_position(&self, line: u32) -> usize {
        let mut pos = self.start_char;
        if line > 1 {
            let mut seen = 1;
            for (i, c) in self.sql.chars().enumerate() {
                if c == '\n' {
                    seen += 1;
                    if seen == line {
                        pos = self.start_char + i + 1;
                        break;
                    }
                }
            }
        }
        pos + 1
    }
}

/// Os lotes de `script`, na ordem. Lotes vazios (só espaço e comentários
/// de linha) ficam de fora.
pub fn split(script: &str) -> Vec<Batch> {
    let mut batches = Vec::new();
    let mut current = String::new();
    let mut first_line = 0;
    let mut start_char = 0;
    let mut chars_seen = 0;
    // Estado léxico que atravessa linhas.
    let mut in_block_comment = 0u32;
    let mut in_quote: Option<char> = None;

    for (n, raw) in script.split_inclusive('\n').enumerate() {
        let line_chars = raw.chars().count();
        let at_top = in_block_comment == 0 && in_quote.is_none();
        if at_top && let Some(repeat) = go_line(raw) {
            push(&mut batches, &current, first_line, start_char, repeat);
            current.clear();
            first_line = n + 1;
            chars_seen += line_chars;
            start_char = chars_seen;
            continue;
        }
        scan(raw, &mut in_block_comment, &mut in_quote);
        current.push_str(raw);
        chars_seen += line_chars;
    }
    push(&mut batches, &current, first_line, start_char, 1);
    batches
}

fn push(batches: &mut Vec<Batch>, sql: &str, first_line: usize, start_char: usize, repeat: u32) {
    if has_code(sql) {
        let sql = sql.strip_suffix('\n').unwrap_or(sql);
        let sql = sql.strip_suffix('\r').unwrap_or(sql);
        batches.push(Batch {
            sql: sql.to_string(),
            first_line,
            start_char,
            repeat,
        });
    }
}

/// O lote tem algo além de espaço e comentários.
fn has_code(sql: &str) -> bool {
    let mut chars = sql.chars().peekable();
    let mut depth = 0u32;
    while let Some(c) = chars.next() {
        if depth > 0 {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                depth -= 1;
            } else if c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                depth += 1;
            }
            continue;
        }
        match c {
            '-' if chars.peek() == Some(&'-') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                depth = 1;
            }
            c if c.is_whitespace() => {}
            _ => return true,
        }
    }
    false
}

/// `GO`, `go 3`, `GO -- fim`: a linha é um separador. Devolve as
/// repetições.
fn go_line(line: &str) -> Option<u32> {
    let line = line.trim();
    let line = match line.find("--") {
        Some(i) => line[..i].trim_end(),
        None => line,
    };
    let rest = line
        .strip_prefix("GO")
        .or_else(|| line.strip_prefix("go"))
        .or_else(|| line.strip_prefix("Go"))
        .or_else(|| line.strip_prefix("gO"))?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Some(1);
    }
    rest.parse::<u32>().ok().filter(|n| *n > 0)
}

/// Avança o estado de comentário de bloco e de aspas por uma linha.
fn scan(line: &str, depth: &mut u32, quote: &mut Option<char>) {
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = *quote {
            if c == q {
                if chars.peek() == Some(&q) {
                    chars.next();
                } else {
                    *quote = None;
                }
            }
            continue;
        }
        if *depth > 0 {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                *depth -= 1;
            } else if c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                *depth += 1;
            }
            continue;
        }
        match c {
            '-' if chars.peek() == Some(&'-') => return,
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                *depth = 1;
            }
            '\'' => *quote = Some('\''),
            '"' => *quote = Some('"'),
            '[' => *quote = Some(']'),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqls(script: &str) -> Vec<String> {
        split(script).into_iter().map(|b| b.sql).collect()
    }

    #[test]
    fn splits_on_go_lines() {
        let s = "SELECT 1\nGO\nSELECT 2;\nselect 3\n  go  \n";
        assert_eq!(sqls(s), ["SELECT 1", "SELECT 2;\nselect 3"]);
        let b = split(s);
        assert_eq!((b[0].first_line, b[1].first_line), (0, 2));
        assert_eq!(b[1].start_char, "SELECT 1\nGO\n".len());
    }

    #[test]
    fn no_go_is_one_batch() {
        assert_eq!(sqls("SELECT 1;\nSELECT 2"), ["SELECT 1;\nSELECT 2"]);
        assert!(split("  \n-- só comentário\nGO\n/* nada */").is_empty());
    }

    #[test]
    fn go_with_count_and_comment() {
        let b = split("INSERT t DEFAULT VALUES\nGO 3 -- três vezes\nSELECT 1\nGO -- fim");
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].repeat, 3);
        assert_eq!(b[1].repeat, 1);
    }

    #[test]
    fn go_inside_text_or_comments_is_not_a_separator() {
        let s = "SELECT 'a\nGO\nb'\n/* bloco\nGO\n*/\nSELECT [x\nGO\n]\nGO\nSELECT 2";
        let b = sqls(s);
        assert_eq!(b.len(), 2, "{b:?}");
        assert!(b[0].contains("'a\nGO\nb'"));
        assert_eq!(b[1], "SELECT 2");
        // Palavras que começam com GO não separam.
        assert_eq!(sqls("SELECT 1\nGOTO fim\nGOX").len(), 1);
    }

    #[test]
    fn crlf_scripts() {
        assert_eq!(
            sqls("SELECT 1\r\nGO\r\nSELECT 2\r\n"),
            ["SELECT 1", "SELECT 2"]
        );
    }

    #[test]
    fn line_positions_are_in_the_whole_script() {
        let s = "SELECT 1\nGO\nSELECT 2\nSELECT bad";
        let b = split(s);
        // Linha 2 do segundo lote é a 4ª do script.
        let pos = b[1].line_position(2);
        assert_eq!(&s[pos - 1..], "SELECT bad");
        assert_eq!(b[1].line_position(1), "SELECT 1\nGO\n".len() + 1);
        assert_eq!(b[0].line_position(9), 1);
    }
}
