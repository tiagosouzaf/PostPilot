//! O rastro de uma queda no `postpilot.log`: o app de verdade é derrubado
//! (`PGSTUDIO_CRASH_TEST`, antes de abrir a janela) e o log tem de dizer o
//! que houve.
#![cfg(unix)]

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

fn config(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pp-crash-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("PostPilot")).unwrap();
    dir
}

/// Roda o app com a queda `kind` e devolve como ele terminou e o log.
fn crash(config: &Path, kind: &str, stderr: Stdio) -> (ExitStatus, String) {
    let status = Command::new(env!("CARGO_BIN_EXE_postpilot"))
        .env("XDG_CONFIG_HOME", config)
        .env("PGSTUDIO_CRASH_TEST", kind)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdout(Stdio::null())
        .stderr(stderr)
        .status()
        .unwrap();
    let log = std::fs::read_to_string(config.join("PostPilot/postpilot.log")).unwrap_or_default();
    (status, log)
}

fn markers(config: &Path) -> Vec<String> {
    std::fs::read_dir(config.join("PostPilot"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("running-"))
        .collect()
}

#[test]
fn an_invalid_memory_access_leaves_the_signal_and_the_call_stack() {
    let dir = config("segv");
    let (status, log) = crash(&dir, "segv-now", Stdio::null());
    // O sinal volta ao sistema: quem olha o processo vê a mesma morte.
    assert_eq!(status.signal(), Some(11), "{log}");
    assert!(log.contains("fatal signal SIGSEGV (11)"), "{log}");
    assert!(log.contains("thread 'crash-test'"), "{log}");
    assert!(log.contains("self_test"), "{log}");
    assert!(markers(&dir).is_empty(), "{log}");
}

#[test]
fn an_abort_leaves_the_signal_and_the_call_stack() {
    let dir = config("abort");
    let (status, log) = crash(&dir, "abort-now", Stdio::null());
    assert_eq!(status.signal(), Some(6), "{log}");
    assert!(log.contains("fatal signal SIGABRT (6)"), "{log}");
    assert!(log.contains("self_test"), "{log}");
}

#[test]
fn a_stack_overflow_leaves_a_short_call_stack() {
    let dir = config("overflow");
    let (status, log) = crash(&dir, "overflow-now", Stdio::null());
    assert_eq!(status.signal(), Some(11), "{log}");
    assert!(log.contains("fatal signal SIGSEGV (11)"), "{log}");
    assert!(log.contains("deep"), "{log}");
    assert!(log.contains("more lines"), "{log}");
    assert!(log.lines().count() < 400, "{} lines", log.lines().count());
}

/// Quem abriu o app fechou a saída de erro dele: o pânico tem de chegar ao
/// arquivo mesmo assim (o `eprintln!` entraria em pânico de novo dentro do
/// aviso, e o processo morreria sem deixar nada).
#[test]
fn a_panic_reaches_the_log_with_stderr_closed() {
    let dir = config("panic");
    // Um pipe cuja ponta de leitura já fechou: escrever nele dá erro.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let (_, log) = crash(&dir, "panic-now", writer.into());
    assert!(log.contains("panic in thread 'crash-test'"), "{log}");
    assert!(log.contains("PGSTUDIO_CRASH_TEST"), "{log}");
    assert!(log.contains("self_test"), "{log}");
}

/// Morto por fora (`SIGKILL`) nada roda dentro do processo: a próxima
/// abertura é que avisa.
#[test]
fn the_next_start_reports_a_run_that_vanished() {
    let dir = config("lost");
    // O pid de um processo que já terminou.
    let mut gone = Command::new("true").spawn().unwrap();
    let pid = gone.id();
    gone.wait().unwrap();
    std::fs::write(
        dir.join(format!("PostPilot/running-{pid}")),
        "2026-11-16T21:30:00.000Z PostPilot 0.2.45\n",
    )
    .unwrap();
    let (_, log) = crash(&dir, "abort-now", Stdio::null());
    assert!(
        log.contains(&format!(
            "the previous run (pid {pid}, started 2026-11-16T21:30:00.000Z PostPilot 0.2.45) \
             ended without a clean exit"
        )),
        "{log}"
    );
    assert!(markers(&dir).is_empty(), "{log}");
}
