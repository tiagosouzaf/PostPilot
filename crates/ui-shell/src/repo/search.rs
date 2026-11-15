//! Pesquisa nas consultas do workspace: pelo nome do arquivo e pelo que
//! está escrito dentro dele. Os arquivos são lidos fora da thread da
//! interface; enquanto isso a árvore já mostra o que bate pelo nome.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;

use super::Folder;

/// Arquivos maiores que isto só entram pelo nome.
const MAX_CONTENT_BYTES: u64 = 8 * 1024 * 1024;

/// Tamanho do trecho mostrado embaixo do arquivo, em caracteres.
const SNIPPET_CHARS: usize = 110;

/// Uma consulta que a pesquisa achou.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hit {
    /// Algum termo aparece no nome do arquivo.
    pub name: bool,
    /// A melhor linha do conteúdo, quando algum termo aparece nele.
    pub line: Option<Line>,
    /// Quantas linhas do arquivo têm algum termo.
    pub lines: usize,
}

/// Uma linha do arquivo com o termo pesquisado.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    /// Número da linha, a partir de 1.
    pub number: usize,
    /// A linha sem as margens, encurtada em volta do termo.
    pub snippet: String,
    /// Onde o termo fica em `snippet`, em caracteres.
    pub found: std::ops::Range<usize>,
    /// O termo achado, como foi digitado (sem acentos e em minúsculas).
    pub term: String,
}

/// As consultas achadas, por caminho.
pub type Hits = HashMap<PathBuf, Hit>;

/// Minúsculas e sem acentos, caractere a caractere (o tamanho em
/// caracteres não muda, então as posições valem para o texto original).
pub fn fold(text: &str) -> String {
    text.chars().map(fold_char).collect()
}

fn fold_char(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        other => other,
    }
}

/// Os termos da pesquisa: as palavras digitadas, sem acentos e em
/// minúsculas. Todas precisam aparecer, no nome ou no conteúdo.
pub fn terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in fold(query).split_whitespace() {
        if !out.iter().any(|t| t == word) {
            out.push(word.to_string());
        }
    }
    out
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// O texto de um arquivo de consulta: UTF-8 (com ou sem BOM), UTF-16 com
/// BOM (como o SSMS grava às vezes) ou, sem outra saída, o que der para ler.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = |bytes: &[u8], le: bool| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes(*c)
                } else {
                    u16::from_be_bytes(*c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Procura `terms` no nome e no texto de um arquivo. `None` quando algum
/// termo não aparece em nenhum dos dois.
pub fn match_file(name: &str, content: Option<&str>, terms: &[String]) -> Option<Hit> {
    if terms.is_empty() {
        return None;
    }
    let name = fold(name);
    let in_name: Vec<bool> = terms.iter().map(|t| name.contains(t.as_str())).collect();
    let mut in_content = vec![false; terms.len()];
    let mut best: Option<(usize, usize, &str, String)> = None; // (termos, número, linha, dobrada)
    let mut lines = 0;
    for (i, raw) in content.unwrap_or_default().lines().enumerate() {
        let folded = fold(raw);
        let mut here = 0;
        for (t, term) in terms.iter().enumerate() {
            if folded.contains(term.as_str()) {
                in_content[t] = true;
                here += 1;
            }
        }
        if here == 0 {
            continue;
        }
        lines += 1;
        if best.as_ref().is_none_or(|b| here > b.0) {
            best = Some((here, i + 1, raw, folded));
        }
    }
    if !(0..terms.len()).all(|t| in_name[t] || in_content[t]) {
        return None;
    }
    let line = best.and_then(|(_, number, raw, folded)| {
        // Destaca o primeiro termo da pesquisa que a linha tem.
        let (term, byte) = terms
            .iter()
            .find_map(|t| folded.find(t.as_str()).map(|b| (t, b)))?;
        let at = folded[..byte].chars().count();
        let (snippet, found) = snippet(raw, at, term.chars().count());
        Some(Line {
            number,
            snippet,
            found,
            term: term.clone(),
        })
    });
    Some(Hit {
        name: in_name.iter().any(|b| *b),
        line,
        lines,
    })
}

/// A linha sem as margens e, se for comprida, só a parte em volta do
/// termo (que começa no caractere `at` e tem `len` caracteres).
fn snippet(raw: &str, at: usize, len: usize) -> (String, std::ops::Range<usize>) {
    let chars: Vec<char> = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let first = chars.iter().position(|c| !c.is_whitespace()).unwrap_or(0);
    let last = chars
        .iter()
        .rposition(|c| !c.is_whitespace())
        .map_or(0, |i| i + 1);
    let mut start = first.min(at);
    let mut end = last.max(at + len).min(chars.len());
    let mut prefix = "";
    let mut suffix = "";
    if end - start > SNIPPET_CHARS {
        let before = SNIPPET_CHARS / 4;
        if at - start > before {
            start = at - before;
            prefix = "…";
        }
        if end - start > SNIPPET_CHARS {
            end = (start + SNIPPET_CHARS).max(at + len).min(chars.len());
            suffix = "…";
        }
    }
    let mut text = String::from(prefix);
    text.extend(&chars[start..end]);
    text.push_str(suffix);
    let offset = prefix.chars().count() + at - start;
    (text, offset..offset + len)
}

/// Pesquisa `query` em `files`, lendo cada arquivo. Para no meio (e
/// devolve `None`) quando `cancelled` passa a responder `true`.
pub fn search_files(files: &[PathBuf], query: &str, cancelled: impl Fn() -> bool) -> Option<Hits> {
    let terms = terms(query);
    let mut hits = Hits::new();
    if terms.is_empty() {
        return Some(hits);
    }
    for path in files {
        if cancelled() {
            return None;
        }
        let small = std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_CONTENT_BYTES);
        let content = small
            .then(|| std::fs::read(path).ok())
            .flatten()
            .map(|bytes| decode(&bytes));
        if let Some(hit) = match_file(&file_name(path), content.as_deref(), &terms) {
            hits.insert(path.clone(), hit);
        }
    }
    Some(hits)
}

/// Só pelo nome, sem ler nada: o que a árvore mostra enquanto o conteúdo
/// ainda está sendo lido.
fn search_names(files: &[PathBuf], query: &str) -> Hits {
    let terms = terms(query);
    files
        .iter()
        .filter_map(|p| Some((p.clone(), match_file(&file_name(p), None, &terms)?)))
        .collect()
}

fn collect(folder: &Folder, out: &mut Vec<PathBuf>) {
    for sub in &folder.folders {
        collect(sub, out);
    }
    out.extend(folder.files.iter().cloned());
}

/// Todas as consultas da árvore, na ordem em que aparecem.
pub fn all_files(folder: &Folder) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(folder, &mut out);
    out
}

/// Se a pasta (ou alguma subpasta) tem consulta achada.
pub fn folder_has_hit(folder: &Folder, hits: &Hits) -> bool {
    folder.files.iter().any(|f| hits.contains_key(f))
        || folder.folders.iter().any(|f| folder_has_hit(f, hits))
}

/// O campo de pesquisa das consultas e o que ele achou.
#[derive(Default)]
pub struct Search {
    /// O que está escrito no campo.
    pub text: String,
    /// A pesquisa pedida por último (o texto sem as margens).
    requested: String,
    /// Cada pesquisa nova aumenta; a thread antiga vê e desiste.
    generation: Arc<AtomicU64>,
    job: Option<Receiver<(u64, Hits)>>,
    hits: Arc<Hits>,
    /// `hits` ainda é só pelo nome: o conteúdo está sendo lido.
    searching: bool,
}

impl Search {
    /// Se há algo digitado (a árvore mostra só o que foi achado).
    pub fn is_active(&self) -> bool {
        !self.text.trim().is_empty()
    }

    /// Se o conteúdo dos arquivos ainda está sendo lido.
    pub fn is_searching(&self) -> bool {
        self.searching
    }

    /// As consultas achadas pela pesquisa atual.
    pub fn hits(&self) -> Arc<Hits> {
        self.hits.clone()
    }

    /// Apaga a pesquisa.
    pub fn clear(&mut self) {
        self.text.clear();
    }

    /// Os arquivos mudaram no disco: a próxima chamada de `update`
    /// pesquisa de novo.
    pub fn invalidate(&mut self) {
        self.requested.clear();
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.job = None;
    }

    /// Chamada a cada frame: começa a pesquisa quando o texto muda e
    /// recolhe o resultado quando a leitura termina.
    pub fn update(&mut self, ctx: &egui::Context, queries: &Folder) {
        let query = self.text.trim().to_string();
        if query.is_empty() {
            if !self.requested.is_empty() || self.job.is_some() {
                self.invalidate();
            }
            if !self.hits.is_empty() {
                self.hits = Arc::default();
            }
            self.searching = false;
            return;
        }
        if query != self.requested {
            let files = all_files(queries);
            self.hits = Arc::new(search_names(&files, &query));
            self.requested = query.clone();
            self.searching = true;
            let mine = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            let generation = self.generation.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.job = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let cancelled = || generation.load(Ordering::SeqCst) != mine;
                if let Some(hits) = search_files(&files, &query, cancelled) {
                    let _ = tx.send((mine, hits));
                    ctx.request_repaint();
                }
            });
        }
        if let Some(rx) = &self.job {
            match rx.try_recv() {
                Ok((generation, hits)) => {
                    if generation == self.generation.load(Ordering::SeqCst) {
                        self.hits = Arc::new(hits);
                        self.searching = false;
                    }
                    self.job = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.job = None;
                    self.searching = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(query: &str) -> Vec<String> {
        terms(query)
    }

    #[test]
    fn folds_case_and_accents_keeping_the_length() {
        assert_eq!(fold("Migração DIÁRIA"), "migracao diaria");
        assert_eq!(fold("Ação").chars().count(), 4);
        assert_eq!(t("  Status   status MIGRAÇÃO "), ["status", "migracao"]);
    }

    #[test]
    fn matches_by_name_without_reading_the_content() {
        let hit = match_file("Criar indice status_migracao.sql", None, &t("índice")).unwrap();
        assert!(hit.name);
        assert_eq!(hit.line, None);
        assert_eq!(match_file("Query_1.sql", None, &t("indice")), None);
    }

    #[test]
    fn matches_by_content_with_line_and_snippet() {
        let sql = "-- relatório\nselect *\n  from resumo_diario\n where dia = current_date;\n";
        let hit = match_file("Query_3.sql", Some(sql), &t("RESUMO_diario")).unwrap();
        assert!(!hit.name);
        assert_eq!(hit.lines, 1);
        let line = hit.line.unwrap();
        assert_eq!(line.number, 3);
        assert_eq!(line.snippet, "from resumo_diario");
        assert_eq!(line.found, 5..18);
        assert_eq!(line.term, "resumo_diario");
    }

    #[test]
    fn every_term_must_appear_in_the_name_or_in_the_content() {
        let sql = "select 1 from pedidos;\nselect 2 from clientes join pedidos using (id);\n";
        // Um termo no nome e outro no conteúdo.
        let hit = match_file("vendas.sql", Some(sql), &t("vendas clientes")).unwrap();
        assert!(hit.name);
        assert_eq!(hit.line.as_ref().unwrap().number, 2);
        // A linha com mais termos ganha, e conta quantas linhas têm algum.
        let hit = match_file("x.sql", Some(sql), &t("pedidos clientes")).unwrap();
        assert_eq!(hit.line.as_ref().unwrap().number, 2);
        assert_eq!(hit.lines, 2);
        assert_eq!(
            match_file("vendas.sql", Some(sql), &t("vendas estoque")),
            None
        );
        assert_eq!(match_file("vendas.sql", Some(sql), &[]), None);
    }

    #[test]
    fn long_lines_are_cut_around_the_term() {
        let sql = format!("select {} alvo, {}", "a, ".repeat(100), "b, ".repeat(100));
        let hit = match_file("x.sql", Some(&sql), &t("alvo")).unwrap();
        let line = hit.line.unwrap();
        assert!(line.snippet.starts_with('…') && line.snippet.ends_with('…'));
        assert!(line.snippet.chars().count() <= SNIPPET_CHARS + 2);
        let found: String = line
            .snippet
            .chars()
            .skip(line.found.start)
            .take(line.found.len())
            .collect();
        assert_eq!(found, "alvo");
    }

    #[test]
    fn decodes_utf16_and_bom() {
        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "select ação".encode_utf16() {
            utf16.extend(unit.to_le_bytes());
        }
        assert_eq!(decode(&utf16), "select ação");
        assert_eq!(decode(b"\xEF\xBB\xBFselect 1"), "select 1");
        assert_eq!(decode(b"select 1"), "select 1");
    }

    #[test]
    fn searches_files_and_stops_when_cancelled() {
        let dir = std::env::temp_dir().join(format!("postpilot-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("Query_1.sql");
        let b = dir.join("Clientes ativos.sql");
        let c = dir.join("Query_2.sql");
        std::fs::write(&a, "select * from clientes;").unwrap();
        std::fs::write(&b, "select 1;").unwrap();
        std::fs::write(&c, "select * from pedidos;").unwrap();
        let files = vec![a.clone(), b.clone(), c.clone(), dir.join("sumiu.sql")];
        let hits = search_files(&files, "clientes", || false).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(!hits[&a].name && hits[&a].line.is_some());
        assert!(hits[&b].name && hits[&b].line.is_none());
        assert!(search_files(&files, "clientes", || true).is_none());
        assert!(search_files(&files, "  ", || false).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
