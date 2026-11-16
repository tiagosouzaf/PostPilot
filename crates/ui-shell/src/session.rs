//! A sessão: tudo o que o usuário tinha aberto, salvo enquanto ele trabalha
//! e reaberto na próxima vez (também depois de uma queda do app ou da
//! máquina).
//!
//! Fica em `<config>/PostPilot/session.json`, com o texto ainda não salvo
//! das janelas de consulta em arquivos ao lado (`session/query-N.sql`). Hoje
//! guarda:
//!
//! - os servidores conectados no Object Explorer, na ordem, qual é o ativo
//!   e os nós expandidos de cada um;
//! - as janelas de consulta: arquivo, nome, texto não salvo, banco,
//!   servidor e qual estava na frente;
//! - se o Query History estava aberto e o que estava na pesquisa dele (as
//!   execuções ficam no arquivo do próprio histórico, veja
//!   [`crate::history`]).
//!
//! **Senhas nunca entram aqui.** Um servidor é guardado como
//! [`ConnectionSettings`], que não tem senha; ao reabrir, a senha vem do
//! cofre (o do workspace ou o do sistema, com "Remember password") e, sem
//! ela lá, o "Connect to Server" abre preenchido pedindo só a senha.
//!
//! # Para guardar mais uma coisa
//!
//! A regra do app é não perder nada do que o usuário abriu. Um recurso novo
//! com estado entra aqui em três passos:
//!
//! 1. um campo novo em [`SessionState`] (ou em [`SessionServer`] /
//!    [`SessionDocument`]), com `#[serde(default)]` para os arquivos
//!    antigos continuarem abrindo;
//! 2. preencher o campo em [`ShellApp::session_state`];
//! 3. aplicar o campo em `ShellApp::restore_session`.
//!
//! O resto (notar a mudança, gravar sem deixar arquivo pela metade, gravar
//! ao sair) já é feito por [`ShellApp::session_step`]: ele compara o
//! retrato atual com o último gravado, no máximo uma vez por
//! [`CHECK_EVERY`].

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::ConnectionSettings;
use pgstudio_core::metadata::NodeId;
use pgstudio_i18n::tr;
use serde::{Deserialize, Serialize};

use crate::ShellApp;
use crate::servers::server_key;

/// Versão do formato. Mude só numa alteração incompatível; campo novo com
/// `#[serde(default)]` não precisa.
pub const VERSION: u32 = 1;

/// Intervalo mínimo entre duas conferências do que mudou.
pub const CHECK_EVERY: Duration = Duration::from_secs(1);

/// Um servidor do Object Explorer. Sem senha.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionServer {
    pub settings: ConnectionSettings,
    /// Se era o servidor ativo.
    pub active: bool,
    /// Nós expandidos da árvore, o pai antes dos filhos.
    pub expanded: Vec<String>,
}

/// Texto de uma janela que ainda não está no arquivo dela.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Unsaved {
    /// Arquivo com o texto, na pasta `session/`.
    pub file: String,
    /// Tamanho e resumo do texto, só para notar que ele mudou.
    #[serde(skip)]
    pub stamp: (usize, u64),
}

/// Uma janela de consulta.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionDocument {
    /// Nome da aba, como `SQLQuery1.sql`.
    pub name: String,
    /// Arquivo da consulta, se ela já foi salva ou veio de um.
    pub path: Option<PathBuf>,
    /// O texto do editor, quando é diferente do que está no arquivo.
    pub unsaved: Option<Unsaved>,
    /// Banco da janela.
    pub database: Option<String>,
    /// Servidor da janela (`usuario@host:porta`).
    pub server: Option<String>,
    /// "Include Actual Execution Plan" ligado.
    pub actual_plan: bool,
    /// Os servidores de uma janela de vários servidores.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub multi: Vec<crate::multi::TargetId>,
    /// Se era a janela na frente.
    pub active: bool,
}

/// Tudo o que a sessão guarda. Veja o módulo para acrescentar um campo.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionState {
    /// Servidores do Object Explorer, na ordem da árvore.
    pub servers: Vec<SessionServer>,
    /// Janelas de consulta, na ordem em que foram abertas.
    pub documents: Vec<SessionDocument>,
    /// Número da última `SQLQueryN.sql`, para a numeração continuar.
    pub query_counter: u32,
    /// A janela Query History estava aberta.
    pub history_open: bool,
    /// O que estava escrito na pesquisa do Query History.
    pub history_filter: String,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(flatten)]
    state: SessionState,
}

/// Lê a sessão de um texto JSON.
pub fn parse(text: &str) -> Result<SessionState, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(v) if v == u64::from(VERSION) => {}
        Some(v) => return Err(format!("unsupported session version {v}")),
        None => return Err("missing session version".into()),
    }
    let file: File = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Ok(file.state)
}

/// Escreve a sessão como texto JSON.
pub fn to_json(state: &SessionState) -> String {
    let file = File {
        version: VERSION,
        state: state.clone(),
    };
    serde_json::to_string_pretty(&file).expect("session is always serializable")
}

/// Tamanho e resumo de um texto, para notar mudanças sem guardar uma cópia.
pub fn stamp(text: &str) -> (usize, u64) {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    (text.len(), h.finish())
}

/// O arquivo da sessão e o que foi lido dele ao abrir.
#[derive(Debug)]
pub struct SessionStore {
    path: PathBuf,
    /// O que estava salvo, até ser reaberto.
    loaded: Option<SessionState>,
    /// Problema ao ler o arquivo, para avisar o usuário.
    pub error: Option<String>,
    /// Último retrato gravado.
    last: Option<SessionState>,
    /// Textos já gravados nesta execução, por arquivo.
    written: HashMap<String, (usize, u64)>,
}

impl SessionStore {
    /// Lê `path`. Sem arquivo, a sessão começa vazia; um arquivo ruim é
    /// guardado ao lado (`session.json.bad`) e a sessão começa vazia, com o
    /// problema em `error`.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (loaded, error) = match std::fs::read_to_string(&path) {
            Ok(text) => match parse(&text) {
                Ok(state) => (Some(state), None),
                Err(e) => {
                    let _ = std::fs::copy(&path, path.with_extension("json.bad"));
                    (None, Some(e))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
            Err(e) => (None, Some(e.to_string())),
        };
        SessionStore {
            path,
            loaded,
            error,
            last: None,
            written: HashMap::new(),
        }
    }

    /// Caminho do `session.json`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Pasta dos textos não salvos (`session/`, ao lado do arquivo).
    pub fn texts_dir(&self) -> PathBuf {
        self.path.with_extension("")
    }

    /// A sessão lida ao abrir, uma vez só.
    pub fn take_loaded(&mut self) -> Option<SessionState> {
        self.loaded.take()
    }

    /// O texto não salvo de uma janela.
    pub fn read_text(&self, unsaved: &Unsaved) -> Option<String> {
        // Só nomes simples: o arquivo de sessão não escolhe o que o app lê.
        let name = Path::new(&unsaved.file).file_name()?;
        if name != std::ffi::OsStr::new(&unsaved.file) {
            return None;
        }
        std::fs::read_to_string(self.texts_dir().join(name)).ok()
    }

    /// Se `state` é diferente do último retrato gravado.
    pub fn changed(&self, state: &SessionState) -> bool {
        self.last.as_ref() != Some(state)
    }

    /// Grava a sessão: primeiro os textos que mudaram, depois o
    /// `session.json`, e só então apaga os textos que ninguém mais usa.
    /// Cada arquivo é escrito ao lado e renomeado, então uma queda no meio
    /// deixa a sessão anterior inteira.
    pub fn save(&mut self, state: &SessionState, texts: &[(&str, &str)]) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let dir = self.texts_dir();
        if !texts.is_empty() {
            std::fs::create_dir_all(&dir)?;
        }
        for (file, text) in texts {
            let stamp = stamp(text);
            if self.written.get(*file) == Some(&stamp) {
                continue;
            }
            write_atomic(&dir.join(file), text)?;
            self.written.insert((*file).to_string(), stamp);
        }
        write_atomic(&self.path, &to_json(state))?;
        self.last = Some(state.clone());
        // Textos de janelas fechadas ou já salvas.
        let used: Vec<&str> = texts.iter().map(|(f, _)| *f).collect();
        self.written.retain(|f, _| used.contains(&f.as_str()));
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if !used.iter().any(|f| std::ffi::OsStr::new(f) == name) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Ok(())
    }
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// O que a casca guarda da sessão enquanto roda.
#[derive(Debug, Default)]
pub struct Session {
    store: Option<SessionStore>,
    /// Quando o retrato foi conferido pela última vez.
    checked: Option<Instant>,
    /// Servidores da sessão anterior que ainda não reconectaram. Continuam
    /// na sessão até a vez deles passar, para fechar o app no meio da
    /// reconexão não esquecer nenhum.
    restoring: Vec<SessionServer>,
    /// O servidor ativo da sessão anterior, para voltar a ser o ativo.
    active: Option<String>,
    /// Último layout gravado junto com a sessão.
    layout: Option<crate::layout::SavedLayout>,
}

impl Session {
    /// Se há um arquivo de sessão (o app não perde nada ao fechar).
    pub fn is_on(&self) -> bool {
        self.store.is_some()
    }

    /// O arquivo da sessão.
    pub fn store(&self) -> Option<&SessionStore> {
        self.store.as_ref()
    }
}

impl ShellApp {
    /// Reabre o que estava aberto e passa a salvar a sessão neste arquivo
    /// enquanto o usuário trabalha. Chame depois de [`Self::with_connector`]
    /// e do cofre, para os servidores reconectarem.
    pub fn with_session(mut self, mut store: SessionStore) -> Self {
        if let Some(e) = &store.error {
            eprintln!("session: {}: {e}", store.path().display());
            self.status =
                tr("The last session could not be read; starting with an empty one.").to_string();
        }
        if let Some(state) = store.take_loaded() {
            self.restore_session(&store, state);
        }
        self.session.store = Some(store);
        self
    }

    /// A sessão em uso.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Reabre as janelas e põe os servidores na fila para reconectar.
    fn restore_session(&mut self, store: &SessionStore, state: SessionState) {
        self.documents.set_query_counter(state.query_counter);
        self.history.filter = state.history_filter.clone();
        if state.history_open {
            self.show_query_history();
        }
        let mut front = None;
        for d in &state.documents {
            let on_disk = d
                .path
                .as_deref()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| String::from_utf8(b).ok());
            let unsaved = d.unsaved.as_ref().and_then(|u| store.read_text(u));
            let (text, saved) = match (unsaved, on_disk) {
                (Some(text), saved) => (text, saved.unwrap_or_default()),
                (None, Some(saved)) => (saved.clone(), saved),
                // Um arquivo que sumiu, sem texto guardado: não há o que
                // reabrir.
                (None, None) if d.path.is_some() => continue,
                (None, None) => (String::new(), String::new()),
            };
            let id = self
                .documents
                .restore(d.name.clone(), d.path.clone(), text, saved);
            if let Some(doc) = self.documents.get_mut(id) {
                doc.database = d.database.clone();
                doc.actual_plan = d.actual_plan;
                if !d.multi.is_empty() {
                    doc.multi = Some(crate::multi::Selection::new(d.multi.clone()));
                }
            }
            if let Some(key) = &d.server {
                self.doc_servers.insert(id, key.clone());
            }
            self.activate_document(id);
            if d.active {
                front = Some(id);
            }
        }
        if let Some(id) = front {
            self.activate_document(id);
        }
        if self.connector.is_none() || state.servers.is_empty() {
            return;
        }
        // No lugar do "Connect to Server" vazio de toda abertura, os
        // servidores da sessão conectam um depois do outro: com a senha do
        // cofre, direto; sem ela, o diálogo abre preenchido pedindo a senha.
        self.connect_dialog = None;
        for s in &state.servers {
            self.queue_registered_connect(s.settings.clone());
        }
        self.session.active = state
            .servers
            .iter()
            .find(|s| s.active)
            .map(|s| server_key(&s.settings));
        self.session.restoring = state.servers;
    }

    /// Um servidor acabou de conectar e é o ativo: se veio da sessão
    /// anterior, a árvore dele abre como estava.
    pub(crate) fn session_connected(&mut self, settings: &ConnectionSettings) {
        let key = server_key(settings);
        let Some(i) = self
            .session
            .restoring
            .iter()
            .position(|s| server_key(&s.settings) == key)
        else {
            return;
        };
        let server = self.session.restoring.remove(i);
        self.explorer
            .restore_expanded(server.expanded.into_iter().map(NodeId));
    }

    /// Retrato do que está aberto agora, como seria salvo.
    pub fn session_state(&self) -> SessionState {
        let mut servers: Vec<(u32, SessionServer)> = Vec::new();
        let expanded = |e: &pgstudio_object_explorer::ObjectExplorer| {
            e.expanded().into_iter().map(|id| id.0).collect()
        };
        for p in &self.servers.parked {
            if let Some(c) = &p.connection {
                servers.push((
                    p.order,
                    SessionServer {
                        settings: c.settings.clone(),
                        active: false,
                        expanded: expanded(&p.explorer),
                    },
                ));
            }
        }
        if let Some(c) = &self.connection {
            servers.push((
                self.servers.active_order,
                SessionServer {
                    settings: c.settings.clone(),
                    active: true,
                    expanded: expanded(&self.explorer),
                },
            ));
        }
        servers.sort_by_key(|(order, _)| *order);
        let mut servers: Vec<SessionServer> = servers.into_iter().map(|(_, s)| s).collect();
        // Os que ainda vão reconectar continuam na sessão.
        let restoring = !self.session.restoring.is_empty();
        for s in &self.session.restoring {
            let key = server_key(&s.settings);
            if !servers.iter().any(|c| server_key(&c.settings) == key) {
                servers.push(SessionServer {
                    active: false,
                    ..s.clone()
                });
            }
        }
        if restoring && let Some(key) = &self.session.active {
            for s in &mut servers {
                s.active = server_key(&s.settings) == *key;
            }
        }
        let documents = self
            .documents
            .ids()
            .into_iter()
            .filter_map(|id| {
                let d = self.documents.get(id)?;
                Some(SessionDocument {
                    name: d.name().to_string(),
                    path: d.path().map(Path::to_path_buf),
                    unsaved: d.is_dirty().then(|| Unsaved {
                        file: text_file(id),
                        stamp: stamp(&d.text),
                    }),
                    database: d.database.clone(),
                    server: self.doc_servers.get(&id).cloned(),
                    actual_plan: d.actual_plan,
                    multi: d
                        .multi
                        .as_ref()
                        .map(|m| m.targets.clone())
                        .unwrap_or_default(),
                    active: self.active_doc == Some(id),
                })
            })
            .collect();
        SessionState {
            servers,
            documents,
            query_counter: self.documents.query_counter(),
            history_open: self.workspace.is_open(&crate::panels::Panel::QueryHistory),
            history_filter: self.history.filter.clone(),
        }
    }

    /// Grava a sessão agora, se mudou. Chamado ao fechar a janela e pelo
    /// [`Self::session_step`].
    pub fn save_session(&mut self) {
        if self.session.store.is_none() {
            return;
        }
        let state = self.session_state();
        let Some(store) = self.session.store.as_mut() else {
            return;
        };
        if store.changed(&state) {
            let ids = self.documents.ids();
            let files: Vec<(u32, String)> = ids.into_iter().map(|id| (id, text_file(id))).collect();
            let texts: Vec<(&str, &str)> = files
                .iter()
                .filter_map(|(id, file)| {
                    let d = self.documents.get(*id)?;
                    d.is_dirty().then_some((file.as_str(), d.text.as_str()))
                })
                .collect();
            if let Err(e) = store.save(&state, &texts) {
                eprintln!("session: could not save {}: {e}", store.path().display());
            }
        }
        // O layout vai junto: uma queda também não perde a arrumação das
        // janelas.
        let layout = self.current_layout();
        if self.session.layout.as_ref() != Some(&layout) {
            self.save_layout();
            self.session.layout = Some(layout);
        }
    }

    /// Uma vez por frame: termina a reconexão dos servidores da sessão
    /// anterior e grava a sessão quando algo mudou (conferindo no máximo
    /// uma vez por [`CHECK_EVERY`]).
    pub(crate) fn session_step(&mut self, ctx: &egui::Context) {
        if !self.session.restoring.is_empty() || self.session.active.is_some() {
            if self.connect_dialog.is_none() && !self.servers.has_queue() {
                // Todos tiveram a vez: quem não conectou (o usuário cancelou
                // a senha) sai da sessão, e o ativo de antes volta a ser.
                self.session.restoring.clear();
                if let Some(key) = self.session.active.take() {
                    self.activate_key(&key);
                    if let Some(id) = self.active_doc {
                        self.activate_server_of(id);
                    }
                }
            } else if self.connect_dialog.is_none() {
                // A fila anda um servidor por frame.
                ctx.request_repaint();
            }
        }
        if self.session.store.is_none() {
            return;
        }
        let now = Instant::now();
        match self.session.checked {
            Some(last) if now.duration_since(last) < CHECK_EVERY => {
                // Garante um frame para conferir, mesmo sem o usuário mexer.
                ctx.request_repaint_after(CHECK_EVERY - now.duration_since(last));
            }
            _ => {
                self.session.checked = Some(now);
                self.save_session();
            }
        }
    }
}

/// Nome do arquivo com o texto não salvo da janela `id`.
fn text_file(id: u32) -> String {
    format!("query-{id}.sql")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "postpilot-session-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn doc(name: &str, file: Option<&str>) -> SessionDocument {
        SessionDocument {
            name: name.into(),
            unsaved: file.map(|f| Unsaved {
                file: f.into(),
                stamp: (0, 0),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn session_survives_a_restart_with_the_unsaved_text() {
        let dir = temp_dir("restart");
        let path = dir.join("session.json");
        let mut store = SessionStore::open(&path);
        assert!(store.take_loaded().is_none() && store.error.is_none());
        let state = SessionState {
            documents: vec![doc("SQLQuery1.sql", Some("query-1.sql"))],
            query_counter: 1,
            ..Default::default()
        };
        store.save(&state, &[("query-1.sql", "select 1")]).unwrap();
        assert!(!store.changed(&state));

        let mut again = SessionStore::open(&path);
        let loaded = again.take_loaded().expect("saved");
        assert_eq!(loaded.documents[0].name, "SQLQuery1.sql");
        let unsaved = loaded.documents[0].unsaved.as_ref().unwrap();
        assert_eq!(again.read_text(unsaved).as_deref(), Some("select 1"));
    }

    #[test]
    fn texts_nobody_uses_are_removed() {
        let dir = temp_dir("stale");
        let mut store = SessionStore::open(dir.join("session.json"));
        let two = SessionState {
            documents: vec![doc("a", Some("query-1.sql")), doc("b", Some("query-2.sql"))],
            ..Default::default()
        };
        store
            .save(&two, &[("query-1.sql", "a"), ("query-2.sql", "b")])
            .unwrap();
        let one = SessionState {
            documents: vec![doc("b", Some("query-2.sql"))],
            ..Default::default()
        };
        store.save(&one, &[("query-2.sql", "b")]).unwrap();
        let left: Vec<_> = std::fs::read_dir(store.texts_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(left, ["query-2.sql"]);
    }

    #[test]
    fn a_bad_file_starts_empty_and_is_kept_aside() {
        let dir = temp_dir("bad");
        let path = dir.join("session.json");
        std::fs::write(&path, "{ not json").unwrap();
        let mut store = SessionStore::open(&path);
        assert!(store.take_loaded().is_none());
        assert!(store.error.is_some());
        assert!(dir.join("session.json.bad").exists());
        // Um arquivo de versão futura também não derruba o app.
        std::fs::write(&path, r#"{"version": 99}"#).unwrap();
        assert!(SessionStore::open(&path).error.is_some());
    }

    #[test]
    fn old_files_without_new_fields_still_open() {
        let state = parse(r#"{"version": 1, "documents": [{"name": "q.sql"}]}"#).unwrap();
        assert_eq!(state.documents.len(), 1);
        assert!(state.servers.is_empty());
    }

    #[test]
    fn the_text_file_cannot_point_outside_the_session_folder() {
        let dir = temp_dir("escape");
        std::fs::write(dir.join("secret.txt"), "x").unwrap();
        let store = SessionStore::open(dir.join("session.json"));
        let unsaved = Unsaved {
            file: "../secret.txt".into(),
            stamp: (0, 0),
        };
        assert_eq!(store.read_text(&unsaved), None);
    }

    #[test]
    fn the_session_file_has_no_password_field() {
        let state = SessionState {
            servers: vec![SessionServer::default()],
            ..Default::default()
        };
        assert!(!to_json(&state).to_lowercase().contains("password"));
    }
}
