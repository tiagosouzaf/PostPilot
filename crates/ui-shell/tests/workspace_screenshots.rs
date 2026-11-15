//! Capturas de tela de "levar as conexões junto no workspace", desenhadas
//! sem janela. Só com `--features screenshots`; os PNGs vão para
//! `PGSTUDIO_SHOTS` (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
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
