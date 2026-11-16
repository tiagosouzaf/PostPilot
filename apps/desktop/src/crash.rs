//! O rastro de uma queda no `postpilot.log`.
//!
//! Um pânico do Rust já entra no log (`pgstudio_ui_shell::applog`). O que
//! sumia sem deixar nada eram as mortes que não passam por ele:
//!
//! - **sinal fatal** (`SIGSEGV`, `SIGBUS`, `SIGILL`, `SIGFPE`, `SIGABRT`): um
//!   acesso inválido de memória, um estouro de pilha, um `abort` de uma
//!   biblioteca ou do driver de vídeo. O tratador grava o sinal, o endereço
//!   e a pilha de chamadas, e devolve o sinal ao sistema para o dump da
//!   queda continuar existindo.
//! - **pedido de fora** (`SIGTERM`, `SIGHUP`, `SIGINT`, `SIGQUIT`): alguém
//!   mandou o app fechar. A linha diz qual sinal e, quando o sistema
//!   informa, qual processo mandou.
//! - **morte sem aviso** (`SIGKILL`, falta de energia): nada roda dentro do
//!   processo. Cada execução deixa um arquivo `running-<pid>` na pasta de
//!   configuração e o apaga ao sair; a próxima abertura que encontra o
//!   arquivo de um processo que não existe mais escreve isso no log.
//!
//! Dentro do tratador só o começo é seguro de verdade (formata num buffer
//! da pilha e usa `open`/`write`). A pilha de chamadas aloca memória; se a
//! queda foi dentro do alocador ela pode travar, então um `alarm` encerra o
//! processo depois de alguns segundos, com a primeira linha já gravada.
#![cfg_attr(unix, allow(unsafe_code))]

#[cfg(not(unix))]
pub fn install(_config: &std::path::Path) {}

#[cfg(not(unix))]
pub fn clean_exit() {}

#[cfg(unix)]
pub use unix::{clean_exit, install};

#[cfg(unix)]
mod unix {
    use std::ffi::CString;
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use pgstudio_ui_shell::applog;

    /// Prefixo do arquivo que marca uma execução em andamento.
    const MARKER: &str = "running-";

    static LOG: OnceLock<CString> = OnceLock::new();
    static MARK: OnceLock<(PathBuf, CString)> = OnceLock::new();

    /// O app caiu.
    const FATAL: [(libc::c_int, &str); 5] = [
        (libc::SIGSEGV, "SIGSEGV"),
        (libc::SIGBUS, "SIGBUS"),
        (libc::SIGILL, "SIGILL"),
        (libc::SIGFPE, "SIGFPE"),
        (libc::SIGABRT, "SIGABRT"),
    ];
    /// Alguém pediu para o app fechar.
    const ASKED: [(libc::c_int, &str); 4] = [
        (libc::SIGTERM, "SIGTERM"),
        (libc::SIGHUP, "SIGHUP"),
        (libc::SIGINT, "SIGINT"),
        (libc::SIGQUIT, "SIGQUIT"),
    ];

    /// Avisa das execuções que sumiram, marca esta e passa a tratar os
    /// sinais. `config` é a pasta do `postpilot.log`.
    pub fn install(config: &Path) {
        report_lost_runs(config);
        let pid = std::process::id();
        let marker = config.join(format!("{MARKER}{pid}"));
        let stamp = applog::timestamp(std::time::SystemTime::now());
        let _ = std::fs::write(
            &marker,
            format!("{stamp} PostPilot {}\n", env!("CARGO_PKG_VERSION")),
        );
        if let Ok(c) = CString::new(marker.as_os_str().as_bytes()) {
            let _ = MARK.set((marker, c));
        }
        if let Ok(c) = CString::new(config.join("postpilot.log").as_os_str().as_bytes()) {
            let _ = LOG.set(c);
        }
        for (signal, _) in FATAL.iter().chain(&ASKED) {
            // SAFETY: `action` é preenchida por inteiro antes do
            // `sigaction`, e o tratador tem a assinatura de `SA_SIGINFO`.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = on_signal as *const () as usize;
                // Na pilha alternativa que o Rust já reserva por thread: um
                // estouro de pilha não tem pilha para o tratador.
                action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(*signal, &action, std::ptr::null_mut());
            }
        }
        self_test();
    }

    /// `PGSTUDIO_CRASH_TEST=segv|abort|overflow|panic` derruba o app três
    /// segundos depois de abrir, numa thread à parte: é como se confere que
    /// cada tipo de queda deixa o rastro esperado no log. Com `-now` no fim
    /// (`segv-now`) a queda é na hora, antes da janela (os testes).
    fn self_test() {
        let Ok(kind) = std::env::var("PGSTUDIO_CRASH_TEST") else {
            return;
        };
        #[allow(unconditional_recursion)]
        fn deep(n: u64) -> u64 {
            let pad = std::hint::black_box([n; 64]);
            deep(n + 1) + pad[0]
        }
        let now = kind.ends_with("-now");
        let kind = kind.trim_end_matches("-now").to_string();
        let thread = std::thread::Builder::new()
            .name("crash-test".into())
            .spawn(move || {
                if !now {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                }
                match kind.as_str() {
                    // SAFETY: nenhuma; a escrita inválida é o teste.
                    "segv" => unsafe { std::ptr::null_mut::<u8>().write_volatile(1) },
                    "abort" => std::process::abort(),
                    "overflow" => drop(std::hint::black_box(deep(0))),
                    _ => panic!("PGSTUDIO_CRASH_TEST"),
                }
            });
        if let (true, Ok(thread)) = (now, thread) {
            let _ = thread.join();
        }
    }

    /// O app saiu por conta própria: não há o que avisar na próxima vez.
    pub fn clean_exit() {
        if let Some((path, _)) = MARK.get() {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Os `running-<pid>` de processos que não existem mais.
    fn report_lost_runs(config: &Path) {
        let Ok(entries) = std::fs::read_dir(config) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name
                .to_str()
                .and_then(|n| n.strip_prefix(MARKER))
                .and_then(|p| p.parse::<libc::pid_t>().ok())
            else {
                continue;
            };
            if alive(pid) {
                continue;
            }
            let started = std::fs::read_to_string(entry.path()).unwrap_or_default();
            applog::line(&format!(
                "the previous run (pid {pid}, started {}) ended without a clean exit. If no \
                 panic or signal line comes before this one, the process was killed from \
                 outside (SIGKILL, out of memory, power loss).",
                started.trim()
            ));
            let _ = std::fs::remove_file(entry.path());
        }
    }

    fn alive(pid: libc::pid_t) -> bool {
        // SAFETY: o sinal 0 só consulta se o processo existe.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    /// Texto montado na pilha, sem alocar.
    struct Buf {
        bytes: [u8; 512],
        len: usize,
    }

    impl std::fmt::Write for Buf {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            let room = self.bytes.len() - self.len;
            let n = s.len().min(room);
            self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
            self.len += n;
            Ok(())
        }
    }

    fn write_all(fd: libc::c_int, bytes: &[u8]) {
        let mut rest = bytes;
        while !rest.is_empty() {
            // SAFETY: `rest` aponta para `rest.len()` bytes válidos.
            let n = unsafe { libc::write(fd, rest.as_ptr().cast(), rest.len()) };
            if n <= 0 {
                return;
            }
            rest = &rest[n as usize..];
        }
    }

    /// O nome do processo `pid` (`/proc/<pid>/comm`), sem alocar.
    fn comm(pid: libc::pid_t, out: &mut [u8; 32]) -> &str {
        let mut path = Buf {
            bytes: [0; 512],
            len: 0,
        };
        let _ = write!(path, "/proc/{pid}/comm\0");
        // SAFETY: `path` termina em zero e `out` tem o tamanho passado.
        let n = unsafe {
            let fd = libc::open(path.bytes.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC);
            if fd < 0 {
                return "";
            }
            let n = libc::read(fd, out.as_mut_ptr().cast(), out.len());
            libc::close(fd);
            n
        };
        if n <= 0 {
            return "";
        }
        std::str::from_utf8(&out[..n as usize])
            .unwrap_or("")
            .trim_end()
    }

    extern "C" fn on_signal(
        signal: libc::c_int,
        info: *mut libc::siginfo_t,
        _context: *mut libc::c_void,
    ) {
        // SAFETY: só chamadas que podem rodar num tratador de sinal até a
        // parte marcada abaixo; `info` vem do sistema (`SA_SIGINFO`).
        unsafe {
            // Daqui em diante os sinais voltam ao padrão: uma segunda falha
            // (deste sinal ou de outro, aqui dentro) encerra o processo na
            // hora em vez de entrar aqui de novo.
            for (s, _) in FATAL.iter().chain(&ASKED) {
                libc::signal(*s, libc::SIG_DFL);
            }
            let fatal = FATAL.iter().find(|(s, _)| *s == signal);
            let name = fatal
                .or_else(|| ASKED.iter().find(|(s, _)| *s == signal))
                .map_or("?", |(_, n)| n);

            let mut text = Buf {
                bytes: [0; 512],
                len: 0,
            };
            let mut now: libc::timespec = std::mem::zeroed();
            libc::clock_gettime(libc::CLOCK_REALTIME, &mut now);
            let _ = applog::write_timestamp(
                &mut text,
                now.tv_sec as u64,
                (now.tv_nsec / 1_000_000) as u32,
            );
            if fatal.is_some() {
                let _ = write!(
                    text,
                    " fatal signal {name} ({signal}): PostPilot {} crashed",
                    env!("CARGO_PKG_VERSION")
                );
                if !info.is_null() && (signal == libc::SIGSEGV || signal == libc::SIGBUS) {
                    let _ = write!(
                        text,
                        " reading or writing address {:p} (a stack overflow looks like this too)",
                        (*info).si_addr()
                    );
                }
                let _ = writeln!(text, ". Call stack:");
            } else {
                let _ = write!(text, " terminated by signal {name} ({signal})");
                // `si_code <= 0`: mandado por um processo (`kill`), e não
                // pelo sistema (Ctrl+C no terminal, por exemplo).
                if !info.is_null() && (*info).si_code <= 0 {
                    let pid = (*info).si_pid();
                    let mut raw = [0u8; 32];
                    let _ = write!(text, " sent by pid {pid} ({})", comm(pid, &mut raw));
                }
                let _ = writeln!(text, ": something asked PostPilot to close.");
            }
            let fd = match LOG.get() {
                Some(path) => libc::open(
                    path.as_ptr(),
                    libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT | libc::O_CLOEXEC,
                    0o600,
                ),
                None => -1,
            };
            for out in [fd, 2] {
                if out >= 0 {
                    write_all(out, &text.bytes[..text.len]);
                }
            }
            if fatal.is_some() {
                // A pilha aloca memória e pode travar se a queda foi dentro
                // do alocador: o alarme encerra o processo nesse caso.
                libc::signal(libc::SIGALRM, libc::SIG_DFL);
                libc::alarm(5);
                let thread = std::thread::current();
                let name = thread.name().unwrap_or("?").to_string();
                // Aqui só os endereços (barato). Achar os nomes das funções
                // gasta muito mais pilha do que a pilha alternativa de um
                // sinal tem, então isso roda numa thread nova.
                let trace = std::backtrace::Backtrace::force_capture();
                let writer = std::thread::Builder::new()
                    .name("postpilot-crash".into())
                    .stack_size(4 * 1024 * 1024)
                    .spawn(move || {
                        let trace = applog::short_trace(&trace.to_string());
                        let text = format!("thread '{name}'\n{trace}\n");
                        for out in [fd, 2] {
                            if out >= 0 {
                                write_all(out, text.as_bytes());
                            }
                        }
                    });
                if let Ok(writer) = writer {
                    let _ = writer.join();
                }
                libc::alarm(0);
            }
            if fd >= 0 {
                libc::close(fd);
            }
            // O motivo ficou no log: a próxima abertura não precisa avisar.
            if let Some((_, marker)) = MARK.get() {
                libc::unlink(marker.as_ptr());
            }
            // De volta ao sistema, com a ação padrão: o código de saída e o
            // dump da queda ficam como seriam sem este tratador.
            libc::raise(signal);
        }
    }
}
