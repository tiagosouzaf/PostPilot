//! Capturas de tela de vários workspaces abertos e da nova tela de
//! workspaces, desenhadas sem janela. Só com `--features screenshots`; os
//! PNGs vão para `PGSTUDIO_SHOTS` (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::{
    PRODUCTION, RegisteredServer, RegisteredServers, STAGING, ServerGroup,
};
use pgstudio_ui_shell::repo::vault::Kdf;
use pgstudio_ui_shell::repo::{QUERIES, Workspace};
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

fn cancel(h: &mut Harness<'static, ShellApp>) {
    if let Some(cancel) = h.query_by_label("Cancelar") {
        cancel.click();
    }
    h.run();
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git")
        .status
        .success();
    assert!(ok, "git {args:?}");
}

fn server(name: &str, host: &str, environment: Option<&str>) -> RegisteredServer {
    RegisteredServer {
        name: name.into(),
        description: String::new(),
        environment: environment.map(Into::into),
        settings: ConnectionSettings {
            host: host.into(),
            user: "tiago".into(),
            database: "app".into(),
            ..Default::default()
        },
    }
}

/// Um workspace com git e um remoto, já enviado.
fn workspace(
    base: &Path,
    name: &str,
    servers: &[(&str, RegisteredServer)],
    queries: &[&str],
) -> PathBuf {
    let root = base.join(name);
    let ws = Workspace::create(&root, name).unwrap();
    let mut tree = ws.servers();
    for (folder, s) in servers {
        let path = match tree.root().groups.iter().position(|g| g.name == *folder) {
            Some(i) => vec![i],
            None => tree
                .add_group(
                    &[],
                    ServerGroup {
                        name: (*folder).into(),
                        ..Default::default()
                    },
                )
                .unwrap(),
        };
        tree.add_server(&path, s.clone()).unwrap();
    }
    for q in queries {
        let file = root.join(QUERIES).join(q);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "select 1;\n").unwrap();
    }
    root
}

fn with_remote(base: &Path, root: &Path) {
    let name = root.file_name().unwrap().to_string_lossy().into_owned();
    let remote = base.join(format!("{name}.git"));
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare", "-b", "main"]);
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Tiago"]);
    git(root, &["config", "user.email", "tiago@example.com"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "Primeiro commit"]);
    git(root, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(root, &["push", "-u", "origin", "main"]);
}

fn wait_git(h: &mut Harness<'static, ShellApp>) {
    for i in 0..h.state().workspace_panel().count() {
        let sc = h
            .state_mut()
            .workspace_panel_mut()
            .source_control_at_mut(i)
            .unwrap();
        sc.refresh();
        sc.wait();
    }
    h.run();
}

fn size() -> [f32; 2] {
    std::env::var("PGSTUDIO_SHOT_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([1280.0, 800.0])
}

#[test]
fn several_workspaces() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let base = std::env::temp_dir().join(format!("postpilot-multi-shots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    let app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(RegisteredServers::in_memory(Default::default()))
        .with_connector(Arc::new(FakeConnector));
    let mut h = Harness::builder()
        .with_size(size())
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    cancel(&mut h);
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::WorkspaceExplorer);
    h.run();
    save(&mut h, "1-sem-workspace.png");

    // "Adicionar": clonar um repositório ou usar uma pasta.
    h.state_mut().workspace_panel_mut().open_add();
    h.state_mut()
        .workspace_panel_mut()
        .set_clone_url("git@github.com:acme/sql-acme.git");
    h.state_mut()
        .workspace_panel_mut()
        .set_clone_folder(Path::new("/home/tiago/PostPilot"));
    h.run();
    save(&mut h, "2-adicionar-workspace.png");
    h.state_mut().workspace_panel_mut().close_add();

    let acme = workspace(
        &base,
        "Cliente Acme",
        &[
            (
                "SaaS",
                server("Acme Produção", "acme-prod.example.com", Some(PRODUCTION)),
            ),
            (
                "SaaS",
                server("Acme Homologação", "acme-hml.example.com", Some(STAGING)),
            ),
        ],
        &["relatorios/vendas-do-mes.sql", "clientes-ativos.sql"],
    );
    with_remote(&base, &acme);
    let dados = workspace(
        &base,
        "Time de Dados",
        &[
            (
                "Data Warehouse",
                server("DW Produção", "dw.example.com", Some(PRODUCTION)),
            ),
            (
                "Data Warehouse",
                server("DW Homologação", "dw-hml.example.com", Some(STAGING)),
            ),
            (
                "Aplicações",
                server("Faturamento", "fat.example.com", Some(PRODUCTION)),
            ),
        ],
        &["cargas/carga-diaria.sql", "indicadores.sql"],
    );
    with_remote(&base, &dados);
    // Um commit a enviar e duas alterações a salvar.
    std::fs::write(
        dados.join(QUERIES).join("novo-relatorio.sql"),
        "select 2;\n",
    )
    .unwrap();
    git(&dados, &["add", "-A"]);
    git(&dados, &["commit", "-m", "Novo relatório"]);
    std::fs::write(dados.join(QUERIES).join("indicadores.sql"), "select 3;\n").unwrap();
    std::fs::write(dados.join(QUERIES).join("rascunho.sql"), "select 4;\n").unwrap();
    let pessoal = workspace(
        &base,
        "Pessoal",
        &[("Local", server("Postgres local", "localhost", None))],
        &["anotacoes.sql"],
    );

    assert!(h.state_mut().open_workspace(&acme));
    assert!(h.state_mut().open_workspace(&pessoal));
    assert!(h.state_mut().open_workspace(&dados));
    h.state_mut()
        .workspace_panel_mut()
        .answer_import_offer(false);
    for (i, master) in [(0, "senha mestra do acme"), (2, "senha mestra dos dados")] {
        let p = h
            .state_mut()
            .workspace_panel_mut()
            .passwords_at_mut(i)
            .unwrap();
        p.set_kdf(FAST);
        p.create(master, master, false).unwrap();
        p.wait(None);
    }
    // O secrets.json do Acme já foi enviado: ele está em dia.
    git(&acme, &["add", "-A"]);
    git(&acme, &["commit", "-m", "Senhas"]);
    git(&acme, &["push"]);
    // O cofre do Acme fica trancado: o cartão dele mostra o cadeado.
    h.state_mut()
        .workspace_panel_mut()
        .passwords_at_mut(0)
        .unwrap()
        .lock();
    wait_git(&mut h);
    save(&mut h, "3-varios-workspaces.png");

    // Trocar para o Acme: em dia com o repositório.
    h.state_mut().activate_workspace(0);
    wait_git(&mut h);
    save(&mut h, "4-workspace-em-dia.png");

    // As conexões de todos no "Conectar ao Servidor", por workspace.
    h.state_mut().open_connect_dialog();
    h.run();
    save(&mut h, "5-conectar-por-workspace.png");
    cancel(&mut h);

    // O Pesquisador de Objetos diz de qual workspace são as conexões.
    h.state_mut().execute(&ctx, CommandId::ObjectExplorer);
    h.run();
    save(&mut h, "6-pesquisador-de-objetos.png");
}
