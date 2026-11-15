//! Preferências da checagem, salvas em JSON na pasta de configuração.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Intervalo entre as checagens automáticas com o app aberto. Abrir o app
/// sempre checa, sem esperar este intervalo.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Preferências e estado da checagem de versão nova.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Procurar versão nova ao abrir o app.
    pub check_on_startup: bool,
    /// Quando foi a última checagem (segundos desde 1970).
    pub last_check: u64,
    /// Versão que o usuário mandou pular: ela não é mais anunciada.
    pub skipped_version: Option<String>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings {
            check_on_startup: true,
            last_check: 0,
            skipped_version: None,
            path: None,
        }
    }
}

impl UpdateSettings {
    /// Lê `path`; sem o arquivo (ou com ele ilegível), os padrões.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut s: UpdateSettings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        s.path = Some(path);
        s
    }

    /// Só na memória (testes, screenshots): nunca checa ao abrir.
    pub fn in_memory() -> Self {
        UpdateSettings {
            check_on_startup: false,
            ..Default::default()
        }
    }

    /// Grava no arquivo de onde veio; sem arquivo, não faz nada.
    pub fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// O arquivo de onde as preferências vieram.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Se a checagem automática deve rodar agora.
    pub fn due(&self, now: SystemTime) -> bool {
        self.check_on_startup
            && now
                .duration_since(UNIX_EPOCH + Duration::from_secs(self.last_check))
                .is_ok_and(|elapsed| elapsed >= CHECK_INTERVAL)
    }

    /// Anota uma checagem feita em `now`.
    pub fn mark_checked(&mut self, now: SystemTime) {
        self.last_check = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_at_most_every_few_hours() {
        let mut s = UpdateSettings::default();
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert!(s.due(now));
        s.mark_checked(now);
        assert!(!s.due(now + Duration::from_secs(60)));
        assert!(s.due(now + CHECK_INTERVAL));
        // Relógio voltou no tempo: não checa até passar a última checagem.
        assert!(!s.due(now - Duration::from_secs(60)));
        s.check_on_startup = false;
        assert!(!s.due(now + CHECK_INTERVAL));
        assert!(!UpdateSettings::in_memory().due(now));
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("updater-settings-{}", std::process::id()));
        let file = dir.join("sub").join("update.json");
        let mut s = UpdateSettings::open(&file);
        assert_eq!(s.path(), Some(file.as_path()));
        assert!(s.check_on_startup);
        s.check_on_startup = false;
        s.skipped_version = Some("0.2.0".into());
        s.save().unwrap();
        let back = UpdateSettings::open(&file);
        assert_eq!(back, s);
        std::fs::write(&file, "not json").unwrap();
        assert!(UpdateSettings::open(&file).check_on_startup);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
