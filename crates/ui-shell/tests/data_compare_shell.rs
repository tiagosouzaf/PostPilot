//! O Data Compare dentro do app, com dois servidores de mentira: abre pelo
//! menu, compara, o script abre numa janela de consulta no servidor de
//! destino e a janela volta ao reabrir o app. Com `--features screenshots`
//! grava também a captura de tela em `PGSTUDIO_SHOTS`.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
#[cfg(not(feature = "screenshots"))]
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector, Engine};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::data_compare::Side;
use pgstudio_ui_shell::data_compare::engine::tables_sql;
use pgstudio_ui_shell::secrets::MemoryStore;
use pgstudio_ui_shell::session::SessionStore;

const SOURCE: &str = "producao.cliente-a.com.br";
const TARGET: &str = "homologacao.cliente-a.com.br";

/// Um servidor de mentira: responde o catálogo, a lista de bancos e o
/// `SELECT` de cada tabela com linhas fixas, diferentes por servidor.
struct Fake(String);

impl QueryService for Fake {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession(self.0.clone(), database.to_string()))
    }
}

struct FakeSession(String, String);

fn rows(lines: &[&str]) -> Vec<Vec<Option<String>>> {
    lines
        .iter()
        .map(|l| {
            l.split('|')
                .map(|v| (v != "NULL").then(|| v.to_string()))
                .collect()
        })
        .collect()
}

impl QuerySession for FakeSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        let source = self.0 == SOURCE;
        let answer = if sql == tables_sql(Engine::PostgreSql) {
            rows(&[
                "public|cliente|id|int4|1|0|0",
                "public|cliente|nome|text|0|0|0",
                "public|cliente|cidade|text|0|0|0",
                "public|cliente|limite|numeric|0|0|0",
                "public|log_acesso|quando|timestamptz|0|0|0",
                "public|pedido|id|int8|1|0|0",
                "public|pedido|cliente_id|int4|0|0|0",
                "public|pedido|total|numeric|0|0|0",
                "public|pedido|status|text|0|0|0",
                "public|produto|sku|varchar|1|0|0",
                "public|produto|descricao|text|0|0|0",
            ])
        } else if sql.contains("pg_database") {
            rows(&["postgres", "vendas"])
        } else if sql.contains("FROM public.cliente ") {
            if source {
                rows(&[
                    "1|Ana Souza|Curitiba|5000.00",
                    "2|Bruno Lima|São Paulo|12000.00",
                    "3|Carla Dias|Recife|NULL",
                    "5|Elisa Prado|Porto Alegre|800.00",
                    "6|Fábio Nunes|Manaus|1500.00",
                ])
            } else {
                rows(&[
                    "1|Ana Souza|Curitiba|5000.00",
                    "2|Bruno Lima|Campinas|9000.00",
                    "3|Carla Dias|Recife|2500.00",
                    "4|Cliente de teste|NULL|0.00",
                ])
            }
        } else if sql.contains("FROM public.pedido ") {
            if source {
                rows(&[
                    "1041|1|350.90|faturado",
                    "1042|2|1280.00|faturado",
                    "1043|5|99.00|aberto",
                ])
            } else {
                rows(&["1041|1|350.90|faturado", "1042|2|1280.00|aberto"])
            }
        } else if sql.contains("FROM public.produto ") {
            rows(&["A-100|Parafuso|A-100", "B-200|Porca|B-200"])
        } else {
            Vec::new()
        };
        std::thread::spawn(move || {
            for r in answer {
                sink(QueryEvent::Row(r));
            }
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.1.clone()
    }
}

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(Fake(s.host.clone())),
        })
    }
}

fn settings(host: &str) -> ConnectionSettings {
    ConnectionSettings {
        host: host.into(),
        user: "tiib".into(),
        ..ConnectionSettings::default()
    }
}

fn start(dir: &Path, store: Arc<MemoryStore>) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector))
        .with_secret_store(store)
        .with_session(SessionStore::open(dir.join("session.json")));
    let h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h
}

fn wait(h: &mut Harness<'static, ShellApp>, what: &str, done: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    loop {
        h.step();
        if done(h.state()) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "never: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
    h.run_steps(3);
}

fn connect(h: &mut Harness<'static, ShellApp>, host: &str) {
    let settings = settings(host);
    h.state_mut().open_connect_dialog();
    {
        let dialog = h.state_mut().connect_dialog_mut().expect("dialog");
        dialog.fill(&settings);
        dialog.password = "x".into();
        dialog.remember = true;
    }
    h.step();
    h.state_mut().connect_to(settings, "x".into());
    wait(h, "connect", |app| {
        app.connect_dialog().is_none() && app.connection().is_some_and(|c| c.settings.host == host)
    });
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("postpilot-dc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Conecta os dois servidores, abre o Data Compare e compara produção
/// (origem) com homologação (destino).
fn compared(dir: &Path, store: Arc<MemoryStore>) -> Harness<'static, ShellApp> {
    let mut h = start(dir, store);
    connect(&mut h, SOURCE);
    connect(&mut h, TARGET);
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::DataCompare);
    h.step();
    {
        let d = h.state_mut().data_compare_mut().expect("window");
        d.source = Side {
            server: pgstudio_ui_shell::servers::server_key(&settings(SOURCE)),
            database: "vendas".into(),
        };
        d.target = Side {
            server: pgstudio_ui_shell::servers::server_key(&settings(TARGET)),
            database: "vendas".into(),
        };
        d.compare(&ctx);
    }
    wait(&mut h, "compared", |app| {
        app.data_compare()
            .is_some_and(|d| !d.is_busy() && d.results().len() == 3)
    });
    h
}

// Sem `screenshots`: a captura troca o idioma do processo inteiro.
#[cfg(not(feature = "screenshots"))]
#[test]
fn the_script_opens_on_the_target_server_and_the_window_comes_back() {
    let dir = temp_dir("shell");
    let store = Arc::new(MemoryStore::default());
    let setup = {
        let mut h = compared(&dir, store.clone());
        let d = h.state().data_compare().unwrap();
        assert_eq!(d.error(), None);
        let counts: Vec<(String, u64)> = d
            .results()
            .iter()
            .map(|r| (r.plan.label(), r.counts.differences()))
            .collect();
        assert_eq!(
            counts,
            [
                ("public.cliente".to_string(), 5),
                ("public.pedido".to_string(), 2),
                ("public.produto".to_string(), 0)
            ]
        );
        assert_eq!(d.statements(), 7);
        assert!(
            h.state()
                .output()
                .iter()
                .any(|l| l.contains("7 different rows in 3 tables"))
        );

        // Desmarcar uma tabela tira os comandos dela do script.
        h.state_mut()
            .data_compare_mut()
            .unwrap()
            .skipped
            .insert("public.pedido".into());
        h.run_steps(2);
        let before = h.state().documents().ids().len();
        h.get_by_label_contains("Generate Script").click();
        h.run_steps(3);
        let ids = h.state().documents().ids().to_vec();
        assert_eq!(ids.len(), before + 1);
        let doc = h.state().documents().get(*ids.last().unwrap()).unwrap();
        assert_eq!(doc.database.as_deref(), Some("vendas"));
        assert!(doc.text.contains("BEGIN;"), "{}", doc.text);
        assert!(
            doc.text
                .contains("DELETE FROM public.cliente WHERE id = '4';")
        );
        assert!(doc.text.contains(
            "UPDATE public.cliente SET cidade = 'São Paulo', limite = '12000.00' WHERE id = '2';"
        ));
        assert!(
            doc.text
                .contains("UPDATE public.cliente SET limite = NULL WHERE id = '3';")
        );
        assert!(doc.text.contains(
            "INSERT INTO public.cliente (id, nome, cidade, limite) \
             VALUES ('6', 'Fábio Nunes', 'Manaus', '1500.00');"
        ));
        assert!(!doc.text.contains("public.pedido"));
        // A janela de consulta é do servidor de destino: é nele que o
        // script roda (e onde vale a confirmação do ambiente).
        let state = h.state().session_state();
        assert_eq!(
            state.documents.last().unwrap().server,
            Some(pgstudio_ui_shell::servers::server_key(&settings(TARGET)))
        );
        let setup = state.data_compare.clone().expect("in the session");
        assert_eq!(setup.source.database, "vendas");
        assert!(setup.skipped.contains("public.pedido"));
        h.state_mut().save_session();
        setup
    };

    // O app abre de novo: a janela volta com os mesmos lados.
    let mut h = start(&dir, store);
    wait(&mut h, "reopened", |app| {
        app.connected_servers().len() == 2 && app.connect_dialog().is_none()
    });
    let d = h.state().data_compare().expect("the window came back");
    assert_eq!(d.setup(), setup);
    // E compara de novo com um clique.
    let ctx = h.ctx.clone();
    h.state_mut().data_compare_mut().unwrap().compare(&ctx);
    wait(&mut h, "compared again", |app| {
        app.data_compare()
            .is_some_and(|d| !d.is_busy() && d.results().len() == 2)
    });
    assert_eq!(h.state().data_compare().unwrap().error(), None);
}

#[cfg(feature = "screenshots")]
#[test]
fn screenshot() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let dir = temp_dir("shot");
    let mut h = compared(&dir, Arc::new(MemoryStore::default()));
    h.state_mut().data_compare_mut().unwrap().selected = Some("public.cliente".into());
    h.run_steps(5);
    let out = std::env::var_os("PGSTUDIO_SHOTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    h.render()
        .expect("render")
        .save(out.join("comparar-dados.png"))
        .expect("save png");
}
