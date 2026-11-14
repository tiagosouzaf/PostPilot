//! Senhas no workspace: a ponte entre o "Remember password" e o cofre
//! criptografado (`secrets.json`), e a linha "Passwords" do Workspace
//! Explorer com os diálogos da senha mestra.
//!
//! Com o cofre destrancado, a senha lembrada de um servidor que está no
//! workspace vai para o cofre; a dos outros servidores continua no cofre do
//! sistema. Na leitura, o cofre do workspace vem primeiro.
//!
//! A senha mestra pode ser lembrada nesta máquina, no cofre do sistema
//! (nunca no repositório): aí o workspace destranca sozinho ao abrir.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use eframe::egui;
use egui_phosphor::regular as icon;

use super::vault::{self, DEFAULT_KDF, Kdf, Key, Vault, VaultError};
use crate::secrets::{SecretError, SecretStore};

/// O cofre do workspace aberto e as chaves dos servidores dele.
#[derive(Debug, Default)]
pub struct VaultSlot {
    pub vault: Option<Vault>,
    /// `usuario@host:porta` dos servidores do workspace.
    pub keys: HashSet<String>,
}

/// O cofre do workspace, dividido entre a interface e as threads que leem
/// senhas.
pub type SharedVault = Arc<Mutex<VaultSlot>>;

/// O cofre que a casca usa para "Remember password": o do workspace (para
/// os servidores dele, destrancado) e o do sistema.
pub struct WorkspaceSecrets {
    system: Option<Arc<dyn SecretStore>>,
    slot: SharedVault,
}

impl WorkspaceSecrets {
    pub fn new(system: Option<Arc<dyn SecretStore>>, slot: SharedVault) -> Self {
        WorkspaceSecrets { system, slot }
    }
}

fn no_system() -> SecretError {
    SecretError("No system keyring.".into())
}

impl SecretStore for WorkspaceSecrets {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        {
            let mut slot = self.slot.lock().expect("vault lock");
            if let Some(v) = slot.vault.as_mut().filter(|v| v.is_unlocked()) {
                match v.get(key) {
                    Ok(Some(s)) => return Ok(Some(s)),
                    Ok(None) | Err(VaultError::Locked) => {}
                    Err(e) => eprintln!("workspace passwords: {e}"),
                }
            }
        }
        match &self.system {
            Some(s) => s.get(key),
            None => Err(no_system()),
        }
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        {
            let mut slot = self.slot.lock().expect("vault lock");
            let ours = slot.keys.contains(key);
            if let Some(v) = slot.vault.as_mut().filter(|v| ours && v.is_unlocked()) {
                return v.set(key, secret).map_err(|e| SecretError(e.to_string()));
            }
        }
        match &self.system {
            Some(s) => s.set(key, secret),
            None => Err(no_system()),
        }
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        {
            let mut slot = self.slot.lock().expect("vault lock");
            let ours = slot.keys.contains(key);
            if let Some(v) = slot.vault.as_mut().filter(|v| ours && v.exists()) {
                v.delete(key).map_err(|e| SecretError(e.to_string()))?;
            }
        }
        match &self.system {
            Some(s) => s.delete(key),
            None => Ok(()),
        }
    }
}

/// Nome com que a senha mestra fica no cofre do sistema.
pub fn remembered_key(vault_id: &str) -> String {
    format!("workspace-vault:{vault_id}")
}

/// Qual diálogo da senha mestra está aberto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    /// "Set Master Password": cria o cofre.
    Create,
    /// "Unlock Workspace Passwords".
    Unlock,
    /// "Change Master Password".
    Change,
}

struct Dialog {
    kind: DialogKind,
    password: String,
    confirm: String,
    remember: bool,
    error: Option<String>,
}

enum Done {
    /// Cofre criado ou senha trocada (já destrancado).
    Vault(Result<Vault, VaultError>, &'static str),
    /// Chave derivada para destrancar.
    Key(Result<Key, VaultError>),
}

/// A linha "Passwords" do Workspace Explorer.
pub struct Passwords {
    root: PathBuf,
    slot: SharedVault,
    job: Option<Receiver<Done>>,
    dialog: Option<Dialog>,
    message: Option<String>,
    /// Senha mestra a lembrar no cofre do sistema quando o trabalho acabar.
    remember: Option<String>,
    /// Custo da derivação (os testes usam um menor).
    kdf: Kdf,
}

impl Passwords {
    /// Põe o cofre do workspace em `slot`.
    pub fn new(root: &Path, slot: SharedVault) -> Self {
        slot.lock().expect("vault lock").vault = Some(Vault::at(root));
        Passwords {
            root: root.to_path_buf(),
            slot,
            job: None,
            dialog: None,
            message: None,
            remember: None,
            kdf: DEFAULT_KDF,
        }
    }

    /// Usa outro custo de derivação para cofres novos (testes).
    pub fn set_kdf(&mut self, kdf: Kdf) {
        self.kdf = kdf;
    }

    /// Se o workspace já tem cofre.
    pub fn exists(&self) -> bool {
        self.slot
            .lock()
            .expect("vault lock")
            .vault
            .as_ref()
            .is_some_and(Vault::exists)
    }

    /// Se a senha mestra foi digitada.
    pub fn is_unlocked(&self) -> bool {
        self.slot
            .lock()
            .expect("vault lock")
            .vault
            .as_ref()
            .is_some_and(Vault::is_unlocked)
    }

    /// Quantas senhas o cofre guarda.
    pub fn count(&self) -> usize {
        self.slot
            .lock()
            .expect("vault lock")
            .vault
            .as_ref()
            .map_or(0, |v| v.names().len())
    }

    /// Se a senha mestra está sendo conferida.
    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    /// Mensagem para a status bar, se houver.
    pub fn take_message(&mut self) -> Option<String> {
        self.message.take()
    }

    /// O diálogo aberto, se algum.
    pub fn dialog(&self) -> Option<DialogKind> {
        self.dialog.as_ref().map(|d| d.kind)
    }

    /// Abre um diálogo da senha mestra.
    pub fn open_dialog(&mut self, kind: DialogKind) {
        self.dialog = Some(Dialog {
            kind,
            password: String::new(),
            confirm: String::new(),
            remember: true,
            error: None,
        });
    }

    /// Ao abrir o workspace: destranca sozinho se a senha mestra está
    /// lembrada nesta máquina; se não, abre "Unlock".
    pub fn auto_unlock(&mut self, store: Option<&Arc<dyn SecretStore>>) {
        if !self.exists() || self.is_unlocked() {
            return;
        }
        let id = self
            .slot
            .lock()
            .expect("vault lock")
            .vault
            .as_ref()
            .and_then(Vault::id);
        let remembered = match (store, id) {
            (Some(s), Some(id)) => s.get(&remembered_key(&id)).ok().flatten(),
            _ => None,
        };
        match remembered {
            Some(password) => self.start_unlock(password, false),
            None => self.open_dialog(DialogKind::Unlock),
        }
    }

    fn spawn(&mut self, work: impl FnOnce() -> Done + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("postpilot-vault".into())
            .spawn(move || {
                let _ = tx.send(work());
            })
            .expect("spawn vault thread");
        self.job = Some(rx);
    }

    fn start_unlock(&mut self, password: String, remember: bool) {
        let root = self.root.clone();
        if remember {
            self.remember = Some(password.clone());
        }
        self.spawn(move || Done::Key(Vault::at(&root).derive_key(&password)));
    }

    /// Cria o cofre com esta senha mestra.
    pub fn create(&mut self, password: &str, confirm: &str, remember: bool) -> Result<(), String> {
        vault::check_new_password(password, confirm).map_err(|e| e.to_string())?;
        let (root, kdf, password) = (self.root.clone(), self.kdf, password.to_string());
        if remember {
            self.remember = Some(password.clone());
        }
        self.spawn(move || {
            let mut v = Vault::at(&root);
            Done::Vault(v.create(&password, kdf).map(|()| v), "Created")
        });
        Ok(())
    }

    /// Destranca com a senha mestra.
    pub fn unlock(&mut self, password: &str, remember: bool) {
        self.start_unlock(password.to_string(), remember);
    }

    /// Troca a senha mestra (o cofre precisa estar destrancado).
    pub fn change(&mut self, password: &str, confirm: &str, remember: bool) -> Result<(), String> {
        vault::check_new_password(password, confirm).map_err(|e| e.to_string())?;
        let current = {
            let slot = self.slot.lock().expect("vault lock");
            match slot.vault.as_ref() {
                Some(v) if v.is_unlocked() => v.clone(),
                _ => return Err(VaultError::Locked.to_string()),
            }
        };
        let (kdf, password) = (self.kdf, password.to_string());
        if remember {
            self.remember = Some(password.clone());
        }
        self.spawn(move || {
            let mut v = current;
            Done::Vault(v.change_password(&password, kdf).map(|()| v), "Changed")
        });
        Ok(())
    }

    /// Tranca: a chave sai da memória.
    pub fn lock(&mut self) {
        if let Some(v) = self.slot.lock().expect("vault lock").vault.as_mut() {
            v.lock();
        }
        self.message = Some("Workspace passwords locked.".into());
    }

    /// Trata o fim do trabalho em andamento; `true` quando algo terminou.
    pub fn poll(&mut self, store: Option<&Arc<dyn SecretStore>>) -> bool {
        let done = match self.job.as_ref().map(Receiver::try_recv) {
            Some(Ok(done)) => done,
            Some(Err(TryRecvError::Disconnected)) => {
                self.job = None;
                self.message = Some("The master password check stopped unexpectedly.".into());
                return true;
            }
            _ => return false,
        };
        self.job = None;
        self.apply(done, store);
        true
    }

    /// Espera o trabalho em andamento (testes).
    pub fn wait(&mut self, store: Option<&Arc<dyn SecretStore>>) {
        if let Some(rx) = self.job.take()
            && let Ok(done) = rx.recv()
        {
            self.apply(done, store);
        }
    }

    fn apply(&mut self, done: Done, store: Option<&Arc<dyn SecretStore>>) {
        let remember = self.remember.take();
        let ok = match done {
            Done::Vault(Ok(v), what) => {
                self.message = Some(match what {
                    "Created" => "Workspace passwords are on: remembered passwords of this workspace's servers are saved encrypted in secrets.json.".into(),
                    _ => "Master password changed. Commit and push secrets.json.".into(),
                });
                self.slot.lock().expect("vault lock").vault = Some(v);
                true
            }
            Done::Key(Ok(key)) => {
                if let Some(v) = self.slot.lock().expect("vault lock").vault.as_mut() {
                    v.unlock_with(key);
                }
                self.message = Some("Workspace passwords unlocked.".into());
                true
            }
            Done::Vault(Err(e), _) | Done::Key(Err(e)) => {
                let text = e.to_string();
                match &mut self.dialog {
                    Some(d) => d.error = Some(text.clone()),
                    None if e == VaultError::WrongPassword => {
                        // A senha lembrada nesta máquina ficou velha.
                        self.open_dialog(DialogKind::Unlock);
                    }
                    None => {}
                }
                self.message = Some(text);
                false
            }
        };
        if ok {
            self.dialog = None;
            if let (Some(password), Some(store)) = (remember, store) {
                let id = self
                    .slot
                    .lock()
                    .expect("vault lock")
                    .vault
                    .as_ref()
                    .and_then(Vault::id);
                if let Some(id) = id
                    && let Err(e) = store.set(&remembered_key(&id), &password)
                {
                    self.message = Some(format!(
                        "Unlocked, but the master password could not be remembered on this computer: {e}"
                    ));
                }
            }
        }
    }

    /// Desenha a linha "Passwords".
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Checking the master password...");
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        let (exists, unlocked, count) = (self.exists(), self.is_unlocked(), self.count());
        ui.horizontal(|ui| {
            if !exists {
                ui.weak(format!("{} Passwords stay on this computer.", icon::KEY));
                if ui
                    .small_button("Save in workspace...")
                    .on_hover_text("Set a master password and save the remembered passwords of this workspace's servers, encrypted, in secrets.json")
                    .clicked()
                {
                    self.open_dialog(DialogKind::Create);
                }
            } else if !unlocked {
                ui.label(format!("{} Passwords locked", icon::LOCK));
                if ui.small_button("Unlock...").clicked() {
                    self.open_dialog(DialogKind::Unlock);
                }
            } else {
                let text = match count {
                    0 => "No passwords saved yet".to_string(),
                    1 => "1 password in the workspace".to_string(),
                    n => format!("{n} passwords in the workspace"),
                };
                ui.label(format!("{} {text}", icon::LOCK_OPEN))
                    .on_hover_text("Remembered passwords of this workspace's servers are saved here, encrypted with the master password.");
                ui.menu_button(icon::DOTS_THREE, |ui| {
                    if ui.button("Change Master Password...").clicked() {
                        self.open_dialog(DialogKind::Change);
                        ui.close();
                    }
                    if ui.button("Lock").clicked() {
                        self.lock();
                        ui.close();
                    }
                });
            }
        });
    }

    /// Desenha o diálogo aberto. Devolve uma mensagem para a status bar.
    pub fn show_windows(
        &mut self,
        ctx: &egui::Context,
        store: Option<&Arc<dyn SecretStore>>,
    ) -> Option<String> {
        if self.poll(store) {
            ctx.request_repaint();
        }
        let busy = self.job.is_some();
        let mut ok = false;
        let mut cancel = false;
        if let Some(d) = &mut self.dialog {
            let title = match d.kind {
                DialogKind::Create => "Save Passwords in Workspace",
                DialogKind::Unlock => "Unlock Workspace Passwords",
                DialogKind::Change => "Change Master Password",
            };
            crate::theme::modal(egui::Id::new("workspace_vault")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.heading(title);
                match d.kind {
                    DialogKind::Create => {
                        ui.label("Choose a master password. It encrypts the passwords that go into the repository (secrets.json); it is never saved there.");
                        ui.weak(format!("At least {} characters. If you forget it, the saved passwords cannot be recovered: you type them again.", vault::MIN_PASSWORD));
                    }
                    DialogKind::Unlock => {
                        ui.label("This workspace keeps its passwords encrypted. Enter the master password.");
                    }
                    DialogKind::Change => {
                        ui.label("Enter the new master password. The other computers will ask for it after the next pull.");
                    }
                }
                ui.add_space(4.0);
                egui::Grid::new("vault_fields").num_columns(2).show(ui, |ui| {
                    ui.label(if d.kind == DialogKind::Unlock {
                        "Master password:"
                    } else {
                        "New master password:"
                    });
                    let first = ui.add(
                        egui::TextEdit::singleline(&mut d.password)
                            .password(true)
                            .desired_width(220.0),
                    );
                    ui.end_row();
                    let mut enter = first.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if d.kind != DialogKind::Unlock {
                        ui.label("Confirm password:");
                        let second = ui.add(
                            egui::TextEdit::singleline(&mut d.confirm)
                                .password(true)
                                .desired_width(220.0),
                        );
                        ui.end_row();
                        enter = second.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    }
                    if enter {
                        ok = true;
                    }
                });
                ui.checkbox(&mut d.remember, "Remember on this computer")
                    .on_hover_text("Keeps the master password in this computer's keyring, so the workspace unlocks by itself.");
                if let Some(e) = &d.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.add_enabled(!busy, egui::Button::new("OK")).clicked() {
                        ok = true;
                    }
                    if busy {
                        ui.spinner();
                    }
                });
            });
        }
        if cancel {
            self.dialog = None;
        } else if ok
            && !busy
            && let Some(d) = &self.dialog
        {
            let (kind, password, confirm, remember) =
                (d.kind, d.password.clone(), d.confirm.clone(), d.remember);
            let result = match kind {
                DialogKind::Create => self.create(&password, &confirm, remember),
                DialogKind::Unlock => {
                    self.unlock(&password, remember);
                    Ok(())
                }
                DialogKind::Change => self.change(&password, &confirm, remember),
            };
            if let (Err(e), Some(d)) = (result, &mut self.dialog) {
                d.error = Some(e);
            } else if let Some(d) = &mut self.dialog {
                d.error = None;
            }
        }
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.message.take()
    }
}
