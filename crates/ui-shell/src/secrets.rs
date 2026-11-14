//! Senhas lembradas ("Remember password") no cofre do sistema.
//!
//! O cofre é o Credential Manager no Windows, o Keychain no macOS e o
//! Secret Service (GNOME Keyring, KWallet) no Linux, pela crate `keyring`.
//! Quando não há cofre (Linux sem D-Bus, por exemplo), toda leitura falha
//! com [`SecretError`] e quem chamou simplesmente pede a senha: lembrar a
//! senha é uma conveniência, nunca um motivo para não conectar.
//!
//! As chamadas podem esperar o D-Bus, então a interface nunca chama o cofre
//! na thread dela: veja [`lookup`] e [`save_in_background`].

use pgstudio_i18n::{tr, trf};
use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use pgstudio_core::connection::ConnectionSettings;

/// Nome do serviço com que as senhas ficam no cofre.
pub const SERVICE: &str = "PostPilot";

/// Serviço do nome antigo do app. Uma senha que só existe nele é copiada
/// para [`SERVICE`] na primeira leitura.
pub const LEGACY_SERVICE: &str = "PGStudio";

/// O cofre falhou ou não existe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretError(pub String);

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SecretError {}

/// Onde as senhas lembradas ficam.
pub trait SecretStore: Send + Sync {
    /// Senha guardada para `key`; `Ok(None)` quando não há.
    fn get(&self, key: &str) -> Result<Option<String>, SecretError>;
    /// Guarda (ou troca) a senha de `key`.
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError>;
    /// Esquece a senha de `key`. Esquecer o que não existe não é erro.
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}

/// Chave da senha de uma conexão: usuário, servidor e porta. O banco não
/// entra, porque a senha do PostgreSQL é do usuário no servidor.
pub fn password_key(settings: &ConnectionSettings) -> String {
    format!("{}@{}:{}", settings.user, settings.host, settings.port)
}

/// O cofre do sistema.
#[derive(Clone, Debug)]
pub struct KeyringStore {
    service: String,
    legacy: Option<String>,
}

impl Default for KeyringStore {
    fn default() -> Self {
        KeyringStore {
            service: SERVICE.into(),
            legacy: Some(LEGACY_SERVICE.into()),
        }
    }
}

impl KeyringStore {
    /// Cofre com outro nome de serviço (testes), sem o serviço antigo.
    pub fn with_service(service: impl Into<String>) -> Self {
        KeyringStore {
            service: service.into(),
            legacy: None,
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(&self.service, key).map_err(explain)
    }

    /// Senha guardada pelo app com o nome antigo; copiada para o serviço
    /// novo, para a próxima leitura já achar lá.
    fn get_legacy(&self, key: &str) -> Result<Option<String>, SecretError> {
        let Some(legacy) = &self.legacy else {
            return Ok(None);
        };
        let entry = keyring::Entry::new(legacy, key).map_err(explain)?;
        match entry.get_password() {
            Ok(p) => {
                // Se a cópia falhar, a senha antiga continua servindo.
                let _ = self.set(key, &p);
                Ok(Some(p))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(explain(e)),
        }
    }
}

fn explain(e: keyring::Error) -> SecretError {
    SecretError(match e {
        keyring::Error::PlatformFailure(e) | keyring::Error::NoStorageAccess(e) => {
            trf!("The system keyring is not available: {error}", error = e)
        }
        other => trf!("The system keyring failed: {error}", error = other),
    })
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        match self.entry(key)?.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => self.get_legacy(key),
            Err(e) => Err(explain(e)),
        }
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.entry(key)?.set_password(secret).map_err(explain)
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(explain(e)),
        }
        // Senão a leitura seguinte traria de volta a senha do nome antigo.
        if let Some(legacy) = &self.legacy {
            let entry = keyring::Entry::new(legacy, key).map_err(explain)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(explain(e)),
            }
        }
        Ok(())
    }
}

/// Cofre em memória (testes e demonstração).
#[derive(Debug, Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl MemoryStore {
    /// Quantas senhas estão guardadas.
    pub fn len(&self) -> usize {
        self.0.lock().expect("store lock").len()
    }

    /// Se não há senha guardada.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        Ok(self.0.lock().expect("store lock").get(key).cloned())
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.0
            .lock()
            .expect("store lock")
            .insert(key.into(), secret.into());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.0.lock().expect("store lock").remove(key);
        Ok(())
    }
}

/// Cofre que não existe: tudo falha, como num Linux sem D-Bus.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoStore;

impl SecretStore for NoStore {
    fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
        Err(SecretError(tr("No system keyring.").into()))
    }

    fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
        Err(SecretError(tr("No system keyring.").into()))
    }

    fn delete(&self, _: &str) -> Result<(), SecretError> {
        Err(SecretError(tr("No system keyring.").into()))
    }
}

/// Uma leitura do cofre em andamento.
pub struct Lookup {
    /// Chave procurada.
    pub key: String,
    rx: Receiver<Option<String>>,
}

impl Lookup {
    /// A senha, quando a leitura terminar: `Some(None)` quando não há senha
    /// guardada ou o cofre falhou.
    pub fn poll(&self) -> Option<Option<String>> {
        match self.rx.try_recv() {
            Ok(found) => Some(found),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(None),
        }
    }

    /// Espera a leitura (testes).
    pub fn wait(&self) -> Option<String> {
        self.rx.recv().ok().flatten()
    }
}

/// Lê a senha de `key` numa thread separada. Erros do cofre viram "sem
/// senha": quem chamou pede a senha ao usuário.
pub fn lookup(store: &Arc<dyn SecretStore>, key: String) -> Lookup {
    let (tx, rx) = mpsc::channel();
    let store = store.clone();
    let k = key.clone();
    std::thread::Builder::new()
        .name("pgstudio-keyring".into())
        .spawn(move || {
            let found = store.get(&k).unwrap_or_else(|e| {
                eprintln!("keyring: {e}");
                None
            });
            let _ = tx.send(found);
        })
        .expect("spawn keyring thread");
    Lookup { key, rx }
}

/// Guarda (`Some`) ou esquece (`None`) a senha numa thread separada. Uma
/// falha do cofre só vai para o log: a conexão já funcionou.
pub fn save_in_background(store: &Arc<dyn SecretStore>, key: String, secret: Option<String>) {
    let store = store.clone();
    std::thread::Builder::new()
        .name("pgstudio-keyring".into())
        .spawn(move || {
            let result = match &secret {
                Some(s) => store.set(&key, s),
                None => store.delete(&key),
            };
            if let Err(e) = result {
                eprintln!("keyring: {e}");
            }
        })
        .expect("spawn keyring thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_ignores_the_database() {
        let a = ConnectionSettings {
            host: "db".into(),
            port: 6432,
            user: "alice".into(),
            ..Default::default()
        };
        let b = ConnectionSettings {
            database: "other".into(),
            ..a.clone()
        };
        assert_eq!(password_key(&a), "alice@db:6432");
        assert_eq!(password_key(&a), password_key(&b));
    }

    #[test]
    fn memory_store_round_trips() {
        let store = MemoryStore::default();
        assert_eq!(store.get("k"), Ok(None));
        store.set("k", "s1").unwrap();
        store.set("k", "s2").unwrap();
        assert_eq!(store.get("k"), Ok(Some("s2".into())));
        store.delete("k").unwrap();
        store.delete("k").unwrap();
        assert!(store.is_empty());
    }

    #[test]
    fn lookup_turns_errors_into_no_password() {
        let none: Arc<dyn SecretStore> = Arc::new(NoStore);
        assert_eq!(lookup(&none, "k".into()).wait(), None);
        let mem: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
        mem.set("k", "pw").unwrap();
        assert_eq!(lookup(&mem, "k".into()).wait().as_deref(), Some("pw"));
    }

    /// Sem sessão D-Bus (CI, contêiner) o cofre do sistema falha com erro,
    /// sem travar nem entrar em pânico. Com cofre, a senha vai e volta.
    #[test]
    fn keyring_works_or_fails_cleanly() {
        let store = KeyringStore::with_service("PostPilot-test");
        let key = format!("test@localhost:{}", std::process::id());
        let no_bus =
            cfg!(target_os = "linux") && std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none();
        match store.set(&key, "pw") {
            Ok(()) => {
                assert!(!no_bus);
                assert_eq!(store.get(&key).unwrap().as_deref(), Some("pw"));
                store.delete(&key).unwrap();
                assert_eq!(store.get(&key).unwrap(), None);
            }
            Err(e) => {
                assert!(e.0.contains("keyring"), "{e}");
                assert!(store.get(&key).is_err());
            }
        }
    }

    /// Uma senha guardada com o nome antigo do app aparece no novo, e
    /// esquecer apaga as duas.
    #[test]
    fn legacy_passwords_migrate_and_are_forgotten() {
        let old = KeyringStore::with_service("PGStudio-legacy-test");
        let new = KeyringStore {
            service: "PostPilot-legacy-test".into(),
            legacy: Some("PGStudio-legacy-test".into()),
        };
        let key = format!("legacy@localhost:{}", std::process::id());
        if old.set(&key, "pw").is_err() {
            // Sem cofre (CI, contêiner): nada a migrar.
            assert!(new.get(&key).is_err());
            return;
        }
        assert_eq!(new.get(&key).unwrap().as_deref(), Some("pw"));
        let copied = KeyringStore::with_service("PostPilot-legacy-test");
        assert_eq!(copied.get(&key).unwrap().as_deref(), Some("pw"));
        new.delete(&key).unwrap();
        assert_eq!(new.get(&key).unwrap(), None);
        assert_eq!(old.get(&key).unwrap(), None);
    }
}
