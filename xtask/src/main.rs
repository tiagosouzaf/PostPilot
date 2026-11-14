//! Tarefas de desenvolvimento: `cargo xtask <tarefa>`.
//!
//! - `bench`: mede o `pgstudio` compilado em release contra as metas do
//!   projeto: tempo até o primeiro frame e memória em repouso, rolagem de
//!   uma grade de 1 milhão de linhas, IntelliSense num catálogo de 10 mil
//!   tabelas, diagramas, Table Designer, um plano de 2.000 nós e o Activity
//!   Monitor com 5.000 sessões.
//! - `dist`: pacotes de instalação (.deb, AppImage e .zip do Windows); veja
//!   [`dist`].

mod dist;

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
/// Início da linha de resultado de um cenário. Igual a
/// `pgstudio_ui_shell::bench::RESULT_PREFIX`.
const RESULT: &str = "pgstudio-bench: result";
/// Meta da grade: 60 FPS rolando, em ms por frame (p95). Um frame a 60 Hz
/// dura 16,7 ms; os 20% de folga absorvem a variação do vsync, que num
/// monitor de 60 Hz deixa metade dos intervalos um pouco acima de 16,7 ms
/// mesmo sem nenhum frame perdido (um frame perdido dobra o intervalo).
const GRID_FRAME_TARGET_MS: f64 = 20.0;
/// Meta da grade: memória a mais que o app em repouso depois de carregar 1
/// milhão de linhas: o buffer de 64 MB do resultado mais 32 MB de folga.
const GRID_RAM_TARGET_MB: f64 = 96.0;
/// Meta do IntelliSense: sugestões em menos de 50 ms (pior tecla).
const INTELLISENSE_TARGET_MS: f64 = 50.0;
/// Meta do frame do diagrama com zoom e pan (p95): 60 FPS, como a grade.
/// Vale também para o plano e o Activity Monitor.
const DIAGRAM_FRAME_TARGET_MS: f64 = 20.0;
/// Nós do plano do cenário `plan`. Igual a
/// `pgstudio_ui_shell::bench::PLAN_NODES`.
const PLAN_NODES: f64 = 2000.0;
/// Sessões do cenário `activity`. Igual a
/// `pgstudio_ui_shell::bench::ACTIVITY_SESSIONS`.
const ACTIVITY_SESSIONS: f64 = 5000.0;
/// Linhas do Edit Top 200 Rows sintético.
const EDIT_ROWS: f64 = 200.0;
/// `pgstudio_ui_shell::bench::EXPORT_ROWS`.
const EXPORT_ROWS: f64 = 1_000_000.0;
/// Results to File não guarda as linhas: no fim, a RAM fica perto da de
/// repouso (folga para a janela de consulta e o buffer do arquivo).
const EXPORT_RAM_TARGET_MB: f64 = 32.0;

const USAGE: &str = "\
Uso: cargo xtask bench [opções]

Mede o pgstudio (release) em oito cenários e compara com as metas:

  startup       do início do processo até o primeiro frame (< 1 s) e memória
                depois de um tempo parado (< 100 MB; RSS no Linux, working
                set no Windows)
  grid          1 milhão de linhas sintéticas chegam e a grade rola do começo
                ao fim em 10 s: 60 FPS (p95 < 20 ms por frame: 16,7 ms
                mais folga para o vsync) e memória a mais que o repouso
                < 96 MB (buffer de 64 MB + folga)
  intellisense  digita SQL uma tecla por frame num banco de 10 mil tabelas
                com a lista de sugestões aberta: pior tecla < 50 ms
  diagram       um Database Diagram de 500 tabelas passeia de um canto a
                outro com zoom e pan por 10 s: 60 FPS (p95 < 20 ms por
                frame)
  diagram2000   o mesmo com 2.000 tabelas: 60 FPS
  designer      Table Designer num banco de 2.000 tabelas, 300 colunas
                acrescentadas uma por frame: 60 FPS; mede também o tempo de
                gerar o script de alterações
  plan          Ctrl+L devolve um plano de 2.000 nós (duas tabelas com 997
                partições), que passeia do topo ao fim com zoom por 10 s:
                60 FPS
  activity      Activity Monitor com 5.000 sessões e 10.000 locks,
                atualizado a cada segundo por 10 s: 60 FPS; mede também o
                tempo de cada atualização
  edit          Edit Top 200 Rows de uma tabela de 12 colunas, mudando uma
                célula e gravando a linha a cada frame por 10 s: 60 FPS;
                mede também a ida e volta de cada gravação
  export        Results to File de 1 milhão de linhas para um CSV: 60 FPS
                enquanto o arquivo é escrito, todas as linhas no arquivo e
                memória a mais que o repouso < 32 MB (as linhas não ficam
                na memória)

Opções:
  --scenario S   startup, grid, intellisense, diagram, diagram2000, designer,
                 plan, activity, edit, export ou all (padrão all)
  --runs N       execuções de cada cenário (padrão 5)
  --idle S       segundos parado antes de medir a memória (padrão 30)
  --no-build     não recompila; usa o binário existente
  --markdown     imprime o resultado como tabela Markdown
  --check        sai com erro se alguma meta não for atingida

No Linux sem tela, rode sob Xvfb: xvfb-run -a cargo xtask bench";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bench") => parse_bench(&args[1..]).and_then(bench),
        Some("dist") => dist::parse(&args[1..]).and_then(|a| dist::run(a, &workspace_root())),
        Some("-h" | "--help") | None => {
            println!("{USAGE}\n\n{}", dist::USAGE);
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
    scenarios: Vec<Scenario>,
    runs: usize,
    idle: u64,
    build: bool,
    markdown: bool,
    check: bool,
}

fn parse_bench(args: &[String]) -> Result<BenchArgs, String> {
    let mut out = BenchArgs {
        scenarios: Scenario::ALL.to_vec(),
        runs: 5,
        idle: 30,
        build: true,
        markdown: false,
        check: false,
    };
    let mut default_scenarios = true;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut number = |name: &str| -> Result<u64, String> {
            it.next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| format!("{name} precisa de um número"))
        };
        match arg.as_str() {
            "--runs" => out.runs = number("--runs")?.max(1) as usize,
            // Repetido, junta os cenários (`--scenario startup --scenario
            // edit` mede a RAM de repouso junto).
            "--scenario" => {
                let chosen = match it.next().map(String::as_str) {
                    Some("all") => Scenario::ALL.to_vec(),
                    Some(name) => vec![
                        Scenario::parse(name)
                            .ok_or_else(|| format!("cenário desconhecido: {name}"))?,
                    ],
                    None => return Err("--scenario precisa de um nome".into()),
                };
                if std::mem::take(&mut default_scenarios) {
                    out.scenarios.clear();
                }
                for s in chosen {
                    if !out.scenarios.contains(&s) {
                        out.scenarios.push(s);
                    }
                }
            }
            "--idle" => out.idle = number("--idle")?,
            "--no-build" => out.build = false,
            "--markdown" => out.markdown = true,
            "--check" => out.check = true,
            other => return Err(format!("opção desconhecida: {other}\n\n{USAGE}")),
        }
    }
    Ok(out)
}

/// O que medir.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Startup,
    Grid,
    IntelliSense,
    Diagram,
    DiagramLarge,
    Designer,
    Plan,
    Activity,
    Edit,
    Export,
}

impl Scenario {
    const ALL: [Scenario; 10] = [
        Scenario::Startup,
        Scenario::Grid,
        Scenario::IntelliSense,
        Scenario::Diagram,
        Scenario::DiagramLarge,
        Scenario::Designer,
        Scenario::Plan,
        Scenario::Activity,
        Scenario::Edit,
        Scenario::Export,
    ];

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    /// Nome no `--scenario` e no `PGSTUDIO_BENCH` do app.
    fn name(self) -> &'static str {
        match self {
            Scenario::Startup => "startup",
            Scenario::Grid => "grid",
            Scenario::IntelliSense => "intellisense",
            Scenario::Diagram => "diagram",
            Scenario::DiagramLarge => "diagram2000",
            Scenario::Designer => "designer",
            Scenario::Plan => "plan",
            Scenario::Activity => "activity",
            Scenario::Edit => "edit",
            Scenario::Export => "export",
        }
    }
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
    let mut grid = Vec::new();
    let mut intellisense = Vec::new();
    let mut diagram = Vec::new();
    let mut diagram_large = Vec::new();
    let mut designer = Vec::new();
    let mut plan = Vec::new();
    let mut activity = Vec::new();
    let mut edit = Vec::new();
    let mut export = Vec::new();
    for scenario in &args.scenarios {
        for run in 1..=args.runs {
            match scenario {
                Scenario::Startup => {
                    let s = measure(&exe, &root, Duration::from_secs(args.idle))?;
                    eprintln!(
                        "startup {run}/{}: primeiro frame {:.0} ms, RAM em repouso {:.1} MB{}",
                        args.runs,
                        s.startup_ms,
                        s.idle_mb,
                        s.peak_mb
                            .map(|p| format!(", pico {p:.1} MB"))
                            .unwrap_or_default()
                    );
                    samples.push(s);
                }
                _ => {
                    let r = measure_scenario(&exe, &root, *scenario)?;
                    eprintln!(
                        "{} {run}/{}: {} (RAM {:.1} MB)",
                        scenario.name(),
                        args.runs,
                        r.line,
                        r.ram_mb
                    );
                    match scenario {
                        Scenario::Grid => grid.push(r),
                        Scenario::IntelliSense => intellisense.push(r),
                        Scenario::Diagram => diagram.push(r),
                        Scenario::DiagramLarge => diagram_large.push(r),
                        Scenario::Plan => plan.push(r),
                        Scenario::Activity => activity.push(r),
                        Scenario::Edit => edit.push(r),
                        Scenario::Export => export.push(r),
                        _ => designer.push(r),
                    }
                }
            }
        }
    }

    let report = Report::new(&samples, args.idle);
    let scenarios = ScenarioReport {
        idle_mb: (!samples.is_empty()).then_some(report.idle_median),
        grid,
        intellisense,
        diagram,
        diagram_large,
        designer,
        plan,
        activity,
        edit,
        export,
    };
    let startup = !samples.is_empty();
    if args.markdown {
        if startup {
            println!("{}", report.markdown());
        }
        println!("{}", scenarios.markdown());
    } else {
        if startup {
            println!("{}", report.text());
        }
        println!("{}", scenarios.text());
    }
    let passed = (!startup || report.passed()) && scenarios.passed();
    Ok(!args.check || passed)
}

/// Uma execução de um cenário do app (grade ou IntelliSense).
#[derive(Clone, Debug, Default)]
struct ScenarioRun {
    /// A linha de resultado do app.
    line: String,
    values: std::collections::BTreeMap<String, f64>,
    /// Memória lida logo depois do resultado.
    ram_mb: f64,
}

impl ScenarioRun {
    fn parse(line: &str) -> Self {
        let values = line
            .strip_prefix(RESULT)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                Some((k.to_string(), v.parse().ok()?))
            })
            .collect();
        ScenarioRun {
            line: line.strip_prefix(RESULT).unwrap_or(line).trim().to_string(),
            values,
            ram_mb: 0.0,
        }
    }

    fn get(&self, key: &str) -> f64 {
        self.values.get(key).copied().unwrap_or(f64::NAN)
    }
}

/// Roda um cenário do app uma vez e lê a memória quando ele termina.
fn measure_scenario(exe: &Path, root: &Path, scenario: Scenario) -> Result<ScenarioRun, String> {
    let mut child = Command::new(exe)
        .current_dir(root)
        .env("PGSTUDIO_BENCH", scenario.name())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("não consegui abrir {}: {e}", exe.display()))?;
    let stdout = child.stdout.take().expect("stdout foi pedido");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.starts_with(RESULT) {
                let _ = tx.send(line);
            }
        }
    });
    let result = (|| {
        let line = rx
            .recv_timeout(Duration::from_secs(180))
            .map_err(|_| format!("o cenário {} não terminou em 180 s", scenario.name()))?;
        let mut run = ScenarioRun::parse(&line);
        run.ram_mb = memory_mb(child.id())?.0;
        Ok(run)
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Resumo dos cenários da grade e do IntelliSense. O pior caso conta para a
/// meta.
#[derive(Debug, Default)]
struct ScenarioReport {
    /// RAM em repouso (mediana do cenário startup), base da meta da grade.
    idle_mb: Option<f64>,
    grid: Vec<ScenarioRun>,
    intellisense: Vec<ScenarioRun>,
    diagram: Vec<ScenarioRun>,
    diagram_large: Vec<ScenarioRun>,
    designer: Vec<ScenarioRun>,
    plan: Vec<ScenarioRun>,
    activity: Vec<ScenarioRun>,
    edit: Vec<ScenarioRun>,
    export: Vec<ScenarioRun>,
}

/// Mediana e pior valor de uma chave nas execuções.
fn stats(runs: &[ScenarioRun], value: impl Fn(&ScenarioRun) -> f64) -> (f64, f64) {
    let mut v: Vec<f64> = runs.iter().map(value).collect();
    let worst = max(&v);
    (median(&mut v), worst)
}

impl ScenarioReport {
    fn grid_frame(&self) -> (f64, f64) {
        stats(&self.grid, |r| r.get("frame_p95"))
    }

    fn grid_ram(&self) -> Option<(f64, f64)> {
        let idle = self.idle_mb?;
        Some(stats(&self.grid, |r| r.ram_mb - idle))
    }

    fn intellisense_key(&self) -> (f64, f64) {
        stats(&self.intellisense, |r| r.get("ui_max"))
    }

    fn grid_ok(&self) -> bool {
        self.grid.is_empty()
            || (self.grid_frame().1 < GRID_FRAME_TARGET_MS
                && self.grid_ram().is_none_or(|r| r.1 < GRID_RAM_TARGET_MB)
                && self.grid.iter().all(|r| r.get("rows") == 1_000_000.0))
    }

    fn intellisense_ok(&self) -> bool {
        self.intellisense.is_empty()
            || (self.intellisense_key().1 < INTELLISENSE_TARGET_MS
                && self.intellisense.iter().all(|r| r.get("popup_keys") > 0.0))
    }

    fn diagram_frame(&self) -> (f64, f64) {
        stats(&self.diagram, |r| r.get("frame_p95"))
    }

    /// Frame p95 abaixo da meta em todas as execuções, com `tables` tabelas.
    fn frames_ok(runs: &[ScenarioRun], tables: f64) -> bool {
        Self::frames_ok_with(runs, "tables", tables)
    }

    /// Frame p95 abaixo da meta em todas as execuções, com `key` = `n`.
    fn frames_ok_with(runs: &[ScenarioRun], key: &str, n: f64) -> bool {
        runs.is_empty()
            || (stats(runs, |r| r.get("frame_p95")).1 < DIAGRAM_FRAME_TARGET_MS
                && runs.iter().all(|r| r.get(key) == n))
    }

    fn diagram_ok(&self) -> bool {
        Self::frames_ok(&self.diagram, 500.0)
    }

    fn passed(&self) -> bool {
        self.grid_ok()
            && self.intellisense_ok()
            && self.diagram_ok()
            && Self::frames_ok(&self.diagram_large, 2000.0)
            && Self::frames_ok(&self.designer, 2000.0)
            && Self::frames_ok_with(&self.plan, "nodes", PLAN_NODES)
            && Self::frames_ok_with(&self.activity, "sessions", ACTIVITY_SESSIONS)
            && Self::frames_ok_with(&self.edit, "rows", EDIT_ROWS)
            && self.export_ok()
    }

    /// RAM no fim da exportação, a mais que o repouso.
    fn export_ram(&self) -> Option<(f64, f64)> {
        let idle = self.idle_mb?;
        Some(stats(&self.export, |r| r.ram_mb - idle))
    }

    /// Exportação: frames na meta, todas as linhas no arquivo e a memória
    /// sem crescer com as linhas.
    fn export_ok(&self) -> bool {
        Self::frames_ok_with(&self.export, "rows", EXPORT_ROWS)
            && self.export_ram().is_none_or(|r| r.1 < EXPORT_RAM_TARGET_MB)
    }

    /// Linhas da tabela: medida, meta, mediana, pior e se passou.
    fn rows(&self) -> Vec<[String; 5]> {
        let ok = |pass: bool| if pass { "✅ dentro" } else { "❌ acima" }.to_string();
        let ms = |v: f64| format!("{v:.1} ms");
        let mb = |v: f64| format!("{v:.1} MB");
        let mut rows = Vec::new();
        if !self.grid.is_empty() {
            let (load_med, load_max) = stats(&self.grid, |r| r.get("load_ms"));
            rows.push([
                "Grade: 1 milhão de linhas chegam".into(),
                "—".into(),
                ms(load_med),
                ms(load_max),
                "—".into(),
            ]);
            let (med, worst) = self.grid_frame();
            rows.push([
                "Grade: frame rolando (p95)".into(),
                format!("< {GRID_FRAME_TARGET_MS:.0} ms (60 FPS)"),
                ms(med),
                ms(worst),
                ok(worst < GRID_FRAME_TARGET_MS),
            ]);
            let (med, worst) = stats(&self.grid, |r| r.get("ui_p95"));
            rows.push([
                "Grade: CPU da interface por frame (p95)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            match self.grid_ram() {
                Some((med, worst)) => rows.push([
                    "Grade: RAM a mais que o repouso".into(),
                    format!("< {GRID_RAM_TARGET_MB:.0} MB"),
                    mb(med),
                    mb(worst),
                    ok(worst < GRID_RAM_TARGET_MB),
                ]),
                None => {
                    let (med, worst) = stats(&self.grid, |r| r.ram_mb);
                    rows.push([
                        "Grade: RAM com 1 milhão de linhas".into(),
                        "—".into(),
                        mb(med),
                        mb(worst),
                        "—".into(),
                    ]);
                }
            }
        }
        if !self.intellisense.is_empty() {
            let (med, worst) = stats(&self.intellisense, |r| r.get("load_ms"));
            rows.push([
                "IntelliSense: catálogo de 10 mil tabelas carregado".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(&self.intellisense, |r| r.get("ui_p95"));
            rows.push([
                "IntelliSense: tecla (p95)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = self.intellisense_key();
            rows.push([
                "IntelliSense: pior tecla".into(),
                format!("< {INTELLISENSE_TARGET_MS:.0} ms"),
                ms(med),
                ms(worst),
                ok(self.intellisense_ok()),
            ]);
            let (med, worst) = stats(&self.intellisense, |r| r.ram_mb);
            rows.push([
                "IntelliSense: RAM com o catálogo".into(),
                "—".into(),
                mb(med),
                mb(worst),
                "—".into(),
            ]);
        }
        if !self.diagram.is_empty() {
            let (med, worst) = stats(&self.diagram, |r| r.get("load_ms"));
            rows.push([
                "Diagrama: 500 tabelas montadas (layout)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = self.diagram_frame();
            rows.push([
                "Diagrama: frame com zoom e pan (p95)".into(),
                format!("< {DIAGRAM_FRAME_TARGET_MS:.0} ms (60 FPS)"),
                ms(med),
                ms(worst),
                ok(self.diagram_ok()),
            ]);
            let (med, worst) = stats(&self.diagram, |r| r.get("ui_p95"));
            rows.push([
                "Diagrama: CPU da interface por frame (p95)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(&self.diagram, |r| r.ram_mb);
            rows.push([
                "Diagrama: RAM com 500 tabelas".into(),
                "—".into(),
                mb(med),
                mb(worst),
                "—".into(),
            ]);
        }
        if !self.diagram_large.is_empty() {
            let runs = &self.diagram_large;
            let (med, worst) = stats(runs, |r| r.get("load_ms"));
            rows.push([
                "Diagrama: 2.000 tabelas montadas (layout)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.get("frame_p95"));
            rows.push([
                "Diagrama 2.000: frame com zoom e pan (p95)".into(),
                format!("< {DIAGRAM_FRAME_TARGET_MS:.0} ms (60 FPS)"),
                ms(med),
                ms(worst),
                ok(Self::frames_ok(runs, 2000.0)),
            ]);
            let (med, worst) = stats(runs, |r| r.get("ui_p95"));
            rows.push([
                "Diagrama 2.000: CPU da interface por frame (p95)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.ram_mb);
            rows.push([
                "Diagrama: RAM com 2.000 tabelas".into(),
                "—".into(),
                mb(med),
                mb(worst),
                "—".into(),
            ]);
        }
        if !self.designer.is_empty() {
            let runs = &self.designer;
            let (med, worst) = stats(runs, |r| r.get("load_ms"));
            rows.push([
                "Table Designer: abre num banco de 2.000 tabelas".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.get("frame_p95"));
            rows.push([
                "Table Designer: frame com uma coluna nova (p95)".into(),
                format!("< {DIAGRAM_FRAME_TARGET_MS:.0} ms (60 FPS)"),
                ms(med),
                ms(worst),
                ok(Self::frames_ok(runs, 2000.0)),
            ]);
            let (med, worst) = stats(runs, |r| r.get("ui_p95"));
            rows.push([
                "Table Designer: CPU da interface por edição (p95)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.get("script_ms"));
            rows.push([
                "Table Designer: script de 300 colunas novas".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
        }
        if !self.plan.is_empty() {
            let runs = &self.plan;
            let (med, worst) = stats(runs, |r| r.get("load_ms"));
            rows.push([
                "Plano: 2.000 nós chegam e são desenhados (Ctrl+L)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.get("frame_p95"));
            rows.push([
                "Plano: frame com zoom e rolagem (p95)".into(),
                format!("< {DIAGRAM_FRAME_TARGET_MS:.0} ms (60 FPS)"),
                ms(med),
                ms(worst),
                ok(Self::frames_ok_with(runs, "nodes", PLAN_NODES)),
            ]);
            let (med, worst) = stats(runs, |r| r.get("ui_p95"));
            rows.push([
                "Plano: CPU da interface por frame (p95)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.ram_mb);
            rows.push([
                "Plano: RAM com o plano de 2.000 nós".into(),
                "—".into(),
                mb(med),
                mb(worst),
                "—".into(),
            ]);
        }
        if !self.activity.is_empty() {
            let runs = &self.activity;
            let (med, worst) = stats(runs, |r| r.get("load_ms"));
            rows.push([
                "Activity Monitor: primeira leitura de 5.000 sessões".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.get("frame_p95"));
            rows.push([
                "Activity Monitor: frame atualizando a cada 1 s (p95)".into(),
                format!("< {DIAGRAM_FRAME_TARGET_MS:.0} ms (60 FPS)"),
                ms(med),
                ms(worst),
                ok(Self::frames_ok_with(runs, "sessions", ACTIVITY_SESSIONS)),
            ]);
            let (med, worst) = stats(runs, |r| r.get("ui_p95"));
            rows.push([
                "Activity Monitor: CPU da interface por frame (p95)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.get("refresh_p95"));
            rows.push([
                "Activity Monitor: atualização pronta (p95)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.ram_mb);
            rows.push([
                "Activity Monitor: RAM com 5.000 sessões".into(),
                "—".into(),
                mb(med),
                mb(worst),
                "—".into(),
            ]);
        }
        if !self.edit.is_empty() {
            let runs = &self.edit;
            let (med, worst) = stats(runs, |r| r.get("load_ms"));
            rows.push([
                "Edição na grade: 200 linhas de 12 colunas abertas".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.get("frame_p95"));
            rows.push([
                "Edição na grade: frame editando e gravando (p95)".into(),
                format!("< {DIAGRAM_FRAME_TARGET_MS:.0} ms (60 FPS)"),
                ms(med),
                ms(worst),
                ok(Self::frames_ok_with(runs, "rows", EDIT_ROWS)),
            ]);
            let (med, worst) = stats(runs, |r| r.get("ui_p95"));
            rows.push([
                "Edição na grade: CPU da interface por frame (p95)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.get("save_p95"));
            rows.push([
                "Edição na grade: linha gravada, ida e volta (p95)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.get("saves"));
            rows.push([
                "Edição na grade: linhas gravadas em 10 s".into(),
                "—".into(),
                format!("{med:.0}"),
                format!("{worst:.0}"),
                "—".into(),
            ]);
        }
        if !self.export.is_empty() {
            let runs = &self.export;
            let (med, worst) = stats(runs, |r| r.get("export_ms"));
            rows.push([
                "Exportação: 1 milhão de linhas para CSV (Results to File)".into(),
                "—".into(),
                ms(med),
                ms(worst),
                "—".into(),
            ]);
            let (med, worst) = stats(runs, |r| r.get("frame_p95"));
            rows.push([
                "Exportação: frame enquanto o arquivo é escrito (p95)".into(),
                format!("< {DIAGRAM_FRAME_TARGET_MS:.0} ms (60 FPS)"),
                ms(med),
                ms(worst),
                ok(Self::frames_ok_with(runs, "rows", EXPORT_ROWS)),
            ]);
            match self.export_ram() {
                Some((med, worst)) => rows.push([
                    "Exportação: RAM a mais que o repouso".into(),
                    format!("< {EXPORT_RAM_TARGET_MB:.0} MB"),
                    mb(med),
                    mb(worst),
                    ok(worst < EXPORT_RAM_TARGET_MB),
                ]),
                None => {
                    let (med, worst) = stats(runs, |r| r.ram_mb);
                    rows.push([
                        "Exportação: RAM depois de 1 milhão de linhas".into(),
                        "—".into(),
                        mb(med),
                        mb(worst),
                        "—".into(),
                    ]);
                }
            }
        }
        rows
    }

    fn markdown(&self) -> String {
        let mut s = String::from(
            "| Cenário | Meta | Mediana | Pior | Situação |\n| --- | --- | --- | --- | --- |",
        );
        for r in self.rows() {
            s.push_str(&format!("\n| {} |", r.join(" | ")));
        }
        s
    }

    fn text(&self) -> String {
        self.rows()
            .iter()
            .map(|[name, target, med, worst, ok]| {
                let ok = ok
                    .replace("✅ dentro", "ok")
                    .replace("❌ acima", "ACIMA DA META");
                format!("{name}: mediana {med}, pior {worst} (meta {target}) {ok}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
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
        let a = args("--runs 3 --idle 10 --no-build --markdown --check --scenario grid").unwrap();
        assert_eq!(
            a,
            BenchArgs {
                scenarios: vec![Scenario::Grid],
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
        assert_eq!(args("--scenario all").unwrap().scenarios, Scenario::ALL);
        assert!(args("--scenario fast").is_err());
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
        let bench = include_str!("../../crates/ui-shell/src/bench.rs");
        assert!(bench.contains(&format!("\"{RESULT}\"")));
        for s in [Scenario::Grid, Scenario::IntelliSense] {
            assert!(bench.contains(&format!("\"{}\" =>", s.name())));
        }
    }

    fn scenario(line: &str, ram_mb: f64) -> ScenarioRun {
        ScenarioRun {
            ram_mb,
            ..ScenarioRun::parse(line)
        }
    }

    #[test]
    fn scenarios_check_the_worst_run_against_the_targets() {
        let grid = |p95: f64, ram| {
            scenario(
                &format!(
                    "{RESULT} scenario=grid load_ms=300 ui_p95=1 frame_p95={p95} rows=1000000"
                ),
                ram,
            )
        };
        let keys = |worst: f64| {
            scenario(
                &format!("{RESULT} scenario=intellisense ui_p95=2 ui_max={worst} popup_keys=100"),
                140.0,
            )
        };
        let diagram = |p95: f64, tables: u32| {
            scenario(
                &format!(
                    "{RESULT} scenario=diagram load_ms=4 ui_p95=3 frame_p95={p95} tables={tables}"
                ),
                130.0,
            )
        };
        let good = ScenarioReport {
            idle_mb: Some(120.0),
            grid: vec![grid(15.0, 190.0), grid(16.0, 200.0)],
            intellisense: vec![keys(4.0), keys(6.0)],
            diagram: vec![diagram(16.0, 500), diagram(17.0, 500)],
            diagram_large: vec![diagram(18.0, 2000)],
            designer: vec![scenario(
                &format!(
                    "{RESULT} scenario=designer load_ms=40 ui_p95=4 frame_p95=17 \
                     tables=2000 script_ms=12"
                ),
                150.0,
            )],
            plan: vec![scenario(
                &format!("{RESULT} scenario=plan load_ms=60 ui_p95=5 frame_p95=17 nodes=2000"),
                160.0,
            )],
            activity: vec![scenario(
                &format!(
                    "{RESULT} scenario=activity load_ms=80 ui_p95=6 frame_p95=17 \
                     sessions=5000 refresh_p95=90"
                ),
                170.0,
            )],
            edit: vec![scenario(
                &format!(
                    "{RESULT} scenario=edit load_ms=5 ui_p95=2 frame_p95=17 rows=200 \
                     saves=500 save_p95=12"
                ),
                130.0,
            )],
            export: vec![scenario(
                &format!(
                    "{RESULT} scenario=export ui_p95=1 frame_p95=17 rows=1000000 export_ms=900"
                ),
                130.0,
            )],
        };
        assert!(good.passed(), "{}", good.text());
        let md = good.markdown();
        assert!(
            md.contains(
                "| Grade: frame rolando (p95) | < 20 ms (60 FPS) | 15.5 ms | 16.0 ms | ✅ dentro |"
            ),
            "{md}"
        );
        assert!(
            md.contains(
                "| Grade: RAM a mais que o repouso | < 96 MB | 75.0 MB | 80.0 MB | ✅ dentro |"
            ),
            "{md}"
        );
        assert!(
            md.contains("| IntelliSense: pior tecla | < 50 ms | 5.0 ms | 6.0 ms | ✅ dentro |"),
            "{md}"
        );

        let janky = ScenarioReport {
            grid: vec![grid(15.0, 190.0), grid(30.0, 190.0)],
            ..Default::default()
        };
        assert!(!janky.passed());
        assert!(janky.text().contains("ACIMA DA META"));
        let heavy = ScenarioReport {
            idle_mb: Some(100.0),
            grid: vec![grid(10.0, 250.0)],
            ..Default::default()
        };
        assert!(!heavy.passed());
        let slow = ScenarioReport {
            intellisense: vec![keys(80.0)],
            ..Default::default()
        };
        assert!(!slow.passed());
        assert!(
            md.contains(
                "| Diagrama: frame com zoom e pan (p95) | < 20 ms (60 FPS) | 16.5 ms | 17.0 ms | ✅ dentro |"
            ),
            "{md}"
        );
        assert!(
            md.contains(
                "| Table Designer: script de 300 colunas novas | — | 12.0 ms | 12.0 ms | — |"
            ),
            "{md}"
        );
        assert!(
            md.contains(
                "| Activity Monitor: atualização pronta (p95) | — | 90.0 ms | 90.0 ms | — |"
            ),
            "{md}"
        );
        assert!(
            md.contains(
                "| Exportação: RAM a mais que o repouso | < 32 MB | 10.0 MB | 10.0 MB | ✅ dentro |"
            ),
            "{md}"
        );
        let heavy_export = ScenarioReport {
            idle_mb: Some(100.0),
            export: vec![scenario(
                &format!("{RESULT} scenario=export frame_p95=10 rows=1000000"),
                400.0,
            )],
            ..Default::default()
        };
        assert!(!heavy_export.passed());
        let lost_rows = ScenarioReport {
            export: vec![scenario(
                &format!("{RESULT} scenario=export frame_p95=10 rows=10"),
                100.0,
            )],
            ..Default::default()
        };
        assert!(!lost_rows.passed());
        let few_nodes = ScenarioReport {
            plan: vec![scenario(
                &format!("{RESULT} scenario=plan frame_p95=10 nodes=20"),
                100.0,
            )],
            ..Default::default()
        };
        assert!(!few_nodes.passed());
        let slow_large = ScenarioReport {
            diagram_large: vec![diagram(30.0, 2000)],
            ..Default::default()
        };
        assert!(!slow_large.passed());
        for bad in [diagram(25.0, 500), diagram(10.0, 3)] {
            let r = ScenarioReport {
                diagram: vec![bad],
                ..Default::default()
            };
            assert!(!r.passed());
        }
    }
}
