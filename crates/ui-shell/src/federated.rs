//! Consulta entre servidores na janela de consulta.
//!
//! Uma janela de consulta pode ser "entre servidores" (Query > Cross-Server
//! Query): o F5 dela não roda numa conexão, e sim no
//! [`pgstudio_federated`], que lê de cada conexão guardada o que a consulta
//! pede e faz o join aqui. Os servidores são as conexões guardadas (as do
//! workspace e as dos Registered Servers), pelo nome; os que já estão
//! conectados no Object Explorer usam a conexão deles, os outros conectam
//! com a senha guardada.
//!
//! O resultado sai como o de qualquer consulta (grade, texto, arquivo,
//! exportar). A consulta só lê, então não há confirmação de Produção. A
//! janela continua "entre servidores" ao reabrir o app (veja
//! [`crate::session`]).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, mpsc};

use eframe::egui;
use pgstudio_core::catalog::{Catalog, Relation, Schema};
use pgstudio_core::connection::{Connector, Engine};
use pgstudio_core::metadata::MetadataProvider;
use pgstudio_core::query::QuerySession;
use pgstudio_federated::{FederatedSession, Home, Options, Remote, Server, written_name};
use pgstudio_i18n::{tr, trf};

use crate::ShellApp;
use crate::multi::{Connections, RunTarget, Target, TargetId};
use crate::secrets::SecretStore;

/// O que uma janela entre servidores guarda.
#[derive(Default)]
pub struct Window {
    /// Conexões abertas para a janela (as dos servidores que não estão no
    /// Object Explorer), reaproveitadas entre execuções.
    connections: Connections,
    /// A sessão da última execução.
    session: Option<Arc<FederatedSession>>,
}

impl std::fmt::Debug for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Window").finish_non_exhaustive()
    }
}

impl Window {
    pub fn session(&self) -> Option<Arc<dyn QuerySession>> {
        self.session.clone().map(|s| s as Arc<dyn QuerySession>)
    }
}

/// As conexões guardadas como servidores de uma execução.
struct Servers {
    targets: Vec<RunTarget>,
    connector: Option<Arc<dyn Connector>>,
    secrets: Option<Arc<dyn SecretStore>>,
    connected: Connections,
}

impl Remote for Servers {
    fn session(&self, server: usize, database: &str) -> Result<Arc<dyn QuerySession>, String> {
        let target = self
            .targets
            .get(server)
            .ok_or_else(|| tr("This server is no longer in the saved connections.").to_string())?;
        let service = crate::multi::open_service(
            target,
            self.connector.as_ref(),
            self.secrets.as_ref(),
            &self.connected,
        )?;
        Ok(service.open(database))
    }

    fn read_only(&self, engine: Engine, query: &str) -> Result<String, String> {
        use pgstudio_automation::safety::{Safety, classify_for};
        match (classify_for(engine, query), engine) {
            // No PostgreSQL o servidor também garante: a transação é só
            // de leitura.
            (Safety::ReadOnly | Safety::Unknown { .. }, Engine::PostgreSql) => Ok(format!(
                "BEGIN TRANSACTION READ ONLY;\n{query}\n;\nROLLBACK;"
            )),
            (Safety::ReadOnly, _) => Ok(query.to_string()),
            (Safety::Writes { reason }, _) | (Safety::Unknown { reason }, _) => Err(reason),
        }
    }
}

/// O que o IntelliSense das janelas entre servidores sabe: os servidores
/// (como schemas) e as tabelas do banco do registro, dos que já têm
/// conexão aberta e dos que a consulta da janela cita (esses conectam em
/// segundo plano, com a senha guardada).
#[derive(Default)]
pub struct Cache {
    /// Tabelas lidas, por servidor.
    relations: HashMap<TargetId, Vec<Relation>>,
    asked: HashSet<TargetId>,
    /// Leituras em andamento e as que falharam.
    loads: HashMap<TargetId, Load>,
    channel: Option<Channel>,
    /// Os nomes dos servidores do catálogo montado.
    names: Vec<(String, TargetId)>,
    /// A janela e o texto de onde saíram os `mentioned`.
    scanned: Option<(u32, u64)>,
    /// Os servidores que a consulta da janela ativa cita.
    mentioned: Vec<TargetId>,
    catalog: Catalog,
}

/// Como está a leitura das tabelas de um servidor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Load {
    Reading,
    Failed(String),
}

type Loaded = (TargetId, Result<Vec<Relation>, String>);

struct Channel(mpsc::Sender<Loaded>, mpsc::Receiver<Loaded>);

impl Cache {
    /// O catálogo para o editor: um schema por servidor.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Como está a leitura das tabelas do servidor de nome `name`: `None`
    /// quando já chegou ou ninguém pediu.
    pub fn load(&self, name: &str) -> Option<&Load> {
        let (_, id) = self.names.iter().find(|(n, _)| n == name)?;
        self.loads.get(id)
    }

    fn rebuild(&mut self) {
        self.catalog = Catalog {
            // Aspas e nomes pelas regras do PostgreSQL, que o SQLite e o
            // leitor da consulta também aceitam.
            engine: Engine::PostgreSql,
            database: String::new(),
            search_path: Vec::new(),
            schemas: self
                .names
                .iter()
                .map(|(name, id)| Schema {
                    name: name.clone(),
                    relations: self.relations.get(id).cloned().unwrap_or_default(),
                    functions: Vec::new(),
                })
                .collect(),
        };
    }
}

/// As tabelas que valem sem dizer o schema: as dos schemas do
/// `search_path`, ou todas quando o motor não tem um.
fn default_relations(catalog: Catalog) -> Vec<Relation> {
    let path = catalog.search_path;
    let mut out: Vec<Relation> = Vec::new();
    for schema in catalog.schemas {
        if path.is_empty() || path.contains(&schema.name) {
            out.extend(schema.relations);
        }
    }
    out
}

/// Os servidores que a consulta cita: os nomes antes de um `.` ou de um
/// `(` no lugar de uma tabela (depois de `FROM`, `JOIN` ou vírgula).
fn mentioned(text: &str, names: &[(String, TargetId)]) -> Vec<TargetId> {
    use pgstudio_sql_editor::lexer::{TokenKind, tokenize};
    let known: Vec<&str> = names.iter().map(|(n, _)| n.as_str()).collect();
    let tokens = tokenize(text);
    let tokens: Vec<_> = tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();
    let word = |i: usize| &text[tokens[i].range.clone()];
    let mut out: Vec<TargetId> = Vec::new();
    for i in 1..tokens.len().saturating_sub(1) {
        let written = match tokens[i].kind {
            TokenKind::Identifier => word(i).to_string(),
            TokenKind::QuotedIdentifier => {
                let w = word(i);
                match w.strip_prefix('"').and_then(|w| w.strip_suffix('"')) {
                    Some(inner) if !inner.is_empty() => inner.replace("\"\"", "\""),
                    _ => continue,
                }
            }
            _ => continue,
        };
        let before = word(i - 1);
        let table_position = before == ","
            || (tokens[i - 1].kind == TokenKind::Keyword
                && (before.eq_ignore_ascii_case("from") || before.eq_ignore_ascii_case("join")));
        if !table_position || !matches!(word(i + 1), "." | "(") {
            continue;
        }
        if let Some(n) = pgstudio_federated::server_named(&known, &written) {
            let id = &names[n].1;
            if !out.contains(id) {
                out.push(id.clone());
            }
        }
    }
    out
}

/// Se o cursor (em caracteres) está logo depois de um `nome.`, com ou sem
/// o começo de outro nome: é onde a lista de tabelas de um servidor abre.
fn after_dot(text: &str, cursor: usize) -> bool {
    let before: Vec<char> = text.chars().take(cursor).collect();
    let word = before
        .iter()
        .rev()
        .take_while(|c| c.is_alphanumeric() || **c == '_')
        .count();
    before.len() > word && before[before.len() - word - 1] == '.'
}

/// O texto de exemplo de uma janela nova, com os nomes dos servidores
/// guardados.
fn template(servers: &[Target]) -> String {
    let names: Vec<String> = servers.iter().map(|t| written_name(&t.name)).collect();
    let list = if names.is_empty() {
        tr("(no saved connections yet: register servers in the workspace or in Registered Servers)")
            .to_string()
    } else {
        names.join(", ")
    };
    let first = names.first().cloned().unwrap_or_else(|| "a".into());
    let second = names.get(1).cloned().unwrap_or_else(|| "b".into());
    let lines = [
        tr("Cross-server query: one SELECT that joins data from saved connections.").to_string(),
        tr("A table:        server.database.schema.table  (or server.table; no server: this window's)")
            .to_string(),
        tr("Filters, join keys, ORDER BY and LIMIT run on each server; a huge table stops first.")
            .to_string(),
        tr("A server query: server($$ select ... where ... $$) alias  (filters on the server)")
            .to_string(),
        tr("MongoDB:        server.collection  or  server($$ db.orders.find({...}) $$)")
            .to_string(),
        tr("Elasticsearch:  server.index       Redis: server.\"user:*\"  (key, type, value)")
            .to_string(),
        tr("The outer SQL is SQLite; json_extract reads inside documents. Read-only.").to_string(),
        trf!("Servers: {servers}", servers = list),
    ];
    let mut text: String = lines.iter().map(|l| format!("-- {l}\n")).collect();
    text.push_str(&format!(
        "\nSELECT a.*, b.*\nFROM {first}.{table} a\nJOIN {second}.{table} b ON b.id = a.id\nLIMIT 100;\n",
        table = tr("table")
    ));
    text
}

/// Faixa de status de uma janela entre servidores que ainda não executou.
pub fn strip(ui: &mut egui::Ui) {
    crate::fit::ends(
        ui,
        |ui| {
            ui.label(egui_phosphor::regular::INTERSECT);
            crate::fit::text(
                ui,
                tr("Cross-server query: joins data from the saved connections."),
            );
        },
        |_| {},
    );
}

impl ShellApp {
    /// Se a janela `id` é uma consulta entre servidores.
    pub fn is_federated(&self, id: u32) -> bool {
        self.documents
            .get(id)
            .is_some_and(|d| d.federated.is_some())
    }

    /// Liga ou desliga a consulta entre servidores na janela `id`. Ao
    /// ligar numa janela vazia, entra um exemplo com os servidores.
    pub fn set_federated(&mut self, id: u32, on: bool) {
        let targets = self.multi_targets();
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        if doc.execution.as_ref().is_some_and(|e| e.is_running()) {
            self.status = tr("A query is already executing in this window.").to_string();
            return;
        }
        if on {
            doc.federated = Some(Window::default());
            doc.multi = None;
            if doc.text.trim().is_empty() {
                doc.text = template(&targets);
            }
            self.status =
                tr("This query window joins data from the saved connections.").to_string();
        } else {
            doc.federated = None;
            self.status = tr("This query window runs on its own connection.").to_string();
        }
    }

    /// "Cross-Server Query": liga na janela ativa (ou desliga, se já
    /// estava ligada); sem janela, abre uma.
    pub(crate) fn toggle_federated(&mut self) {
        let id = match self.active_doc {
            Some(id) => id,
            None => {
                let id = self.documents.new_query();
                self.workspace.open(crate::panels::Panel::Query(id));
                self.active_doc = Some(id);
                id
            }
        };
        let on = !self.is_federated(id);
        self.set_federated(id, on);
    }

    /// F5 numa janela entre servidores. Devolve `false` quando a janela é
    /// comum (o F5 comum segue).
    pub(crate) fn execute_federated(&mut self, ctx: &egui::Context, id: u32, plain: bool) -> bool {
        if !self.is_federated(id) {
            return false;
        }
        if !plain {
            self.status =
                tr("Plans and Object Information are not available in a cross-server query.")
                    .to_string();
            return true;
        }
        let Some(doc) = self.documents.get(id) else {
            return true;
        };
        if doc.execution.as_ref().is_some_and(|e| e.is_running()) {
            self.status = tr("A query is already executing in this window.").to_string();
            return true;
        }
        let (sql, offset) = match pgstudio_sql_editor::selection(ctx, crate::panels::editor_id(id))
        {
            Some((a, b)) if a != b => {
                let (a, b) = (a.min(b), a.max(b));
                (doc.text.chars().skip(a).take(b - a).collect(), a)
            }
            _ => (doc.text.clone(), 0),
        };
        // Os registros e as conexões do Object Explorer como estão agora.
        let saved = self.multi_targets();
        let ids: Vec<TargetId> = saved.iter().map(Target::id).collect();
        let targets: Vec<RunTarget> = self
            .run_targets(&ids, &saved)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        let mut targets = targets;
        // O servidor da janela: as tabelas escritas sem servidor na frente
        // são dele, no banco escolhido na barra.
        let key = self.doc_servers.get(&id).cloned().or_else(|| {
            self.connection
                .as_ref()
                .map(|c| crate::servers::server_key(&c.settings))
        });
        let window_database = self.document_database(id).filter(|d| !d.is_empty());
        let home = key.and_then(|key| {
            // Vários registros podem ser do mesmo servidor (um por banco):
            // vale o do banco da janela.
            let same =
                |t: &RunTarget| !t.missing && crate::servers::key_on_server(&key, &t.settings);
            let server = match targets
                .iter()
                .position(|t| same(t) && Some(&t.settings.database) == window_database.as_ref())
                .or_else(|| targets.iter().position(same))
            {
                Some(at) => at,
                None => {
                    // Conectado, mas sem registro guardado: vale a conexão.
                    let live = self
                        .connection
                        .iter()
                        .chain(
                            self.servers
                                .parked
                                .iter()
                                .filter_map(|p| p.connection.as_ref()),
                        )
                        .find(|c| crate::servers::server_key(&c.settings) == key)?;
                    targets.push(RunTarget {
                        name: live.settings.host.clone(),
                        settings: live.settings.clone(),
                        live: Some(live.queries.clone()),
                        missing: false,
                    });
                    targets.len() - 1
                }
            };
            let database = window_database
                .clone()
                .unwrap_or_else(|| targets[server].settings.database.clone());
            Some(Home { server, database })
        });
        let servers: Vec<Server> = targets
            .iter()
            .map(|t| Server {
                name: t.name.clone(),
                engine: t.settings.engine,
                database: t.settings.database.clone(),
            })
            .collect();
        let (connector, secrets) = (self.connector.clone(), self.secrets.clone());
        let Some(window) = self
            .documents
            .get_mut(id)
            .and_then(|d| d.federated.as_mut())
        else {
            return true;
        };
        let remote = Servers {
            targets,
            connector,
            secrets,
            connected: window.connections.clone(),
        };
        window.session = Some(Arc::new(
            FederatedSession::new(servers, Arc::new(remote), Options::default()).with_home(home),
        ));
        self.launch(ctx, id, sql, offset, crate::Run::Results);
        true
    }

    /// As janelas entre servidores, para a faixa de status e o histórico.
    pub(crate) fn federated_labels(&self) -> Vec<(u32, String)> {
        self.documents
            .ids()
            .into_iter()
            .filter(|id| self.is_federated(*id))
            .map(|id| (id, tr("Cross-server").to_string()))
            .collect()
    }

    /// Mantém o catálogo do IntelliSense das janelas entre servidores: os
    /// nomes das conexões guardadas e as tabelas das que têm conexão
    /// aberta e das que a consulta cita. Tudo é lido em threads, uma vez
    /// por servidor; um servidor citado e ainda sem conexão conecta com a
    /// senha guardada (a conexão fica com a janela, e o F5 usa a mesma).
    pub(crate) fn federated_step(&mut self, ctx: &egui::Context) {
        let Some(doc_id) = self.active_doc.filter(|id| self.is_federated(*id)) else {
            return;
        };
        let saved = self.multi_targets();
        let names: Vec<(String, TargetId)> =
            saved.iter().map(|t| (t.name.clone(), t.id())).collect();
        let mut changed = names != self.federated.names;
        if changed {
            self.federated.scanned = None;
        }
        self.federated.names = names;

        // De quem dá para ler as tabelas: os servidores conectados no
        // Object Explorer e os que as janelas entre servidores já abriram.
        let mut providers: Vec<(TargetId, Arc<dyn MetadataProvider>)> = Vec::new();
        let live = self.connection.iter().chain(
            self.servers
                .parked
                .iter()
                .filter_map(|p| p.connection.as_ref()),
        );
        for c in live {
            let key = crate::servers::login_key(&c.settings);
            for t in saved.iter().filter(|t| t.id().server == key) {
                providers.push((t.id(), c.metadata.clone()));
            }
        }
        for id in self.documents.ids() {
            let Some(window) = self.documents.get(id).and_then(|d| d.federated.as_ref()) else {
                continue;
            };
            if let Ok(open) = window.connections.try_lock() {
                for (id, c) in open.iter() {
                    providers.push((id.clone(), c.metadata.clone()));
                }
            }
        }

        // Os servidores que a consulta cita (relido quando o texto muda).
        let mut connections: Option<Connections> = None;
        if let Some(doc) = self.documents.get(doc_id) {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            doc.text.hash(&mut hasher);
            let stamp = Some((doc_id, hasher.finish()));
            if self.federated.scanned != stamp {
                self.federated.scanned = stamp;
                // O exemplo de uma janela nova cita os dois primeiros
                // servidores: ninguém os pediu ainda.
                self.federated.mentioned = if doc.text == template(&saved) {
                    Vec::new()
                } else {
                    mentioned(&doc.text, &self.federated.names)
                };
            }
            connections = doc.federated.as_ref().map(|w| w.connections.clone());
        }

        let (connector, secrets) = (self.connector.clone(), self.secrets.clone());
        let cache = &mut self.federated;
        let Channel(tx, rx) = cache.channel.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel();
            Channel(tx, rx)
        });
        let mut status: Option<String> = None;
        for (id, provider) in &providers {
            if !cache.asked.insert(id.clone()) {
                continue;
            }
            cache.loads.insert(id.clone(), Load::Reading);
            let (tx, ctx, id, provider) = (tx.clone(), ctx.clone(), id.clone(), provider.clone());
            std::thread::spawn(move || {
                let result = provider
                    .catalog(&id.database)
                    .map(default_relations)
                    .map_err(|e| e.to_string());
                let _ = tx.send((id, result));
                ctx.request_repaint();
            });
        }
        for id in cache.mentioned.clone() {
            let (Some(target), Some(connections)) =
                (saved.iter().find(|t| t.id() == id), connections.as_ref())
            else {
                continue;
            };
            if !cache.asked.insert(id.clone()) {
                continue;
            }
            cache.loads.insert(id.clone(), Load::Reading);
            status = Some(trf!(
                "Reading the tables of {server} in the background...",
                server = written_name(&target.name)
            ));
            let target = RunTarget {
                name: target.name.clone(),
                settings: target.settings.clone(),
                live: None,
                missing: false,
            };
            let (tx, ctx, connections) = (tx.clone(), ctx.clone(), connections.clone());
            let (connector, secrets) = (connector.clone(), secrets.clone());
            std::thread::spawn(move || {
                let result = crate::multi::open_service(
                    &target,
                    connector.as_ref(),
                    secrets.as_ref(),
                    &connections,
                )
                .and_then(|_| {
                    let open = connections.lock().unwrap_or_else(|e| e.into_inner());
                    open.get(&id)
                        .map(|c| c.metadata.clone())
                        .ok_or_else(|| tr("Connect to a server to run queries.").to_string())
                })
                .and_then(|metadata| metadata.catalog(&id.database).map_err(|e| e.to_string()))
                .map(default_relations);
                let _ = tx.send((id, result));
                ctx.request_repaint();
            });
        }
        let mut arrived = false;
        while let Ok((id, result)) = rx.try_recv() {
            let name = cache
                .names
                .iter()
                .find(|(_, i)| *i == id)
                .map(|(n, _)| written_name(n));
            let cited = cache.mentioned.contains(&id);
            match result {
                Ok(relations) => {
                    if let (Some(name), true) = (name, cited) {
                        status = Some(trf!(
                            "{server}: {count} tables and views ready for IntelliSense.",
                            server = name,
                            count = relations.len()
                        ));
                        arrived = true;
                    }
                    cache.loads.remove(&id);
                    cache.relations.insert(id, relations);
                    changed = true;
                }
                Err(error) => {
                    if let (Some(name), true) = (name, cited) {
                        status = Some(trf!(
                            "Could not read the tables of {server}: {error} (Ctrl+Shift+R tries again)",
                            server = name,
                            error = error
                        ));
                    }
                    cache.loads.insert(id, Load::Failed(error));
                }
            }
        }
        if changed {
            cache.rebuild();
        }
        if let Some(status) = status {
            self.status = status;
        }
        // As tabelas chegaram e o cursor ainda está depois do ponto: a
        // lista abre sozinha, como se a pessoa tivesse acabado de digitar.
        if arrived
            && let Some((a, c)) =
                pgstudio_sql_editor::selection(ctx, crate::panels::editor_id(doc_id))
            && a == c
            && let Some(doc) = self.documents.get_mut(doc_id)
            && after_dot(&doc.text, c)
        {
            doc.editor.list_members();
        }
    }

    /// As tabelas que o IntelliSense das janelas entre servidores conhece.
    pub fn federated_catalog(&self) -> &Catalog {
        self.federated.catalog()
    }

    /// Como está a leitura das tabelas do servidor `name` para o
    /// IntelliSense das janelas entre servidores.
    pub fn federated_load(&self, name: &str) -> Option<&Load> {
        self.federated.load(name)
    }

    /// Lê de novo as tabelas dos servidores (Ctrl+Shift+R numa janela
    /// entre servidores).
    pub(crate) fn federated_refresh(&mut self) {
        self.federated.asked.clear();
        self.federated.loads.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgstudio_core::catalog::{Column, RelationKind};

    #[test]
    fn the_default_tables_are_the_ones_of_the_search_path() {
        let rel = |name: &str| Relation {
            name: name.into(),
            kind: RelationKind::Table,
            columns: vec![Column {
                name: "id".into(),
                ty: "int".into(),
            }],
        };
        let schema = |name: &str, relations| Schema {
            name: name.into(),
            relations,
            functions: Vec::new(),
        };
        let catalog = Catalog {
            search_path: vec!["public".into()],
            schemas: vec![
                schema("public", vec![rel("clientes")]),
                schema("pg_catalog", vec![rel("pg_class")]),
            ],
            ..Default::default()
        };
        let names: Vec<String> = default_relations(catalog)
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(names, ["clientes"]);
    }

    #[test]
    fn the_cache_lists_servers_as_schemas() {
        let id = TargetId {
            server: "u@h:1".into(),
            database: "d".into(),
        };
        let mut cache = Cache {
            names: vec![("PG Vendas".into(), id.clone())],
            ..Default::default()
        };
        cache.relations.insert(
            id,
            vec![Relation {
                name: "clientes".into(),
                kind: RelationKind::Table,
                columns: Vec::new(),
            }],
        );
        cache.rebuild();
        let schema = cache.catalog().schema("\"PG Vendas\"").unwrap();
        assert_eq!(schema.relations[0].name, "clientes");
    }

    #[test]
    fn the_servers_a_query_cites_are_the_ones_in_table_position() {
        let id = |n: &str| TargetId {
            server: n.into(),
            database: "d".into(),
        };
        let names = vec![
            ("Tiib@Production".to_string(), id("a")),
            ("PG Vendas".to_string(), id("b")),
            ("d".to_string(), id("c")),
        ];
        let found = |sql: &str| mentioned(sql, &names);
        assert_eq!(
            found("SELECT * FROM documento d inner join \"Tiib@Production\". ORDER BY d.id"),
            [id("a")]
        );
        assert_eq!(
            found("select * from pg_vendas.x, \"tiib@production\"($$ select 1 $$) t"),
            [id("b"), id("a")]
        );
        // Um apelido com nome de servidor não é o servidor.
        assert!(found("select d.id from documento d where d.id = 1").is_empty());
        // Ainda digitando o nome: nada a ler.
        assert!(found("select * from \"Tiib@Prod").is_empty());
        assert!(found("select * from \"Tiib@Production\"").is_empty());
    }

    #[test]
    fn the_list_reopens_only_right_after_a_dot() {
        assert!(after_dot("from \"A\".", 9));
        assert!(after_dot("from a.doc", 10));
        assert!(!after_dot("from a.doc ", 11));
        assert!(!after_dot("from a", 6));
        assert!(!after_dot("", 0));
    }

    #[test]
    fn the_template_names_the_servers() {
        let text = template(&[]);
        assert!(text.contains("FROM a."), "{text}");
        assert!(text.lines().take(7).all(|l| l.starts_with("-- ")));
    }
}
