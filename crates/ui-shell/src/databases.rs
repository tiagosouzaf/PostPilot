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

/// Linhas que a lista do seletor mostra de uma vez; o resto rola. A lista
/// abre sempre com esta altura, mesmo vazia ou filtrada, para não mudar de
/// tamanho enquanto carrega ou enquanto se digita.
pub const VISIBLE_ROWS: usize = 13;

/// Os nomes que contêm `filter`, sem diferenciar maiúsculas, na ordem em
/// que vieram. Um filtro vazio deixa todos.
pub fn matching<'a>(names: &[&'a str], filter: &str) -> Vec<&'a str> {
    let filter = filter.trim().to_lowercase();
    names
        .iter()
        .copied()
        .filter(|n| filter.is_empty() || n.to_lowercase().contains(&filter))
        .collect()
}

/// O que a lista aberta guarda entre um frame e outro.
#[derive(Clone, Default)]
struct ListState {
    filter: String,
    /// Linha marcada pelo teclado, entre as que o filtro deixou.
    highlight: Option<usize>,
    /// A lista acabou de abrir: o cursor vai para a pesquisa e a lista
    /// rola até o banco atual.
    opening: bool,
}

fn list_state_id() -> egui::Id {
    egui::Id::new("database_picker_list")
}

/// A lista vai abrir: começa sem filtro e com o cursor na pesquisa.
pub(crate) fn reset_list(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        d.insert_temp(
            list_state_id(),
            ListState {
                opening: true,
                ..Default::default()
            },
        );
    });
}

impl Picker<'_> {
    /// O conteúdo da lista aberta: a pesquisa em cima e, embaixo, os
    /// bancos que ela deixa passar. Devolve o banco escolhido (com um
    /// clique ou com Enter).
    pub(crate) fn list_ui(&self, ui: &mut egui::Ui) -> Option<String> {
        let mut state: ListState = ui.data(|d| d.get_temp(list_state_id())).unwrap_or_default();
        let opening = std::mem::take(&mut state.opening);
        let mut picked: Option<String> = None;

        let search = ui.add(
            egui::TextEdit::singleline(&mut state.filter)
                .hint_text(tr("Search databases"))
                .desired_width(f32::INFINITY),
        );
        if opening {
            search.request_focus();
        }
        if search.changed() {
            state.highlight = None;
        }

        let mut names: Vec<&str> = self.list.iter().map(String::as_str).collect();
        if !self.current.is_empty() && !names.contains(&self.current.as_str()) {
            names.insert(0, &self.current);
        }
        let shown = matching(&names, &state.filter);

        // Setas andam pela lista e Enter escolhe, com o cursor na pesquisa.
        let (down, up, enter) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
            )
        });
        let filtering = !state.filter.trim().is_empty();
        let start = shown.iter().position(|n| *n == self.current);
        let mut moved = false;
        if !shown.is_empty() && (down || up) {
            let last = shown.len() - 1;
            let at = state.highlight.or(if filtering { None } else { start });
            state.highlight = Some(match (at, down) {
                (None, true) => 0,
                (None, false) => last,
                (Some(i), true) => (i + 1).min(last),
                (Some(i), false) => i.saturating_sub(1),
            });
            moved = true;
        }
        // Com filtro, Enter sem seta escolhe o primeiro que sobrou.
        let highlight =
            state
                .highlight
                .filter(|i| *i < shown.len())
                .or(if filtering && !shown.is_empty() {
                    Some(0)
                } else {
                    None
                });
        if enter && let Some(i) = highlight {
            picked = Some(shown[i].to_string());
        }

        let spacing = ui.spacing().item_spacing.y;
        // A altura de um `selectable_label`: o texto com a folga do botão,
        // ou a altura mínima dos controles, o que for maior.
        let row = (ui.text_style_height(&egui::TextStyle::Button)
            + 2.0 * ui.spacing().button_padding.y)
            .max(ui.spacing().interact_size.y);
        let height = VISIBLE_ROWS as f32 * (row + spacing) - spacing;
        egui::ScrollArea::vertical()
            .id_salt("database_picker_rows")
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_height(height);
                for (i, name) in shown.iter().enumerate() {
                    let current = *name == self.current;
                    let mut response = ui.selectable_label(current, *name);
                    if highlight == Some(i) && !current {
                        response = response.highlight();
                    }
                    if (moved && highlight == Some(i)) || (opening && current) {
                        response.scroll_to_me(Some(egui::Align::Center));
                    }
                    if response.clicked() {
                        picked = Some(name.to_string());
                    }
                }
                if self.loading {
                    ui.weak(tr("Loading..."));
                } else if let Some(e) = self.error {
                    ui.weak(e);
                } else if shown.is_empty() {
                    ui.weak(tr("No databases found."));
                }
            });

        ui.data_mut(|d| d.insert_temp(list_state_id(), state));
        if picked.is_some() {
            ui.close();
        }
        picked
    }
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
            crate::fit::dialog_width(ui, 440.0);
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
    fn the_search_ignores_case_and_keeps_the_order() {
        let names = ["Evolution", "finops", "tiib", "tiib_signer_prd"];
        assert_eq!(matching(&names, ""), names);
        assert_eq!(matching(&names, " TIIB "), ["tiib", "tiib_signer_prd"]);
        assert_eq!(matching(&names, "evo"), ["Evolution"]);
        assert!(matching(&names, "xyz").is_empty());
    }

    #[test]
    fn sql_server_transaction_count() {
        assert!(in_transaction(&[vec![Some("2".into())]]));
        assert!(!in_transaction(&[vec![Some("0".into())]]));
        assert!(!in_transaction(&[]));
    }
}
