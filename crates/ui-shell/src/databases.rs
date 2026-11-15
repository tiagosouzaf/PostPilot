//! Seletor de banco da janela de consulta ("Available Databases" do SSMS):
//! a lista de bancos de cada servidor e a checagem de transação aberta
//! antes de trocar o banco de uma janela.
//!
//! No PostgreSQL uma conexão fica num banco só; trocar o banco da janela
//! fecha a conexão dela, e a próxima execução abre outra no banco novo.

use pgstudio_i18n::{tr, trf};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui;
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};

/// Bancos que aparecem no seletor (PostgreSQL): os que aceitam conexão,
/// sem os modelos. O SQL de cada motor está em [`pgstudio_core::connection::Engine::list_databases_sql`].
pub const LIST_SQL: &str = "SELECT datname FROM pg_catalog.pg_database \
     WHERE datallowconn AND NOT datistemplate ORDER BY datname";

/// Verdadeiro dentro de um bloco de transação (PostgreSQL): lá o `now()` é
/// o começo da transação, e esta consulta já é um comando posterior.
pub const IN_TRANSACTION_SQL: &str = "SELECT now() <> statement_timestamp()";

/// Resultado que chega da thread do banco.
type Slot<T> = Arc<Mutex<Option<T>>>;

enum Entry {
    Loading(Slot<Result<Vec<String>, String>>),
    Ready(Vec<String>),
    Failed(String),
}

/// Listas de bancos, por servidor (a chave de [`crate::servers::server_key`]).
#[derive(Default)]
pub struct DatabaseLists {
    lists: HashMap<String, Entry>,
}

impl DatabaseLists {
    /// Lê (de novo) a lista do servidor `key`, numa conexão ao banco
    /// `database`. A lista antiga some só quando a nova chegar.
    pub fn refresh(
        &mut self,
        key: &str,
        service: &Arc<dyn QueryService>,
        database: &str,
        ctx: &egui::Context,
    ) {
        if matches!(self.lists.get(key), Some(Entry::Loading(_))) {
            return;
        }
        let slot: Slot<Result<Vec<String>, String>> = Arc::default();
        let session = service.open(database);
        let sql = service.engine().list_databases_sql();
        run_rows(&session, sql, slot.clone(), ctx, |rows| {
            rows.into_iter()
                .filter_map(|r| r.into_iter().next().flatten())
                .collect()
        });
        if let Some(Entry::Ready(old)) = self.lists.remove(key) {
            // Mantém a lista antiga visível enquanto a nova não chega.
            self.lists
                .insert(format!("{key}\u{0}old"), Entry::Ready(old));
        }
        self.lists.insert(key.to_string(), Entry::Loading(slot));
    }

    /// Recebe as listas que chegaram.
    pub fn poll(&mut self) {
        let mut done = Vec::new();
        for (key, entry) in &self.lists {
            if let Entry::Loading(slot) = entry
                && let Some(result) = slot.lock().ok().and_then(|mut s| s.take())
            {
                done.push((key.clone(), result));
            }
        }
        for (key, result) in done {
            self.lists.remove(&format!("{key}\u{0}old"));
            let entry = match result {
                Ok(list) => Entry::Ready(list),
                Err(e) => Entry::Failed(e),
            };
            self.lists.insert(key, entry);
        }
    }

    /// Bancos do servidor `key`: a lista lida, ou a anterior enquanto a
    /// nova carrega.
    pub fn get(&self, key: &str) -> &[String] {
        match self.lists.get(key) {
            Some(Entry::Ready(list)) => list,
            Some(Entry::Loading(_)) => match self.lists.get(&format!("{key}\u{0}old")) {
                Some(Entry::Ready(list)) => list,
                _ => &[],
            },
            _ => &[],
        }
    }

    /// Se a lista do servidor está sendo lida.
    pub fn is_loading(&self, key: &str) -> bool {
        matches!(self.lists.get(key), Some(Entry::Loading(_)))
    }

    /// Se alguma lista está sendo lida.
    pub fn is_loading_any(&self) -> bool {
        self.lists.values().any(|e| matches!(e, Entry::Loading(_)))
    }

    /// Erro da última leitura da lista do servidor.
    pub fn error(&self, key: &str) -> Option<&str> {
        match self.lists.get(key) {
            Some(Entry::Failed(e)) => Some(e),
            _ => None,
        }
    }

    /// Esquece as listas (ao desconectar).
    pub fn clear(&mut self) {
        self.lists.clear();
    }

    /// Esquece a lista de um servidor.
    pub fn forget(&mut self, key: &str) {
        self.lists.remove(key);
        self.lists.remove(&format!("{key}\u{0}old"));
    }
}

/// Pergunta à sessão de uma janela se ela tem uma transação aberta.
/// Uma transação abortada (erro 25P02) também conta como aberta.
pub struct TransactionProbe(Slot<bool>);

impl TransactionProbe {
    pub fn start(session: &Arc<dyn QuerySession>, ctx: &egui::Context) -> Self {
        let answer: Slot<bool> = Arc::default();
        let out = answer.clone();
        let ctx2 = ctx.clone();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();
        let mut aborted = false;
        session.execute(
            session.engine().in_transaction_sql().to_string(),
            Box::new(move |event| match event {
                QueryEvent::Row(r) => rows.push(r),
                QueryEvent::Error(e) => aborted |= e.code == "25P02",
                QueryEvent::Finished { .. } => {
                    let open = aborted || in_transaction(&rows);
                    if let Ok(mut o) = out.lock() {
                        *o = Some(open);
                    }
                    ctx2.request_repaint();
                }
                _ => {}
            }),
        );
        TransactionProbe(answer)
    }

    /// A resposta, quando chegar.
    pub fn poll(&self) -> Option<bool> {
        self.0.lock().ok().and_then(|mut s| s.take())
    }
}

/// Lê a resposta de [`pgstudio_core::connection::Engine::in_transaction_sql`]: `t` no PostgreSQL, um
/// `@@TRANCOUNT` maior que zero no SQL Server.
pub fn in_transaction(rows: &[Vec<Option<String>>]) -> bool {
    match rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_deref())
    {
        Some("t" | "true") => true,
        Some(n) => n.parse::<u32>().is_ok_and(|n| n > 0),
        None => false,
    }
}

/// Roda `sql` e põe em `slot` as linhas convertidas por `convert`, ou o
/// primeiro erro.
fn run_rows<T: Send + 'static>(
    session: &Arc<dyn QuerySession>,
    sql: &str,
    slot: Slot<Result<T, String>>,
    ctx: &egui::Context,
    convert: impl FnOnce(Vec<Vec<Option<String>>>) -> T + Send + 'static,
) {
    let ctx = ctx.clone();
    let mut rows = Vec::new();
    let mut error: Option<String> = None;
    let mut convert = Some(convert);
    session.execute(
        sql.to_string(),
        Box::new(move |event| match event {
            QueryEvent::Row(r) => rows.push(r),
            QueryEvent::Error(e) => {
                error.get_or_insert(e.message);
            }
            QueryEvent::Finished { .. } => {
                let result = match (error.take(), convert.take()) {
                    (Some(e), _) => Err(e),
                    (None, Some(f)) => Ok(f(std::mem::take(&mut rows))),
                    (None, None) => return,
                };
                if let Ok(mut s) = slot.lock() {
                    *s = Some(result);
                }
                ctx.request_repaint();
            }
            _ => {}
        }),
    );
}

/// O que o seletor da toolbar mostra para a janela ativa.
pub struct Picker<'a> {
    /// Banco da janela.
    pub current: String,
    /// Bancos do servidor da janela (pode estar vazia enquanto carrega).
    pub list: &'a [String],
    pub loading: bool,
    pub error: Option<&'a str>,
    /// Se dá para trocar agora (há janela e conexão, e nada executando).
    pub enabled: bool,
}

impl crate::ShellApp {
    /// Servidor da janela `id`, se é o servidor ativo (o único de onde se
    /// pode ler a lista e abrir conexões agora).
    fn active_server_of(&self, id: u32) -> Option<(String, Arc<dyn QueryService>, String)> {
        let c = self.connection.as_ref()?;
        let key = crate::servers::server_key(&c.settings);
        match self.doc_servers.get(&id) {
            Some(k) if *k != key => None,
            _ => Some((key, c.queries.clone(), c.settings.database.clone())),
        }
    }

    /// Para a toolbar: o banco da janela ativa e os outros do servidor.
    pub(crate) fn database_picker(&self) -> Picker<'_> {
        let id = self.active_doc;
        let current = id
            .and_then(|id| self.document_database(id))
            .unwrap_or_default();
        let key = id
            .and_then(|id| self.active_server_of(id))
            .map(|(k, _, _)| k)
            .unwrap_or_default();
        let running = id
            .and_then(|id| self.documents.get(id))
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| e.is_running());
        Picker {
            enabled: id.is_some() && !current.is_empty() && !running && self.db_switch.is_none(),
            current,
            list: self.databases.get(&key),
            loading: self.databases.is_loading(&key),
            error: self.databases.error(&key),
        }
    }

    /// Lê de novo a lista de bancos do servidor da janela ativa (ao abrir
    /// o seletor, como o SSMS).
    pub(crate) fn refresh_database_list(&mut self, ctx: &egui::Context) {
        if let Some((key, service, db)) = self.active_doc.and_then(|id| self.active_server_of(id)) {
            self.databases.refresh(&key, &service, &db, ctx);
        }
    }

    /// Uma vez por frame: recebe as listas e acompanha a troca de banco
    /// pendente. A lista só é lida quando o seletor abre (como no SSMS),
    /// para não abrir conexões que ninguém pediu.
    pub(crate) fn databases_step(&mut self) {
        self.databases.poll();
        if self.database_list_on_load
            && self.database_picker().enabled
            && self.selected_action.is_none()
            && self.scripts.is_empty()
            && !self.is_loading_catalog()
        {
            self.database_list_on_load = false;
            self.open_database_list = true;
        }
        let answer = self
            .db_switch
            .as_ref()
            .and_then(|p| p.probe.as_ref())
            .and_then(TransactionProbe::poll);
        match answer {
            Some(false) => {
                if let Some(p) = self.db_switch.take() {
                    self.switch_database(p.doc, &p.database);
                }
            }
            Some(true) => {
                if let Some(p) = &mut self.db_switch {
                    p.probe = None;
                }
            }
            None => {}
        }
    }

    /// O usuário escolheu `database` para a janela `id`. Sem conexão aberta
    /// troca na hora; com conexão, antes confere se há transação aberta.
    pub(crate) fn request_database(&mut self, ctx: &egui::Context, id: u32, database: String) {
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        if self.document_database(id).as_deref() == Some(database.as_str()) {
            return;
        }
        if doc.execution.as_ref().is_some_and(|e| e.is_running()) {
            self.status = tr("Cannot change the database while a query is executing.").to_string();
            return;
        }
        match doc.session.as_ref().map(|s| s.0.clone()) {
            None => self.switch_database(id, &database),
            Some(session) => {
                self.status = trf!("Changing database to {database}...", database = database);
                self.db_switch = Some(crate::PendingSwitch {
                    doc: id,
                    database,
                    probe: Some(TransactionProbe::start(&session, ctx)),
                });
            }
        }
    }

    /// Troca o banco da janela: solta a conexão dela (a próxima execução
    /// abre outra no banco novo).
    fn switch_database(&mut self, id: u32, database: &str) {
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        if doc.execution.as_ref().is_some_and(|e| e.is_running()) {
            self.status = tr("Cannot change the database while a query is executing.").to_string();
            return;
        }
        doc.session = None;
        doc.database = Some(database.to_string());
        if let Some(exec) = &mut doc.execution {
            // A faixa de status mostra onde a janela está agora.
            exec.database = database.to_string();
        }
        // Como no SSMS, o cursor volta para o editor.
        self.focus_editor = Some(id);
        let text = trf!(
            "Changed database context to '{database}'.",
            database = database
        );
        self.output.push(format!("{}: {text}", doc.name()));
        self.status = text;
    }

    /// Pergunta antes de trocar o banco de uma janela com transação aberta.
    pub(crate) fn database_switch_window(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.db_switch else {
            return;
        };
        if pending.probe.is_some() {
            return;
        }
        let name = self
            .documents
            .get(pending.doc)
            .map(|d| d.name().to_string())
            .unwrap_or_default();
        let database = pending.database.clone();
        let mut answer: Option<bool> = None;
        let modal = crate::theme::modal(egui::Id::new("change_database_tx")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(tr("Change Database"));
            ui.label(trf!(
                "{name} has an open transaction. Changing to {database} closes this \
                 window's connection, and PostgreSQL rolls the transaction back.",
                name = name,
                database = database
            ));
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Cancel")).clicked() {
                    answer = Some(false);
                }
                if ui.button(tr("Roll Back and Change")).clicked() {
                    answer = Some(true);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        let Some(answer) = answer else { return };
        let Some(pending) = self.db_switch.take() else {
            return;
        };
        if answer {
            self.switch_database(pending.doc, &pending.database);
        } else {
            self.status = tr("The database was not changed.").to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgstudio_core::query::{QuerySink, ServerMessage};

    /// Sessão falsa que responde sempre o mesmo.
    struct Canned(Vec<QueryEvent>);

    impl QuerySession for Canned {
        fn execute(&self, _sql: String, mut sink: QuerySink) {
            for e in self.0.clone() {
                sink(e);
            }
            sink(QueryEvent::Finished { canceled: false });
        }
        fn cancel(&self) {}
        fn database(&self) -> String {
            "postgres".into()
        }
    }

    struct Service(Vec<QueryEvent>);

    impl QueryService for Service {
        fn open(&self, _database: &str) -> Arc<dyn QuerySession> {
            Arc::new(Canned(self.0.clone()))
        }
    }

    fn row(v: &str) -> QueryEvent {
        QueryEvent::Row(vec![Some(v.to_string())])
    }

    #[test]
    fn reads_the_list_of_databases() {
        let ctx = egui::Context::default();
        let service: Arc<dyn QueryService> = Arc::new(Service(vec![
            QueryEvent::Columns(vec!["datname".into()]),
            row("loja"),
            row("postgres"),
        ]));
        let mut lists = DatabaseLists::default();
        lists.refresh("srv", &service, "postgres", &ctx);
        lists.poll();
        assert_eq!(lists.get("srv"), ["loja", "postgres"]);
        assert!(!lists.is_loading("srv"));
        assert!(lists.get("other").is_empty());
    }

    #[test]
    fn keeps_the_error_of_the_list() {
        let ctx = egui::Context::default();
        let service: Arc<dyn QueryService> = Arc::new(Service(vec![QueryEvent::Error(
            ServerMessage::error("permission denied"),
        )]));
        let mut lists = DatabaseLists::default();
        lists.refresh("srv", &service, "postgres", &ctx);
        lists.poll();
        assert_eq!(lists.error("srv"), Some("permission denied"));
        assert!(lists.get("srv").is_empty());
    }

    #[test]
    fn probe_sees_open_and_aborted_transactions() {
        let ctx = egui::Context::default();
        let open: Arc<dyn QuerySession> = Arc::new(Canned(vec![row("t")]));
        assert_eq!(TransactionProbe::start(&open, &ctx).poll(), Some(true));
        let idle: Arc<dyn QuerySession> = Arc::new(Canned(vec![row("f")]));
        assert_eq!(TransactionProbe::start(&idle, &ctx).poll(), Some(false));
        let aborted: Arc<dyn QuerySession> =
            Arc::new(Canned(vec![QueryEvent::Error(ServerMessage {
                code: "25P02".into(),
                ..ServerMessage::error("current transaction is aborted")
            })]));
        assert_eq!(TransactionProbe::start(&aborted, &ctx).poll(), Some(true));
    }

    #[test]
    fn sql_server_transaction_count() {
        assert!(in_transaction(&[vec![Some("2".into())]]));
        assert!(!in_transaction(&[vec![Some("0".into())]]));
        assert!(!in_transaction(&[]));
    }
}
