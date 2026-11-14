//! Testes do marco R.2: o workspace num repositório git, com duas cópias
//! (duas máquinas) sincronizando por um repositório bare local.

use std::path::{Path, PathBuf};
use std::process::Command;

use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::repo::git::{Git, GitError};
use pgstudio_ui_shell::repo::{MANIFEST, QUERIES, Workspace};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("postpilot-r2-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

/// Nome e e-mail locais, para não depender do .gitconfig da máquina.
fn identity(dir: &Path, name: &str) {
    git(dir, &["config", "user.name", name]);
    git(
        dir,
        &["config", "user.email", &format!("{name}@example.com")],
    );
}

fn bare(base: &Path) -> PathBuf {
    let remote = base.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare"]);
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    remote
}

/// A "máquina" que cria o workspace, inicializa o git, aponta o remoto e
/// faz o primeiro push.
fn first_machine(base: &Path, remote: &Path) -> ShellApp {
    let root = base.join("a").join("vendas");
    let mut app = ShellApp::default();
    assert!(app.create_workspace(&root));
    std::fs::write(root.join(QUERIES).join("total.sql"), "select 1;\n").unwrap();
    let sc = app.workspace_panel_mut().source_control_mut().unwrap();
    sc.wait();
    assert_eq!(sc.state(), Some(&Err(GitError::NotRepo)));
    sc.init();
    sc.wait();
    identity(&root, "a");
    sc.refresh();
    sc.wait();
    let status = sc.status().unwrap().clone();
    assert_eq!(status.branch, "main");
    assert!(status.changes.iter().any(|c| c.path == "queries/total.sql"));
    assert!(status.changes.iter().any(|c| c.path == MANIFEST));

    // Sem remoto, o Push explica o que fazer.
    sc.commit("Primeiro commit", true);
    sc.wait();
    assert!(sc.take_message().unwrap().contains("no remote"));
    assert!(sc.status().unwrap().is_clean());

    sc.set_remote(remote.to_str().unwrap());
    sc.wait();
    assert_eq!(sc.status().unwrap().remote.as_deref(), remote.to_str());
    sc.push();
    sc.wait();
    assert_eq!(sc.take_message().as_deref(), Some("Pushed."));
    let s = sc.status().unwrap();
    assert_eq!(s.upstream.as_deref(), Some("origin/main"));
    assert_eq!((s.ahead, s.behind), (0, 0));
    app
}

/// A outra "máquina": clona pelo PostPilot.
fn second_machine(base: &Path, remote: &Path) -> ShellApp {
    let parent = base.join("b");
    std::fs::create_dir_all(&parent).unwrap();
    let mut app = ShellApp::default();
    app.clone_workspace(remote.to_str().unwrap(), &parent);
    assert!(app.workspace_panel().is_cloning());
    let result = app.workspace_panel_mut().wait_clone().unwrap();
    app.finish_clone(result);
    let ws = app.current_workspace().expect("cloned workspace opened");
    assert_eq!(ws.name(), "vendas");
    assert_eq!(ws.root(), parent.join("remote"));
    identity(ws.root(), "b");
    app
}

#[test]
fn two_machines_sync_through_the_remote() {
    let base = temp_dir("sync");
    let remote = bare(&base);
    let mut a = first_machine(&base, &remote);
    let mut b = second_machine(&base, &remote);
    assert_eq!(b.workspace_panel().queries().unwrap().count(), 1);

    // B muda a consulta, commita e envia de uma vez.
    let b_root = b.current_workspace().unwrap().root().to_path_buf();
    std::fs::write(b_root.join(QUERIES).join("total.sql"), "select 2;\n").unwrap();
    std::fs::write(b_root.join(QUERIES).join("nova.sql"), "select 3;\n").unwrap();
    let sc = b.workspace_panel_mut().source_control_mut().unwrap();
    sc.refresh();
    sc.wait();
    assert_eq!(sc.status().unwrap().changes.len(), 2);
    sc.commit("Muda total e cria nova", true);
    sc.wait();
    assert_eq!(sc.take_message().as_deref(), Some("Pushed."));

    // A puxa: os arquivos mudam e o Workspace Explorer relê.
    let a_root = a.current_workspace().unwrap().root().to_path_buf();
    let sc = a.workspace_panel_mut().source_control_mut().unwrap();
    sc.pull();
    sc.wait();
    assert!(sc.take_message().unwrap().starts_with("Pulled"));
    assert!(sc.take_files_changed());
    assert_eq!(
        std::fs::read_to_string(a_root.join(QUERIES).join("total.sql")).unwrap(),
        "select 2;\n"
    );
    assert!(sc.status().unwrap().is_clean());
    a.workspace_panel_mut().refresh();
    assert_eq!(a.workspace_panel().queries().unwrap().count(), 2);
}

#[test]
fn a_conflict_is_reported_with_the_files() {
    let base = temp_dir("conflict");
    let remote = bare(&base);
    let mut a = first_machine(&base, &remote);
    let mut b = second_machine(&base, &remote);
    let a_root = a.current_workspace().unwrap().root().to_path_buf();
    let b_root = b.current_workspace().unwrap().root().to_path_buf();

    std::fs::write(a_root.join(QUERIES).join("total.sql"), "select 'a';\n").unwrap();
    let sc = a.workspace_panel_mut().source_control_mut().unwrap();
    sc.commit("A", true);
    sc.wait();
    assert_eq!(sc.take_message().as_deref(), Some("Pushed."));

    std::fs::write(b_root.join(QUERIES).join("total.sql"), "select 'b';\n").unwrap();
    let sc = b.workspace_panel_mut().source_control_mut().unwrap();
    sc.commit("B", true);
    sc.wait();
    // O push é recusado: o remoto tem um commit que B não tem.
    assert!(sc.take_message().is_some());
    assert_eq!(sc.status().unwrap().ahead, 1);
    sc.pull();
    sc.wait();
    let message = sc.take_message().unwrap();
    assert!(
        message.contains("conflicts in queries/total.sql"),
        "{message}"
    );
    assert_eq!(sc.status().unwrap().conflicts, ["queries/total.sql"]);
    // Com conflito, o Push explica que falta resolver.
    sc.push();
    sc.wait();
    assert!(sc.take_message().unwrap().contains("Fix the conflicts"));

    // Resolve, commita e envia.
    std::fs::write(b_root.join(QUERIES).join("total.sql"), "select 'a e b';\n").unwrap();
    sc.commit("Resolve", true);
    sc.wait();
    assert_eq!(sc.take_message().as_deref(), Some("Pushed."));
    assert!(sc.status().unwrap().conflicts.is_empty());
}

#[test]
fn cloning_an_empty_repository_makes_it_a_workspace() {
    let base = temp_dir("empty");
    let remote = bare(&base);
    let parent = base.join("c");
    std::fs::create_dir_all(&parent).unwrap();
    let mut app = ShellApp::default();
    app.clone_workspace(remote.to_str().unwrap(), &parent);
    let result = app.workspace_panel_mut().wait_clone().unwrap();
    app.finish_clone(result);
    let root = parent.join("remote");
    assert!(root.join(MANIFEST).exists());
    assert_eq!(app.current_workspace().unwrap().name(), "remote");
    let sc = app.workspace_panel_mut().source_control_mut().unwrap();
    sc.wait();
    let status = sc.status().unwrap();
    assert_eq!(status.remote.as_deref(), remote.to_str());
    assert!(!status.is_clean());

    // Clonar de novo na mesma pasta não apaga nada.
    app.clone_workspace(remote.to_str().unwrap(), &parent);
    assert!(app.status().contains("already exists"));
    assert!(!app.workspace_panel().is_cloning());

    // URL ruim: o erro do git chega à status bar.
    app.clone_workspace(
        base.join("nao-existe.git").to_str().unwrap(),
        &base.join("d"),
    );
    let result = app.workspace_panel_mut().wait_clone().unwrap();
    app.finish_clone(result);
    assert!(app.status().starts_with("Clone failed"), "{}", app.status());
}

#[test]
fn git_errors_read_well() {
    assert!(
        GitError::NotInstalled
            .to_string()
            .contains("Git is not installed")
    );
    let dir = temp_dir("plain");
    Workspace::create(&dir, "x").unwrap();
    let g = Git::new(&dir);
    assert!(!g.is_repo());
    assert_eq!(g.status(), Err(GitError::NotRepo));
    assert!(Git::is_installed());
}
