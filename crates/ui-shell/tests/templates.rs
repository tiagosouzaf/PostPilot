//! Template Explorer: o comando abre o painel, o duplo clique põe o modelo
//! no editor, os trechos do usuário ficam no workspace e a sessão guarda a
//! pesquisa e as pastas abertas.

use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::panels::Panel;
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::repo::Workspace;
use pgstudio_ui_shell::secrets::MemoryStore;
use pgstudio_ui_shell::session::SessionStore;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "postpilot-templates-ui-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn app() -> ShellApp {
    ShellApp::default()
        .with_secret_store(std::sync::Arc::new(MemoryStore::default()))
        .with_registered_servers(RegisteredServers::default())
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
}

fn harness(app: ShellApp) -> Harness<'static, ShellApp> {
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    if let Some(cancel) = harness.query_by_label("Cancel") {
        cancel.click();
    }
    harness.run();
    harness
}

fn execute(h: &mut Harness<'static, ShellApp>, cmd: CommandId) {
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, cmd);
    h.run();
}

fn text(h: &Harness<'static, ShellApp>) -> String {
    let app = h.state();
    let id = app.active_document().expect("a query window");
    app.documents().get(id).unwrap().text.clone()
}

#[test]
fn the_command_is_available_and_opens_the_panel() {
    assert!(ShellApp::is_available(CommandId::TemplateExplorer));
    assert!(ShellApp::is_available(CommandId::SaveAsSnippet));
    let mut h = harness(app());
    assert!(!h.state().is_panel_open(Panel::TemplateExplorer));
    execute(&mut h, CommandId::TemplateExplorer);
    assert!(h.state().is_panel_open(Panel::TemplateExplorer));
    // Os dois servidores aparecem, com as pastas fechadas.
    assert!(h.query_by_label_contains("PostgreSQL").is_some());
    assert!(h.query_by_label_contains("SQL Server").is_some());
    assert!(h.query_by_label_contains("Create Table").is_none());
}

#[test]
fn search_shows_the_matches_and_double_click_inserts_into_the_editor() {
    let mut h = harness(app());
    execute(&mut h, CommandId::NewQuery);
    execute(&mut h, CommandId::TemplateExplorer);
    h.state_mut().templates_mut().search = "pg_terminate_backend".into();
    h.run();
    assert!(h.query_by_label_contains("1 template found.").is_some());
    let row = h.get_by_label_contains("End a Session");
    row.click();
    h.run();
    // A prévia mostra o SQL do item selecionado.
    assert!(
        h.query_by_label_contains("pg_terminate_backend(0 /* pid */)")
            .is_some()
    );
    h.get_by_label("Insert").click();
    h.run();
    assert!(
        text(&h).contains("pg_terminate_backend(0 /* pid */)"),
        "{}",
        text(&h)
    );

    // Numa janela com texto, o modelo entra depois, sem apagar nada.
    let ctx = h.ctx.clone();
    h.state_mut().insert_template(&ctx, "select 42;\n");
    h.run();
    let all = text(&h);
    assert!(
        all.contains("pg_terminate_backend") && all.trim_end().ends_with("select 42;"),
        "{all}"
    );
}

#[test]
fn without_a_query_window_the_template_opens_in_a_new_one() {
    let mut h = harness(app());
    assert!(h.state().active_document().is_none());
    let ctx = h.ctx.clone();
    h.state_mut().insert_template(&ctx, "select 1;\n");
    h.run();
    assert_eq!(text(&h), "select 1;\n");
}

fn workspace(root: &Path) {
    Workspace::create(root, "sql").unwrap();
}

#[test]
fn snippets_live_in_the_workspace_and_are_not_listed_as_queries() {
    let root = temp("ws").join("sql");
    workspace(&root);
    let mut h = harness(app());
    assert!(h.state_mut().open_workspace(&root));
    execute(&mut h, CommandId::NewQuery);
    let id = h.state().active_document().unwrap();
    h.state_mut().documents_mut().get_mut(id).unwrap().text =
        "select versao from parametros;".into();
    h.run();

    // "Save as Snippet" abre o diálogo com o texto do editor.
    execute(&mut h, CommandId::SaveAsSnippet);
    assert!(h.state().is_panel_open(Panel::TemplateExplorer));
    assert!(h.state().templates().has_dialog());
    assert!(h.query_by_label("New Snippet").is_some());
    h.get_by_label("Cancel").click();
    h.run();
    assert!(!h.state().templates().has_dialog());

    let folder = root.join("snippets").join("Clientes");
    let file = h
        .state_mut()
        .templates_mut()
        .save_snippet(
            &folder,
            "Versão do sistema",
            "select versao from parametros;",
        )
        .unwrap();
    assert!(file.starts_with(root.join("snippets")));
    h.run();
    assert!(h.query_by_label_contains("Workspace Snippets").is_some());
    // Na árvore e, por estar selecionado, na prévia.
    assert!(h.query_all_by_label_contains("Versão do sistema").count() >= 1);

    // A pasta de trechos não aparece entre as consultas do workspace.
    h.state_mut().workspace_panel_mut().refresh();
    let queries = h.state().workspace_panel().queries().unwrap().clone();
    assert_eq!(queries.count(), 0, "{queries:?}");

    // Pesquisa pelo conteúdo, sem acento.
    h.state_mut().templates_mut().search = "versao clientes".into();
    h.run();
    assert_eq!(h.state().templates().found(), vec!["Versão do sistema"]);

    // Fechar o workspace tira os trechos dele do painel.
    h.state_mut().close_workspace();
    h.run();
    assert!(h.state().templates().snippets().is_empty());
}

#[test]
fn the_session_keeps_the_search_and_the_open_folders() {
    let dir = temp("session");
    let file = dir.join("session.json");
    {
        let mut h = harness(app().with_session(SessionStore::open(&file)));
        execute(&mut h, CommandId::TemplateExplorer);
        h.get_by_label_contains("SQL Server").click();
        h.run();
        h.state_mut().templates_mut().search = "índice".into();
        h.run();
        h.state_mut().save_session();
    }
    let h = harness(app().with_session(SessionStore::open(&file)));
    let saved = h.state().templates().session();
    assert_eq!(saved.search, "índice");
    assert!(saved.expanded.contains(&"builtin:postgresql".to_string()));
}
