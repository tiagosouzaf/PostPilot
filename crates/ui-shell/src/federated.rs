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
use pgstudio_federated::{FederatedSession, Options, Remote, Server, written_name};
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
/// (como schemas) e, dos que já têm conexão aberta, as tabelas do banco do
/// registro.
#[derive(Default)]
pub struct Cache {
    /// Tabelas lidas, por servidor.
    relations: HashMap<TargetId, Vec<Relation>>,
    asked: HashSet<TargetId>,
    channel: Option<Channel>,
    /// Os nomes dos servidores do catálogo montado.
    names: Vec<(String, TargetId)>,
    catalog: Catalog,
}

type Loaded = (TargetId, Vec<Relation>);

struct Channel(mpsc::Sender<Loaded>, mpsc::Receiver<Loaded>);

impl Cache {
    /// O catálogo para o editor: um schema por servidor.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
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
        tr("A whole table:  server.database.schema.table  (or server.table)").to_string(),
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
        window.session = Some(Arc::new(FederatedSession::new(
            servers,
            Arc::new(remote),
            Options::default(),
        )));
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
    /// nomes das conexões guardadas e, das que têm conexão aberta, as
    /// tabelas (lidas numa thread, uma vez por servidor).
    pub(crate) fn federated_step(&mut self, ctx: &egui::Context) {
        if !self.active_doc.is_some_and(|id| self.is_federated(id)) {
            return;
        }
        let saved = self.multi_targets();
        let names: Vec<(String, TargetId)> =
            saved.iter().map(|t| (t.name.clone(), t.id())).collect();
        let mut changed = names != self.federated.names;
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
            let key = crate::servers::server_key(&c.settings);
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
        let cache = &mut self.federated;
        let Channel(tx, rx) = cache.channel.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel();
            Channel(tx, rx)
        });
        for (id, provider) in providers {
            if !cache.asked.insert(id.clone()) {
                continue;
            }
            let (tx, ctx) = (tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                if let Ok(catalog) = provider.catalog(&id.database) {
                    let _ = tx.send((id, default_relations(catalog)));
                    ctx.request_repaint();
                }
            });
        }
        while let Ok((id, relations)) = rx.try_recv() {
            cache.relations.insert(id, relations);
            changed = true;
        }
        if changed {
            cache.rebuild();
        }
    }

    /// Lê de novo as tabelas dos servidores (Ctrl+Shift+R numa janela
    /// entre servidores).
    pub(crate) fn federated_refresh(&mut self) {
        self.federated.asked.clear();
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
    fn the_template_names_the_servers() {
        let text = template(&[]);
        assert!(text.contains("FROM a."), "{text}");
        assert!(text.lines().take(7).all(|l| l.starts_with("-- ")));
    }
}
