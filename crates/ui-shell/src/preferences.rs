//! Preferências do usuário, gravadas em `<config>/PGStudio/settings.json` e
//! editadas em Tools > Options.
//!
//! Para uma preferência nova: um campo em [`Preferences`] (com
//! `#[serde(default)]` a struct inteira aceita arquivos antigos) e uma página
//! em [`crate::options`].

use std::path::{Path, PathBuf};

use pgstudio_i18n::Language;
use serde::{Deserialize, Serialize};

/// Idioma escolhido nas opções.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum LanguageSetting {
    /// O idioma do sistema operacional ("Same as system").
    #[default]
    System,
    /// Um idioma fixo.
    Fixed(Language),
}

impl LanguageSetting {
    /// Idioma efetivo da interface.
    pub fn resolve(self) -> Language {
        match self {
            LanguageSetting::System => pgstudio_i18n::system_language(),
            LanguageSetting::Fixed(l) => l,
        }
    }
}

impl From<String> for LanguageSetting {
    /// Valor desconhecido (de uma versão futura, por exemplo) vira "System".
    fn from(s: String) -> Self {
        Language::from_code(&s)
            .map(LanguageSetting::Fixed)
            .unwrap_or_default()
    }
}

impl From<LanguageSetting> for String {
    fn from(l: LanguageSetting) -> Self {
        match l {
            LanguageSetting::System => "system".to_string(),
            LanguageSetting::Fixed(l) => l.code().to_string(),
        }
    }
}

/// As preferências.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Idioma da interface.
    pub language: LanguageSetting,
}

impl Preferences {
    /// Aplica o que vale para o processo todo (hoje, o idioma).
    pub fn apply(&self) {
        pgstudio_i18n::set_language(self.language.resolve());
    }
}

/// Arquivo de preferências.
#[derive(Debug)]
pub struct PreferencesStore {
    path: PathBuf,
    /// Problema ao ler o arquivo, para avisar o usuário.
    pub error: Option<String>,
}

impl PreferencesStore {
    /// Lê `path`. Arquivo inexistente dá as preferências padrão; arquivo
    /// ruim também, com o problema em `error`.
    pub fn open(path: impl Into<PathBuf>) -> (Self, Preferences) {
        let path = path.into();
        let (prefs, error) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(prefs) => (prefs, None),
                Err(e) => (Preferences::default(), Some(e.to_string())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Preferences::default(), None),
            Err(e) => (Preferences::default(), Some(e.to_string())),
        };
        (PreferencesStore { path, error }, prefs)
    }

    /// Caminho do arquivo.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Grava as preferências (arquivo temporário e renomeia).
    pub fn save(&self, prefs: &Preferences) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(prefs).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json + "\n")?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_round_trips() {
        for setting in [
            LanguageSetting::System,
            LanguageSetting::Fixed(Language::English),
            LanguageSetting::Fixed(Language::PortugueseBrazil),
        ] {
            let prefs = Preferences { language: setting };
            let json = serde_json::to_string(&prefs).unwrap();
            assert_eq!(serde_json::from_str::<Preferences>(&json).unwrap(), prefs);
        }
        let json = serde_json::to_string(&Preferences {
            language: LanguageSetting::Fixed(Language::PortugueseBrazil),
        })
        .unwrap();
        assert_eq!(json, r#"{"language":"pt-BR"}"#);
    }

    #[test]
    fn tolerates_old_and_unknown_values() {
        let p: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(p, Preferences::default());
        let p: Preferences = serde_json::from_str(r#"{"language":"fr","x":1}"#).unwrap();
        assert_eq!(p.language, LanguageSetting::System);
    }

    #[test]
    fn saves_and_opens() {
        let dir = std::env::temp_dir().join(format!("pgstudio-prefs-{}", std::process::id()));
        let file = dir.join("settings.json");
        let (store, prefs) = PreferencesStore::open(&file);
        assert_eq!(prefs, Preferences::default());
        assert!(store.error.is_none());
        let prefs = Preferences {
            language: LanguageSetting::Fixed(Language::PortugueseBrazil),
        };
        store.save(&prefs).unwrap();
        assert_eq!(PreferencesStore::open(&file).1, prefs);
        std::fs::write(&file, "nope").unwrap();
        let (store, p) = PreferencesStore::open(&file);
        assert!(store.error.is_some());
        assert_eq!(p, Preferences::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
