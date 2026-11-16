//! Workspace Explorer: o painel dos workspaces, como o Solution Explorer do
//! SSMS. Vários workspaces ficam abertos ao mesmo tempo (um por cliente ou
//! por time, por exemplo), cada um com o seu git e o seu cofre de senhas.
//! Em cima, um cartão por workspace, com o estado do repositório em uma
//! frase; o cartão do workspace ativo tem os botões dele. Embaixo, os
//! servidores do workspace ativo (a mesma árvore do Registered Servers,
//! gravada em `servers.json`) e as consultas de `queries/`.

use pgstudio_i18n::{tr, tr_noop, trf};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use egui_phosphor::regular as icon;

use super::git::{Done, Git, GitError, Job};
use super::passwords::{Passwords, SharedVault, SharedVaults, WorkspaceSecrets};
use super::search::{self, Hits, Search};
use super::source_control::{SourceControl, SyncKind};
use super::{Folder, Workspace};
use crate::registered::{self, RegisteredPanel};
use crate::secrets::SecretStore;

/// O que o usuário pediu no Workspace Explorer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// File > New > Workspace...
    New,
    /// File > Open > Workspace...
    Open,
    /// Abrir uma consulta (duplo clique ou "Open").
    OpenQuery(PathBuf),
    /// Abrir uma consulta achada pela pesquisa, na linha onde o termo
    /// está.
    OpenQueryAt {
        path: PathBuf,
        /// Linha, a partir de 1.
        line: usize,
        /// O termo a selecionar nela.
        term: String,
    },
    /// "New Query" numa pasta: criar o arquivo nela e abrir.
    NewQuery(PathBuf),
    /// Pedido da árvore de servidores do workspace.
    Servers(registered::Action),
    /// Trazer os Registered Servers desta máquina (e as senhas lembradas)
    /// para o workspace.
    ImportRegistered,
    /// Pôr no workspace o servidor conectado agora (com a senha em uso).
    AddCurrent,
    /// "Use a folder on this computer": escolher a pasta; ela abre se já
    /// é um workspace, senão vira um.
    AddFolder,
    /// "Choose..." do destino do clone: escolher a pasta.
    BrowseCloneFolder,
}

/// Senhas a copiar para o cofre do workspace assim que ele for destrancado.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingCopy {
    /// Chaves (`usuario@host:porta`) a procurar no cofre do sistema.
    pub keys: Vec<String>,
    /// Senhas já em mãos (a da conexão ativa): chave e senha.
    pub known: Vec<(String, String)>,
}

impl PendingCopy {
    fn merge(&mut self, other: PendingCopy) {
        for key in other.keys {
            if !self.keys.contains(&key) {
                self.keys.push(key);
            }
        }
        for (key, secret) in other.known {
            self.known.retain(|(k, _)| *k != key);
            self.known.push((key, secret));
        }
    }
}

struct Opened {
    workspace: Workspace,
    servers: RegisteredPanel,
    queries: Folder,
    /// Pesquisa nas consultas, pelo nome e pelo conteúdo.
    search: Search,
    /// Pastas recolhidas (as demais aparecem abertas).
    collapsed: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    git: SourceControl,
    passwords: Passwords,
    /// Cofre de senhas deste workspace, dividido com o "Remember
    /// password".
    vault: SharedVault,
    /// Senhas esperando o cofre ser criado ou destrancado.
    pending_copy: Option<PendingCopy>,
}

impl Opened {
    fn new(workspace: Workspace) -> Opened {
        let vault = SharedVault::default();
        Opened {
            servers: RegisteredPanel::new(workspace.servers()),
            queries: workspace.queries(),
            search: Search::default(),
            collapsed: HashSet::new(),
            selected: None,
            git: SourceControl::new(workspace.root()),
            passwords: Passwords::new(workspace.root(), vault.clone()),
            vault,
            pending_copy: None,
            workspace,
        }
    }

    /// Lê de novo as consultas e os servidores do disco.
    fn reload_files(&mut self) {
        self.queries = self.workspace.queries();
        self.search.invalidate();
        if !self.servers.has_dialog() {
            let current = self.servers.current.take();
            let collapsed = self.servers.collapsed_names();
            self.servers = RegisteredPanel::new(self.workspace.servers());
            self.servers.collapse_names(&collapsed);
            self.servers.current = current;
        }
    }

    /// Chaves dos servidores do workspace: o "Remember password" deles vai
    /// para o cofre do workspace.
    fn sync_vault_keys(&self) {
        let keys = self
            .servers
            .servers()
            .all_servers()
            .into_iter()
            .flat_map(|s| {
                // Com túnel SSH, o segredo dele também é do workspace.
                let ssh = s.settings.ssh.as_ref().map(|ssh| ssh.secret_key());
                std::iter::once(crate::secrets::password_key(&s.settings)).chain(ssh)
            })
            .collect();
        self.vault.lock().expect("vault lock").keys = keys;
    }
}

/// "Add Workspace": clonar um repositório (URL e pasta de destino) ou usar
/// uma pasta deste computador.
struct AddDialog {
    url: String,
    /// Pasta onde o clone fica (dentro dela, uma pasta com o nome do
    /// repositório).
    dest: String,
    error: Option<String>,
}

/// Onde os clones ficam quando o usuário não escolhe: `~/PostPilot`.
fn default_clone_folder() -> String {
    std::env::home_dir()
        .map(|h| h.join("PostPilot").display().to_string())
        .unwrap_or_default()
}

/// Um workspace aberto, para quem lista (menus, o Object Explorer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub name: String,
    pub root: PathBuf,
    pub active: bool,
}

/// "New Folder" esperando o nome.
struct NewFolder {
    parent: PathBuf,
    name: String,
    error: Option<String>,
}

/// O painel Workspace Explorer.
#[derive(Default)]
pub struct WorkspacePanel {
    /// Os workspaces abertos, na ordem em que aparecem.
    workspaces: Vec<Opened>,
    /// Índice do workspace ativo: o que o painel mostra por inteiro e o
    /// destino do que é registrado ou importado.
    active: usize,
    /// Os cofres dos workspaces abertos, para o "Remember password".
    vaults: SharedVaults,
    new_folder: Option<NewFolder>,
    message: Option<String>,
    add: Option<AddDialog>,
    clone_job: Option<Job>,
    /// "Clone" confirmado: a URL e a pasta onde o clone fica.
    pending_clone: Option<(String, PathBuf)>,
    /// O workspace ativo mudou (a casca grava a lista).
    changed: bool,
    /// Pedidos de "Add Workspace", que é desenhado fora do painel.
    add_actions: Vec<Action>,
    /// Cópia das senhas do cofre do sistema para o do workspace, rodando.
    copying: Option<std::thread::JoinHandle<Result<usize, String>>>,
    /// "Trazer as conexões deste computador?", com quantas são, logo depois
    /// de criar o workspace.
    offer_import: Option<usize>,
    /// Quantas conexões deste computador ainda não estão no workspace (a
    /// casca atualiza a cada frame).
    pub importable: usize,
    /// Nome do servidor conectado agora, quando ele não está no workspace.
    pub unsaved_current: Option<String>,
    /// "Import connections" foi escolhido na pergunta acima.
    accepted_import: bool,
}

impl WorkspacePanel {
    fn opened(&self) -> Option<&Opened> {
        self.workspaces.get(self.active)
    }

    fn opened_mut(&mut self) -> Option<&mut Opened> {
        self.workspaces.get_mut(self.active)
    }

    fn sync_vaults(&self) {
        *self.vaults.lock().expect("vaults lock") =
            self.workspaces.iter().map(|o| o.vault.clone()).collect();
    }

    /// Abre este workspace ao lado dos que já estão abertos e o torna o
    /// ativo. Um que já está aberto só vem para a frente.
    pub fn open(&mut self, workspace: Workspace) {
        self.new_folder = None;
        if let Some(i) = self.index_of_root(workspace.root()) {
            self.activate(i);
            return;
        }
        self.workspaces.push(Opened::new(workspace));
        self.active = self.workspaces.len() - 1;
        self.sync_vaults();
        self.changed = true;
    }

    /// Fecha o workspace ativo. Os arquivos continuam na pasta.
    pub fn close(&mut self) {
        self.close_at(self.active);
    }

    /// Fecha o workspace de índice `index`.
    pub fn close_at(&mut self, index: usize) {
        if index >= self.workspaces.len() {
            return;
        }
        let was_active = index == self.active;
        self.workspaces.remove(index);
        if index < self.active || self.active >= self.workspaces.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.sync_vaults();
        self.changed = true;
        if was_active {
            self.new_folder = None;
            self.offer_import = None;
        }
    }

    /// Torna ativo o workspace de índice `index`.
    pub fn activate(&mut self, index: usize) {
        if index < self.workspaces.len() && index != self.active {
            self.active = index;
            self.new_folder = None;
            self.offer_import = None;
            self.changed = true;
        }
    }

    /// Se a lista ou o workspace ativo mudou desde a última pergunta.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Quantos workspaces estão abertos.
    pub fn count(&self) -> usize {
        self.workspaces.len()
    }

    /// Índice do workspace ativo.
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// Os workspaces abertos, na ordem.
    pub fn list(&self) -> Vec<WorkspaceInfo> {
        self.workspaces
            .iter()
            .enumerate()
            .map(|(i, o)| WorkspaceInfo {
                name: o.workspace.name().to_string(),
                root: o.workspace.root().to_path_buf(),
                active: i == self.active,
            })
            .collect()
    }

    /// Índice do workspace desta pasta.
    pub fn index_of_root(&self, root: &Path) -> Option<usize> {
        let same = |a: &Path| {
            a == root
                || matches!(
                    (std::fs::canonicalize(a), std::fs::canonicalize(root)),
                    (Ok(a), Ok(b)) if a == b
                )
        };
        self.workspaces
            .iter()
            .position(|o| same(o.workspace.root()))
    }

    /// O workspace aberto que tem `path` dentro da pasta dele.
    pub fn workspace_of(&self, path: &Path) -> Option<&Workspace> {
        self.index_of_path(path)
            .map(|i| &self.workspaces[i].workspace)
    }

    /// Índice do workspace que tem `path` dentro (o ativo primeiro).
    pub fn index_of_path(&self, path: &Path) -> Option<usize> {
        if self.opened().is_some_and(|o| o.workspace.contains(path)) {
            return Some(self.active);
        }
        self.workspaces
            .iter()
            .position(|o| o.workspace.contains(path))
    }

    /// Os servidores do workspace de índice `index`.
    pub fn servers_at(&self, index: usize) -> Option<&RegisteredPanel> {
        self.workspaces.get(index).map(|o| &o.servers)
    }

    /// Os servidores de todos os workspaces abertos, o ativo primeiro, com
    /// o nome do workspace.
    pub fn all_servers(&self) -> Vec<(&str, &RegisteredPanel)> {
        let rest = self
            .workspaces
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.active)
            .map(|(_, o)| o);
        self.opened()
            .into_iter()
            .chain(rest)
            .map(|o| (o.workspace.name(), &o.servers))
            .collect()
    }

    /// O git do workspace de índice `index`.
    pub fn source_control_at(&self, index: usize) -> Option<&SourceControl> {
        self.workspaces.get(index).map(|o| &o.git)
    }

    /// O git do workspace de índice `index`, mutável.
    pub fn source_control_at_mut(&mut self, index: usize) -> Option<&mut SourceControl> {
        self.workspaces.get_mut(index).map(|o| &mut o.git)
    }

    /// As senhas do workspace de índice `index`, mutável.
    pub fn passwords_at_mut(&mut self, index: usize) -> Option<&mut Passwords> {
        self.workspaces.get_mut(index).map(|o| &mut o.passwords)
    }

    /// Quantos cofres de workspace estão destrancados.
    pub fn unlocked_count(&self) -> usize {
        self.workspaces
            .iter()
            .filter(|o| o.passwords.is_unlocked())
            .count()
    }

    /// Se algum cofre está conferindo a senha mestra agora.
    pub fn is_unlocking(&self) -> bool {
        self.workspaces.iter().any(|o| o.passwords.is_busy())
    }

    /// As pastas recolhidas de cada workspace aberto (para a sessão): a
    /// pasta dele, as pastas de consultas e as de servidores.
    pub fn trees(&self) -> Vec<(PathBuf, Vec<PathBuf>, Vec<Vec<String>>)> {
        self.workspaces
            .iter()
            .map(|o| {
                let mut folders: Vec<PathBuf> = o.collapsed.iter().cloned().collect();
                folders.sort();
                (
                    o.workspace.root().to_path_buf(),
                    folders,
                    o.servers.collapsed_names(),
                )
            })
            .collect()
    }

    /// Recolhe estas pastas no workspace de pasta `root`, se ele está
    /// aberto.
    pub fn collapse_tree(&mut self, root: &Path, folders: &[PathBuf], servers: &[Vec<String>]) {
        if let Some(i) = self.index_of_root(root) {
            let o = &mut self.workspaces[i];
            o.collapsed.extend(folders.iter().cloned());
            o.servers.collapse_names(servers);
        }
    }

    /// Pergunta se as `count` conexões deste computador devem vir para o
    /// workspace (logo depois de criá-lo).
    pub fn offer_import(&mut self, count: usize) {
        self.offer_import = (count > 0).then_some(count);
    }

    /// Quantas conexões a pergunta "trazer as conexões?" está oferecendo.
    pub fn import_offer(&self) -> Option<usize> {
        self.offer_import
    }

    /// Responde à pergunta "trazer as conexões?" (o botão e os testes).
    pub fn answer_import_offer(&mut self, import: bool) {
        if self.offer_import.take().is_some() {
            self.accepted_import = import;
        }
    }

    /// Se "Import connections" acabou de ser escolhido.
    pub fn take_accepted_import(&mut self) -> bool {
        std::mem::take(&mut self.accepted_import)
    }

    /// Guarda senhas para copiar quando o cofre for destrancado.
    pub fn copy_when_unlocked(&mut self, pending: PendingCopy) {
        if let Some(o) = self.opened_mut() {
            o.pending_copy
                .get_or_insert_with(PendingCopy::default)
                .merge(pending);
        }
    }

    /// Se há senhas esperando o cofre.
    pub fn has_pending_copy(&self) -> bool {
        self.opened().is_some_and(|o| o.pending_copy.is_some())
    }

    /// As senhas que esperavam o cofre, se ele já está destrancado e
    /// nenhuma cópia está rodando.
    pub fn take_pending_copy(&mut self) -> Option<PendingCopy> {
        if self.copying.is_some() {
            return None;
        }
        let o = self.opened_mut().filter(|o| o.passwords.is_unlocked())?;
        o.pending_copy.take()
    }

    /// As pastas de consultas recolhidas do workspace aberto, em ordem
    /// (para a sessão).
    pub fn collapsed_folders(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .opened()
            .into_iter()
            .flat_map(|o| o.collapsed.iter().cloned())
            .collect();
        out.sort();
        out
    }

    /// Recolhe estas pastas de consultas.
    pub fn collapse_folders(&mut self, folders: &[PathBuf]) {
        if let Some(o) = self.opened_mut() {
            o.collapsed.extend(folders.iter().cloned());
        }
    }

    /// O workspace aberto.
    pub fn workspace(&self) -> Option<&Workspace> {
        self.opened().map(|o| &o.workspace)
    }

    /// A árvore de servidores do workspace.
    pub fn servers(&self) -> Option<&RegisteredPanel> {
        self.opened().map(|o| &o.servers)
    }

    /// A árvore de servidores do workspace, mutável.
    pub fn servers_mut(&mut self) -> Option<&mut RegisteredPanel> {
        self.opened_mut().map(|o| &mut o.servers)
    }

    /// Se a pesquisa nas consultas ainda está lendo os arquivos.
    pub fn is_searching(&self) -> bool {
        self.opened().is_some_and(|o| o.search.is_searching())
    }

    /// As consultas, como lidas da última vez.
    pub fn queries(&self) -> Option<&Folder> {
        self.opened().map(|o| &o.queries)
    }

    /// Lê de novo as consultas e os servidores do disco (depois de salvar,
    /// de um pull do git ou de mudanças fora do PostPilot).
    pub fn refresh(&mut self) {
        if let Some(o) = self.opened_mut() {
            o.git.refresh();
            o.reload_files();
        }
    }

    /// Como [`Self::refresh`], no workspace de índice `index`.
    pub fn refresh_at(&mut self, index: usize) {
        if let Some(o) = self.workspaces.get_mut(index) {
            o.git.refresh();
            o.reload_files();
        }
    }

    /// O cofre do workspace ativo (vazio sem workspace aberto).
    pub fn vault_slot(&self) -> SharedVault {
        self.opened().map(|o| o.vault.clone()).unwrap_or_default()
    }

    /// Os cofres de todos os workspaces abertos, divididos com o "Remember
    /// password".
    pub fn vault_slots(&self) -> SharedVaults {
        self.vaults.clone()
    }

    /// As senhas do workspace.
    pub fn passwords(&self) -> Option<&Passwords> {
        self.opened().map(|o| &o.passwords)
    }

    /// As senhas do workspace, mutável.
    pub fn passwords_mut(&mut self) -> Option<&mut Passwords> {
        self.opened_mut().map(|o| &mut o.passwords)
    }

    /// O git do workspace.
    pub fn source_control(&self) -> Option<&SourceControl> {
        self.opened().map(|o| &o.git)
    }

    /// O git do workspace, mutável.
    pub fn source_control_mut(&mut self) -> Option<&mut SourceControl> {
        self.opened_mut().map(|o| &mut o.git)
    }

    /// Abre "Add Workspace" (clonar um repositório ou usar uma pasta).
    pub fn open_add(&mut self) {
        self.add = Some(AddDialog {
            url: String::new(),
            dest: default_clone_folder(),
            error: None,
        });
    }

    /// Se "Add Workspace" está aberto.
    pub fn is_adding(&self) -> bool {
        self.add.is_some()
    }

    /// Fecha "Add Workspace".
    pub fn close_add(&mut self) {
        self.add = None;
    }

    /// Preenche "Add Workspace" (a pasta escolhida em "Choose..." e os
    /// testes).
    pub fn set_clone_folder(&mut self, folder: &Path) {
        if let Some(d) = &mut self.add {
            d.dest = folder.display().to_string();
        }
    }

    /// A URL do repositório em "Add Workspace" (testes e capturas).
    pub fn set_clone_url(&mut self, url: &str) {
        if let Some(d) = &mut self.add {
            d.url = url.to_string();
        }
    }

    /// Clona `url` em `dest` numa thread separada; o fim chega por
    /// [`Self::take_cloned`].
    pub fn start_clone(&mut self, url: &str, dest: &Path) {
        if self.clone_job.is_some() {
            self.message = Some(tr("Wait for the current clone.").into());
            return;
        }
        let (url, dest) = (url.to_string(), dest.to_path_buf());
        self.clone_job = Some(Job::spawn(tr_noop("Cloning..."), move || {
            Done::Cloned(Git::clone(&url, &dest).map(|g| g.root().to_path_buf()))
        }));
    }

    /// "Clone" confirmado: a URL e a pasta onde o clone fica.
    pub fn take_clone_request(&mut self) -> Option<(String, PathBuf)> {
        self.pending_clone.take()
    }

    /// Se um clone está rodando.
    pub fn is_cloning(&self) -> bool {
        self.clone_job.is_some()
    }

    /// O clone que terminou, se algum.
    pub fn take_cloned(&mut self) -> Option<Result<PathBuf, GitError>> {
        let done = self.clone_job.as_ref()?.poll()?;
        self.clone_job = None;
        match done {
            Done::Cloned(r) => Some(r),
            _ => None,
        }
    }

    /// Espera o clone (testes).
    pub fn wait_clone(&mut self) -> Option<Result<PathBuf, GitError>> {
        let job = self.clone_job.take()?;
        match job.wait() {
            Done::Cloned(r) => Some(r),
            Done::Status(Err(e)) => Some(Err(e)),
            _ => None,
        }
    }

    /// Abre "New Folder" dentro de `parent`.
    pub fn new_folder(&mut self, parent: &Path) {
        self.new_folder = Some(NewFolder {
            parent: parent.to_path_buf(),
            name: tr("New Folder").into(),
            error: None,
        });
    }

    /// Se "New Folder" está aberto.
    pub fn is_naming_folder(&self) -> bool {
        self.new_folder.is_some()
    }

    /// Confirma "New Folder" com este nome (testes e o botão OK).
    pub fn create_folder(&mut self, name: &str) -> Result<PathBuf, String> {
        let Some(dialog) = &mut self.new_folder else {
            return Err(tr("New Folder is not open.").into());
        };
        let name = name.trim();
        let bad = name.is_empty()
            || name == "."
            || name == ".."
            || name.starts_with('.')
            || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']);
        if bad {
            let e = tr("Enter a valid folder name.").to_string();
            dialog.error = Some(e.clone());
            return Err(e);
        }
        let path = dialog.parent.join(name);
        if path.exists() {
            let e = trf!("\"{name}\" already exists.", name = name);
            dialog.error = Some(e.clone());
            return Err(e);
        }
        if let Err(e) = std::fs::create_dir_all(&path) {
            let e = trf!("Could not create the folder: {e}", e = e);
            dialog.error = Some(e.clone());
            return Err(e);
        }
        self.new_folder = None;
        self.refresh();
        Ok(path)
    }

    /// Copia, numa thread, as senhas lembradas destas chaves (`usuario@host:porta`)
    /// de `from` para o cofre do workspace, que precisa estar destrancado.
    /// Senhas que já estão no cofre não mudam.
    pub fn copy_passwords(&mut self, pending: PendingCopy, from: Arc<dyn SecretStore>) {
        let slot = self.vault_slot();
        let PendingCopy { keys, known } = pending;
        self.copying = Some(std::thread::spawn(move || {
            let mut copied = 0;
            // As senhas em mãos (a da conexão ativa) valem mais que as
            // lembradas: são as que acabaram de funcionar.
            for (key, secret) in known {
                let mut slot = slot.lock().expect("vault lock");
                let Some(v) = slot.vault.as_mut().filter(|v| v.is_unlocked()) else {
                    return Err(tr("The workspace passwords are locked.").into());
                };
                if v.get(&key).ok().flatten().as_deref() == Some(secret.as_str()) {
                    continue;
                }
                v.set(&key, &secret)
                    .map_err(|e| super::passwords::vault_error_text(&e))?;
                copied += 1;
            }
            for key in keys {
                {
                    let mut slot = slot.lock().expect("vault lock");
                    let Some(v) = slot.vault.as_mut().filter(|v| v.is_unlocked()) else {
                        return Err(tr("The workspace passwords are locked.").into());
                    };
                    if v.get(&key).ok().flatten().is_some() {
                        continue;
                    }
                }
                // O cofre do sistema pode demorar: fora da trava.
                let Ok(Some(secret)) = from.get(&key) else {
                    continue;
                };
                let mut slot = slot.lock().expect("vault lock");
                let Some(v) = slot.vault.as_mut().filter(|v| v.is_unlocked()) else {
                    return Err(tr("The workspace passwords are locked.").into());
                };
                v.set(&key, &secret)
                    .map_err(|e| super::passwords::vault_error_text(&e))?;
                copied += 1;
            }
            Ok(copied)
        }));
    }

    /// Se a cópia das senhas está rodando.
    pub fn is_copying(&self) -> bool {
        self.copying.is_some()
    }

    /// Espera a cópia das senhas (testes) e devolve a mensagem dela.
    pub fn wait_copy(&mut self) -> Option<String> {
        self.finish_copy()
    }

    fn finish_copy(&mut self) -> Option<String> {
        let handle = self.copying.take()?;
        let result = handle
            .join()
            .unwrap_or_else(|_| Err(tr("Copying the passwords failed.").into()));
        let send = tr("Commit and Push to send them to your other computers.");
        Some(match result {
            Ok(0) => tr("No saved passwords to copy: the workspace passwords already have them, or this computer has none.").into(),
            Ok(1) => format!(
                "{} {send}",
                tr("Copied 1 saved password to the workspace passwords.")
            ),
            Ok(n) => format!(
                "{} {send}",
                trf!(
                    "Copied {n} saved passwords to the workspace passwords.",
                    n = n
                )
            ),
            Err(e) => e,
        })
    }

    /// O cofre que a árvore de servidores do workspace usa: as senhas dos
    /// servidores dela vão para o cofre do workspace (quando destrancado),
    /// até as de um servidor que acabou de ser registrado.
    fn workspace_store(
        &self,
        store: Option<&Arc<dyn SecretStore>>,
    ) -> Option<Arc<dyn SecretStore>> {
        store.map(|s| {
            Arc::new(WorkspaceSecrets::workspace_only(
                Some(s.clone()),
                self.vault_slot(),
            )) as Arc<dyn SecretStore>
        })
    }

    /// A árvore de servidores do workspace e o cofre dela, para o Object
    /// Explorer mostrar e mexer nas mesmas pastas.
    pub fn saved_servers(
        &mut self,
        store: Option<&Arc<dyn SecretStore>>,
    ) -> Option<(&mut RegisteredPanel, Option<Arc<dyn SecretStore>>)> {
        let store = self.workspace_store(store);
        let o = self.opened_mut()?;
        Some((&mut o.servers, store))
    }

    /// Abre o "New Server Registration" na raiz da árvore do workspace,
    /// como o menu de contexto dela.
    pub fn new_server(&mut self, store: Option<&Arc<dyn SecretStore>>) {
        let store = self.workspace_store(store);
        if let Some(o) = self.opened_mut() {
            o.servers.new_server(&[], store.as_ref());
        }
    }

    /// O registro, no workspace, do servidor desta conexão: edita, se ele
    /// já está lá; se não, abre o "New Server Registration" preenchido. A
    /// senha lembrada vai para o cofre do workspace.
    pub fn register_or_edit(
        &mut self,
        settings: &pgstudio_core::connection::ConnectionSettings,
        store: Option<&Arc<dyn SecretStore>>,
    ) -> Option<&mut RegisteredPanel> {
        let store = self.workspace_store(store);
        let o = self.opened_mut()?;
        o.servers.register_or_edit(settings, store.as_ref());
        Some(&mut o.servers)
    }

    /// Desenha o painel e devolve os pedidos.
    pub fn show(&mut self, ui: &mut egui::Ui, store: Option<&Arc<dyn SecretStore>>) -> Vec<Action> {
        let mut actions = Vec::new();
        let workspace_store = self.workspace_store(store);
        let store = workspace_store.as_ref();
        if self.workspaces.is_empty() {
            self.welcome(ui, &mut actions);
            return actions;
        }
        let mut refresh = false;
        self.cards(ui, &mut refresh);
        let Some(o) = self.workspaces.get_mut(self.active) else {
            return actions;
        };
        ui.add_space(2.0);

        // Servidores em cima, num retângulo próprio: a árvore do Registered
        // Servers tem rolagem que ocupa todo o espaço que recebe.
        let empty = o.servers.servers().all_servers().is_empty();
        let offer = empty && self.importable > 0;
        // A largura visível: uma linha larga acima (a do git ou a das
        // senhas) pode ter alargado o painel além do que aparece.
        let panel_width = ui
            .available_width()
            .min(ui.clip_rect().right() - ui.cursor().left() - 4.0)
            .max(120.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("Servers")).small().weak());
            // Com o botão grande embaixo, o atalho pequeno seria repetido.
            if offer {
                return;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        // Num painel estreito fica só o ícone, para não
                        // passar por cima do título.
                        egui::Button::new(if panel_width < 230.0 {
                            egui::RichText::new(icon::DOWNLOAD_SIMPLE)
                        } else {
                            egui::RichText::new(format!(
                                "{} {}",
                                icon::DOWNLOAD_SIMPLE,
                                tr("Import from this computer")
                            ))
                            .small()
                        })
                        .frame(false),
                    )
                    .on_hover_text(tr(
                        "Import Registered Servers from this computer, with their saved passwords",
                    ))
                    .clicked()
                {
                    actions.push(Action::ImportRegistered);
                }
            });
        });
        // O que falta trazer: sem isto, um workspace novo vai para o
        // repositório com o servers.json vazio e ninguém percebe.
        if offer {
            let label = if self.importable == 1 {
                tr("Import the connection from this computer").to_string()
            } else {
                trf!(
                    "Import the {n} connections from this computer",
                    n = self.importable
                )
            };
            ui.add_space(2.0);
            ui.scope(|ui| {
                // O painel é estreito: o texto quebra em vez de alargá-lo.
                ui.set_max_width(panel_width);
                let text = format!("{} {label}", icon::DOWNLOAD_SIMPLE); // i18n-ignore
                if ui.add(egui::Button::new(text).wrap()).clicked() {
                    actions.push(Action::ImportRegistered);
                }
                ui.add(egui::Label::new(egui::RichText::new(tr("This workspace has no connections yet. Import them so that they go to the repository and appear when you clone it on another computer.")).weak()).wrap());
            });
            ui.add_space(2.0);
        } else if let Some(name) = &self.unsaved_current {
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(panel_width);
                ui.weak(trf!(
                    "Connected to {name}, which is not in this workspace.",
                    name = name
                ));
                if ui
                    .small_button(tr("Add to workspace"))
                    .on_hover_text(tr("Register this connection in the workspace, with its password, so that it goes to the repository"))
                    .clicked()
                {
                    actions.push(Action::AddCurrent);
                }
            });
        }
        let width = ui.available_width();
        let height = (ui.available_height() * 0.4).max(80.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("workspace_servers")
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        actions.extend(
            o.servers
                .show(&mut child, store)
                .into_iter()
                .map(Action::Servers),
        );
        ui.separator();

        ui.label(egui::RichText::new(tr("Queries")).small().weak());
        let first = search_ui(ui, o, panel_width);
        if let Some(open) = first {
            actions.push(open);
        }
        let hits = o.search.is_active().then(|| o.search.hits());
        let mut folder_menu: Option<PathBuf> = None;
        egui::ScrollArea::both()
            .id_salt("workspace_queries")
            .auto_shrink(false)
            .show(ui, |ui| {
                let root = o.queries.clone();
                let hits = hits.as_deref();
                folder_rows(ui, o, &root, 0, hits, &mut actions, &mut folder_menu);
                if hits.is_some() {
                    return;
                }
                if root.count() == 0 && root.folders.is_empty() {
                    ui.add_space(4.0);
                    ui.weak(tr("No queries yet. Right-click \"Queries\" and choose \"New Query\", or save a query inside the queries folder."));
                }
            });
        if refresh {
            self.refresh();
        }
        if let Some(parent) = folder_menu {
            self.new_folder(&parent);
        }
        actions
    }

    /// Desenha os diálogos (os do Registered Servers e o "New Folder").
    /// Devolve uma mensagem para a status bar quando algo mudou.
    pub fn show_windows(
        &mut self,
        ctx: &egui::Context,
        store: Option<&Arc<dyn SecretStore>>,
        ai: Option<&pgstudio_automation::llm::Backend>,
    ) -> Option<String> {
        let mut status = self.message.take();
        let active = self.active;
        let several = self.workspaces.len() > 1;
        for (i, o) in self.workspaces.iter_mut().enumerate() {
            let mut said = None;
            if i == active {
                // Os diálogos são os do workspace ativo.
                if let Some(s) = o.servers.show_windows(ctx) {
                    said = Some(s);
                }
                if let Some(s) = o.git.show_windows(ctx, ai) {
                    said = Some(s);
                }
                if let Some(s) = o.passwords.show_windows(ctx, store) {
                    said = Some(s);
                }
            } else {
                // Os outros continuam trabalhando (um Pull pedido antes de
                // trocar de workspace, a senha mestra lembrada).
                if o.git.poll() {
                    said = o.git.take_message();
                }
                if o.passwords.poll(store) {
                    said = o.passwords.take_message().or(said);
                }
                if o.git.is_busy() || o.passwords.is_busy() {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
            if let Some(s) = said {
                // Com vários abertos, a mensagem diz de qual workspace é.
                status = Some(if several {
                    format!("{}: {s}", o.workspace.name()) // i18n-ignore
                } else {
                    s
                });
            }
            o.sync_vault_keys();
            if o.git.take_files_changed() {
                o.reload_files();
            }
        }
        if self.copying.as_ref().is_some_and(|h| h.is_finished()) {
            status = self.finish_copy().or(status);
        } else if self.copying.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.clone_job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.add_window(ctx);
        if let Some(count) = self.offer_import {
            let mut answer = None;
            crate::theme::modal(egui::Id::new("workspace_offer_import")).show(ctx, |ui| {
                crate::fit::dialog_width(ui, 440.0);
                ui.heading(tr("Bring your connections?"));
                ui.label(if count == 1 {
                    tr("This computer has 1 connection that is not in the workspace.").to_string()
                } else {
                    trf!(
                        "This computer has {n} connections that are not in the workspace.",
                        n = count
                    )
                });
                ui.label(tr("Import them so that they go to the repository and appear when you clone the workspace on another computer. Their passwords go too, encrypted with a master password that you choose next."));
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Not now")).clicked() {
                        answer = Some(false);
                    }
                    if ui.button(tr("Import connections")).clicked() {
                        answer = Some(true);
                    }
                });
            });
            if let Some(import) = answer {
                self.offer_import = None;
                self.accepted_import = import;
            }
        }
        let mut ok = None;
        let mut cancel = false;
        if let Some(dialog) = &mut self.new_folder {
            crate::theme::modal(egui::Id::new("workspace_new_folder")).show(ctx, |ui| {
                crate::fit::dialog_width(ui, 320.0);
                ui.heading(tr("New Folder"));
                ui.label(tr("Folder name:"));
                let edit = ui.text_edit_singleline(&mut dialog.name);
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ok = Some(dialog.name.clone());
                }
                if let Some(e) = &dialog.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(tr("OK")).clicked() {
                        ok = Some(dialog.name.clone());
                    }
                });
            });
        }
        if cancel {
            self.new_folder = None;
        } else if let Some(name) = ok
            && let Ok(path) = self.create_folder(&name)
        {
            status = Some(trf!("Created {path}.", path = path.display()));
        }
        status
    }
}

/// A largura visível do painel: uma linha larga pode ter alargado o
/// painel além do que aparece.
fn visible_width(ui: &egui::Ui) -> f32 {
    ui.available_width()
        .min(ui.clip_rect().right() - ui.cursor().left() - 4.0)
        .max(120.0)
}

/// Um botão principal, preenchido com a cor de destaque.
fn primary_button(ui: &egui::Ui, text: String) -> egui::Button<'static> {
    let (accent, on_accent) = crate::theme::accent_pair(ui);
    egui::Button::new(egui::RichText::new(text).color(on_accent)).fill(accent)
}

/// Uma opção grande e clicável: ícone, título e uma frase que explica.
fn option_card(ui: &mut egui::Ui, glyph: &str, title: &str, text: &str) -> egui::Response {
    let (accent, _) = crate::theme::accent_pair(ui);
    let scope = ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(("workspace_option", title))
            .sense(egui::Sense::click()),
        |ui| {
            let over = ui.response().hovered() || ui.response().has_focus();
            let v = ui.visuals();
            let (fill, stroke) = if over {
                (
                    v.widgets.hovered.weak_bg_fill,
                    egui::Stroke::new(1.0, accent),
                )
            } else {
                (v.faint_bg_color, v.widgets.noninteractive.bg_stroke)
            };
            egui::Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(6.0)
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(glyph).size(24.0).color(accent));
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(title).strong())
                                    .selectable(false)
                                    .wrap(),
                            );
                            ui.add(
                                egui::Label::new(egui::RichText::new(text).weak())
                                    .selectable(false)
                                    .wrap(),
                            );
                        });
                    });
                });
        },
    );
    scope
        .response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// O que foi pedido num cartão de workspace.
#[derive(Default)]
struct CardEvents {
    activate: Option<usize>,
    close: Option<usize>,
    refresh: bool,
}

/// O cartão de um workspace: o nome, o estado do repositório em uma frase
/// e, no ativo, os botões e a linha das senhas.
fn card(ui: &mut egui::Ui, o: &mut Opened, index: usize, active: bool, events: &mut CardEvents) {
    let summary = o.git.summary();
    let (accent, _) = crate::theme::accent_pair(ui);
    let scope = ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(("workspace_card", index))
            .sense(egui::Sense::click()),
        |ui| {
            let over = ui.response().hovered();
            let v = ui.visuals();
            let (fill, stroke) = if active {
                (
                    v.selection.bg_fill.gamma_multiply(0.22),
                    egui::Stroke::new(1.5, accent),
                )
            } else if over {
                (v.widgets.hovered.weak_bg_fill, v.widgets.hovered.bg_stroke)
            } else {
                (v.faint_bg_color, v.widgets.noninteractive.bg_stroke)
            };
            egui::Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    card_body(ui, o, index, active, &summary, events);
                });
        },
    );
    let response = scope.response;
    if !active {
        let response = response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(trf!(
                "Switch to {name} ({folder})",
                name = o.workspace.name(),
                folder = o.workspace.root().display()
            ));
        if response.clicked() {
            events.activate = Some(index);
        }
    }
}

fn card_body(
    ui: &mut egui::Ui,
    o: &mut Opened,
    index: usize,
    active: bool,
    summary: &super::source_control::Summary,
    events: &mut CardEvents,
) {
    let status = o.git.status().cloned();
    let is_repo = status.is_some();
    let has_remote = status.as_ref().is_some_and(|s| s.remote.is_some());
    let changes = status.as_ref().map_or(0, |s| s.changes.len());
    let mut open_changes = false;
    crate::fit::ends(
        ui,
        |ui| {
            let mut name =
                egui::RichText::new(format!("{} {}", icon::BRIEFCASE, o.workspace.name())).strong();
            if active {
                name = name.size(egui::TextStyle::Body.resolve(ui.style()).size + 1.5);
            }
            if let Some(name) = crate::fit::text(ui, name) {
                name.on_hover_text(o.workspace.root().display().to_string());
            }
        },
        |ui| {
            ui.menu_button(icon::DOTS_THREE, |ui| {
                if is_repo {
                    if ui
                        .button(format!("{} {}", icon::ARROW_DOWN, tr("Pull")))
                        .on_hover_text(tr("Bring the changes from the remote"))
                        .clicked()
                    {
                        o.git.pull();
                        ui.close();
                    }
                    if ui
                        .button(format!("{} {}", icon::ARROW_UP, tr("Push")))
                        .on_hover_text(tr("Send your commits to the remote"))
                        .clicked()
                    {
                        o.git.push();
                        ui.close();
                    }
                }
                // Os diálogos são os do workspace ativo.
                if is_repo && active {
                    if ui
                        .add_enabled(
                            changes > 0,
                            egui::Button::new(format!("{} {}", icon::CHECK, tr("Commit..."))),
                        )
                        .clicked()
                    {
                        o.git.open_commit();
                        ui.close();
                    }
                    if ui
                        .add_enabled(changes > 0, egui::Button::new(tr("Changes and Discard...")))
                        .clicked()
                    {
                        open_changes = true;
                        ui.close();
                    }
                    if ui.button(tr("Remote...")).clicked() {
                        o.git.open_remote();
                        ui.close();
                    }
                }
                if ui.button(tr("Refresh")).clicked() {
                    if active {
                        events.refresh = true;
                    } else {
                        o.git.refresh();
                    }
                    ui.close();
                }
                ui.separator();
                if ui
                    .button(format!("{} {}", icon::X, tr("Close Workspace")))
                    .on_hover_text(tr(
                        "Takes the workspace out of this list. Its files stay in the folder.",
                    ))
                    .clicked()
                {
                    events.close = Some(index);
                    ui.close();
                }
            });
        },
    );
    // O estado em uma frase, com a cor dizendo se há algo a fazer.
    crate::fit::ends(
        ui,
        |ui| {
            if let Some(busy) = &summary.busy {
                ui.spinner();
                crate::fit::text(ui, egui::RichText::new(busy).weak());
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            let v = ui.visuals();
            let color = match summary.kind {
                SyncKind::Synced => egui::Color32::from_rgb(0x4C, 0xAF, 0x50),
                SyncKind::Pending => v.warn_fg_color,
                SyncKind::Problem => v.error_fg_color,
                SyncKind::Local | SyncKind::Reading => v.weak_text_color(),
            };
            let glyph = match summary.kind {
                SyncKind::Synced => icon::CHECK_CIRCLE,
                SyncKind::Pending => icon::ARROWS_DOWN_UP,
                SyncKind::Problem => icon::WARNING,
                SyncKind::Local => icon::DESKTOP,
                SyncKind::Reading => icon::CIRCLE_DASHED,
            };
            ui.label(egui::RichText::new(glyph).color(color));
            // Na dica: o branch, o remoto e os arquivos alterados.
            if let Some(text) = crate::fit::text(ui, &summary.text)
                && let Some(s) = &status
            {
                let branch = if s.branch.is_empty() {
                    tr("(detached)").to_string()
                } else {
                    s.branch.clone()
                };
                let remote = s.remote.clone().unwrap_or_else(|| tr("No remote").into());
                let mut lines = vec![format!("{} {branch}  {remote}", icon::GIT_BRANCH)]; // i18n-ignore
                lines.extend(s.changes.iter().map(|c| format!("{} {}", c.kind(), c.path)));
                text.on_hover_text(lines.join("\n"));
            }
        },
        |ui| {
            // Nos outros cartões, o cadeado avisa que as senhas daquele
            // workspace ainda estão trancadas.
            if !active && o.passwords.exists() && !o.passwords.is_unlocked() {
                ui.label(egui::RichText::new(icon::LOCK).weak())
                    .on_hover_text(tr("Passwords locked"));
            }
        },
    );
    if !active {
        return;
    }
    match o.git.state() {
        Some(Err(GitError::NotRepo)) | None => {}
        Some(Err(e)) => {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(e.to_string()).color(ui.visuals().warn_fg_color),
                )
                .wrap(),
            );
        }
        Some(Ok(s)) if !s.conflicts.is_empty() => {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(trf!("Conflicts: {files}", files = s.conflicts.join(", ")))
                        .color(ui.visuals().error_fg_color),
                )
                .wrap(),
            );
        }
        Some(Ok(_)) => {}
    }
    let busy = summary.busy.is_some();
    ui.horizontal_wrapped(|ui| {
        if matches!(o.git.state(), Some(Err(GitError::NotRepo))) {
            if ui
                .add_enabled(!busy, egui::Button::new(tr("Initialize Git")))
                .on_hover_text(tr(
                    "Start keeping the history of this workspace with Git, to send it to a repository later.",
                ))
                .clicked()
            {
                o.git.init();
            }
            return;
        }
        if !is_repo {
            return;
        }
        // O que há para fazer agora vem preenchido: salvar as alterações
        // ou, sem nenhuma, sincronizar.
        let save = format!("{} {}", icon::CHECK, tr("Save changes..."));
        let sync = format!("{} {}", icon::ARROWS_CLOCKWISE, tr("Synchronize"));
        if changes > 0
            && ui
                .add_enabled(!busy, primary_button(ui, save))
                .on_hover_text(tr(
                    "Describe what you changed and save it in the history (Commit). You can send it to the repository in the same step.",
                ))
                .clicked()
        {
            o.git.open_commit();
        }
        if has_remote {
            let button = if changes > 0 {
                egui::Button::new(sync)
            } else {
                primary_button(ui, sync)
            };
            if ui
                .add_enabled(!busy, button)
                .on_hover_text(tr(
                    "Receive what is new in the repository and send what you saved (Pull, then Push).",
                ))
                .clicked()
            {
                o.git.sync();
            }
        } else if ui
            .add_enabled(!busy, egui::Button::new(tr("Connect to a repository...")))
            .on_hover_text(tr(
                "Set the address of the repository (GitHub, GitLab...) where this workspace is sent.",
            ))
            .clicked()
        {
            o.git.open_remote();
        }
    });
    if open_changes {
        o.git.open_changes();
    }
    o.passwords.show(ui);
}

impl WorkspacePanel {
    /// Sem workspace aberto: o que é um workspace e as duas formas de
    /// começar, em botões grandes.
    fn welcome(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let width = visible_width(ui);
        ui.scope(|ui| {
            ui.set_max_width(width);
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!("{} {}", icon::BRIEFCASE, tr("Workspaces")))
                    .heading()
                    .strong(),
            );
            ui.add_space(2.0);
            ui.add(egui::Label::new(egui::RichText::new(tr("A workspace keeps your connections, queries and snippets together in a folder, usually a Git repository, so that you have them on every computer and can share them with your team. You can open several at the same time, for example one per client.")).weak()).wrap());
            ui.add_space(8.0);
            self.add_options(ui, actions);
        });
    }

    /// As duas formas de adicionar um workspace e o clone em andamento.
    fn add_options(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        if option_card(
            ui,
            icon::CLOUD_ARROW_DOWN,
            tr("Clone a repository"),
            tr("Download a workspace from GitHub, GitLab or another Git server, using its address."),
        )
        .clicked()
        {
            self.open_add();
        }
        ui.add_space(4.0);
        if option_card(
            ui,
            icon::FOLDER_OPEN,
            tr("Use a folder on this computer"),
            tr("Open the workspace that is in a folder, or start a new one in it."),
        )
        .clicked()
        {
            actions.push(Action::AddFolder);
        }
        if let Some(job) = &self.clone_job {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(tr(job.label));
            });
        }
    }

    /// O cabeçalho "Workspaces", com o botão de adicionar, e um cartão por
    /// workspace aberto.
    fn cards(&mut self, ui: &mut egui::Ui, refresh: &mut bool) {
        let width = visible_width(ui);
        let mut add = false;
        let mut events = CardEvents::default();
        let active = self.active;
        ui.scope(|ui| {
            ui.set_max_width(width);
            crate::fit::ends(
                ui,
                |ui| {
                    let title = match self.workspaces.len() {
                        1 => tr("Workspace").to_string(),
                        n => trf!("Workspaces ({n})", n = n),
                    };
                    crate::fit::text(ui, egui::RichText::new(title).small().weak());
                },
                |ui| {
                    if ui
                        .button(format!("{} {}", icon::PLUS, tr("Add")))
                        .on_hover_text(tr(
                            "Add another workspace: clone a repository or use a folder on this computer. The ones already open stay open.",
                        ))
                        .clicked()
                    {
                        add = true;
                    }
                },
            );
            // Muitos workspaces rolam aqui, sem tirar o lugar dos
            // servidores e das consultas.
            let max = (ui.available_height() * 0.6).max(150.0);
            egui::ScrollArea::vertical()
                .id_salt("workspace_cards")
                .max_height(max)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, o) in self.workspaces.iter_mut().enumerate() {
                        card(ui, o, i, i == active, &mut events);
                    }
                    if let Some(job) = &self.clone_job {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.weak(tr(job.label));
                        });
                    }
                });
        });
        if add {
            self.open_add();
        }
        if let Some(i) = events.activate {
            self.activate(i);
        }
        if let Some(i) = events.close {
            let name = self.workspaces[i].workspace.name().to_string();
            self.close_at(i);
            self.message = Some(trf!(
                "Closed workspace {name}. Its files are still in the folder.",
                name = name
            ));
        }
        *refresh |= events.refresh;
    }

    /// O diálogo "Add Workspace".
    fn add_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.add else {
            return;
        };
        let mut clone = false;
        let mut close = false;
        let mut browse = false;
        let mut folder = false;
        crate::theme::modal(egui::Id::new("workspace_add")).show(ctx, |ui| {
            crate::fit::dialog_width(ui, 500.0);
            ui.heading(tr("Add Workspace"));
            ui.add(egui::Label::new(egui::RichText::new(tr("The workspaces that are already open stay open. Each one keeps its own connections, queries, snippets and passwords.")).weak()).wrap());
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!(
                    "{} {}",
                    icon::CLOUD_ARROW_DOWN,
                    tr("Clone a repository")
                ))
                .strong(),
            );
            let label = ui.label(tr("Repository address:"));
            let edit = ui
                .add(
                    egui::TextEdit::singleline(&mut dialog.url)
                        .hint_text("git@github.com:you/sql.git") // i18n-ignore
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                clone = true;
            }
            let label = ui.label(tr("Save in:"));
            crate::fit::ends(
                ui,
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.dest)
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label.id);
                },
                |ui| {
                    if ui.button(tr("Choose...")).clicked() {
                        browse = true;
                    }
                },
            );
            ui.add(egui::Label::new(egui::RichText::new(tr("The clone goes into a new folder there, named after the repository. Git uses your SSH keys and credential helper.")).weak()).wrap());
            if let Some(e) = &dialog.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.add_space(4.0);
            let text = format!("{} {}", icon::CLOUD_ARROW_DOWN, tr("Clone"));
            if ui.add(primary_button(ui, text)).clicked() {
                clone = true;
            }
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            if option_card(
                ui,
                icon::FOLDER_OPEN,
                tr("Use a folder on this computer"),
                tr("Open the workspace that is in a folder, or start a new one in it."),
            )
            .clicked()
            {
                folder = true;
            }
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if browse {
            self.add_actions.push(Action::BrowseCloneFolder);
        }
        if folder {
            self.add = None;
            self.add_actions.push(Action::AddFolder);
        } else if close {
            self.add = None;
        } else if clone {
            self.confirm_clone();
        }
    }

    /// Confirma o clone de "Add Workspace" (o botão e os testes). Sem a
    /// URL ou a pasta, o diálogo fica aberto com o erro.
    pub fn confirm_clone(&mut self) -> bool {
        let Some(dialog) = &mut self.add else {
            return false;
        };
        let (url, dest) = (dialog.url.trim(), dialog.dest.trim());
        if url.is_empty() {
            dialog.error = Some(tr("Enter the repository URL.").into());
            return false;
        }
        if dest.is_empty() {
            dialog.error = Some(tr("Choose the folder where the clone is saved.").into());
            return false;
        }
        self.pending_clone = Some((url.to_string(), PathBuf::from(dest)));
        self.add = None;
        true
    }

    /// Pedidos feitos dentro de "Add Workspace" (a casca trata junto com
    /// os do painel).
    pub fn take_add_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.add_actions)
    }
}

fn folder_rows(
    ui: &mut egui::Ui,
    o: &mut Opened,
    folder: &Folder,
    depth: usize,
    hits: Option<&Hits>,
    actions: &mut Vec<Action>,
    folder_menu: &mut Option<PathBuf>,
) {
    // Pesquisando: só as pastas com consulta achada, todas abertas.
    if let Some(hits) = hits
        && depth > 0
        && !search::folder_has_hit(folder, hits)
    {
        return;
    }
    let expanded = hits.is_some() || !o.collapsed.contains(&folder.path);
    let mut toggle = false;
    let name = if depth == 0 {
        tr("Queries").to_string()
    } else {
        folder.name.clone()
    };
    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 16.0 + 4.0);
        let caret = if expanded {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        };
        if ui.add(egui::Button::new(caret).frame(false)).clicked() {
            toggle = true;
        }
        let glyph = if expanded {
            icon::FOLDER_OPEN
        } else {
            icon::FOLDER
        };
        let label = ui.add(
            egui::Button::selectable(
                o.selected.as_deref() == Some(folder.path.as_path()),
                format!("{glyph} {name}"),
            )
            .frame_when_inactive(false),
        );
        if label.clicked() {
            o.selected = Some(folder.path.clone());
        }
        if label.double_clicked() {
            toggle = true;
        }
        label.context_menu(|ui| {
            o.selected = Some(folder.path.clone());
            if ui.button(tr("New Query")).clicked() {
                actions.push(Action::NewQuery(folder.path.clone()));
                ui.close();
            }
            if ui.button(tr("New Folder...")).clicked() {
                *folder_menu = Some(folder.path.clone());
                ui.close();
            }
        });
    });
    if toggle && hits.is_none() {
        if expanded {
            o.collapsed.insert(folder.path.clone());
        } else {
            o.collapsed.remove(&folder.path);
        }
    }
    if !expanded {
        return;
    }
    for sub in &folder.folders {
        folder_rows(ui, o, sub, depth + 1, hits, actions, folder_menu);
    }
    for file in &folder.files {
        let hit = hits.and_then(|h| h.get(file));
        if hits.is_some() && hit.is_none() {
            continue;
        }
        let open = || match hit.and_then(|h| h.line.as_ref()) {
            Some(line) => Action::OpenQueryAt {
                path: file.clone(),
                line: line.number,
                term: line.term.clone(),
            },
            None => Action::OpenQuery(file.clone()),
        };
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add_space((depth + 1) as f32 * 16.0 + 4.0 + 14.0);
            let label = ui.add(
                egui::Button::selectable(
                    o.selected.as_deref() == Some(file.as_path()),
                    format!("{} {name}", icon::FILE_SQL),
                )
                .frame_when_inactive(false),
            );
            if label.clicked() {
                o.selected = Some(file.clone());
            }
            if label.double_clicked() {
                actions.push(open());
            }
            label.context_menu(|ui| {
                o.selected = Some(file.clone());
                if ui.button(tr("Open")).clicked() {
                    actions.push(open());
                    ui.close();
                }
            });
        });
        if let Some(line) = hit.and_then(|h| h.line.as_ref()) {
            // O trecho fica colado no arquivo a que pertence.
            ui.add_space(-ui.spacing().item_spacing.y);
            let more = hit.map_or(0, |h| h.lines.saturating_sub(1));
            ui.horizontal(|ui| {
                ui.add_space((depth + 1) as f32 * 16.0 + 4.0 + 14.0 + 20.0);
                let row = ui
                    .add(
                        egui::Label::new(snippet_job(ui, line, more))
                            .selectable(false)
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(tr("Found inside the file. Click to open it at this line."));
                if row.clicked() {
                    o.selected = Some(file.clone());
                    actions.push(open());
                }
            });
        }
    }
}

/// O campo de pesquisa das consultas e, embaixo, quantas ele achou.
/// Devolve a primeira consulta achada quando o usuário aperta Enter.
fn search_ui(ui: &mut egui::Ui, o: &mut Opened, width: f32) -> Option<Action> {
    let mut open_first = false;
    // Da direita para a esquerda: o botão de limpar fica no canto e o
    // campo ocupa o resto da largura visível do painel (`width`).
    let row = egui::vec2(width, ui.spacing().interact_size.y);
    let layout = egui::Layout::right_to_left(egui::Align::Center);
    ui.allocate_ui_with_layout(row, layout, |ui| {
        ui.set_max_width(width);
        let clear = o.search.is_active()
            && ui
                .add(egui::Button::new(icon::X).frame(false))
                .on_hover_text(tr("Clear the search"))
                .clicked();
        let field = ui.add(
            egui::TextEdit::singleline(&mut o.search.text)
                .id_salt("workspace_queries_search")
                .hint_text(format!(
                    "{} {}",
                    icon::MAGNIFYING_GLASS,
                    tr("Search queries by name or content")
                ))
                .desired_width(f32::INFINITY),
        );
        if field.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            o.search.clear();
        }
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            open_first = true;
        }
        if clear {
            o.search.clear();
            field.request_focus();
        }
    });
    o.search.update(ui.ctx(), &o.queries);
    if !o.search.is_active() {
        return None;
    }
    let hits = o.search.hits();
    let text = if o.search.is_searching() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
        tr("Searching inside the files...").to_string()
    } else {
        match hits.len() {
            0 => tr("No queries found.").to_string(),
            1 => tr("1 query found.").to_string(),
            n => trf!("{n} queries found.", n = n),
        }
    };
    ui.label(egui::RichText::new(text).small().weak());
    if !open_first {
        return None;
    }
    let first = search::all_files(&o.queries)
        .into_iter()
        .find(|f| hits.contains_key(f))?;
    o.selected = Some(first.clone());
    Some(match hits[&first].line.as_ref() {
        Some(line) => Action::OpenQueryAt {
            path: first,
            line: line.number,
            term: line.term.clone(),
        },
        None => Action::OpenQuery(first),
    })
}

/// "12: select ... termo ...", com o termo em destaque, e quantas outras
/// linhas do arquivo também o têm.
fn snippet_job(ui: &egui::Ui, line: &search::Line, more: usize) -> egui::text::LayoutJob {
    let mut font = egui::TextStyle::Body.resolve(ui.style());
    font.size = (font.size * 0.9).round();
    let weak = egui::TextFormat::simple(font.clone(), ui.visuals().weak_text_color());
    let strong = egui::TextFormat {
        background: ui.visuals().selection.bg_fill.gamma_multiply(0.5),
        ..egui::TextFormat::simple(font, ui.visuals().strong_text_color())
    };
    let part = |from: usize, to: usize| -> String {
        line.snippet.chars().skip(from).take(to - from).collect()
    };
    let total = line.snippet.chars().count();
    let mut job = egui::text::LayoutJob::default();
    job.append(&format!("{}: ", line.number), 0.0, weak.clone()); // i18n-ignore
    job.append(&part(0, line.found.start), 0.0, weak.clone());
    job.append(&part(line.found.start, line.found.end), 0.0, strong);
    job.append(&part(line.found.end, total), 0.0, weak.clone());
    if more > 0 {
        let rest = if more == 1 {
            tr("(+1 line)").to_string()
        } else {
            trf!("(+{n} lines)", n = more)
        };
        job.append(&format!("  {rest}"), 0.0, weak); // i18n-ignore
    }
    job
}
