//! Cobertura da tradução, lendo o código-fonte do workspace:
//!
//! 1. todo texto passado a `tr`, `trc`, `trf!` e `tr_noop` tem tradução em
//!    `locales/pt-BR.po`, e o catálogo não tem entrada sobrando;
//! 2. texto visível escrito direto num widget do egui (`ui.label("...")`,
//!    `Button::new("...")`, `self.status = "..."`...) sem passar por `tr`
//!    é acusado, fora dos arquivos da lista [`PENDING`].
//!
//! Para um texto que não deve ser traduzido (um nome próprio, SQL), ponha
//! `// i18n-ignore` na linha ou na linha de cima.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use pgstudio_i18n::{Language, catalog_keys, has_translation};

/// Arquivos ainda não convertidos, que o teste 2 não olha. A lista só
/// diminui: um arquivo daqui sem nenhum texto solto também falha, para ser
/// tirado da lista.
const PENDING: &[&str] = &[];

/// Pastas com código da interface.
const UI_DIRS: &[&str] = &[
    "apps/desktop/src",
    "crates/ui-shell/src",
    "crates/object-explorer/src",
    "crates/result-grid/src",
    "crates/sql-editor/src",
    "crates/execution-plan/src",
    "crates/docking/src",
    "crates/diagram-engine/src",
    "crates/schema-model/src",
    "crates/pg-metadata/src",
    "crates/mssql/src",
    "crates/ssh/src",
];

/// Arquivos que não são interface (medição, dados de demonstração).
const NOT_UI: &[&str] = &[
    "crates/ui-shell/src/bench.rs",
    "crates/object-explorer/src/fake.rs",
];

/// Chamadas cujo texto aparece na tela.
const UI_CALLS: &[&str] = &[
    "label",
    "button",
    "small_button",
    "heading",
    "strong",
    "weak",
    "small",
    "on_hover_text",
    "on_disabled_hover_text",
    "hint_text",
    "selectable_label",
    "selectable_value",
    "menu_button",
    "collapsing",
    "colored_label",
    "checkbox",
    "radio",
    "radio_value",
    "selected_text",
    "with_status",
    "set_title",
    "add_filter",
    "Button::new",
    "Label::new",
    "RichText::new",
    "Window::new",
    "CollapsingHeader::new",
    "Checkbox::new",
    "SelectableLabel::new",
];

/// Funções de tradução.
const TR_CALLS: &[&str] = &["tr", "trc", "trf!", "tr_noop"];

/// Textos que ficam iguais em qualquer idioma.
const UNTRANSLATED: &[&str] = &[
    "OK",
    "SQL",
    "PostgreSQL",
    "PGStudio",
    "IntelliSense",
    "JSON",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Fontes do workspace (sem `tests/`, `spikes/` e `target/`), com o caminho
/// relativo.
fn sources() -> Vec<(String, String)> {
    let root = root();
    let mut files = Vec::new();
    for top in ["apps", "crates"] {
        let Ok(entries) = std::fs::read_dir(root.join(top)) else {
            continue;
        };
        for e in entries.flatten() {
            rust_files(&e.path().join("src"), &mut files);
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&p).unwrap();
            (rel, without_tests(&text))
        })
        .collect()
}

/// O texto até o primeiro `#[cfg(test)]` no começo de linha (o módulo de
/// testes fica no fim dos arquivos), com o resto trocado por linhas vazias.
fn without_tests(text: &str) -> String {
    match text.find("\n#[cfg(test)]") {
        Some(i) => text[..i].to_string(),
        None => text.to_string(),
    }
}

/// Um literal de texto no código.
#[derive(Debug)]
struct Literal {
    /// Valor, já sem escapes.
    value: String,
    /// Linha (de 1).
    line: usize,
    /// Byte onde começa (a aspa).
    start: usize,
}

/// Acha os literais de texto, pulando comentários e caracteres.
fn literals(src: &str) -> Vec<Literal> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < b.len() {
        match b[i] {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
            }
            b'\'' => {
                // Caractere ('a', '\n', '"') ou lifetime ('a).
                if b.get(i + 1) == Some(&b'\\') {
                    i += 2;
                    while i < b.len() && b[i] != b'\'' {
                        i += 1;
                    }
                    i += 1;
                } else {
                    let ch_len = src[i + 1..].chars().next().map_or(1, char::len_utf8);
                    if b.get(i + 1 + ch_len) == Some(&b'\'') {
                        i += 2 + ch_len;
                    } else {
                        i += 1;
                    }
                }
            }
            b'r' if (b.get(i + 1) == Some(&b'"') || b.get(i + 1) == Some(&b'#'))
                && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let close = format!("\"{}", "#".repeat(hashes));
                let body = j + 1;
                let end = src[body..].find(&close).map_or(b.len(), |e| body + e);
                out.push(Literal {
                    value: src[body..end].to_string(),
                    line,
                    start: j,
                });
                line += src[i..end].matches('\n').count();
                i = end + close.len();
            }
            b'"' => {
                let start = i;
                let mut value = String::new();
                let mut j = i + 1;
                let start_line = line;
                while j < b.len() && b[j] != b'"' {
                    if b[j] == b'\\' {
                        match b.get(j + 1) {
                            Some(b'n') => value.push('\n'),
                            Some(b't') => value.push('\t'),
                            Some(b'"') => value.push('"'),
                            Some(b'\\') => value.push('\\'),
                            Some(b'\'') => value.push('\''),
                            Some(b'\n') => {
                                // Continuação de linha: pula o espaço.
                                line += 1;
                                j += 2;
                                while j < b.len() && (b[j] as char).is_whitespace() {
                                    if b[j] == b'\n' {
                                        line += 1;
                                    }
                                    j += 1;
                                }
                                continue;
                            }
                            _ => value.push('\\'),
                        }
                        j += 2;
                        continue;
                    }
                    if b[j] == b'\n' {
                        line += 1;
                    }
                    let ch = src[j..].chars().next().unwrap();
                    value.push(ch);
                    j += ch.len_utf8();
                }
                out.push(Literal {
                    value,
                    line: start_line,
                    start,
                });
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// Nome da chamada que contém a posição `at` (a função ou macro antes do
/// parêntese aberto mais próximo), e onde esse parêntese está.
fn enclosing_call(src: &str, at: usize) -> Option<(String, usize)> {
    let b = src.as_bytes();
    let mut depth = 0i32;
    let mut i = at;
    while i > 0 {
        i -= 1;
        match b[i] {
            b')' | b']' => depth += 1,
            b'(' | b'[' if depth > 0 => depth -= 1,
            b'(' => {
                let mut j = i;
                while j > 0 && {
                    let c = b[j - 1];
                    c.is_ascii_alphanumeric() || c == b'_' || c == b':' || c == b'!'
                } {
                    j -= 1;
                }
                let name = &src[j..i];
                // `Type::new` fica inteiro; `a::b::func` vira `func`.
                let name = match name.rsplit_once("::") {
                    Some((ty, "new")) => format!("{}::new", ty.rsplit("::").next().unwrap()),
                    Some((_, f)) => f.to_string(),
                    None => name.to_string(),
                };
                return Some((name, i));
            }
            b'{' | b';' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// Se o literal em `at` é o valor de um braço de `match` que está dentro de
/// `tr(match ... { ... })`: o `tr` traduz o resultado do `match`.
fn match_arm_in_tr(src: &str, at: usize) -> bool {
    if !src[..at].trim_end().ends_with("=>") {
        return false;
    }
    let b = src.as_bytes();
    let mut depth = 0i32;
    let mut i = at;
    while i > 0 {
        i -= 1;
        match b[i] {
            b'}' => depth += 1,
            b'{' if depth > 0 => depth -= 1,
            b'{' => {
                let before = &src[..i];
                let Some(m) = before.rfind("match ") else {
                    return false;
                };
                let head = src[..m].trim_end();
                return head.ends_with("tr(") || head.ends_with("tr_noop(");
            }
            _ => {}
        }
    }
    false
}

/// Começo da instrução que contém `at`, sem espaços.
fn statement_start(src: &str, at: usize) -> &str {
    let start = src[..at].rfind([';', '{', '}']).map_or(0, |i| i + 1);
    src[start..at].trim_start()
}

/// Se o texto parece frase da interface: tem uma palavra de duas letras ou
/// mais e não é um identificador em minúsculas (`sql`, `public`, `id_x`).
fn has_words(s: &str) -> bool {
    let identifier = s
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.*-".contains(c));
    !identifier
        && s.split(|c: char| !c.is_alphabetic())
            .any(|w| w.chars().count() >= 2)
}

fn ignored(lines: &[&str], line: usize) -> bool {
    let at = |n: usize| lines.get(n.wrapping_sub(1)).copied().unwrap_or_default();
    at(line).contains("i18n-ignore") || at(line - 1).contains("i18n-ignore")
}

#[test]
fn every_translatable_text_has_a_translation() {
    let mut missing = BTreeMap::<String, Vec<String>>::new();
    let mut used = BTreeSet::new();
    for (file, src) in sources() {
        let lits = literals(&src);
        for (k, lit) in lits.iter().enumerate() {
            if match_arm_in_tr(&src, lit.start) {
                used.insert((None, lit.value.clone()));
                if !has_translation(Language::PortugueseBrazil, None, &lit.value) {
                    missing
                        .entry(lit.value.clone())
                        .or_default()
                        .push(format!("{file}:{}", lit.line));
                }
                continue;
            }
            let Some((call, paren)) = enclosing_call(&src, lit.start) else {
                continue;
            };
            if !TR_CALLS.contains(&call.as_str()) {
                continue;
            }
            // Primeiro argumento (ou segundo, para trc).
            let first = src[paren + 1..lit.start].trim().is_empty();
            let context = if call == "trc" && !first {
                match lits.get(k.wrapping_sub(1)) {
                    Some(prev) if prev.start > paren => Some(prev.value.clone()),
                    _ => continue,
                }
            } else if call == "trc" || !first {
                continue;
            } else {
                None
            };
            used.insert((context.clone(), lit.value.clone()));
            if !has_translation(Language::PortugueseBrazil, context.as_deref(), &lit.value) {
                missing
                    .entry(lit.value.clone())
                    .or_default()
                    .push(format!("{file}:{}", lit.line));
            }
        }
    }
    let report: Vec<_> = missing
        .iter()
        .map(|(text, at)| format!("  {text:?} ({})", at.join(", ")))
        .collect();
    assert!(
        report.is_empty(),
        "textos sem tradução em locales/pt-BR.po:\n{}",
        report.join("\n")
    );
    // Entradas que nenhum `tr` literal usa precisam estar numa tabela
    // conhecida (rótulos de comandos, menus e outras listas `const`). Isso é
    // verificado nos testes de cada crate; aqui só o catálogo.
    let mut seen = BTreeSet::new();
    for key in catalog_keys(Language::PortugueseBrazil) {
        assert!(
            seen.insert(key.clone()),
            "entrada repetida no catálogo: {key:?}"
        );
    }
}

#[test]
fn catalog_placeholders_match() {
    let catalog = std::fs::read_to_string(root().join("crates/i18n/locales/pt-BR.po")).unwrap();
    let names = |s: &str| -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = s;
        while let Some(i) = rest.find('{') {
            rest = &rest[i + 1..];
            if let Some(stripped) = rest.strip_prefix('{') {
                rest = stripped;
                continue;
            }
            if let Some(end) = rest.find('}') {
                out.insert(rest[..end].to_string());
            }
        }
        out
    };
    pgstudio_i18n::set_language(Language::PortugueseBrazil);
    for (ctx, id) in catalog_keys(Language::PortugueseBrazil) {
        let text = match &ctx {
            Some(c) => pgstudio_i18n::trc(c, &id),
            None => pgstudio_i18n::tr(&id),
        };
        assert!(!text.is_empty(), "tradução vazia: {id:?}");
        assert_eq!(names(&id), names(text), "marcadores diferentes em {id:?}");
        let ellipsis = |s: &str| s.ends_with('…') || s.ends_with("...");
        assert_eq!(
            ellipsis(&id),
            ellipsis(text),
            "reticências diferentes em {id:?}"
        );
    }
    pgstudio_i18n::set_language(Language::English);
    assert!(catalog.contains("Language: pt_BR"));
}

#[test]
fn no_visible_text_outside_tr() {
    let mut problems = Vec::new();
    let mut pending_clean = Vec::new();
    for (file, src) in sources() {
        if !UI_DIRS.iter().any(|d| file.starts_with(d)) || NOT_UI.contains(&file.as_str()) {
            continue;
        }
        let lines: Vec<&str> = src.lines().collect();
        let mut found = Vec::new();
        for lit in literals(&src) {
            let text = lit.value.trim();
            if !has_words(text) || UNTRANSLATED.contains(&text) || ignored(&lines, lit.line) {
                continue;
            }
            let mut call = enclosing_call(&src, lit.start);
            // format!("...") dentro de um widget conta como o widget.
            if let Some((name, paren)) = &call
                && (name == "format!" || name == "to_string")
            {
                call = enclosing_call(&src, *paren);
            }
            let visible = match &call {
                Some((name, _)) if TR_CALLS.contains(&name.as_str()) => false,
                Some((name, _)) => UI_CALLS.contains(&name.as_str()),
                None => {
                    let stmt = statement_start(&src, lit.start);
                    stmt.starts_with("self.status =") || stmt.starts_with("status =")
                }
            };
            if visible {
                found.push(format!("  {file}:{}: {:?}", lit.line, lit.value));
            }
        }
        if PENDING.contains(&file.as_str()) {
            if found.is_empty() {
                pending_clean.push(file);
            }
        } else {
            problems.extend(found);
        }
    }
    assert!(
        problems.is_empty(),
        "texto visível sem tr() (use tr/trf!, ou // i18n-ignore se não se traduz):\n{}",
        problems.join("\n")
    );
    assert!(
        pending_clean.is_empty(),
        "estes arquivos já não têm texto solto; tire-os de PENDING: {pending_clean:?}"
    );
}

#[test]
fn scanner_finds_literals() {
    let src = r##"
fn f() { // "no"
    let c = '"'; let l: &'static str = "a\"b";
    ui.label(tr("Hello"));
    ui.label("World");
    x(r#"raw "x""#);
    self.status = "Busy".to_string();
}"##;
    let lits = literals(src);
    let values: Vec<_> = lits.iter().map(|l| l.value.as_str()).collect();
    assert_eq!(values, ["a\"b", "Hello", "World", "raw \"x\"", "Busy"]);
    let call = |v: &str| {
        let l = lits.iter().find(|l| l.value == v).unwrap();
        enclosing_call(src, l.start).map(|c| c.0)
    };
    assert_eq!(call("Hello").as_deref(), Some("tr"));
    assert_eq!(call("World").as_deref(), Some("label"));
    assert_eq!(call("Busy"), None);
    let busy = lits.iter().find(|l| l.value == "Busy").unwrap();
    assert!(statement_start(src, busy.start).starts_with("self.status ="));
    assert_eq!(lits[2].line, 5);

    let src = "fn l(k: K) -> &str { tr(match k { K::A => \"Alpha\", K::B => \"Beta\" }) }";
    let lits = literals(src);
    assert!(lits.iter().all(|l| match_arm_in_tr(src, l.start)));
    let src = "fn l(k: K) -> &str { match k { K::A => \"Alpha\" } }";
    assert!(!match_arm_in_tr(src, literals(src)[0].start));
}
