//! Cofre de senhas do workspace: `secrets.json`, que vai para o
//! repositório junto com o resto, com as senhas criptografadas.
//!
//! A chave vem da senha mestra pelo Argon2id (64 MiB, 3 passadas) com um
//! sal aleatório guardado no arquivo; cada senha é criptografada à parte
//! com XChaCha20-Poly1305 e um nonce aleatório de 24 bytes, com o nome da
//! senha (`usuario@host:porta`) como dado associado, então trocar duas
//! linhas de lugar ou mexer num byte é detectado. Tudo das crates do
//! RustCrypto (`argon2`, `chacha20poly1305`); nada é feito à mão.
//!
//! A senha mestra e a chave nunca vão para o arquivo. Uma linha `check`
//! (um texto fixo criptografado) diz se a senha mestra está certa antes de
//! tentar as senhas.
//!
//! ```json
//! {
//!   "format": 1,
//!   "kdf": { "algorithm": "argon2id", "m_cost": 65536, "t_cost": 3, "p_cost": 1, "salt": "..." },
//!   "check": "...",
//!   "secrets": {
//!     "alice@db.exemplo.com:5432": "...",
//!     "app@vendas:5432": "..."
//!   }
//! }
//! ```
//!
//! Uma senha por linha, em ordem alfabética: duas máquinas que salvam
//! senhas de servidores diferentes fazem merge no git sem conflito.
//!
//! O arquivo é relido a cada leitura e a cada gravação, para não perder o
//! que um pull trouxe. Se a senha mestra mudou em outra máquina (o sal
//! mudou), o cofre volta a ficar trancado.

use pgstudio_i18n::{tr, trf};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload, rand_core::RngCore};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Arquivo do cofre na pasta do workspace.
pub const FILE: &str = "secrets.json";
/// Versão do formato.
pub const FORMAT: u32 = 1;
/// Menor senha mestra aceita.
pub const MIN_PASSWORD: usize = 10;

/// Custo padrão do Argon2id: 64 MiB, 3 passadas, 1 via (o mínimo que a
/// OWASP recomenda é 19 MiB e 2 passadas).
pub const DEFAULT_KDF: Kdf = Kdf {
    m_cost: 64 * 1024,
    t_cost: 3,
    p_cost: 1,
};

/// Custos abaixo disto são recusados ao ler (um arquivo adulterado não
/// consegue enfraquecer a derivação).
const MIN_KDF: Kdf = Kdf {
    m_cost: 8 * 1024,
    t_cost: 1,
    p_cost: 1,
};

const CHECK_TEXT: &[u8] = b"postpilot-vault-check";
const AAD_PREFIX: &str = "postpilot-vault-v1:";
const NONCE_LEN: usize = 24;

/// Custos do Argon2id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kdf {
    /// Memória em KiB.
    pub m_cost: u32,
    /// Passadas.
    pub t_cost: u32,
    /// Vias paralelas.
    pub p_cost: u32,
}

/// Falha do cofre.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultError {
    /// A senha mestra não confere.
    WrongPassword,
    /// O cofre está trancado.
    Locked,
    /// A senha mestra nova é curta ou não confere com a confirmação.
    WeakPassword(String),
    /// Uma senha guardada não decifra (arquivo alterado).
    Tampered(String),
    /// O arquivo não existe, não abre ou é inválido.
    File(String),
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultError::WrongPassword => f.write_str("The master password is not correct."),
            VaultError::Locked => f.write_str(tr("The workspace passwords are locked.")),
            VaultError::WeakPassword(m) | VaultError::File(m) => f.write_str(m),
            VaultError::Tampered(key) => f.write_str(&trf!(
                "The saved password for {key} was changed outside PostPilot and cannot be read.",
                key = key
            )),
        }
    }
}

impl std::error::Error for VaultError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct KdfFile {
    algorithm: String,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
    salt: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct VaultFile {
    format: u32,
    kdf: KdfFile,
    check: String,
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

/// Chave derivada da senha mestra, apagada da memória ao sair de cena.
#[derive(Clone)]
pub struct Key {
    bytes: Zeroizing<[u8; 32]>,
    /// O sal com que foi derivada (identifica o cofre).
    salt: String,
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(..)")
    }
}

fn derive(password: &str, salt: &[u8], kdf: Kdf) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    let params = Params::new(kdf.m_cost, kdf.t_cost, kdf.p_cost, Some(32))
        .map_err(|e| VaultError::File(trf!("Invalid key derivation settings: {e}", e = e)))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, out.as_mut())
        .map_err(|e| VaultError::File(trf!("Key derivation failed: {e}", e = e)))?;
    Ok(out)
}

fn seal(key: &[u8; 32], name: &str, plain: &[u8]) -> String {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let aad = format!("{AAD_PREFIX}{name}");
    let ct = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plain,
                aad: aad.as_bytes(),
            },
        )
        .expect("encryption never fails for in-memory data");
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    B64.encode(out)
}

fn open(key: &[u8; 32], name: &str, sealed: &str) -> Option<Zeroizing<Vec<u8>>> {
    let raw = B64.decode(sealed.trim()).ok()?;
    if raw.len() < NONCE_LEN + 16 {
        return None;
    }
    let (nonce, ct) = raw.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(key.into());
    let aad = format!("{AAD_PREFIX}{name}");
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ct,
                aad: aad.as_bytes(),
            },
        )
        .ok()
        .map(Zeroizing::new)
}

/// Confere uma senha mestra nova e a confirmação.
pub fn check_new_password(password: &str, confirm: &str) -> Result<(), VaultError> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(VaultError::WeakPassword(format!(
            "Use at least {MIN_PASSWORD} characters for the master password."
        )));
    }
    if password != confirm {
        return Err(VaultError::WeakPassword(
            tr("The passwords do not match.").into(),
        ));
    }
    Ok(())
}

/// O cofre de um workspace.
#[derive(Clone, Debug)]
pub struct Vault {
    path: PathBuf,
    key: Option<Key>,
}

impl Vault {
    /// O cofre de `root` (o arquivo pode ainda não existir).
    pub fn at(root: &Path) -> Vault {
        Vault {
            path: root.join(FILE),
            key: None,
        }
    }

    /// Caminho do arquivo.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Se o workspace já tem cofre.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Se a senha mestra foi digitada (e ainda vale).
    pub fn is_unlocked(&self) -> bool {
        match (&self.key, self.read()) {
            (Some(k), Ok(f)) => k.salt == f.kdf.salt,
            _ => false,
        }
    }

    fn read(&self) -> Result<VaultFile, VaultError> {
        let text = std::fs::read_to_string(&self.path).map_err(|e| {
            VaultError::File(trf!("Could not read {file}: {e}", file = FILE, e = e))
        })?;
        let file: VaultFile = serde_json::from_str(&text)
            .map_err(|e| VaultError::File(format!("{FILE} is invalid: {e}")))?;
        if file.format > FORMAT {
            return Err(VaultError::File(format!(
                "{FILE} was saved by a newer PostPilot. Update PostPilot to read it."
            )));
        }
        if file.kdf.algorithm != "argon2id" {
            return Err(VaultError::File(format!(
                "{FILE} uses an unknown key derivation ({}).",
                file.kdf.algorithm
            )));
        }
        if file.kdf.m_cost < MIN_KDF.m_cost
            || file.kdf.t_cost < MIN_KDF.t_cost
            || file.kdf.p_cost < MIN_KDF.p_cost
        {
            return Err(VaultError::File(format!(
                "{FILE} has key derivation settings that are too weak."
            )));
        }
        Ok(file)
    }

    fn write(&self, file: &VaultFile) -> Result<(), VaultError> {
        let text = serde_json::to_string_pretty(file).expect("always serializable") + "\n";
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .map_err(|e| VaultError::File(trf!("Could not save {file}: {e}", file = FILE, e = e)))
    }

    fn kdf_of(file: &VaultFile) -> Kdf {
        Kdf {
            m_cost: file.kdf.m_cost,
            t_cost: file.kdf.t_cost,
            p_cost: file.kdf.p_cost,
        }
    }

    /// Cria o cofre com esta senha mestra (já destrancado). Recusa se já
    /// existe: para trocar a senha, use [`Self::change_password`].
    pub fn create(&mut self, password: &str, kdf: Kdf) -> Result<(), VaultError> {
        if self.exists() {
            return Err(VaultError::File(trf!(
                "The workspace already has {file}. Unlock it instead.",
                file = FILE
            )));
        }
        self.write_new(password, kdf, BTreeMap::new())
    }

    fn write_new(
        &mut self,
        password: &str,
        kdf: Kdf,
        plain: BTreeMap<String, Zeroizing<Vec<u8>>>,
    ) -> Result<(), VaultError> {
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let bytes = derive(password, &salt, kdf)?;
        let salt = B64.encode(salt);
        let file = VaultFile {
            format: FORMAT,
            kdf: KdfFile {
                algorithm: "argon2id".into(),
                m_cost: kdf.m_cost,
                t_cost: kdf.t_cost,
                p_cost: kdf.p_cost,
                salt: salt.clone(),
            },
            check: seal(&bytes, "check", CHECK_TEXT),
            secrets: plain
                .iter()
                .map(|(k, v)| (k.clone(), seal(&bytes, k, v)))
                .collect(),
        };
        self.write(&file)?;
        self.key = Some(Key { bytes, salt });
        Ok(())
    }

    /// Deriva a chave (lento: rode fora da thread da interface) e confere a
    /// senha mestra, sem guardar nada no cofre.
    pub fn derive_key(&self, password: &str) -> Result<Key, VaultError> {
        let file = self.read()?;
        let salt = B64
            .decode(&file.kdf.salt)
            .map_err(|_| VaultError::File(format!("{FILE} has an invalid salt.")))?;
        let bytes = derive(password, &salt, Self::kdf_of(&file))?;
        match open(&bytes, "check", &file.check) {
            Some(text) if text.as_slice() == CHECK_TEXT => Ok(Key {
                bytes,
                salt: file.kdf.salt,
            }),
            _ => Err(VaultError::WrongPassword),
        }
    }

    /// Destranca com uma chave já derivada ([`Self::derive_key`]).
    pub fn unlock_with(&mut self, key: Key) {
        self.key = Some(key);
    }

    /// Deriva e destranca de uma vez (testes e quem pode esperar).
    pub fn unlock(&mut self, password: &str) -> Result<(), VaultError> {
        let key = self.derive_key(password)?;
        self.key = Some(key);
        Ok(())
    }

    /// Esquece a chave.
    pub fn lock(&mut self) {
        self.key = None;
    }

    /// A chave e o arquivo atual, conferindo que a chave ainda vale.
    fn unlocked(&mut self) -> Result<(Key, VaultFile), VaultError> {
        let file = self.read()?;
        match &self.key {
            Some(k) if k.salt == file.kdf.salt => Ok((k.clone(), file)),
            Some(_) => {
                // A senha mestra mudou em outra máquina.
                self.key = None;
                Err(VaultError::Locked)
            }
            None => Err(VaultError::Locked),
        }
    }

    /// Nomes das senhas guardadas (não precisa destrancar).
    pub fn names(&self) -> Vec<String> {
        self.read()
            .map(|f| f.secrets.into_keys().collect())
            .unwrap_or_default()
    }

    /// A senha guardada com este nome.
    pub fn get(&mut self, name: &str) -> Result<Option<String>, VaultError> {
        let (key, file) = self.unlocked()?;
        let Some(sealed) = file.secrets.get(name) else {
            return Ok(None);
        };
        let plain = open(&key.bytes, name, sealed).ok_or(VaultError::Tampered(name.into()))?;
        String::from_utf8(plain.to_vec())
            .map(Some)
            .map_err(|_| VaultError::Tampered(name.into()))
    }

    /// Guarda (ou troca) a senha com este nome.
    pub fn set(&mut self, name: &str, secret: &str) -> Result<(), VaultError> {
        let (key, mut file) = self.unlocked()?;
        // A mesma senha de novo (conectar com "Remember password" grava a
        // cada conexão): cifrar outra vez mudaria o arquivo, por causa do
        // nonce novo, e o git mostraria uma alteração que não existe.
        let same = file
            .secrets
            .get(name)
            .and_then(|sealed| open(&key.bytes, name, sealed))
            .is_some_and(|old| old.as_slice() == secret.as_bytes());
        if same {
            return Ok(());
        }
        file.secrets
            .insert(name.into(), seal(&key.bytes, name, secret.as_bytes()));
        self.write(&file)
    }

    /// Esquece a senha com este nome. Esquecer o que não existe não é erro.
    pub fn delete(&mut self, name: &str) -> Result<(), VaultError> {
        let mut file = self.read()?;
        if file.secrets.remove(name).is_some() {
            self.write(&file)?;
        }
        Ok(())
    }

    /// Troca a senha mestra: decifra tudo com a chave atual e cifra de novo
    /// com sal e chave novos.
    pub fn change_password(&mut self, new: &str, kdf: Kdf) -> Result<(), VaultError> {
        let (key, file) = self.unlocked()?;
        let mut plain = BTreeMap::new();
        for (name, sealed) in &file.secrets {
            let text = open(&key.bytes, name, sealed).ok_or(VaultError::Tampered(name.clone()))?;
            plain.insert(name.clone(), text);
        }
        self.write_new(new, kdf, plain)
    }

    /// Identifica o cofre (o sal), para lembrar a senha mestra nesta
    /// máquina.
    pub fn id(&self) -> Option<String> {
        self.read().ok().map(|f| f.kdf.salt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Barato, para os testes não demorarem.
    const FAST: Kdf = Kdf {
        m_cost: 8 * 1024,
        t_cost: 1,
        p_cost: 1,
    };

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("postpilot-vault-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_and_wrong_password() {
        let dir = temp_dir("round");
        let mut v = Vault::at(&dir);
        assert!(!v.exists());
        v.create("senha-mestra-1", FAST).unwrap();
        assert!(v.is_unlocked());
        v.set("alice@db:5432", "s3cr3t!").unwrap();
        v.set("app@vendas:5432", "outra").unwrap();
        assert_eq!(v.get("alice@db:5432").unwrap().as_deref(), Some("s3cr3t!"));
        assert_eq!(v.get("nada@x:1").unwrap(), None);

        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(!text.contains("s3cr3t!"));
        assert!(!text.contains("senha-mestra-1"));
        // Uma linha por senha, em ordem.
        let a = text.find("alice@db:5432").unwrap();
        let b = text.find("app@vendas:5432").unwrap();
        assert!(a < b);

        let mut other = Vault::at(&dir);
        assert!(!other.is_unlocked());
        assert_eq!(other.get("alice@db:5432"), Err(VaultError::Locked));
        assert_eq!(
            other.unlock("errada-errada"),
            Err(VaultError::WrongPassword)
        );
        other.unlock("senha-mestra-1").unwrap();
        assert_eq!(
            other.get("app@vendas:5432").unwrap().as_deref(),
            Some("outra")
        );
        other.delete("app@vendas:5432").unwrap();
        other.delete("app@vendas:5432").unwrap();
        assert_eq!(other.names(), ["alice@db:5432"]);
        other.lock();
        assert!(!other.is_unlocked());
        assert!(v.create("x".repeat(12).as_str(), FAST).is_err());
    }

    #[test]
    fn saving_the_same_password_again_leaves_the_file_alone() {
        let dir = temp_dir("same");
        let mut v = Vault::at(&dir);
        v.create("uma senha mestra", FAST).unwrap();
        v.set("alice@db:5432", "segredo").unwrap();
        let before = std::fs::read_to_string(v.path()).unwrap();
        v.set("alice@db:5432", "segredo").unwrap();
        assert_eq!(std::fs::read_to_string(v.path()).unwrap(), before);
        v.set("alice@db:5432", "outro").unwrap();
        assert_ne!(std::fs::read_to_string(v.path()).unwrap(), before);
        assert_eq!(v.get("alice@db:5432").unwrap().as_deref(), Some("outro"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tampering_is_detected() {
        let dir = temp_dir("tamper");
        let mut v = Vault::at(&dir);
        v.create("senha-mestra-1", FAST).unwrap();
        v.set("a@h:1", "um").unwrap();
        v.set("b@h:1", "dois").unwrap();
        let path = dir.join(FILE);
        let mut file: VaultFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Trocar as linhas de lugar não engana: o nome entra na cifra.
        let a = file.secrets["a@h:1"].clone();
        let b = file.secrets["b@h:1"].clone();
        file.secrets.insert("a@h:1".into(), b);
        file.secrets.insert("b@h:1".into(), a.clone());
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!(v.get("a@h:1"), Err(VaultError::Tampered("a@h:1".into())));
        // Um byte a mais também não.
        let mut raw = B64.decode(&a).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        file.secrets.insert("a@h:1".into(), B64.encode(raw));
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
        assert!(matches!(v.get("a@h:1"), Err(VaultError::Tampered(_))));
        // Custos enfraquecidos são recusados.
        file.kdf.m_cost = 16;
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
        assert!(matches!(
            Vault::at(&dir).unlock("senha-mestra-1"),
            Err(VaultError::File(_))
        ));
    }

    #[test]
    fn change_password_on_another_machine_locks_this_one() {
        let dir = temp_dir("change");
        let mut a = Vault::at(&dir);
        a.create("senha-mestra-1", FAST).unwrap();
        a.set("x@h:1", "segredo").unwrap();
        let mut b = Vault::at(&dir);
        b.unlock("senha-mestra-1").unwrap();
        b.change_password("senha-mestra-2", FAST).unwrap();
        assert_eq!(b.get("x@h:1").unwrap().as_deref(), Some("segredo"));
        // A outra cópia percebe que o sal mudou.
        assert!(!a.is_unlocked());
        assert_eq!(a.get("x@h:1"), Err(VaultError::Locked));
        assert_eq!(a.unlock("senha-mestra-1"), Err(VaultError::WrongPassword));
        a.unlock("senha-mestra-2").unwrap();
        assert_eq!(a.get("x@h:1").unwrap().as_deref(), Some("segredo"));
    }

    #[test]
    fn new_password_rules() {
        assert!(check_new_password("curta", "curta").is_err());
        assert!(check_new_password("0123456789", "0123456789x").is_err());
        assert!(check_new_password("0123456789", "0123456789").is_ok());
    }

    #[test]
    fn default_cost_is_strong() {
        const { assert!(DEFAULT_KDF.m_cost >= 19 * 1024) };
        const { assert!(DEFAULT_KDF.t_cost >= 2) };
        // Derivar com o custo padrão funciona (e é o que o app usa).
        let key = derive("senha-mestra-1", &[7u8; 16], DEFAULT_KDF).unwrap();
        assert_ne!(*key, [0u8; 32]);
    }
}
