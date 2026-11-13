//! Tarefas de desenvolvimento: `cargo xtask <tarefa>`.
//!
//! - `bench`: mede o tempo até o primeiro frame e a memória em repouso do
//!   `pgstudio` compilado em release, contra as metas do projeto.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Linha que o app escreve em modo de medição quando o primeiro frame foi
/// exibido. Igual a `pgstudio_ui_shell::BENCH_FIRST_FRAME`.
const FIRST_FRAME: &str = "pgstudio-bench: first-frame";

/// Meta de inicialização a frio, em milissegundos.
const STARTUP_TARGET_MS: f64 = 1000.0;
/// Meta de memória em repouso, em MB.
const IDLE_RAM_TARGET_MB: f64 = 100.0;

const USAGE: &str = "\
Uso: cargo xtask bench [opções]

Mede o pgstudio (release): tempo do início do processo até o primeiro frame
exibido e memória (RSS no Linux, working set no Windows) depois de um tempo
parado. Compara com as metas: inicialização < 1 s e RAM em repouso < 100 MB.

Opções:
  --runs N       execuções (padrão 5)
  --idle S       segundos parado antes de medir a memória (padrão 30)
  --no-build     não recompila; usa o binário existente
  --markdown     imprime o resultado como tabela Markdown
  --check        sai com erro se alguma meta não for atingida

No Linux sem tela, rode sob Xvfb: xvfb-run -a cargo xtask bench";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bench") => parse_bench(&args[1..]).and_then(bench),
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("tarefa desconhecida: {other}\n\n{USAGE}")),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, PartialEq)]
struct BenchArgs {
    runs: usize,
    idle: u64,
    build: bool,
    markdown: bool,
    check: bool,
}

fn parse_bench(args: &[String]) -> Result<BenchArgs, String> {
    let mut out = BenchArgs {
        runs: 5,
        idle: 30,
        build: true,
        markdown: false,
        check: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut number = |name: &str| -> Result<u64, String> {
            it.next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| format!("{name} precisa de um número"))
        };
        match arg.as_str() {
            "--runs" => out.runs = number("--runs")?.max(1) as usize,
            "--idle" => out.idle = number("--idle")?,
            "--no-build" => out.build = false,
            "--markdown" => out.markdown = true,
            "--check" => out.check = true,
            other => return Err(format!("opção desconhecida: {other}\n\n{USAGE}")),
        }
    }
    Ok(out)
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask fica dentro do workspace")
        .to_path_buf()
}

/// Uma execução medida.
#[derive(Clone, Copy, Debug)]
struct Sample {
    startup_ms: f64,
    idle_mb: f64,
    peak_mb: Option<f64>,
}

fn bench(args: BenchArgs) -> Result<bool, String> {
    let root = workspace_root();
    if args.build {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "--release", "--locked", "-p", "pgstudio-desktop"])
            .current_dir(&root)
            .status()
            .map_err(|e| format!("não consegui rodar o cargo: {e}"))?;
        if !status.success() {
            return Err("a compilação falhou".into());
        }
    }
    let exe = root
        .join("target")
        .join("release")
        .join(format!("pgstudio{}", std::env::consts::EXE_SUFFIX));
    if !exe.exists() {
        return Err(format!("{} não existe; rode sem --no-build", exe.display()));
    }

    let mut samples = Vec::new();
    for run in 1..=args.runs {
        let s = measure(&exe, &root, Duration::from_secs(args.idle))?;
        eprintln!(
            "execução {run}/{}: primeiro frame {:.0} ms, RAM em repouso {:.1} MB{}",
            args.runs,
            s.startup_ms,
            s.idle_mb,
            s.peak_mb
                .map(|p| format!(", pico {p:.1} MB"))
                .unwrap_or_default()
        );
        samples.push(s);
    }

    let report = Report::new(&samples, args.idle);
    if args.markdown {
        println!("{}", report.markdown());
    } else {
        println!("{}", report.text());
    }
    Ok(!args.check || report.passed())
}

/// Roda o app uma vez em modo de medição.
fn measure(exe: &Path, root: &Path, idle: Duration) -> Result<Sample, String> {
    let started = Instant::now();
    let mut child = Command::new(exe)
        .current_dir(root)
        .env("PGSTUDIO_BENCH", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("não consegui abrir {}: {e}", exe.display()))?;
    let stdout = child.stdout.take().expect("stdout foi pedido");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.trim() == FIRST_FRAME {
                let _ = tx.send(Instant::now());
            }
        }
    });

    let result = (|| {
        let shown = rx
            .recv_timeout(Duration::from_secs(60))
            .map_err(|_| "o primeiro frame não apareceu em 60 s".to_string())?;
        let startup_ms = (shown - started).as_secs_f64() * 1000.0;
        std::thread::sleep(idle);
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("o app fechou sozinho ({status})"));
        }
        let (idle_mb, peak_mb) = memory_mb(child.id())?;
        Ok(Sample {
            startup_ms,
            idle_mb,
            peak_mb,
        })
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Memória atual e máxima do processo, em MB.
#[cfg(target_os = "linux")]
fn memory_mb(pid: u32) -> Result<(f64, Option<f64>), String> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .map_err(|e| format!("não consegui ler /proc/{pid}/status: {e}"))?;
    let field = |name: &str| {
        status
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<f64>().ok())
            .map(|kb| kb / 1024.0)
    };
    let rss = field("VmRSS:").ok_or("VmRSS não encontrado")?;
    Ok((rss, field("VmHWM:")))
}

/// Working set atual e máximo, pelo PowerShell (sem código `unsafe`).
#[cfg(windows)]
fn memory_mb(pid: u32) -> Result<(f64, Option<f64>), String> {
    let script =
        format!("$p = Get-Process -Id {pid}; \"$($p.WorkingSet64) $($p.PeakWorkingSet64)\"");
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| format!("não consegui rodar o PowerShell: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut nums = text
        .split_whitespace()
        .filter_map(|n| n.parse::<f64>().ok())
        .map(|b| b / (1024.0 * 1024.0));
    let ws = nums
        .next()
        .ok_or_else(|| format!("resposta inesperada do PowerShell: {text}"))?;
    Ok((ws, nums.next()))
}

/// RSS pelo `ps` nos demais sistemas.
#[cfg(not(any(target_os = "linux", windows)))]
fn memory_mb(pid: u32) -> Result<(f64, Option<f64>), String> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .map_err(|e| format!("não consegui rodar o ps: {e}"))?;
    let kb: f64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| "resposta inesperada do ps".to_string())?;
    Ok((kb / 1024.0, None))
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n == 0 {
        return f64::NAN;
    }
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

fn max(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::NAN, f64::max)
}

/// Resumo das execuções.
#[derive(Debug)]
struct Report {
    runs: usize,
    idle: u64,
    first_startup_ms: f64,
    startup_median: f64,
    startup_max: f64,
    idle_median: f64,
    idle_max: f64,
    peak_max: Option<f64>,
}

impl Report {
    fn new(samples: &[Sample], idle: u64) -> Self {
        let mut startup: Vec<f64> = samples.iter().map(|s| s.startup_ms).collect();
        let mut ram: Vec<f64> = samples.iter().map(|s| s.idle_mb).collect();
        let peaks: Vec<f64> = samples.iter().filter_map(|s| s.peak_mb).collect();
        Report {
            runs: samples.len(),
            idle,
            first_startup_ms: samples.first().map_or(f64::NAN, |s| s.startup_ms),
            startup_median: median(&mut startup),
            startup_max: max(&startup),
            idle_median: median(&mut ram),
            idle_max: max(&ram),
            peak_max: (!peaks.is_empty()).then(|| max(&peaks)),
        }
    }

    fn startup_ok(&self) -> bool {
        self.startup_max < STARTUP_TARGET_MS
    }

    fn ram_ok(&self) -> bool {
        self.idle_max < IDLE_RAM_TARGET_MB
    }

    fn passed(&self) -> bool {
        self.startup_ok() && self.ram_ok()
    }

    fn system() -> String {
        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
    }

    fn text(&self) -> String {
        let verdict = |ok| if ok { "ok" } else { "ACIMA DA META" };
        let mut s = format!(
            "{} execuções em {}, {} s em repouso\n\
             primeiro frame: mediana {:.0} ms, pior {:.0} ms, 1ª execução {:.0} ms (meta < {:.0} ms): {}\n\
             RAM em repouso: mediana {:.1} MB, pior {:.1} MB (meta < {:.0} MB): {}",
            self.runs,
            Self::system(),
            self.idle,
            self.startup_median,
            self.startup_max,
            self.first_startup_ms,
            STARTUP_TARGET_MS,
            verdict(self.startup_ok()),
            self.idle_median,
            self.idle_max,
            IDLE_RAM_TARGET_MB,
            verdict(self.ram_ok()),
        );
        if let Some(peak) = self.peak_max {
            s.push_str(&format!("\nRAM máxima durante a execução: {peak:.1} MB"));
        }
        s
    }

    fn markdown(&self) -> String {
        let verdict = |ok| if ok { "✅ dentro" } else { "❌ acima" };
        let mut s = format!(
            "{} execuções em `{}`, {} s em repouso.\n\n\
             | Medida | Meta | Mediana | Pior | Situação |\n\
             | --- | --- | --- | --- | --- |\n\
             | Até o primeiro frame | < 1 s | {:.0} ms | {:.0} ms | {} |\n\
             | RAM em repouso | < 100 MB | {:.1} MB | {:.1} MB | {} |",
            self.runs,
            Self::system(),
            self.idle,
            self.startup_median,
            self.startup_max,
            verdict(self.startup_ok()),
            self.idle_median,
            self.idle_max,
            verdict(self.ram_ok()),
        );
        if let Some(peak) = self.peak_max {
            s.push_str(&format!("\n| RAM máxima | — | — | {peak:.1} MB | — |"));
        }
        s.push_str(&format!(
            "\n\nPrimeira execução (mais perto de uma partida a frio): {:.0} ms.",
            self.first_startup_ms
        ));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Result<BenchArgs, String> {
        let v: Vec<String> = s.split_whitespace().map(String::from).collect();
        parse_bench(&v)
    }

    #[test]
    fn parses_options() {
        let a = args("--runs 3 --idle 10 --no-build --markdown --check").unwrap();
        assert_eq!(
            a,
            BenchArgs {
                runs: 3,
                idle: 10,
                build: false,
                markdown: true,
                check: true
            }
        );
        assert_eq!(args("").unwrap().runs, 5);
        assert!(args("--runs").is_err());
        assert!(args("--runs x").is_err());
        assert!(args("--fast").is_err());
    }

    #[test]
    fn median_and_max() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), 2.5);
        assert!(median(&mut []).is_nan());
        assert_eq!(max(&[1.0, 5.0, 2.0]), 5.0);
    }

    #[test]
    fn report_checks_the_worst_run_against_the_targets() {
        let s = |startup_ms, idle_mb| Sample {
            startup_ms,
            idle_mb,
            peak_mb: Some(idle_mb + 10.0),
        };
        let good = Report::new(&[s(300.0, 70.0), s(250.0, 72.0)], 30);
        assert!(good.passed());
        assert!(
            good.markdown()
                .contains("| Até o primeiro frame | < 1 s | 275 ms | 300 ms | ✅ dentro |")
        );
        let slow = Report::new(&[s(300.0, 70.0), s(1200.0, 72.0)], 30);
        assert!(!slow.passed());
        assert!(slow.text().contains("ACIMA DA META"));
        let heavy = Report::new(&[s(300.0, 140.0)], 30);
        assert!(!heavy.passed());
    }

    #[test]
    fn first_frame_line_matches_the_app() {
        // Se mudar no app, mude aqui (o xtask não depende da casca para
        // compilar rápido).
        let shell = include_str!("../../crates/ui-shell/src/lib.rs");
        assert!(shell.contains(&format!("\"{FIRST_FRAME}\"")));
    }
}
