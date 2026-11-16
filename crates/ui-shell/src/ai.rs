//! A IA do PostPilot: qual assistente o painel, o Commit e o nome de
//! arquivo sugerido usam, e as chaves dos provedores.
//!
//! O assistente é o Claude Code instalado (o padrão, com o login dele), o
//! ChatGPT pela conta do usuário (o Codex CLI instalado, com o login dele;
//! veja `pgstudio_automation::codex`) ou um provedor por chave de API: OpenAI, OpenRouter, GLM ou qualquer
//! serviço compatível (veja `pgstudio_automation::llm`). A lista e a
//! escolha ficam em `<config>/PostPilot/ai.json`, sem segredo nenhum.
//!
//! As chaves ficam no cofre do sistema (Secret Service, Credential
//! Manager, Keychain). Sem cofre, vão para `<config>/PostPilot/ai-keys.json`,
//! que só o usuário lê (0600). Nos dois casos ficam fora da pasta do
//! workspace: nunca vão para o Git.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pgstudio_automation::llm::{Backend, CHATGPT_LABEL, Provider, ProviderKind};
use serde::{Deserialize, Serialize};

use crate::secrets::SecretStore;

/// O valor de [`AiSettings::active`] para o Claude Code.
pub const CLAUDE_CODE: &str = "claude-code";

/// O valor de [`AiSettings::active`] para o ChatGPT pela conta (Codex).
pub const CHATGPT: &str = "chatgpt-account";

/// De quanto em quanto tempo procurar o `claude` de novo.
const PROGRAM_TTL: Duration = Duration::from_secs(20);

/// O que fica em `ai.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// [`CLAUDE_CODE`], [`CHATGPT`] ou o `id` de um provedor.
    pub active: String,
    pub providers: Vec<Provider>,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            active: CLAUDE_CODE.into(),
            providers: Vec::new(),
        }
    }
}

impl AiSettings {
    /// O provedor por chave escolhido, se for um.
    pub fn active_provider(&self) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == self.active)
    }

    /// Se o escolhido é o ChatGPT pela conta.
    pub fn uses_chatgpt(&self) -> bool {
        self.active == CHATGPT
    }

    /// Nome do assistente escolhido.
    pub fn active_label(&self) -> String {
        match self.active_provider() {
            Some(p) => p.name.clone(),
            None if self.uses_chatgpt() => CHATGPT_LABEL.to_string(),
            None => "Claude Code".to_string(),
        }
    }

    /// O nome que o painel mostra: vazio com o Claude Code.
    pub fn panel_name(&self) -> String {
        match self.active_provider() {
            Some(p) => p.name.clone(),
            None if self.uses_chatgpt() => CHATGPT_LABEL.to_string(),
            None => String::new(),
        }
    }

    /// Um `id` que nenhum provedor usa.
    pub fn new_id(&self, kind: ProviderKind) -> String {
        let base = match kind {
            ProviderKind::OpenAi => "openai",
            ProviderKind::OpenRouter => "openrouter",
            ProviderKind::Glm => "glm",
            ProviderKind::Custom => "custom",
        };
        (1..)
            .map(|n| format!("{base}-{n}"))
            .find(|id| self.providers.iter().all(|p| &p.id != id))
            .unwrap_or_default()
    }
}

/// Onde a chave de um provedor foi guardada.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// No cofre do sistema.
    Keyring,
    /// No arquivo privado (não há cofre).
    File,
}

/// As chaves dos provedores: o cofre do sistema e, sem ele, um arquivo
/// 0600.
pub struct KeyVault {
    store: Option<Arc<dyn SecretStore>>,
    file: Option<PathBuf>,
}

impl KeyVault {
    pub fn new(store: Option<Arc<dyn SecretStore>>, file: Option<PathBuf>) -> Self {
        KeyVault { store, file }
    }

    fn name(id: &str) -> String {
        format!("ai:{id}")
    }

    fn read_file(&self) -> HashMap<String, String> {
        self.file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn write_file(&self, keys: &HashMap<String, String>) -> Result<(), String> {
        let Some(file) = &self.file else {
            return Err("there is nowhere to keep the key".into());
        };
        if keys.is_empty() {
            return match std::fs::remove_file(file) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("{}: {e}", file.display())),
            };
        }
        let text = serde_json::to_string_pretty(keys).map_err(|e| e.to_string())?;
        write_private(file, &text).map_err(|e| format!("{}: {e}", file.display()))
    }

    /// A chave do provedor `id`.
    pub fn get(&self, id: &str) -> Option<String> {
        if let Some(store) = &self.store
            && let Ok(Some(key)) = store.get(&Self::name(id))
        {
            return Some(key);
        }
        self.read_file().remove(id)
    }

    /// Guarda a chave do provedor `id`.
    pub fn set(&self, id: &str, key: &str) -> Result<Kept, String> {
        if let Some(store) = &self.store
            && store.set(&Self::name(id), key).is_ok()
        {
            // Uma cópia antiga no arquivo não tem mais por que existir.
            let mut keys = self.read_file();
            if keys.remove(id).is_some() {
                let _ = self.write_file(&keys);
            }
            return Ok(Kept::Keyring);
        }
        let mut keys = self.read_file();
        keys.insert(id.to_string(), key.to_string());
        self.write_file(&keys).map(|()| Kept::File)
    }

    /// Esquece a chave do provedor `id`.
    pub fn delete(&self, id: &str) {
        if let Some(store) = &self.store {
            let _ = store.delete(&Self::name(id));
        }
        let mut keys = self.read_file();
        if keys.remove(id).is_some() {
            let _ = self.write_file(&keys);
        }
    }
}

/// Grava `text` num arquivo que só o usuário lê.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// As chaves já lidas, por `id` do provedor. Dividido com as threads que
/// leem e gravam o cofre e com o diálogo de opções.
pub type Keys = Arc<Mutex<HashMap<String, String>>>;

/// A IA dentro da casca.
pub struct Ai {
    settings: AiSettings,
    /// `ai.json`; `None` nos testes e na demonstração (nada é gravado).
    path: Option<PathBuf>,
    vault: Arc<KeyVault>,
    keys: Keys,
    /// Procurar o Claude Code instalado. Desligado no padrão (testes e
    /// quem embute a casca), para nada chamar o `claude` da máquina sem
    /// querer.
    detect: bool,
    /// Programa no lugar do `claude` (testes).
    program_override: Option<PathBuf>,
    /// O `claude` achado e quando foi procurado.
    program: Mutex<Option<(Instant, Option<PathBuf>)>>,
    /// Programa no lugar do `codex` (testes).
    codex_override: Option<PathBuf>,
    /// O `codex` achado e quando foi procurado.
    codex: Mutex<Option<(Instant, Option<PathBuf>)>>,
}

impl Default for Ai {
    fn default() -> Self {
        Ai {
            settings: AiSettings::default(),
            path: None,
            vault: Arc::new(KeyVault::new(None, None)),
            keys: Keys::default(),
            detect: false,
            program_override: None,
            program: Mutex::new(None),
            codex_override: None,
            codex: Mutex::new(None),
        }
    }
}

impl Ai {
    /// Lê `ai.json` em `dir` e, numa thread, as chaves dos provedores.
    pub fn open(dir: &Path, store: Option<Arc<dyn SecretStore>>) -> Ai {
        let path = dir.join("ai.json");
        let settings: AiSettings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let ai = Ai {
            settings,
            path: Some(path),
            detect: true,
            vault: Arc::new(KeyVault::new(store, Some(dir.join("ai-keys.json")))),
            ..Ai::default()
        };
        ai.load_keys();
        ai
    }

    /// Sem arquivo de configuração, mas com o Claude Code da máquina (os
    /// modos de demonstração do aplicativo).
    pub fn system() -> Ai {
        Ai {
            detect: true,
            ..Ai::default()
        }
    }

    /// Com estes provedores e chaves, sem arquivo (testes).
    pub fn with(settings: AiSettings, keys: &[(&str, &str)]) -> Ai {
        let ai = Ai {
            settings,
            ..Ai::default()
        };
        {
            let mut map = ai.keys.lock().expect("ai keys");
            for (id, key) in keys {
                map.insert(id.to_string(), key.to_string());
            }
        }
        ai
    }

    /// O cofre pode esperar o D-Bus: a leitura não roda na thread da
    /// interface.
    fn load_keys(&self) {
        let ids: Vec<String> = self
            .settings
            .providers
            .iter()
            .map(|p| p.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let (vault, keys) = (self.vault.clone(), self.keys.clone());
        let _ = std::thread::Builder::new()
            .name("pgstudio-ai-keys".into())
            .spawn(move || {
                for id in ids {
                    if let Some(key) = vault.get(&id) {
                        keys.lock().expect("ai keys").entry(id).or_insert(key);
                    }
                }
            });
    }

    pub fn settings(&self) -> &AiSettings {
        &self.settings
    }

    /// As chaves lidas até agora.
    pub fn keys(&self) -> Keys {
        self.keys.clone()
    }

    /// Usa este programa no lugar do `claude` (testes).
    pub fn set_program(&mut self, program: Option<PathBuf>) {
        self.program_override = program;
    }

    /// Onde está o Claude Code, se instalado (a busca no PATH vale por
    /// alguns segundos).
    pub fn claude_code(&self) -> Option<PathBuf> {
        if let Some(p) = &self.program_override {
            return Some(p.clone());
        }
        if !self.detect {
            return None;
        }
        let mut cached = self.program.lock().expect("ai program");
        match &*cached {
            Some((at, found)) if at.elapsed() < PROGRAM_TTL => found.clone(),
            _ => {
                let found = pgstudio_automation::agent::find_program();
                *cached = Some((Instant::now(), found.clone()));
                found
            }
        }
    }

    /// Usa este programa no lugar do `codex` (testes).
    pub fn set_codex(&mut self, program: Option<PathBuf>) {
        self.codex_override = program;
    }

    /// Onde está o Codex CLI, se instalado (a busca no PATH vale por
    /// alguns segundos).
    pub fn codex(&self) -> Option<PathBuf> {
        if let Some(p) = &self.codex_override {
            return Some(p.clone());
        }
        if !self.detect {
            return None;
        }
        let mut cached = self.codex.lock().expect("ai codex");
        match &*cached {
            Some((at, found)) if at.elapsed() < PROGRAM_TTL => found.clone(),
            _ => {
                let found = pgstudio_automation::codex::find_program();
                *cached = Some((Instant::now(), found.clone()));
                found
            }
        }
    }

    /// A IA conectada: o assistente escolhido, se está pronto para usar
    /// (Claude Code ou Codex instalado, ou provedor com chave).
    pub fn backend(&self) -> Option<Backend> {
        match self.settings.active_provider() {
            None if self.settings.uses_chatgpt() => {
                self.codex().map(|program| Backend::Codex { program })
            }
            None => self
                .claude_code()
                .map(|program| Backend::ClaudeCode { program }),
            Some(p) => {
                let key = self.keys.lock().expect("ai keys").get(&p.id).cloned();
                // Um servidor local (Ollama, LM Studio) não pede chave.
                let key = match key {
                    Some(k) => k,
                    None if p.kind == ProviderKind::Custom => String::new(),
                    None => return None,
                };
                Some(Backend::Api {
                    name: p.name.clone(),
                    conn: p.connect(&key),
                })
            }
        }
    }

    /// Nome do assistente escolhido, pronto ou não.
    pub fn active_label(&self) -> String {
        self.settings.active_label()
    }

    /// Aplica o que saiu de Tools > Options: grava `ai.json`, guarda as
    /// chaves novas e esquece as dos provedores removidos. Devolve um
    /// problema para a status bar, se houver.
    pub fn apply(
        &mut self,
        settings: AiSettings,
        new_keys: HashMap<String, String>,
    ) -> Option<String> {
        let removed: Vec<String> = self
            .settings
            .providers
            .iter()
            .filter(|old| settings.providers.iter().all(|p| p.id != old.id))
            .map(|p| p.id.clone())
            .collect();
        {
            let mut keys = self.keys.lock().expect("ai keys");
            for id in &removed {
                keys.remove(id);
            }
            for (id, key) in &new_keys {
                if key.is_empty() {
                    keys.remove(id);
                } else {
                    keys.insert(id.clone(), key.clone());
                }
            }
        }
        self.settings = settings;
        let vault = self.vault.clone();
        let _ = std::thread::Builder::new()
            .name("pgstudio-ai-keys".into())
            .spawn(move || {
                for id in removed {
                    vault.delete(&id);
                }
                for (id, key) in new_keys {
                    if key.is_empty() {
                        vault.delete(&id);
                    } else if let Err(e) = vault.set(&id, &key) {
                        eprintln!("ai: could not keep the API key: {e}");
                    }
                }
            });
        let path = self.path.as_ref()?;
        let text = serde_json::to_string_pretty(&self.settings).unwrap_or_default();
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, text + "\n"))
            .err()
            .map(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{MemoryStore, NoStore};

    #[test]
    fn chatgpt_by_account_needs_the_codex_program_and_no_key() {
        let dir = temp("chatgpt");
        let mut ai = Ai::open(&dir, Some(Arc::new(MemoryStore::default())));
        ai.set_program(None);
        ai.set_codex(Some(PathBuf::from("/opt/codex")));
        let mut settings = ai.settings().clone();
        settings.active = CHATGPT.into();
        assert_eq!(ai.apply(settings, HashMap::new()), None);
        assert_eq!(
            ai.backend(),
            Some(Backend::Codex {
                program: PathBuf::from("/opt/codex")
            })
        );
        assert_eq!(ai.active_label(), "ChatGPT");
        assert_eq!(ai.settings().panel_name(), "ChatGPT");
        // A escolha fica salva; credencial nenhuma passa pelo PostPilot.
        let text = std::fs::read_to_string(dir.join("ai.json")).unwrap();
        assert!(text.contains("\"active\": \"chatgpt-account\""), "{text}");
        assert!(!dir.join("ai-keys.json").exists());
        let again = Ai::open(&dir, None);
        assert!(again.settings().uses_chatgpt());
        // Sem o Codex instalado, não há IA conectada.
        assert_eq!(Ai::with(again.settings().clone(), &[]).backend(), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("postpilot-ai-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn keys_go_to_the_keyring_or_to_a_private_file() {
        let dir = temp("vault");
        let file = dir.join("ai-keys.json");
        let store = Arc::new(MemoryStore::default());
        let vault = KeyVault::new(Some(store.clone()), Some(file.clone()));
        assert_eq!(vault.set("glm-1", "sk-1"), Ok(Kept::Keyring));
        assert_eq!(store.get("ai:glm-1").unwrap().as_deref(), Some("sk-1"));
        assert!(!file.exists());
        assert_eq!(vault.get("glm-1").as_deref(), Some("sk-1"));
        vault.delete("glm-1");
        assert_eq!(vault.get("glm-1"), None);

        // Sem cofre: arquivo que só o usuário lê.
        let vault = KeyVault::new(Some(Arc::new(NoStore)), Some(file.clone()));
        assert_eq!(vault.set("glm-1", "sk-2"), Ok(Kept::File));
        assert_eq!(vault.get("glm-1").as_deref(), Some("sk-2"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        vault.delete("glm-1");
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_file_has_no_key_and_backend_needs_one() {
        let dir = temp("settings");
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
        let mut ai = Ai::open(&dir, Some(store.clone()));
        ai.set_program(None);
        assert_eq!(ai.settings().active, CLAUDE_CODE);

        let mut settings = ai.settings().clone();
        let id = settings.new_id(ProviderKind::Glm);
        assert_eq!(id, "glm-1");
        settings
            .providers
            .push(Provider::new(ProviderKind::Glm, &id));
        settings.active = id.clone();
        assert_eq!(ai.apply(settings.clone(), HashMap::new()), None);
        // Escolhido, mas sem chave: não há IA conectada.
        assert_eq!(ai.backend(), None);
        assert_eq!(ai.active_label(), "GLM (Z.ai)");

        let keys = HashMap::from([(id.clone(), "sk-secret".to_string())]);
        assert_eq!(ai.apply(settings, keys), None);
        let Some(Backend::Api { name, conn }) = ai.backend() else {
            panic!("no backend")
        };
        assert_eq!(name, "GLM (Z.ai)");
        assert_eq!(conn.api_key, "sk-secret");
        let text = std::fs::read_to_string(dir.join("ai.json")).unwrap();
        assert!(!text.contains("sk-secret"), "{text}");
        assert!(text.contains("\"active\": \"glm-1\""), "{text}");

        // O cofre recebe a chave numa thread; outra abertura acha.
        let start = Instant::now();
        while store.get("ai:glm-1").unwrap().is_none() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        let again = Ai::open(&dir, Some(store));
        let start = Instant::now();
        while again.backend().is_none() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
