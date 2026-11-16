//! Capturas de tela da IA (Commit e Tools > AI Providers), desenhadas sem
//! janela. Só com `--features screenshots`; os PNGs vão para
//! `PGSTUDIO_SHOTS` (ou a pasta temporária).
//!
//! O Commit usa o Claude Code de verdade quando `PGSTUDIO_CLAUDE` aponta
//! para ele; sem isso, o teste só desenha as opções.
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_automation::llm::ProviderKind;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::ai::Ai;
use pgstudio_ui_shell::repo::QUERIES;

fn out(name: &str) -> PathBuf {
    let dir = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn harness(app: ShellApp) -> Harness<'static, ShellApp> {
    let h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    // Como o aplicativo ao abrir: fonte de ícones e tema escuro.
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h
}

fn save(h: &mut Harness<'static, ShellApp>, name: &str) {
    h.run_steps(3);
    let image = h.render().expect("render");
    image.save(out(name)).expect("save png");
}

#[test]
fn options_and_commit() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);

    // Tools > AI Providers com um GLM acrescentado.
    let mut h = harness(ShellApp::default().with_ai(Ai::system()));
    h.run();
    h.get_by_label("Ferramentas").click();
    h.run();
    h.get_by_label("Provedores de IA…").click();
    h.run();
    {
        let dialog = h.state_mut().options_mut().expect("options open");
        dialog.ai.add(ProviderKind::OpenRouter);
        dialog.ai.add(ProviderKind::Glm);
        let id = dialog.ai.settings.providers[1].id.clone();
        dialog.ai.settings.active = id.clone();
        dialog
            .ai
            .new_keys
            .insert(id, "sk-exemplo-0123456789".into());
    }
    save(&mut h, "ia-provedores.png");

    // Commit com a mensagem escrita pelo Claude Code de verdade.
    let Some(claude) = std::env::var_os("PGSTUDIO_CLAUDE").map(PathBuf::from) else {
        return;
    };
    let root = std::env::temp_dir()
        .join(format!("postpilot-shots-{}", std::process::id()))
        .join("sql");
    let _ = std::fs::remove_dir_all(&root);
    let mut ai = Ai::default();
    ai.set_program(Some(claude));
    let mut app = ShellApp::default().with_ai(ai);
    assert!(app.create_workspace(&root));
    std::fs::write(
        root.join(QUERIES).join("clientes_inativos.sql"),
        "-- Clientes sem pedido nos últimos 90 dias\nselect c.id, c.nome, max(p.criado_em) as ultimo_pedido\nfrom clientes c\nleft join pedidos p on p.cliente_id = c.id\ngroup by c.id, c.nome\nhaving max(p.criado_em) < now() - interval '90 days' or max(p.criado_em) is null;\n",
    )
    .unwrap();
    std::fs::write(
        root.join(QUERIES).join("faturamento_mensal.sql"),
        "select date_trunc('month', criado_em) as mes, sum(total) as faturamento\nfrom pedidos\nwhere status = 'pago'\ngroup by 1\norder by 1 desc;\n",
    )
    .unwrap();
    {
        let sc = app.workspace_panel_mut().source_control_mut().unwrap();
        sc.wait();
        sc.init();
        sc.wait();
        sc.refresh();
        sc.wait();
    }
    let mut h = harness(app);
    h.run();
    h.state_mut()
        .workspace_panel_mut()
        .source_control_mut()
        .unwrap()
        .open_commit();
    h.run();
    h.get_by_label_contains("Gerar com IA").click();
    h.run_steps(2);
    save(&mut h, "commit-gerando.png");
    let start = Instant::now();
    while h
        .state()
        .workspace_panel()
        .source_control()
        .unwrap()
        .is_generating()
    {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(120), "no answer");
        std::thread::sleep(Duration::from_millis(20));
    }
    let sc = h.state().workspace_panel().source_control().unwrap();
    println!(
        "commit message: {:?} error: {:?}",
        sc.commit_message(),
        sc.commit_error()
    );
    save(&mut h, "commit-com-ia.png");
}

/// Tools > AI Providers com o ChatGPT pela conta: antes e depois de
/// entrar. O `codex` é um script que só responde ao `login status`.
#[cfg(unix)]
#[test]
fn chatgpt_account() {
    use pgstudio_automation::codex::Login;
    use pgstudio_ui_shell::ai::CHATGPT;
    use std::os::unix::fs::PermissionsExt;

    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let dir = std::env::temp_dir().join(format!("postpilot-shots-codex-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".local/bin")).unwrap();
    let codex = dir.join(".local/bin/codex");
    std::fs::write(
        &codex,
        format!(
            "#!/bin/sh\nif [ -f '{0}/in' ]; then echo 'Logged in using ChatGPT'; exit 0; fi\necho 'Not logged in' >&2; exit 1\n",
            dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();

    for (signed_in, name) in [
        (false, "chatgpt-entrar.png"),
        (true, "chatgpt-conectado.png"),
    ] {
        if signed_in {
            std::fs::write(dir.join("in"), "").unwrap();
        }
        let mut ai = Ai::default();
        ai.set_codex(Some(codex.clone()));
        let mut h = harness(ShellApp::default().with_ai(ai));
        h.run();
        h.get_by_label("Ferramentas").click();
        h.run();
        h.get_by_label("Provedores de IA…").click();
        h.run();
        h.state_mut().options_mut().unwrap().ai.settings.active = CHATGPT.into();
        let start = Instant::now();
        let want = if signed_in {
            Login::ChatGpt
        } else {
            Login::SignedOut
        };
        while h.state().options().unwrap().ai.chatgpt.as_ref() != Some(&want) {
            h.step();
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        save(&mut h, name);
    }
    let _ = std::fs::remove_dir_all(dir);
}
