//! Capturas de tela de "levar as conexões junto no workspace", desenhadas
//! sem janela. Só com `--features screenshots`; os PNGs vão para
//! `PGSTUDIO_SHOTS` (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::repo::vault::Kdf;
use pgstudio_ui_shell::secrets::MemoryStore;

const FAST: Kdf = Kdf {
    m_cost: 8 * 1024,
    t_cost: 1,
    p_cost: 1,
};

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "16.4".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

fn out(name: &str) -> PathBuf {
    let dir = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn save(h: &mut Harness<'static, ShellApp>, name: &str) {
    h.run_steps(3);
    let image = h.render().expect("render");
    image.save(out(name)).expect("save png");
}

fn show_workspace(h: &mut Harness<'static, ShellApp>) {
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::WorkspaceExplorer);
    h.run();
}

#[test]
fn bring_connections_into_the_workspace() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let root = std::env::temp_dir()
        .join(format!("postpilot-ws-shots-{}", std::process::id()))
        .join("sql");
    let _ = std::fs::remove_dir_all(&root);

    let app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(RegisteredServers::in_memory(Default::default()))
        .with_connector(Arc::new(FakeConnector));
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    h.state_mut().connect_to(
        ConnectionSettings {
            host: "prod.abc123.us-east-1.rds.amazonaws.com".into(),
            user: "tiago".into(),
            database: "vendas".into(),
            ..Default::default()
        },
        "senha".into(),
    );
    let start = Instant::now();
    while h.state().connection().is_none() {
        assert!(start.elapsed() < Duration::from_secs(10));
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run();

    // New Workspace: a pergunta.
    assert!(h.state_mut().create_workspace(&root));
    show_workspace(&mut h);
    save(&mut h, "1-trazer-conexoes.png");

    // "Agora não": o painel continua oferecendo.
    h.state_mut()
        .workspace_panel_mut()
        .answer_import_offer(false);
    h.run();
    save(&mut h, "2-workspace-sem-conexoes.png");

    // Importar: pede a senha mestra.
    h.state_mut().import_registered_into_workspace().unwrap();
    h.run();
    save(&mut h, "3-senha-mestra.png");
    {
        let p = h.state_mut().workspace_panel_mut().passwords_mut().unwrap();
        p.set_kdf(FAST);
        p.create("uma senha mestra", "uma senha mestra", false)
            .unwrap();
        p.wait(None);
    }
    let start = Instant::now();
    while h.state().workspace_panel().has_pending_copy() || h.state().workspace_panel().is_copying()
    {
        assert!(start.elapsed() < Duration::from_secs(10));
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    show_workspace(&mut h);
    save(&mut h, "4-conexao-e-senha-no-workspace.png");

    // Conectado a outro servidor, que não está no workspace.
    h.state_mut().connect_to(
        ConnectionSettings {
            host: "localhost".into(),
            user: "postgres".into(),
            ..Default::default()
        },
        "senha".into(),
    );
    let start = Instant::now();
    while h
        .state()
        .connection()
        .is_none_or(|c| c.settings.host != "localhost")
    {
        assert!(start.elapsed() < Duration::from_secs(10));
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    show_workspace(&mut h);
    save(&mut h, "5-adicionar-conexao-ativa.png");

    // Properties (ou Register...) no servidor conectado: o registro no
    // workspace, com pasta e ambiente.
    for name in ["Clientes", "Interno"] {
        h.state_mut()
            .workspace_panel_mut()
            .servers_mut()
            .unwrap()
            .servers_mut()
            .add_group(
                &[],
                pgstudio_ui_shell::registered::ServerGroup {
                    name: name.into(),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    h.state_mut().register_active_server();
    h.run();
    if let Some(d) = h
        .state_mut()
        .workspace_panel_mut()
        .servers_mut()
        .unwrap()
        .server_dialog_mut()
    {
        d.name = "Local".into();
        d.environment = Some("Development".into());
    }
    save(&mut h, "6-registrar-no-workspace.png");

    // Registrado: o Object Explorer mostra a etiqueta e o nome do registro.
    if let Some(d) = h
        .state_mut()
        .workspace_panel_mut()
        .servers_mut()
        .unwrap()
        .server_dialog_mut()
    {
        d.name = "tiib".into();
        d.environment = Some("Production".into());
    }
    h.run();
    {
        use egui_kittest::kittest::Queryable;
        h.get_by_label("Salvar").click();
    }
    h.run();
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::ObjectExplorer);
    h.run();
    save(&mut h, "7-pesquisador-com-etiqueta.png");
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

/// Os campos de pesquisa do painel do workspace, de cima para baixo: o
/// dos servidores e o das consultas.
fn search_fields<'a>(h: &'a Harness<'_, ShellApp>) -> Vec<egui_kittest::Node<'a>> {
    let mut fields: Vec<_> = h
        .query_all_by_role(eframe::egui::accesskit::Role::TextInput)
        .collect();
    fields.sort_by(|a, b| a.rect().top().total_cmp(&b.rect().top()));
    fields
}

fn servers_field<'a>(h: &'a Harness<'_, ShellApp>) -> egui_kittest::Node<'a> {
    search_fields(h).swap_remove(0)
}

fn queries_field<'a>(h: &'a Harness<'_, ShellApp>) -> egui_kittest::Node<'a> {
    search_fields(h).pop().unwrap()
}

/// A pesquisa nas consultas do workspace: acha pelo nome e pelo conteúdo.
#[test]
fn screenshot_of_the_search() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let root = std::env::temp_dir()
        .join(format!("postpilot-ws-search-shot-{}", std::process::id()))
        .join("sql");
    let _ = std::fs::remove_dir_all(&root);
    let ws = pgstudio_ui_shell::repo::Workspace::create(&root, "sql").unwrap();
    let q = ws.queries_dir().join("PostGreSql");
    std::fs::create_dir_all(&q).unwrap();
    for n in 1..=14 {
        std::fs::write(q.join(format!("Query_{n}.sql")), "select now();\n").unwrap();
    }
    std::fs::write(
        q.join("Criar indice status_migracao.sql"),
        "create index on controle_migracao (pagina);\n",
    )
    .unwrap();
    std::fs::write(
        q.join("Query_1.sql"),
        "-- conferência\nselect *\n  from resumo_diario\n where status_migração = 2;\n",
    )
    .unwrap();
    std::fs::write(
        q.join("Query_9.sql"),
        "update controle_migracao\n   set status_migracao = 3\n where status_migracao = 2;\n",
    )
    .unwrap();
    let app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_registered_servers(RegisteredServers::default())
        .with_recent_connections(RecentConnections::in_memory(Vec::new()));
    let mut harness = Harness::builder()
        .with_size([1100.0, 720.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&harness.ctx);
    pgstudio_ui_shell::theme::apply(&harness.ctx, pgstudio_core::Theme::Dark);
    harness.run();
    assert!(harness.state_mut().open_workspace(&root));
    harness.run();
    // Alarga o painel arrastando a divisória.
    {
        use eframe::egui::{Event, Modifiers, PointerButton, pos2};
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let (from, to) = (pos2(225.0, 300.0), pos2(470.0, 300.0));
        harness.event(Event::PointerMoved(from));
        harness.step();
        harness.event(button(from, true));
        harness.step();
        harness.event(Event::PointerMoved(to));
        harness.step();
        harness.event(button(to, false));
        harness.run();
    }
    // O campo das consultas é o de baixo; o de cima é o dos servidores.
    let field = queries_field;
    field(&harness).focus();
    harness.run();
    field(&harness).type_text("status_migracao");
    let start = Instant::now();
    while harness.state().workspace_panel().is_searching() {
        assert!(start.elapsed() < Duration::from_secs(10));
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    save(&mut harness, "pesquisa-nas-consultas.png");
}

/// "Alterações": o Pull parou em alterações locais e o painel mostra quais
/// são, com o descartar.
#[test]
fn changes_and_discard() {
    use pgstudio_ui_shell::repo::QUERIES;
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let base = std::env::temp_dir().join(format!("postpilot-ws-shots-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let git = |dir: &std::path::Path, args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    let remote = base.join("sql.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare"]);
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    // Outra máquina cria o workspace e envia.
    let other = base.join("outra").join("sql");
    let mut a = ShellApp::default();
    assert!(a.create_workspace(&other));
    std::fs::write(other.join(QUERIES).join("clientes.sql"), "select 1;\n").unwrap();
    {
        let sc = a.workspace_panel_mut().source_control_mut().unwrap();
        sc.wait();
        sc.init();
        sc.wait();
        git(&other, &["config", "user.name", "a"]);
        git(&other, &["config", "user.email", "a@example.com"]);
        sc.set_remote(remote.to_str().unwrap());
        sc.wait();
        sc.commit("Workspace", true);
        sc.wait();
        sc.wait();
    }

    // Esta máquina clona, mexe sem commitar, e a outra envia uma mudança
    // nos mesmos arquivos.
    let here = base.join("aqui");
    std::fs::create_dir_all(&here).unwrap();
    let mut app = ShellApp::default();
    app.clone_workspace(remote.to_str().unwrap(), &here);
    let result = app.workspace_panel_mut().wait_clone().unwrap();
    app.finish_clone(result);
    let root = app.current_workspace().unwrap().root().to_path_buf();
    std::fs::write(other.join(QUERIES).join("clientes.sql"), "select 2;\n").unwrap();
    {
        let sc = a.workspace_panel_mut().source_control_mut().unwrap();
        sc.commit("Clientes", true);
        sc.wait();
        sc.wait();
    }
    std::fs::write(root.join(QUERIES).join("clientes.sql"), "select 3;\n").unwrap();
    std::fs::write(root.join(QUERIES).join("rascunho.sql"), "select 0;\n").unwrap();
    {
        let sc = app.workspace_panel_mut().source_control_mut().unwrap();
        sc.wait();
        sc.pull();
        sc.wait();
    }
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    save(&mut h, "8-pull-parou-alteracoes.png");
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn connect_dialog_lists_saved_connections() {
    use pgstudio_ui_shell::registered::{RegisteredServer, ServerGroup};
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let root = std::env::temp_dir()
        .join(format!("postpilot-ws-shots-connect-{}", std::process::id()))
        .join("sql");
    let _ = std::fs::remove_dir_all(&root);

    let server = |name: &str, host: &str, user: &str, env: Option<&str>| RegisteredServer {
        name: name.into(),
        environment: env.map(str::to_string),
        settings: ConnectionSettings {
            host: host.into(),
            user: user.into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut local = RegisteredServers::in_memory(Default::default());
    local
        .add_server(
            &[],
            server("Local", "localhost", "postgres", Some("Development")),
        )
        .unwrap();
    let app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(local)
        .with_connector(Arc::new(FakeConnector));
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    pgstudio_ui_shell::repo::Workspace::create(&root, "sql").unwrap();
    assert!(h.state_mut().open_workspace(&root));
    {
        let servers = h
            .state_mut()
            .workspace_panel_mut()
            .servers_mut()
            .unwrap()
            .servers_mut();
        let clientes = servers
            .add_group(
                &[],
                ServerGroup {
                    name: "Clientes".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        servers
            .add_server(
                &clientes,
                server(
                    "tiib",
                    "trust-saas-cluster.cluster-abc123.sa-east-1.rds.amazonaws.com",
                    "tiibuser",
                    Some("Production"),
                ),
            )
            .unwrap();
        servers
            .add_server(
                &clientes,
                server(
                    "tiib homologação",
                    "hml.abc123.sa-east-1.rds.amazonaws.com",
                    "tiibuser",
                    Some("Staging"),
                ),
            )
            .unwrap();
    }
    show_workspace(&mut h);
    h.state_mut().open_connect_dialog();
    h.run();
    save(&mut h, "9-conectar-lista-conexoes.png");
}

/// A pesquisa nos servidores do workspace: acha pelo nome, pela pasta e
/// pelo que a árvore não mostra (host, banco, usuário).
#[test]
fn screenshot_of_the_servers_search() {
    use pgstudio_core::connection::ConnectionSettings;
    use pgstudio_ui_shell::registered::{PRODUCTION, RegisteredServer, STAGING, ServerGroup};
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let root = std::env::temp_dir()
        .join(format!("postpilot-ws-servers-shot-{}", std::process::id()))
        .join("sql");
    let _ = std::fs::remove_dir_all(&root);
    let ws = pgstudio_ui_shell::repo::Workspace::create(&root, "sql").unwrap();
    std::fs::write(ws.queries_dir().join("Query_1.sql"), "select now();\n").unwrap();
    let app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_registered_servers(RegisteredServers::default())
        .with_recent_connections(RecentConnections::in_memory(Vec::new()));
    let mut harness = Harness::builder()
        .with_size([1100.0, 720.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&harness.ctx);
    pgstudio_ui_shell::theme::apply(&harness.ctx, pgstudio_core::Theme::Dark);
    harness.run();
    assert!(harness.state_mut().open_workspace(&root));
    harness.run();
    {
        let panel = harness.state_mut().workspace_panel_mut();
        let servers = panel.servers_mut().unwrap().servers_mut();
        let mut folder = |name: &str, environment: Option<&str>, items: &[(&str, &str, &str)]| {
            let group = ServerGroup {
                name: name.into(),
                environment: environment.map(Into::into),
                ..Default::default()
            };
            let path = servers.add_group(&[], group).unwrap();
            for (name, host, database) in items {
                let server = RegisteredServer {
                    name: (*name).into(),
                    settings: ConnectionSettings {
                        host: (*host).into(),
                        database: (*database).into(),
                        user: "app".into(),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                servers.add_server(&path, server).unwrap();
            }
        };
        folder(
            "Jucesp",
            Some(STAGING),
            &[
                ("Tiib@Jucesp", "pg.jucesp.exemplo.com", "tiib"),
                ("WebGed@Jucesp", "pg.jucesp.exemplo.com", "webged"),
                (
                    "controle_migracao_e2doc_webged@Jucesp",
                    "pg.jucesp.exemplo.com",
                    "controle_migracao",
                ),
            ],
        );
        folder(
            "SaaS",
            Some(PRODUCTION),
            &[
                ("Tiib@Production", "pg-prod.exemplo.com", "tiib"),
                ("AdmCliente@Production", "pg-prod.exemplo.com", "admcliente"),
                ("Webged@Production", "pg-prod.exemplo.com", "webged"),
                ("AdmCliente@SaaS", "pg-saas.exemplo.com", "admcliente"),
                ("Tiib@SaaS", "pg-saas.exemplo.com", "tiib"),
                ("TiibSigner@SaaS", "pg-saas.exemplo.com", "signer"),
                ("WebGed@SaaS", "pg-saas.exemplo.com", "webged"),
            ],
        );
    }
    harness.run();
    // Alarga o painel arrastando a divisória.
    {
        use eframe::egui::{Event, Modifiers, PointerButton, pos2};
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let (from, to) = (pos2(225.0, 300.0), pos2(470.0, 300.0));
        harness.event(Event::PointerMoved(from));
        harness.step();
        harness.event(button(from, true));
        harness.step();
        harness.event(Event::PointerMoved(to));
        harness.step();
        harness.event(button(to, false));
        harness.run();
    }
    let field = servers_field;
    field(&harness).focus();
    harness.run();
    field(&harness).type_text("webged pg-prod");
    harness.run();
    save(&mut harness, "pesquisa-nos-servidores.png");
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}
