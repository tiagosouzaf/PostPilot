//! O texto do editor visto em linhas, e o histórico de desfazer.
//!
//! O texto continua sendo a `String` do documento. O [`LineIndex`] guarda
//! onde começa cada linha (em bytes e em caracteres), para o editor só
//! montar as linhas visíveis e converter posições sem varrer o texto todo.
//! Posições são índices de caractere, como no resto do editor.

use std::ops::Range;

/// Começo de cada linha do texto.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineIndex {
    bytes: Vec<usize>,
    chars: Vec<usize>,
    total_chars: usize,
    total_bytes: usize,
    max_line_chars: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut bytes = vec![0];
        let mut chars = vec![0];
        let mut n = 0;
        let mut longest = 0;
        for (b, c) in text.char_indices() {
            n += 1;
            if c == '\n' {
                longest = longest.max(n - 1 - chars[chars.len() - 1]);
                bytes.push(b + 1);
                chars.push(n);
            }
        }
        longest = longest.max(n - chars[chars.len() - 1]);
        LineIndex {
            bytes,
            chars,
            total_chars: n,
            total_bytes: text.len(),
            max_line_chars: longest,
        }
    }

    /// Número de linhas (um texto vazio tem uma).
    pub fn line_count(&self) -> usize {
        self.bytes.len()
    }

    /// Número de caracteres do texto.
    pub fn len_chars(&self) -> usize {
        self.total_chars
    }

    /// Caracteres da linha mais longa.
    pub fn max_line_chars(&self) -> usize {
        self.max_line_chars
    }

    /// Linha do caractere `c` (o fim do texto conta como a última linha).
    pub fn line_of(&self, c: usize) -> usize {
        self.chars.partition_point(|&s| s <= c).saturating_sub(1)
    }

    /// Primeiro caractere da linha.
    pub fn line_start(&self, line: usize) -> usize {
        self.chars[line.min(self.chars.len() - 1)]
    }

    /// Caracteres da linha, sem o `\n`.
    pub fn line_len(&self, line: usize) -> usize {
        let start = self.line_start(line);
        let end = match self.chars.get(line + 1) {
            Some(next) => next - 1,
            None => self.total_chars,
        };
        end - start
    }

    /// Bytes da linha, sem o `\n`.
    pub fn line_bytes(&self, line: usize) -> Range<usize> {
        let line = line.min(self.bytes.len() - 1);
        let start = self.bytes[line];
        let end = match self.bytes.get(line + 1) {
            Some(next) => next - 1,
            None => self.total_bytes,
        };
        start..end
    }

    /// Linha e coluna (a partir de 0) do caractere `c`.
    pub fn line_col(&self, c: usize) -> (usize, usize) {
        let c = c.min(self.total_chars);
        let line = self.line_of(c);
        (line, c - self.chars[line])
    }

    /// Caractere na linha e coluna dadas, limitado ao fim da linha.
    pub fn char_at(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.line_count() - 1);
        self.line_start(line) + col.min(self.line_len(line))
    }

    /// Byte do caractere `c` em `text` (o texto deste índice).
    pub fn byte_of(&self, text: &str, c: usize) -> usize {
        let c = c.min(self.total_chars);
        let line = self.line_of(c);
        let start = self.bytes[line];
        let col = c - self.chars[line];
        text[start..]
            .char_indices()
            .nth(col)
            .map_or(text.len(), |(b, _)| start + b)
    }
}

/// Texto da linha `line`.
pub fn line_text<'a>(text: &'a str, index: &LineIndex, line: usize) -> &'a str {
    &text[index.line_bytes(line)]
}

/// Uma mudança no texto: em `at` (caractere), `removed` saiu e `inserted`
/// entrou.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub at: usize,
    pub removed: String,
    pub inserted: String,
}

impl Change {
    /// A mudança que leva `old` a `new`, pelo maior prefixo e sufixo comuns.
    /// `None` se forem iguais.
    pub fn between(old: &str, new: &str) -> Option<Change> {
        if old == new {
            return None;
        }
        let (a, b) = (old.as_bytes(), new.as_bytes());
        let mut p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
        while !old.is_char_boundary(p) || !new.is_char_boundary(p) {
            p -= 1;
        }
        let max_s = a.len().min(b.len()) - p;
        let mut s = a
            .iter()
            .rev()
            .zip(b.iter().rev())
            .take(max_s)
            .take_while(|(x, y)| x == y)
            .count();
        while !old.is_char_boundary(a.len() - s) || !new.is_char_boundary(b.len() - s) {
            s -= 1;
        }
        Some(Change {
            at: old[..p].chars().count(),
            removed: old[p..a.len() - s].to_string(),
            inserted: new[p..b.len() - s].to_string(),
        })
    }
}

/// Que tipo de edição, para juntar várias num desfazer só.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// Digitar caracteres (juntam enquanto contíguos, até um espaço).
    Typing,
    /// Apagar com Backspace ou Delete (juntam enquanto contíguos).
    Deleting,
    /// Qualquer outra (colar, recortar, comentar, indentar...).
    Other,
}

/// Uma edição guardada no histórico.
#[derive(Clone, Debug)]
struct Entry {
    change: Change,
    kind: EditKind,
    /// Seleção antes e depois, para restaurar.
    before: (usize, usize),
    after: (usize, usize),
    /// Os outros cursores (edição com vários cursores), antes e depois.
    before_extra: Vec<Sel>,
    after_extra: Vec<Sel>,
}

/// Uma seleção: (âncora, cursor), em caracteres.
pub type Sel = (usize, usize);

/// Junta duas mudanças seguidas (`second` feita no texto depois de `first`)
/// numa só, se elas se tocam. É o que deixa uma palavra digitada em vários
/// cursores voltar num desfazer só.
fn compose(first: &Change, second: &Change) -> Option<Change> {
    let len = |s: &str| s.chars().count();
    let from = |s: &str, n: usize| s.char_indices().nth(n).map_or(s.len(), |(b, _)| b);
    let (a1, a2) = (first.at, second.at);
    let e1 = a1 + len(&first.inserted);
    let e2 = a2 + len(&second.removed);
    if a2 > e1 || e2 < a1 {
        return None;
    }
    let mut removed = String::new();
    if a2 < a1 {
        removed.push_str(&second.removed[..from(&second.removed, a1 - a2)]);
    }
    removed.push_str(&first.removed);
    if e2 > e1 {
        removed.push_str(&second.removed[from(&second.removed, e1 - a2)..]);
    }
    let mut inserted = String::new();
    if a2 > a1 {
        inserted.push_str(&first.inserted[..from(&first.inserted, a2 - a1)]);
    }
    inserted.push_str(&second.inserted);
    if e2 < e1 {
        inserted.push_str(&first.inserted[from(&first.inserted, e2 - a1)..]);
    }
    Some(Change {
        at: a1.min(a2),
        removed,
        inserted,
    })
}

/// Edições guardadas no histórico, no máximo.
const MAX_HISTORY: usize = 1000;

/// Desfazer e refazer.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

impl History {
    /// Guarda uma edição já aplicada.
    pub fn record(
        &mut self,
        change: Change,
        kind: EditKind,
        before: (usize, usize),
        after: (usize, usize),
    ) {
        self.redo.clear();
        if let Some(last) = self.undo.last_mut() {
            let typed = kind == EditKind::Typing
                && last.kind == EditKind::Typing
                && change.removed.is_empty()
                && last.change.at + last.change.inserted.chars().count() == change.at
                && !change.inserted.starts_with(char::is_whitespace);
            if typed {
                last.change.inserted.push_str(&change.inserted);
                last.after = after;
                return;
            }
            let deleted = kind == EditKind::Deleting
                && last.kind == EditKind::Deleting
                && change.inserted.is_empty()
                && last.change.inserted.is_empty();
            if deleted {
                // Backspace: a nova remoção termina onde a anterior começou.
                if change.at + change.removed.chars().count() == last.change.at {
                    last.change.removed.insert_str(0, &change.removed);
                    last.change.at = change.at;
                    last.after = after;
                    return;
                }
                // Delete: remove a partir do mesmo ponto.
                if change.at == last.change.at {
                    last.change.removed.push_str(&change.removed);
                    last.after = after;
                    return;
                }
            }
        }
        self.push(Entry {
            change,
            kind,
            before,
            after,
            before_extra: Vec::new(),
            after_extra: Vec::new(),
        });
    }

    fn push(&mut self, entry: Entry) {
        self.undo.push(entry);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
    }

    /// Guarda uma edição feita em vários cursores de uma vez. `change` é o
    /// trecho inteiro, do primeiro ao último cursor, então desfazer volta
    /// todos juntos. Digitar e apagar em seguida, com os mesmos cursores,
    /// juntam num passo só.
    pub fn record_multi(
        &mut self,
        change: Change,
        kind: EditKind,
        before: (Sel, Vec<Sel>),
        after: (Sel, Vec<Sel>),
    ) {
        self.redo.clear();
        if let Some(last) = self.undo.last_mut()
            && kind != EditKind::Other
            && last.kind == kind
            && last.after == before.0
            && last.after_extra == before.1
            && let Some(joined) = compose(&last.change, &change)
        {
            last.change = joined;
            last.after = after.0;
            last.after_extra = after.1;
            return;
        }
        self.push(Entry {
            change,
            kind,
            before: before.0,
            after: after.0,
            before_extra: before.1,
            after_extra: after.1,
        });
    }

    /// Fecha a edição atual: a próxima não junta com ela.
    pub fn seal(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.kind = EditKind::Other;
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Desfaz a última edição em `text` e devolve a seleção a restaurar.
    pub fn undo(&mut self, text: &mut String) -> Option<(usize, usize)> {
        self.undo_multi(text).map(|(sel, _)| sel)
    }

    /// Como [`History::undo`], e devolve também os outros cursores.
    pub fn undo_multi(&mut self, text: &mut String) -> Option<(Sel, Vec<Sel>)> {
        let entry = self.undo.pop()?;
        apply(
            text,
            entry.change.at,
            &entry.change.inserted,
            &entry.change.removed,
        );
        let sel = (entry.before, entry.before_extra.clone());
        self.redo.push(entry);
        Some(sel)
    }

    /// Refaz a última edição desfeita.
    pub fn redo(&mut self, text: &mut String) -> Option<(usize, usize)> {
        self.redo_multi(text).map(|(sel, _)| sel)
    }

    /// Como [`History::redo`], e devolve também os outros cursores.
    pub fn redo_multi(&mut self, text: &mut String) -> Option<(Sel, Vec<Sel>)> {
        let entry = self.redo.pop()?;
        apply(
            text,
            entry.change.at,
            &entry.change.removed,
            &entry.change.inserted,
        );
        let sel = (entry.after, entry.after_extra.clone());
        self.undo.push(entry);
        Some(sel)
    }
}

/// Troca `old` (que está em `at`) por `new`.
fn apply(text: &mut String, at: usize, old: &str, new: &str) {
    let start = byte_of_char(text, at);
    let end = start + old.len();
    text.replace_range(start..end.min(text.len()), new);
}

fn byte_of_char(text: &str, c: usize) -> usize {
    text.char_indices().nth(c).map_or(text.len(), |(b, _)| b)
}

/// Se o caractere faz parte de uma palavra (para Ctrl+setas e duplo
/// clique).
pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_converts_positions() {
        let t = "ab\nção\n\nxyz";
        let i = LineIndex::new(t);
        assert_eq!(i.line_count(), 4);
        assert_eq!(i.len_chars(), 11);
        assert_eq!(i.max_line_chars(), 3);
        assert_eq!(i.line_col(0), (0, 0));
        assert_eq!(i.line_col(2), (0, 2));
        assert_eq!(i.line_col(3), (1, 0));
        assert_eq!(i.line_col(7), (2, 0));
        assert_eq!(i.line_col(11), (3, 3));
        assert_eq!(i.line_len(1), 3);
        assert_eq!(i.line_len(2), 0);
        assert_eq!(line_text(t, &i, 1), "ção");
        assert_eq!(i.byte_of(t, 4), 5);
        assert_eq!(i.byte_of(t, 5), 7);
        assert_eq!(i.byte_of(t, 99), t.len());
        assert_eq!(i.char_at(1, 99), 6);
        assert_eq!(i.char_at(9, 0), 8);
        let empty = LineIndex::new("");
        assert_eq!((empty.line_count(), empty.len_chars()), (1, 0));
        let trailing = LineIndex::new("a\n");
        assert_eq!(trailing.line_count(), 2);
        assert_eq!(trailing.line_col(2), (1, 0));
    }

    #[test]
    fn change_between_two_texts() {
        let c = Change::between("select 1", "select 12").unwrap();
        assert_eq!(
            (c.at, c.removed.as_str(), c.inserted.as_str()),
            (8, "", "2")
        );
        let c = Change::between("aaa", "aa").unwrap();
        assert_eq!(
            c.removed.chars().count() as i32 - c.inserted.chars().count() as i32,
            1
        );
        let c = Change::between("ação", "açõo").unwrap();
        assert_eq!(
            (c.at, c.removed.as_str(), c.inserted.as_str()),
            (2, "ã", "õ")
        );
        assert_eq!(Change::between("x", "x"), None);
    }

    #[test]
    fn typing_is_undone_word_by_word() {
        let mut h = History::default();
        let mut t = String::new();
        for (i, ch) in "ab cd".chars().enumerate() {
            t.push(ch);
            h.record(
                Change {
                    at: i,
                    removed: String::new(),
                    inserted: ch.to_string(),
                },
                EditKind::Typing,
                (i, i),
                (i + 1, i + 1),
            );
        }
        assert_eq!(h.undo(&mut t), Some((2, 2)));
        assert_eq!(t, "ab");
        assert_eq!(h.undo(&mut t), Some((0, 0)));
        assert_eq!(t, "");
        assert!(!h.can_undo());
        assert_eq!(h.redo(&mut t), Some((2, 2)));
        assert_eq!(h.redo(&mut t), Some((5, 5)));
        assert_eq!(t, "ab cd");
    }

    #[test]
    fn changes_that_touch_compose_into_one() {
        let c = |at, removed: &str, inserted: &str| Change {
            at,
            removed: removed.into(),
            inserted: inserted.into(),
        };
        // "a|\nb|" -> digitar x nos dois cursores, depois y.
        let first = c(1, "\nb", "x\nbx");
        let second = c(2, "\nbx", "y\nbxy");
        assert_eq!(compose(&first, &second), Some(c(1, "\nb", "xy\nbxy")));
        // Backspace nos dois cursores de "ab|\ncd|", duas vezes.
        let first = c(1, "b\ncd", "\nc");
        let second = c(0, "a\nc", "\n");
        assert_eq!(compose(&first, &second), Some(c(0, "ab\ncd", "\n")));
        // Longe uma da outra: não juntam.
        assert_eq!(compose(&c(0, "", "a"), &c(5, "", "b")), None);
        // A segunda passa do fim da primeira.
        assert_eq!(
            compose(&c(2, "", "çã"), &c(3, "ãxy", "")),
            Some(c(2, "xy", "ç"))
        );
    }

    #[test]
    fn backspaces_join_and_a_new_edit_clears_redo() {
        let mut h = History::default();
        let mut t = String::from("abc");
        for at in [2, 1] {
            let removed = t.remove(at).to_string();
            h.record(
                Change {
                    at,
                    removed,
                    inserted: String::new(),
                },
                EditKind::Deleting,
                (at + 1, at + 1),
                (at, at),
            );
        }
        assert_eq!(t, "a");
        assert_eq!(h.undo(&mut t), Some((3, 3)));
        assert_eq!(t, "abc");
        h.redo(&mut t);
        h.undo(&mut t);
        h.record(
            Change {
                at: 0,
                removed: String::new(),
                inserted: "x".into(),
            },
            EditKind::Other,
            (0, 0),
            (1, 1),
        );
        assert!(!h.can_redo());
    }
}
