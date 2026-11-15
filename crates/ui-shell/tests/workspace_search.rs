//! Pesquisa nas consultas do Workspace Explorer: pelo nome do arquivo e
//! pelo conteúdo, com a leitura fora da thread da interface.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::panels::editor_id;
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::repo::Workspace;
use pgstudio_ui_shell::secrets::MemoryStore;

const HINT: &str = "Search queries by name or content";

fn workspace(name: &str) -> PathBuf {
    let root = std::env::temp_dir()
        .join(format!("postpilot-search-ui-{name}-{}", std::process::id()))
        .join("sql");
    let _ = std::fs::remove_dir_all(&root);
    let ws = Workspace::create(&root, "sql").unwrap();
    let q = ws.queries_dir().join("PostGreSql");
    std::fs::create_dir_all(&q).unwrap();
    std::fs::create_dir_all(ws.queries_dir().join("vazia")).unwrap();
    let write = |name: &str, text: &str| std::fs::write(q.join(name), text).unwrap();
    write(
        "Criar indice status_migracao.sql",
        "create index on t (a);\n",
    );
    write(
        "Query_1.sql",
        "-- conferência\nselect *\n  from resumo_diario\n where status_migração = 2;\n",
    );
    write("Query_2.sql", "select count(*) from pedidos;\n");
    write("Query_3.sql", "select 1;\n");
    root
}

fn harness(root: &Path) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_secret_store(std::sync::Arc::new(MemoryStore::default()))
        .with_registered_servers(RegisteredServers::default())
        .with_recent_connections(RecentConnections::in_memory(Vec::new()));
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    if let Some(cancel) = harness.query_by_label("Cancel") {
        cancel.click();
    }
    harness.run();
    assert!(harness.state_mut().open_workspace(root));
    harness.run();
    harness
}

/// Os campos de pesquisa do painel, de cima para baixo: o dos servidores
/// e o das consultas.
const SERVERS_FIELD: usize = 0;
const QUERIES_FIELD: usize = 1;

fn field<'a>(harness: &'a Harness<'static, ShellApp>, index: usize) -> egui_kittest::Node<'a> {
    let mut fields: Vec<_> = harness
        .query_all_by_role(eframe::egui::accesskit::Role::TextInput)
        .collect();
    fields.sort_by(|a, b| a.rect().top().total_cmp(&b.rect().top()));
    assert_eq!(fields.len(), 2, "servers and queries search fields");
    fields.swap_remove(index)
}

fn search(harness: &mut Harness<'static, ShellApp>, text: &str) {
    field(harness, QUERIES_FIELD).focus();
    harness.run();
    field(harness, QUERIES_FIELD).type_text(text);
    // A leitura dos arquivos termina em outra thread.
    let start = Instant::now();
    loop {
        harness.run();
        if harness
            .query_by_label("Searching inside the files...")
            .is_none()
        {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "search never ended"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn shown(harness: &Harness<'static, ShellApp>, label: &str) -> bool {
    harness.query_all_by_label_contains(label).count() > 0
}

#[test]
fn search_filters_by_name_and_by_content() {
    let root = workspace("filter");
    let mut harness = harness(&root);
    assert!(
        shown(&harness, HINT)
            || harness
                .query_all_by_role(eframe::egui::accesskit::Role::TextInput)
                .count()
                > 0
    );
    assert!(shown(&harness, "Query_3.sql"));
    assert!(shown(&harness, "vazia"));

    // Sem acento e em maiúsculas: acha no nome de um e dentro de outro.
    search(&mut harness, "STATUS_MIGRACAO");
    assert!(shown(&harness, "2 queries found."));
    assert!(shown(&harness, "Criar indice status_migracao.sql"));
    assert!(shown(&harness, "Query_1.sql"));
    assert!(shown(&harness, "4: where status_migração = 2;"));
    assert!(!shown(&harness, "Query_2.sql"));
    assert!(!shown(&harness, "Query_3.sql"));
    // Pasta sem nada achado some; a que tem continua.
    assert!(!shown(&harness, "vazia"));
    assert!(shown(&harness, "PostGreSql"));

    // Clicar no trecho abre o arquivo com o termo selecionado.
    harness.get_by_label_contains("4: where").click();
    harness.run();
    let state = harness.state();
    let id = state.active_document().expect("the query opened");
    let doc = state.documents().get(id).unwrap();
    assert_eq!(doc.name(), "Query_1.sql");
    let (a, b) = pgstudio_sql_editor::selection(&harness.ctx, editor_id(id)).unwrap();
    let selected: String = doc.text.chars().skip(a).take(b - a).collect();
    assert_eq!(selected, "status_migração");
}

#[test]
fn search_with_no_match_and_clearing_it() {
    let root = workspace("clear");
    let mut harness = harness(&root);
    search(&mut harness, "nao existe isto");
    assert!(shown(&harness, "No queries found."));
    assert!(!shown(&harness, "Query_1.sql"));

    harness.get_by_label(egui_phosphor::regular::X).click();
    harness.run();
    assert!(shown(&harness, "Query_1.sql"));
    assert!(shown(&harness, "Query_3.sql"));
    assert!(!shown(&harness, "queries found."));

    // Dois termos: um no nome e outro no conteúdo do mesmo arquivo.
    search(&mut harness, "query_2 pedidos");
    assert!(shown(&harness, "1 query found."));
    assert!(shown(&harness, "Query_2.sql"));
    assert!(!shown(&harness, "Query_1.sql"));
}

/// O campo de cima filtra os servidores do workspace sem mexer nas
/// consultas, e o de baixo não mexe nos servidores.
#[test]
fn servers_search_filters_the_workspace_servers() {
    use pgstudio_core::connection::ConnectionSettings;
    use pgstudio_ui_shell::registered::{RegisteredServer, ServerGroup};
    let root = workspace("servers");
    let mut harness = harness(&root);
    {
        let panel = harness.state_mut().workspace_panel_mut();
        let servers = panel.servers_mut().unwrap().servers_mut();
        let saas = servers
            .add_group(
                &[],
                ServerGroup {
                    name: "SaaS".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        for (name, host, database) in [
            ("Tiib@Production", "db-prod.exemplo.com", "tiib"),
            ("WebGed@SaaS", "db-saas.exemplo.com", "webged"),
        ] {
            let server = RegisteredServer {
                name: name.into(),
                settings: ConnectionSettings {
                    host: host.into(),
                    database: database.into(),
                    ..Default::default()
                },
                ..Default::default()
            };
            servers.add_server(&saas, server).unwrap();
        }
    }
    harness.run();
    assert!(shown(&harness, "Tiib@Production"));
    assert!(shown(&harness, "WebGed@SaaS"));

    // Pelo host, que a árvore não mostra: a linha de baixo diz onde achou.
    field(&harness, SERVERS_FIELD).focus();
    harness.run();
    field(&harness, SERVERS_FIELD).type_text("DB-SAAS");
    harness.run();
    assert!(shown(&harness, "1 server found."));
    assert!(shown(&harness, "WebGed@SaaS"));
    assert!(shown(&harness, "server: db-saas.exemplo.com"));
    assert!(!shown(&harness, "Tiib@Production"));
    // As consultas continuam todas lá.
    assert!(shown(&harness, "Query_3.sql"));
    assert!(!shown(&harness, "queries found."));

    // E a pesquisa das consultas não esconde servidor nenhum.
    harness
        .state_mut()
        .workspace_panel_mut()
        .servers_mut()
        .unwrap()
        .filter
        .clear();
    search(&mut harness, "pedidos");
    assert!(shown(&harness, "1 query found."));
    assert!(shown(&harness, "Tiib@Production"));
    assert!(shown(&harness, "WebGed@SaaS"));
}
