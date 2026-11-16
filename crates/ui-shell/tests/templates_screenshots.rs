//! Capturas de tela do Template Explorer, desenhadas sem janela. Só com
//! `--features screenshots`; os PNGs vão para `PGSTUDIO_SHOTS` (ou a pasta
//! temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::repo::Workspace;
use pgstudio_ui_shell::secrets::MemoryStore;

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

#[test]
fn template_explorer() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let root = std::env::temp_dir()
        .join(format!("postpilot-templates-shots-{}", std::process::id()))
        .join("sql");
    let _ = std::fs::remove_dir_all(&root);
    Workspace::create(&root, "sql").unwrap();
    let snippets = root.join("snippets");
    let write = |folder: &str, name: &str, sql: &str| {
        std::fs::create_dir_all(snippets.join(folder)).unwrap();
        std::fs::write(snippets.join(folder).join(name), sql).unwrap();
    };
    write(
        "Clientes",
        "Versão do sistema.sql",
        "select versao, atualizado_em\n  from parametros_sistema\n order by atualizado_em desc\n limit 1;\n",
    );
    write(
        "Clientes",
        "Notas sem retorno.sql",
        "select count(*) from nota_fiscal where retorno is null;\n",
    );
    write(
        "Migração",
        "Conferir status.sql",
        "select status, count(*) from migracao group by status;\n",
    );

    let app = ShellApp::default()
        .with_secret_store(std::sync::Arc::new(MemoryStore::default()))
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(RegisteredServers::in_memory(Default::default()));
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    if let Some(cancel) = h.query_by_label("Cancelar") {
        cancel.click();
    }
    h.run();
    assert!(h.state_mut().open_workspace(&root));
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::NewQuery);
    h.state_mut().execute(&ctx, CommandId::TemplateExplorer);
    h.run();
    h.get_by_label_contains("Versão do sistema").click();
    h.run();
    h.get_by_label("Inserir").click();
    h.run();
    save(&mut h, "1-modelos-e-trechos.png");

    h.state_mut().templates_mut().search = "bloque".into();
    h.run();
    h.query_all_by_label_contains("Sessões Bloqueadas")
        .next()
        .expect("found")
        .click();
    h.run();
    save(&mut h, "2-pesquisa.png");

    h.state_mut().templates_mut().search.clear();
    h.state_mut().execute(&ctx, CommandId::SaveAsSnippet);
    h.run();
    save(&mut h, "3-novo-trecho.png");
}
