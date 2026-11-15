//! Log em arquivo e vigia da thread da interface.
//!
//! O log fica em `<config>/PostPilot/postpilot.log` (o mesmo lugar das
//! outras configurações). Quando passa de [`MAX_BYTES`], o atual vira
//! `postpilot.log.1` e um novo começa. Cada linha tem a hora local do
//! sistema em UTC (`2026-11-15T17:36:21Z`), para comparar com o
//! `journalctl --utc`.
//!
//! O vigia ([`Watchdog`]) roda numa thread própria e escreve no log quando a
//! janela para de desenhar:
//!
//! - **dentro do código do app**: um frame (`ShellApp::ui`) passou de
//!   [`STALL`] sem terminar. A linha diz em que etapa ele estava (veja
//!   [`phase`]), o que aponta o trecho que bloqueou.
//! - **fora do código do app**: o vigia pediu um frame e nenhum começou em
//!   [`STALL`]. Aí a espera é no desenho (wgpu/glow, driver de vídeo) ou na
//!   conversa com o compositor, não no PostPilot. Com a janela escondida
//!   (outro workspace, minimizada) o compositor pode segurar os frames de
//!   propósito; a linha diz se a janela estava em foco no último frame.
//!
//! Em ambos os casos, quando os frames voltam, uma linha diz quanto tempo
//! a janela ficou parada.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Tamanho a partir do qual o log é trocado por um novo.
pub const MAX_BYTES: u64 = 1024 * 1024;
/// Quanto tempo sem frame conta como janela travada. O Hyprland avisa que o
/// app "não está respondendo" depois de alguns segundos; isto fica abaixo.
pub const STALL: Duration = Duration::from_millis(1500);

struct Log {
    path: PathBuf,
    file: Option<File>,
}

static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

/// Abre (ou cria) o log em `path`. Sem chamar isto, [`line`] só escreve na
/// saída de erro.
pub fn init(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    rotate_if_big(path);
    let file = open(path);
    let _ = LOG.set(Mutex::new(Log {
        path: path.to_path_buf(),
        file,
    }));
    install_panic_hook();
}

/// Caminho do log, se ele foi aberto.
pub fn path() -> Option<PathBuf> {
    LOG.get().map(|l| lock(l).path.clone())
}

fn open(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn rotate_if_big(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(path, old);
    }
}

fn lock(log: &Mutex<Log>) -> std::sync::MutexGuard<'_, Log> {
    log.lock().unwrap_or_else(|e| e.into_inner())
}

/// Escreve uma linha no log (com a hora) e na saída de erro.
pub fn line(message: &str) {
    let stamped = format!("{} {message}", timestamp(SystemTime::now()));
    eprintln!("postpilot: {message}");
    let Some(log) = LOG.get() else {
        return;
    };
    let mut log = lock(log);
    if log
        .file
        .as_ref()
        .is_some_and(|f| f.metadata().is_ok_and(|m| m.len() > MAX_BYTES))
    {
        log.file = None;
        rotate_if_big(&log.path.clone());
        log.file = open(&log.path.clone());
    }
    if let Some(file) = &mut log.file {
        let _ = writeln!(file, "{stamped}");
        let _ = file.flush();
    }
}

/// `line` com `format!`.
#[macro_export]
macro_rules! log_line {
    ($($arg:tt)*) => {
        $crate::applog::line(&format!($($arg)*))
    };
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("?");
        line(&format!("panic in thread '{name}': {info}"));
        previous(info);
    }));
}

/// `2026-11-15T17:36:21.123Z`, sem depender de biblioteca de datas.
pub fn timestamp(t: SystemTime) -> String {
    let since = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        since.subsec_millis()
    )
}

/// Dias desde 1970-01-01 para (ano, mês, dia), pelo algoritmo de Howard
/// Hinnant.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// O que a thread da interface está fazendo, para o vigia.
#[derive(Debug)]
struct Beat {
    /// Dentro de `ShellApp::ui`.
    in_frame: bool,
    /// Etapa do frame atual (ou a última).
    phase: &'static str,
    /// Quando o frame atual começou, ou quando o último terminou.
    since: Instant,
    /// Frames desenhados desde o começo.
    frames: u64,
    /// A janela tinha foco no último frame.
    focused: bool,
}

/// O vigia da thread da interface. Barato: a interface só troca um campo
/// num mutex sem disputa a cada etapa.
#[derive(Clone)]
pub struct Watchdog {
    beat: Arc<Mutex<Beat>>,
}

impl Watchdog {
    /// Começa a vigiar. `wake` pede um frame (o `request_repaint` do egui).
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let beat = Arc::new(Mutex::new(Beat {
            in_frame: false,
            phase: "startup",
            since: Instant::now(),
            frames: 0,
            focused: true,
        }));
        let dog = Self { beat: beat.clone() };
        let watched = dog.clone();
        let _ = std::thread::Builder::new()
            .name("postpilot-watchdog".into())
            .spawn(move || watched.run(&wake));
        dog
    }

    fn beat(&self) -> std::sync::MutexGuard<'_, Beat> {
        self.beat.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Começo de um frame.
    pub fn frame_start(&self) {
        let mut b = self.beat();
        b.in_frame = true;
        b.phase = "frame";
        b.since = Instant::now();
    }

    /// Etapa do frame em andamento.
    pub fn phase(&self, phase: &'static str) {
        self.beat().phase = phase;
    }

    /// Fim de um frame.
    pub fn frame_end(&self, focused: bool) {
        let mut b = self.beat();
        b.in_frame = false;
        b.since = Instant::now();
        b.frames += 1;
        b.focused = focused;
    }

    fn run(&self, wake: &dyn Fn()) {
        let tick = Duration::from_millis(250);
        // Uma parada de cada vez: (frames quando começou, já avisada, desde).
        let mut stall: Option<(u64, Instant)> = None;
        let mut nudged: Option<Instant> = None;
        loop {
            std::thread::sleep(tick);
            let (in_frame, phase, since, frames, focused) = {
                let b = self.beat();
                (b.in_frame, b.phase, b.since, b.frames, b.focused)
            };
            let now = Instant::now();
            // Parada dentro do frame.
            if in_frame && now - since >= STALL {
                if stall.is_none() {
                    line(&format!(
                        "watchdog: the window stopped responding: the UI thread has been in \
                         '{phase}' for {:.1}s",
                        (now - since).as_secs_f32()
                    ));
                    stall = Some((frames, since));
                }
                continue;
            }
            // Fora do frame: pede um de vez em quando e vê se ele vem.
            if !in_frame && now - since >= STALL {
                match nudged {
                    None => {
                        nudged = Some(now);
                        wake();
                    }
                    Some(at) if now - at >= STALL && stall.is_none() => {
                        line(&format!(
                            "watchdog: no frame for {:.1}s after asking for one: waiting \
                             outside the app code (drawing or the compositor); window \
                             focused in the last frame: {focused}",
                            (now - since).as_secs_f32()
                        ));
                        stall = Some((frames, since));
                    }
                    Some(_) => {}
                }
                continue;
            }
            nudged = None;
            if let Some((before, started)) = stall.take()
                && frames >= before
            {
                line(&format!(
                    "watchdog: the window is drawing again after {:.1}s",
                    (now - started).as_secs_f32()
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_iso() {
        let t = UNIX_EPOCH + Duration::from_millis(1_794_764_181_123);
        assert_eq!(timestamp(t), "2026-11-15T17:36:21.123Z");
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29
        assert_eq!(&timestamp(leap)[..10], "2000-02-29");
    }

    #[test]
    fn log_rotates_when_big() {
        let dir = std::env::temp_dir().join(format!("pp-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("postpilot.log");
        std::fs::write(&path, vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();
        rotate_if_big(&path);
        assert!(!path.exists());
        assert!(dir.join("postpilot.log.1").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
