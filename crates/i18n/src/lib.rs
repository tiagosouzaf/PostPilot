//! Tradução da interface do PGStudio.
//!
//! O texto em inglês do código é a chave, como no gettext: quem escreve a
//! interface continua escrevendo em inglês e só envolve o texto em [`tr`]
//! (ou [`trf!`] quando há valores no meio). A tradução para português do
//! Brasil fica em `locales/pt-BR.po`, embutido no binário.
//!
//! ```
//! use pgstudio_i18n::{Language, set_language, tr, trf};
//!
//! set_language(Language::PortugueseBrazil);
//! assert_eq!(tr("Cancel"), "Cancelar");
//! assert_eq!(trf!("Version {version}", version = "0.1.0"), "Versão 0.1.0");
//! set_language(Language::English);
//! assert_eq!(tr("Cancel"), "Cancel");
//! ```
//!
//! Texto sem tradução aparece em inglês. O teste `tests/coverage.rs` acusa
//! texto visível fora de `tr` e chave sem tradução no catálogo.
//!
//! Termos de SQL, nomes de objetos do PostgreSQL e mensagens do servidor não
//! passam por aqui: o servidor traduz as próprias mensagens conforme o
//! `lc_messages`.

use std::collections::HashMap;
use std::fmt::{Display, Write as _};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

mod po;

/// Idioma da interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Language {
    /// Inglês, o texto original do código.
    #[default]
    English,
    /// Português do Brasil.
    PortugueseBrazil,
}

impl Language {
    /// Todos os idiomas, na ordem em que aparecem nas opções.
    pub const ALL: [Language; 2] = [Language::English, Language::PortugueseBrazil];

    /// Código BCP 47 usado no arquivo de preferências, como `pt-BR`.
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::PortugueseBrazil => "pt-BR",
        }
    }

    /// Nome do idioma escrito nele mesmo, como nas opções do SSMS.
    pub fn native_name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::PortugueseBrazil => "Português (Brasil)",
        }
    }

    /// Idioma de um código de locale (`pt-BR`, `pt_BR.UTF-8`, `en-US`...).
    ///
    /// Qualquer português vira português do Brasil, o único que existe por
    /// enquanto; os demais idiomas, inglês. `None` só para código vazio.
    pub fn from_locale(locale: &str) -> Option<Language> {
        let lang = locale
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match lang.as_str() {
            "" => None,
            "pt" => Some(Language::PortugueseBrazil),
            _ => Some(Language::English),
        }
    }

    /// Idioma de um código exato, como gravado por [`Language::code`].
    pub fn from_code(code: &str) -> Option<Language> {
        Language::ALL.into_iter().find(|l| l.code() == code)
    }

    fn from_u8(n: u8) -> Language {
        match n {
            1 => Language::PortugueseBrazil,
            _ => Language::English,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Language::English => 0,
            Language::PortugueseBrazil => 1,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Troca o idioma da interface. Vale a partir do próximo frame.
pub fn set_language(language: Language) {
    CURRENT.store(language.as_u8(), Ordering::Relaxed);
}

/// Idioma em uso.
pub fn language() -> Language {
    Language::from_u8(CURRENT.load(Ordering::Relaxed))
}

/// Idioma do sistema operacional: `GetUserDefaultLocaleName` no Windows,
/// `LC_ALL`, `LC_MESSAGES` e `LANG` nos demais.
pub fn system_language() -> Language {
    sys_locale::get_locale()
        .and_then(|l| Language::from_locale(&l))
        .unwrap_or_default()
}

/// Catálogo de um idioma: chave (contexto + `\u{4}` + texto, como no
/// gettext) para a tradução.
type Catalog = HashMap<String, &'static str>;

fn catalog(language: Language) -> Option<&'static Catalog> {
    static PT_BR: OnceLock<Catalog> = OnceLock::new();
    match language {
        Language::English => None,
        Language::PortugueseBrazil => Some(PT_BR.get_or_init(|| {
            po::parse(include_str!("../locales/pt-BR.po"))
                .expect("locales/pt-BR.po inválido")
                .into_iter()
                .map(|e| (e.key(), &*Box::leak(e.msgstr.into_boxed_str())))
                .collect()
        })),
    }
}

/// Traduz um texto da interface para o idioma em uso.
///
/// Sem tradução, devolve o próprio texto. Aceita texto dinâmico (como o
/// rótulo de um comando) além de literais.
pub fn tr(text: &str) -> &str {
    lookup(language(), None, text).unwrap_or(text)
}

/// Como [`tr`], com um contexto para quando o mesmo texto em inglês tem
/// traduções diferentes (o `msgctxt` do arquivo `.po`).
pub fn trc<'a>(context: &str, text: &'a str) -> &'a str {
    lookup(language(), Some(context), text).unwrap_or(text)
}

/// Marca um texto para tradução sem traduzir agora, para tabelas
/// `const` que passam por [`tr`] na hora de exibir.
pub const fn tr_noop(text: &'static str) -> &'static str {
    text
}

/// Se o texto tem tradução no idioma (inglês sempre tem).
pub fn has_translation(language: Language, context: Option<&str>, text: &str) -> bool {
    language == Language::English || lookup(language, context, text).is_some()
}

fn lookup(language: Language, context: Option<&str>, text: &str) -> Option<&'static str> {
    let catalog = catalog(language)?;
    match context {
        None => catalog.get(text).copied(),
        Some(c) => catalog.get(&po::key(Some(c), text)).copied(),
    }
}

/// Troca os marcadores `{nome}` do modelo pelos valores. `{{` e `}}` viram
/// chaves literais; marcador sem valor fica como está.
///
/// Usado por [`trf!`]; raramente é preciso chamar direto.
pub fn format(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        // Sem let-chains: a MSRV (1.87) ainda não tem.
        let value = tail
            .strip_prefix('{')
            .and_then(|t| t.find('}'))
            .and_then(|end| {
                let name = &tail[1..end + 1];
                args.iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| (end + 2, v))
            });
        if let Some((len, value)) = value {
            let _ = write!(out, "{value}");
            rest = &tail[len..];
            continue;
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// Traduz um modelo com valores nomeados, como `format!` com `tr`:
///
/// ```
/// # use pgstudio_i18n::trf;
/// let n = 3;
/// let text = trf!("{n} rows affected", n = n);
/// assert_eq!(text, "3 rows affected");
/// ```
///
/// O texto em inglês é a chave do catálogo; a tradução pode mudar a ordem
/// dos marcadores. Só marcadores nomeados, sem formatação (`{n:>5}`): formate
/// o valor antes.
#[macro_export]
macro_rules! trf {
    ($text:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::format(
            $crate::tr($text),
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*],
        )
    };
}

/// Os pares (contexto, texto em inglês) do catálogo de um idioma, para os
/// testes de cobertura.
pub fn catalog_keys(language: Language) -> Vec<(Option<String>, String)> {
    match language {
        Language::English => Vec::new(),
        Language::PortugueseBrazil => po::parse(include_str!("../locales/pt-BR.po"))
            .expect("locales/pt-BR.po inválido")
            .into_iter()
            .map(|e| (e.msgctxt, e.msgid))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_codes() {
        assert_eq!(
            Language::from_locale("pt_BR.UTF-8"),
            Some(Language::PortugueseBrazil)
        );
        assert_eq!(
            Language::from_locale("pt-PT"),
            Some(Language::PortugueseBrazil)
        );
        assert_eq!(Language::from_locale("en-US"), Some(Language::English));
        assert_eq!(Language::from_locale("C"), Some(Language::English));
        assert_eq!(Language::from_locale(""), None);
        assert_eq!(
            Language::from_code("pt-BR"),
            Some(Language::PortugueseBrazil)
        );
        assert_eq!(Language::from_code("pt"), None);
    }

    #[test]
    fn format_named_values() {
        let args: [(&str, &dyn Display); 2] = [("a", &1), ("b", &"x")];
        assert_eq!(format("{b}={a} {{a}} {c}", &args), "x=1 {a} {c}");
        assert_eq!(format("sem marcador", &args), "sem marcador");
        assert_eq!(format("{a", &args), "{a");
        assert_eq!(format("ação {a}", &args), "ação 1");
    }

    #[test]
    fn lookup_by_language() {
        assert_eq!(
            lookup(Language::PortugueseBrazil, None, "Cancel"),
            Some("Cancelar")
        );
        assert_eq!(lookup(Language::English, None, "Cancel"), None);
        assert!(has_translation(Language::English, None, "anything"));
        assert!(!has_translation(
            Language::PortugueseBrazil,
            None,
            "texto que não existe"
        ));
    }
}
