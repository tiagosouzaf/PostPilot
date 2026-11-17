//! Importar CSV: a janela com uma sessão de mentira, e a carga contra um
//! PostgreSQL (`PGSTUDIO_TEST_HOST`) e um SQL Server (`MSSQL_TEST_HOST`) de
//! verdade; sem as variáveis, esses testes só avisam.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectionSettings, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage};
use pgstudio_ui_shell::csv_import::{self, ColumnKind, CsvOptions, Plan, Progress};
use pgstudio_ui_shell::csv_import_dialog::{ImportDialog, Phase};
use pgstudio_ui_shell::registered::Rgb;

fn temp_file(name: &str, content: &[u8]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("postpilot-{}-{name}", std::process::id()));
    std::fs::write(&path, content).unwrap();
    path
}

/// Sessão que responde as colunas da tabela e guarda o resto do SQL.
struct Fake(Arc<Mutex<Vec<String>>>);

impl QuerySession for Fake {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        if sql.contains("pg_attribute") {
            let s = |v: &str| Some(v.to_string());
            for (name, kind, flag) in [
                ("id", "integer", "i"),
                ("nome", "text", ""),
                ("valor", "numeric(10,2)", ""),
                ("dobro", "numeric", "c"),
            ] {
                sink(QueryEvent::Row(vec![
                    s("public"),
                    s("cliente"),
                    s(name),
                    s(kind),
                    s(flag),
                ]));
            }
        } else {
            let failed = sql.contains("'erro'");
            self.0.lock().unwrap().push(sql);
            if failed {
                sink(QueryEvent::Error(ServerMessage::error("valor inválido")));
            }
        }
        sink(QueryEvent::Finished { canceled: false });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        "loja".into()
    }
}

type DialogHarness = Harness<'static, ImportDialog>;

fn dialog(
    csv: &[u8],
    name: &str,
    environment: Option<&str>,
) -> (DialogHarness, Arc<Mutex<Vec<String>>>, PathBuf) {
    let path = temp_file(name, csv);
    let log = Arc::new(Mutex::new(Vec::new()));
    let session: Arc<dyn QuerySession> = Arc::new(Fake(log.clone()));
    let environment = environment.map(|e| (e.to_string(), Rgb([200, 30, 30])));
    let ctx = eframe::egui::Context::default();
    let dialog = ImportDialog::open(session, 42, path.clone(), environment, &ctx);
    let mut harness = Harness::builder()
        .with_size([1000.0, 760.0])
        .build_ui_state(
            |ui, d: &mut ImportDialog| {
                d.show(ui.ctx());
            },
            dialog,
        );
    wait(&mut harness, |d| !d.is_loading());
    (harness, log, path)
}

fn wait(harness: &mut DialogHarness, done: impl Fn(&ImportDialog) -> bool) {
    let start = Instant::now();
    loop {
        harness.step();
        if done(harness.state()) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "never happened");
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
}

#[test]
fn the_window_maps_previews_and_loads_the_file() {
    let (mut harness, log, path) = dialog(
        b"Nome;VALOR;extra\nAna;1.50;x\nBia;;y\n",
        "janela.csv",
        None,
    );
    {
        let d = harness.state();
        // O ponto e vírgula foi descoberto e as colunas casaram pelo nome
        // (a identidade não está no arquivo; a calculada nunca entra).
        assert_eq!(d.options.delimiter, b';');
        assert_eq!(d.mapping, vec![None, Some(0), Some(1), None]);
        assert_eq!(d.target().unwrap().columns[3].kind, ColumnKind::Computed);
    }
    harness.get_by_label("Import CSV into public.cliente");
    harness.get_by_label("(computed by the database)");
    // A prévia mostra o valor e o NULL do campo vazio.
    harness.get_by_label("Bia");
    harness.get_by_label("NULL");
    // Fora de Produção também pergunta antes de carregar.
    harness.get_by_label("Import").click();
    harness.run();
    assert_eq!(*harness.state().phase(), Phase::Confirming);
    assert!(
        log.lock().unwrap().is_empty(),
        "nothing runs before the yes"
    );
    harness.get_by_label("The import adds rows to public.cliente.");
    harness.get_by_label("Import").click();
    wait(&mut harness, |d| matches!(d.phase(), Phase::Done(_)));
    assert_eq!(
        *log.lock().unwrap(),
        [
            "BEGIN",
            "INSERT INTO public.cliente (nome, valor) VALUES ('Ana', '1.50'),\n('Bia', NULL)",
            "COMMIT",
        ]
    );
    let Phase::Done(text) = harness.state().phase() else {
        unreachable!()
    };
    assert!(text.starts_with("Imported 2 row(s) from "), "{text}");
    harness.get_by_label("Close");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn an_environment_that_confirms_changes_asks_before_loading() {
    let (mut harness, log, path) =
        dialog(b"nome,valor\nAna,1\n", "producao.csv", Some("Production"));
    harness.get_by_label("Import").click();
    harness.run();
    assert_eq!(*harness.state().phase(), Phase::Confirming);
    assert!(
        log.lock().unwrap().is_empty(),
        "nothing runs before the yes"
    );
    harness
        .get_by_label("This server is in Production, and the import adds rows to public.cliente.");
    // Cancelar volta para as opções sem rodar nada.
    harness.get_by_label("Cancel").click();
    harness.run();
    assert_eq!(*harness.state().phase(), Phase::Editing);
    assert!(log.lock().unwrap().is_empty());
    harness.get_by_label("Import").click();
    harness.run();
    harness.get_by_label("Import").click();
    wait(&mut harness, |d| matches!(d.phase(), Phase::Done(_)));
    assert_eq!(log.lock().unwrap().len(), 3);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_failed_load_says_the_line_and_keeps_the_window_open() {
    let (mut harness, log, path) = dialog(b"nome,valor\nAna,1\nBia,erro\n", "falha.csv", None);
    harness.get_by_label("Import").click();
    harness.run();
    harness.get_by_label("Import").click();
    wait(&mut harness, |d| d.error().is_some());
    assert_eq!(*harness.state().phase(), Phase::Editing);
    assert_eq!(
        harness.state().error(),
        Some(
            "Import into public.cliente failed; nothing was imported. \
             Lines 2 to 3 of the file: valor inválido"
        )
    );
    assert_eq!(
        log.lock().unwrap().last().map(String::as_str),
        Some("ROLLBACK")
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn nothing_mapped_does_not_import() {
    let (mut harness, log, path) = dialog(b"a,b\n1,2\n", "sem-colunas.csv", None);
    assert_eq!(harness.state().mapping, vec![None; 4]);
    harness.get_by_label("Import").click();
    harness.run();
    assert_eq!(
        harness.state().error(),
        Some("Choose a file column for at least one table column.")
    );
    assert!(log.lock().unwrap().is_empty());
    std::fs::remove_file(path).unwrap();
}

// ---- Bancos de verdade ----

fn rows(session: &Arc<dyn QuerySession>, sql: &str) -> Vec<Vec<Option<String>>> {
    csv_import::query(session, sql.to_string()).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn one(session: &Arc<dyn QuerySession>, sql: &str) -> String {
    rows(session, sql)[0][0].clone().unwrap_or_default()
}

/// Importa `path` na tabela `id`, casando as colunas sozinho.
fn import(
    session: &Arc<dyn QuerySession>,
    id: u64,
    path: &std::path::Path,
) -> (Vec<Option<usize>>, Progress) {
    let target = csv_import::load_target(session, id).unwrap();
    let options = CsvOptions {
        delimiter: csv_import::sniff(path).unwrap(),
        ..Default::default()
    };
    let sample = csv_import::sample(path, options.delimiter).unwrap();
    let mapping = csv_import::auto_map(&target.columns, &sample.headers(true), true);
    let plan = Plan::new(&target, &mapping).unwrap();
    let mut last = None;
    csv_import::run(
        session,
        path,
        &options,
        &plan,
        &AtomicBool::new(false),
        &mut |p| last = Some(p),
    );
    (mapping, last.unwrap())
}

/// 2.500 linhas no CSV que o Excel grava em português: ponto e vírgula e
/// Windows-1252, com aspas, quebra de linha e campos vazios.
fn excel_csv(with_id: bool) -> Vec<u8> {
    let mut csv: Vec<u8> = Vec::new();
    if with_id {
        csv.extend_from_slice(b"ID;");
    }
    csv.extend_from_slice(b"Nome;Valor;Nascimento;ATIVO;Obs\r\n");
    for i in 1..=2500 {
        if with_id {
            csv.extend_from_slice(format!("{};", i + 1000).as_bytes());
        }
        // "João" e "ação" em Windows-1252.
        csv.extend_from_slice(b"Jo\xE3o ");
        csv.extend_from_slice(format!("{i};{i}.25;").as_bytes());
        csv.extend_from_slice(if i % 4 == 0 { b"" } else { b"1990-05-17" });
        csv.extend_from_slice(b";1;");
        match i % 3 {
            0 => {}
            1 => csv.extend_from_slice(b"\"a\xE7\xE3o; \"\"x\"\"\r\nlinha 2\""),
            _ => csv.extend_from_slice(b"\"\""),
        }
        csv.extend_from_slice(b"\r\n");
    }
    csv
}

fn check_loaded(session: &Arc<dyn QuerySession>, table: &str, len: &str) {
    assert_eq!(
        one(session, &format!("SELECT count(*) FROM {table}")),
        "2500"
    );
    let r = rows(
        session,
        &format!(
            "SELECT nome, CAST(valor AS varchar(20)), CAST(nascimento AS varchar(20)), obs, \
                    CAST(dobro AS varchar(20)) \
               FROM {table} WHERE nome IN ('João 1', 'João 2', 'João 3', 'João 4') ORDER BY nome"
        ),
    );
    let s = |v: &str| Some(v.to_string());
    assert_eq!(
        r[0],
        [
            s("João 1"),
            s("1.25"),
            s("1990-05-17"),
            s("ação; \"x\"\r\nlinha 2"),
            s("2.50")
        ]
    );
    // `""` é texto vazio; o campo vazio é NULL.
    assert_eq!(r[1][3], s(""));
    assert_eq!(r[2][3], None);
    assert_eq!(r[3][2], None);
    assert_eq!(
        one(
            session,
            &format!("SELECT count(*) FROM {table} WHERE {len}(obs) > 0")
        ),
        "834"
    );
}

#[test]
fn postgres_loads_an_excel_csv_and_rolls_back_on_error() {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let settings = ConnectionSettings {
        engine: Default::default(),
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
        ssh: None,
    };
    let service = pgstudio_pg_client::PgQueryService::new(
        settings,
        var("PGSTUDIO_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    );
    let session = service.open("postgres");
    rows(
        &session,
        "DROP TABLE IF EXISTS public.\"csv Import\"; \
         CREATE TABLE public.\"csv Import\" (\
             id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
             nome text NOT NULL, valor numeric(10,2), nascimento date, ativo boolean, obs text, \
             dobro numeric GENERATED ALWAYS AS (valor * 2) STORED)",
    );
    let id: u64 = one(&session, "SELECT 'public.\"csv Import\"'::regclass::oid")
        .parse()
        .unwrap();
    let target = csv_import::load_target(&session, id).unwrap();
    assert_eq!(target.display(), "public.csv Import");
    assert_eq!(target.columns[0].kind, ColumnKind::Identity);
    assert_eq!(target.columns[2].type_name, "numeric(10,2)");
    assert_eq!(target.columns[6].kind, ColumnKind::Computed);

    // Sem a coluna da identidade: o banco numera.
    let path = temp_file("pg.csv", &excel_csv(false));
    let (mapping, end) = import(&session, id, &path);
    assert_eq!(
        mapping,
        vec![None, Some(0), Some(1), Some(2), Some(3), Some(4), None]
    );
    assert_eq!(end, Progress::Done { rows: 2500 });
    check_loaded(&session, "public.\"csv Import\"", "length");
    assert_eq!(
        one(&session, "SELECT max(id) FROM public.\"csv Import\""),
        "2500"
    );

    // Com a identidade no arquivo (OVERRIDING SYSTEM VALUE).
    rows(&session, "TRUNCATE public.\"csv Import\"");
    std::fs::write(&path, excel_csv(true)).unwrap();
    let (_, end) = import(&session, id, &path);
    assert_eq!(end, Progress::Done { rows: 2500 });
    assert_eq!(
        one(&session, "SELECT min(id) FROM public.\"csv Import\""),
        "1001"
    );

    // Um valor ruim na linha 1203: nada entra, nem os lotes anteriores.
    rows(&session, "TRUNCATE public.\"csv Import\"");
    let mut bad = String::from("nome,valor\n");
    for i in 1..=1500 {
        let valor = if i == 1202 {
            "abc".to_string()
        } else {
            i.to_string()
        };
        bad.push_str(&format!("n{i},{valor}\n"));
    }
    std::fs::write(&path, bad).unwrap();
    let (_, end) = import(&session, id, &path);
    let Progress::Failed(message) = end else {
        panic!("{end:?}")
    };
    assert!(
        message.starts_with("Lines 1002 to 1501 of the file: "),
        "{message}"
    );
    assert!(message.contains("abc"), "{message}");
    assert_eq!(
        one(&session, "SELECT count(*) FROM public.\"csv Import\""),
        "0"
    );
    // A sessão não ficou presa na transação.
    assert_eq!(one(&session, "SELECT 1"), "1");
    rows(&session, "DROP TABLE public.\"csv Import\"");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn sql_server_loads_an_excel_csv_and_rolls_back_on_error() {
    let Ok(host) = std::env::var("MSSQL_TEST_HOST") else {
        eprintln!("MSSQL_TEST_HOST not set; skipping");
        return;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let mut settings = ConnectionSettings::for_engine(Engine::SqlServer);
    settings.host = host;
    settings.port = var("MSSQL_TEST_PORT", "1433").parse().unwrap();
    settings.user = var("MSSQL_TEST_USER", "sa");
    settings.ssl = SslMode::Require;
    let service = pgstudio_mssql::MssqlQueryService::new(
        settings,
        var("MSSQL_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    );
    let session = service.open("tempdb");
    rows(&session, "DROP TABLE IF EXISTS dbo.[csv Import]");
    rows(
        &session,
        "CREATE TABLE dbo.[csv Import] (\
             id int IDENTITY(1,1) PRIMARY KEY, \
             nome nvarchar(60) NOT NULL, valor decimal(10,2), nascimento date, ativo bit, \
             obs nvarchar(max), dobro AS (valor * 2), versao rowversion)",
    );
    let id: u64 = one(&session, "SELECT OBJECT_ID('dbo.[csv Import]')")
        .parse()
        .unwrap();
    let target = csv_import::load_target(&session, id).unwrap();
    assert_eq!(target.display(), "dbo.csv Import");
    assert_eq!(target.columns[0].kind, ColumnKind::Identity);
    assert_eq!(target.columns[6].kind, ColumnKind::Computed);
    assert_eq!(target.columns[7].kind, ColumnKind::Computed);

    let path = temp_file("mssql.csv", &excel_csv(false));
    let (mapping, end) = import(&session, id, &path);
    assert_eq!(
        mapping,
        vec![
            None,
            Some(0),
            Some(1),
            Some(2),
            Some(3),
            Some(4),
            None,
            None
        ]
    );
    assert_eq!(end, Progress::Done { rows: 2500 });
    check_loaded(&session, "dbo.[csv Import]", "len");
    assert_eq!(
        one(&session, "SELECT max(id) FROM dbo.[csv Import]"),
        "2500"
    );

    // Com a identidade no arquivo (SET IDENTITY_INSERT).
    rows(&session, "TRUNCATE TABLE dbo.[csv Import]");
    std::fs::write(&path, excel_csv(true)).unwrap();
    let (_, end) = import(&session, id, &path);
    assert_eq!(end, Progress::Done { rows: 2500 });
    assert_eq!(
        one(&session, "SELECT min(id) FROM dbo.[csv Import]"),
        "1001"
    );

    rows(&session, "TRUNCATE TABLE dbo.[csv Import]");
    let mut bad = String::from("nome,valor\n");
    for i in 1..=1500 {
        let valor = if i == 1202 {
            "abc".to_string()
        } else {
            i.to_string()
        };
        bad.push_str(&format!("n{i},{valor}\n"));
    }
    std::fs::write(&path, bad).unwrap();
    let (_, end) = import(&session, id, &path);
    let Progress::Failed(message) = end else {
        panic!("{end:?}")
    };
    assert!(
        message.starts_with("Lines 1202 to 1301 of the file: "),
        "{message}"
    );
    assert_eq!(one(&session, "SELECT count(*) FROM dbo.[csv Import]"), "0");
    assert_eq!(one(&session, "SELECT @@TRANCOUNT"), "0");
    rows(&session, "DROP TABLE dbo.[csv Import]");
    std::fs::remove_file(path).unwrap();
}

// ---- O aplicativo inteiro ----

mod app {
    use super::*;
    use pgstudio_core::connection::{ConnectError, Connected, Connector};
    use pgstudio_object_explorer::fake::FakeProvider;
    use pgstudio_ui_shell::ShellApp;
    use pgstudio_ui_shell::connect::RecentConnections;
    use pgstudio_ui_shell::documents::FileDialogs;
    use pgstudio_ui_shell::registered::{PRODUCTION, RegisteredServer, RegisteredServers};
    use pgstudio_ui_shell::secrets::MemoryStore;

    struct Queries(Arc<Mutex<Vec<String>>>);

    impl QueryService for Queries {
        fn open(&self, _: &str) -> Arc<dyn QuerySession> {
            Arc::new(Fake(self.0.clone()))
        }
    }

    struct FakeConnector(Arc<Mutex<Vec<String>>>);

    impl Connector for FakeConnector {
        fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
            Ok(Connected {
                settings: s.clone(),
                server_version: "17.2".into(),
                metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
                queries: Arc::new(Queries(self.0.clone())),
            })
        }
    }

    /// Responde o arquivo a importar.
    struct Pick(PathBuf);

    impl FileDialogs for Pick {
        fn save_path(&mut self, _: &str, _: Option<&std::path::Path>) -> Option<PathBuf> {
            None
        }

        fn open_path(&mut self) -> Option<PathBuf> {
            None
        }

        fn open_data_path(&mut self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
    }

    pub fn harness(
        path: &std::path::Path,
        production: bool,
    ) -> (Harness<'static, ShellApp>, Arc<Mutex<Vec<String>>>) {
        let settings = ConnectionSettings {
            host: "db.cliente.example".into(),
            database: "loja".into(),
            ..Default::default()
        };
        let mut registered = RegisteredServers::default();
        if production {
            registered
                .add_server(
                    &[],
                    RegisteredServer {
                        name: "cliente".into(),
                        description: String::new(),
                        environment: Some(PRODUCTION.into()),
                        settings: settings.clone(),
                    },
                )
                .unwrap();
        }
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut app = ShellApp::default()
            .with_secret_store(Arc::new(MemoryStore::default()))
            .with_registered_servers(registered)
            .with_recent_connections(RecentConnections::in_memory(Vec::new()))
            .with_connector(Arc::new(FakeConnector(log.clone())))
            .with_file_dialogs(Pick(path.to_path_buf()));
        app.connect_to(settings, "secret".into());
        let schema = "srv/Databases/app_01/Schemas/public";
        let app = app.with_expanded(
            [
                "srv",
                "srv/Databases",
                "srv/Databases/app_01",
                "srv/Databases/app_01/Schemas",
                schema,
            ]
            .into_iter()
            .map(String::from)
            .chain([format!("{schema}/Tables")]),
        );
        let mut harness = Harness::builder()
            .with_size([1280.0, 900.0])
            .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
        until(&mut harness, |h| {
            h.query_by_label_contains("public.table_00003").is_some()
        });
        (harness, log)
    }

    pub fn until(
        harness: &mut Harness<'static, ShellApp>,
        done: impl Fn(&mut Harness<'static, ShellApp>) -> bool,
    ) {
        let start = Instant::now();
        loop {
            harness.step();
            if done(harness) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "never happened");
            std::thread::sleep(Duration::from_millis(2));
        }
        harness.run_steps(10);
    }

    /// "Import CSV..." no menu de contexto da tabela, até a janela ler o
    /// arquivo e as colunas.
    pub fn open(harness: &mut Harness<'static, ShellApp>) {
        harness
            .get_all_by_label_contains("public.table_00003")
            .next()
            .expect("no such node")
            .click_secondary();
        harness.run_steps(5);
        harness.get_by_label("Import CSV...").click();
        until(harness, |h| {
            h.state_mut()
                .csv_import_mut()
                .is_some_and(|d| !d.is_loading())
        });
    }

    #[test]
    fn the_table_menu_imports_a_csv_and_logs_it() {
        let path = temp_file("app.csv", b"nome,valor\nAna,1\nBia,2\n");
        let (mut harness, log) = harness(&path, false);
        open(&mut harness);
        harness.get_by_label("Import CSV into public.cliente");
        harness.get_by_label("Import").click();
        harness.run_steps(5);
        harness.get_by_label("Import").click();
        until(&mut harness, |h| {
            h.state_mut()
                .csv_import_mut()
                .is_some_and(|d| matches!(d.phase(), Phase::Done(_)))
        });
        assert_eq!(log.lock().unwrap().len(), 3);
        assert!(
            harness
                .state()
                .output()
                .iter()
                .any(|l| l.starts_with("Imported 2 row(s) from ") && l.contains("public.cliente")),
            "{:?}",
            harness.state().output()
        );
        harness.get_by_label("Close").click();
        harness.run_steps(5);
        assert!(harness.state_mut().csv_import_mut().is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_production_server_asks_before_importing() {
        let path = temp_file("app-producao.csv", b"nome,valor\nAna,1\n");
        let (mut harness, log) = harness(&path, true);
        open(&mut harness);
        harness.get_by_label("Import").click();
        harness.run_steps(5);
        assert_eq!(
            harness
                .state_mut()
                .csv_import_mut()
                .map(|d| d.phase() == &Phase::Confirming),
            Some(true)
        );
        assert!(
            log.lock().unwrap().is_empty(),
            "nothing runs before the yes"
        );
        harness.get_by_label(
            "This server is in Production, and the import adds rows to public.cliente.",
        );
        std::fs::remove_file(path).unwrap();
    }

    /// Capturas de tela da janela, em português. Só com
    /// `--features screenshots`; os PNGs vão para `PGSTUDIO_SHOTS`.
    #[cfg(feature = "screenshots")]
    #[test]
    fn screenshots() {
        pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
        let dir = std::env::var_os("PGSTUDIO_SHOTS")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut csv = String::from("Nome;Valor;Observação\n");
        for (nome, valor, obs) in [
            ("Ana Souza", "1250.00", "Cliente desde 2019"),
            ("Bruno Lima", "89.90", ""),
            ("Carla Dias", "430.50", "Paga por boleto"),
            ("Diego Reis", "", "Sem compras"),
        ] {
            csv.push_str(&format!("{nome};{valor};{obs}\n"));
        }
        let path = std::env::temp_dir().join("clientes.csv");
        std::fs::write(&path, csv).unwrap();
        let (mut h, _) = harness(&path, true);
        pgstudio_ui_shell::install_fonts(&h.ctx);
        pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
        h.run_steps(5);
        h.get_all_by_label_contains("public.table_00003")
            .next()
            .unwrap()
            .click_secondary();
        h.run_steps(5);
        h.render()
            .unwrap()
            .save(dir.join("1-menu-da-tabela.png"))
            .unwrap();
        h.get_by_label("Importar CSV...").click();
        until(&mut h, |h| {
            h.state_mut()
                .csv_import_mut()
                .is_some_and(|d| !d.is_loading())
        });
        h.render()
            .unwrap()
            .save(dir.join("2-colunas-e-previa.png"))
            .unwrap();
        h.get_by_label("Importar").click();
        h.run_steps(5);
        h.render()
            .unwrap()
            .save(dir.join("3-confirmar-em-producao.png"))
            .unwrap();
        pgstudio_i18n::set_language(pgstudio_i18n::Language::English);
    }
}
