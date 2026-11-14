//! `cargo xtask pgmatrix`: a suíte de testes contra cada versão do
//! PostgreSQL que o PGStudio promete suportar (13 ou superior).
//!
//! Para cada versão, cria um cluster descartável com o `initdb` daquela
//! versão, sobe o servidor numa porta própria e roda `cargo test` com as
//! variáveis `PGSTUDIO_TEST_*` apontando para ele. `PGSTUDIO_PG_BIN` aponta
//! para os binários da mesma versão, para os testes de backup e restore
//! usarem o `pg_dump` e o `pg_restore` dela. No fim, imprime uma tabela com
//! o resultado de cada versão; a que não tiver binários aparece como "não
//! testada" e faz o comando falhar, a menos que se peça `--allow-missing`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Versões suportadas, da mais antiga para a mais nova.
pub const VERSIONS: [u32; 5] = [13, 14, 15, 16, 17];

/// Senha do superusuário dos clusters de teste.
const PASSWORD: &str = "pgmatrix";

pub const USAGE: &str = "\
Uso: cargo xtask pgmatrix [opções] [-- argumentos do cargo test]

Roda a suíte contra PostgreSQL 13, 14, 15, 16 e 17. Para cada versão cria
um cluster descartável (initdb), sobe o servidor na porta 54300 + versão
(54313 a 54317), roda `cargo test --workspace` apontando para ele e apaga o
cluster no fim. A extensão citext (contrib) precisa estar instalada em cada
versão.

Opções:
  --versions 13,15    só estas versões (padrão: todas)
  --pg-bin V=PASTA    pasta com initdb, pg_ctl e pg_dump da versão V
                      (repetível); também vale PGMATRIX_PG<V>_BIN
  --run-as USUÁRIO    roda initdb e pg_ctl como outro usuário (o servidor
                      não aceita rodar como root; útil em contêineres)
  --allow-missing     versão sem binários não faz o comando falhar
  --keep              não apaga os clusters nem os logs do servidor

Sem --pg-bin, procura em /usr/lib/postgresql/V/bin (Debian/Ubuntu),
/usr/pgsql-V/bin (RHEL/Fedora), /opt/pgmatrix/V/bin, /opt/postgresql-V/bin
e C:\\Program Files\\PostgreSQL\\V\\bin.

Distribuições como o Arch só empacotam a versão atual. As outras se compilam
do código-fonte em poucos minutos cada, por exemplo:
  ./configure --prefix=/opt/pgmatrix/13 --without-icu
  make -j && make install && make -C contrib/citext install

Os logs do cargo test ficam em target/pgmatrix/<V>.log.";

#[derive(Debug, Default, PartialEq)]
pub struct MatrixArgs {
    versions: Vec<u32>,
    bins: BTreeMap<u32, PathBuf>,
    run_as: Option<String>,
    allow_missing: bool,
    keep: bool,
    test_args: Vec<String>,
}

pub fn parse(args: &[String]) -> Result<MatrixArgs, String> {
    let mut out = MatrixArgs {
        versions: VERSIONS.to_vec(),
        ..MatrixArgs::default()
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--versions" => {
                let list = it.next().ok_or("--versions precisa de uma lista")?;
                out.versions = list
                    .split(',')
                    .map(|v| parse_version(v.trim()))
                    .collect::<Result<_, _>>()?;
            }
            "--pg-bin" => {
                let spec = it.next().ok_or("--pg-bin precisa de V=PASTA")?;
                let (v, dir) = spec
                    .split_once('=')
                    .ok_or_else(|| format!("--pg-bin espera V=PASTA, não {spec}"))?;
                out.bins.insert(parse_version(v)?, PathBuf::from(dir));
            }
            "--run-as" => {
                out.run_as = Some(it.next().ok_or("--run-as precisa de um usuário")?.clone());
            }
            "--allow-missing" => out.allow_missing = true,
            "--keep" => out.keep = true,
            "--" => {
                out.test_args = it.cloned().collect();
                break;
            }
            other => return Err(format!("opção desconhecida: {other}\n\n{USAGE}")),
        }
    }
    Ok(out)
}

fn parse_version(v: &str) -> Result<u32, String> {
    match v.parse() {
        Ok(n) if VERSIONS.contains(&n) => Ok(n),
        _ => Err(format!(
            "versão {v} fora da matriz (suportadas: {})",
            VERSIONS.map(|v| v.to_string()).join(", ")
        )),
    }
}

/// Onde procurar os binários de uma versão, na ordem.
fn candidate_dirs(version: u32) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os(format!("PGMATRIX_PG{version}_BIN")) {
        dirs.push(PathBuf::from(dir));
    }
    if cfg!(windows) {
        dirs.push(PathBuf::from(format!(
            r"C:\Program Files\PostgreSQL\{version}\bin"
        )));
    } else {
        for pattern in [
            "/usr/lib/postgresql/{}/bin",
            "/usr/pgsql-{}/bin",
            "/opt/pgmatrix/{}/bin",
            "/opt/postgresql-{}/bin",
        ] {
            dirs.push(PathBuf::from(pattern.replace("{}", &version.to_string())));
        }
    }
    dirs
}

fn exe(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// A versão principal que `postgres --version` informa
/// ("postgres (PostgreSQL) 13.7" dá 13).
fn binary_major(dir: &Path) -> Option<u32> {
    let out = Command::new(exe(dir, "postgres"))
        .arg("--version")
        .output()
        .ok()?;
    parse_major(&String::from_utf8_lossy(&out.stdout))
}

fn parse_major(version_line: &str) -> Option<u32> {
    let number = version_line.split_whitespace().last()?;
    number.split(['.', 'b', 'r']).next()?.parse().ok()
}

/// A pasta de binários da versão: a de `--pg-bin`, ou a primeira candidata
/// cujo `postgres` é mesmo dessa versão.
fn find_bin(version: u32, given: Option<&PathBuf>) -> Result<PathBuf, String> {
    let dirs = match given {
        Some(dir) => vec![dir.clone()],
        None => candidate_dirs(version),
    };
    for dir in &dirs {
        if !exe(dir, "initdb").is_file() {
            continue;
        }
        match binary_major(dir) {
            Some(v) if v == version => return Ok(dir.clone()),
            Some(v) if given.is_some() => {
                return Err(format!("{} é do PostgreSQL {v}", dir.display()));
            }
            _ => {}
        }
    }
    Err(match given {
        Some(dir) => format!("sem initdb em {}", dir.display()),
        None => "binários não encontrados (use --pg-bin)".to_string(),
    })
}

/// Resultado de uma versão.
enum Outcome {
    Missing(String),
    Failed {
        summary: String,
        failures: Vec<String>,
    },
    Passed(String),
}

pub fn run(args: MatrixArgs, root: &Path) -> Result<bool, String> {
    if args.run_as.is_none() && is_root() {
        return Err(
            "o PostgreSQL não roda como root; use --run-as <usuário> (por exemplo, postgres)"
                .into(),
        );
    }
    let logs = root.join("target").join("pgmatrix");
    std::fs::create_dir_all(&logs).map_err(|e| format!("{}: {e}", logs.display()))?;

    // Compila uma vez antes de subir qualquer servidor.
    println!("Compilando os testes...");
    let status = cargo_test(root, &args.test_args)
        .arg("--no-run")
        .status()
        .map_err(|e| format!("cargo test: {e}"))?;
    if !status.success() {
        return Err("os testes não compilam".into());
    }

    let mut results = Vec::new();
    for &version in &args.versions {
        println!("\n== PostgreSQL {version} ==");
        let outcome = match find_bin(version, args.bins.get(&version)) {
            Err(why) => {
                println!("não testada: {why}");
                Outcome::Missing(why)
            }
            Ok(bin) => run_version(version, &bin, &args, root, &logs)?,
        };
        results.push((version, outcome));
    }

    println!("\n| Versão | Resultado |\n|---|---|");
    let mut ok = true;
    for (version, outcome) in &results {
        let text = match outcome {
            Outcome::Passed(summary) => format!("ok ({summary})"),
            Outcome::Failed { summary, failures } => {
                ok = false;
                if failures.is_empty() {
                    format!("FALHOU ({summary})")
                } else {
                    format!("FALHOU ({summary}): {}", failures.join(", "))
                }
            }
            Outcome::Missing(why) => {
                ok &= args.allow_missing;
                format!("não testada: {why}")
            }
        };
        println!("| {version} | {text} |");
    }
    Ok(ok)
}

fn cargo_test(root: &Path, extra: &[String]) -> Command {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(root).args(["test", "--no-fail-fast"]);
    if !extra
        .iter()
        .any(|a| a == "-p" || a.starts_with("--package"))
    {
        cmd.arg("--workspace");
    }
    // Argumentos depois de um `--` interno vão para o executável de teste.
    cmd.args(extra);
    cmd
}

fn run_version(
    version: u32,
    bin: &Path,
    args: &MatrixArgs,
    root: &Path,
    logs: &Path,
) -> Result<Outcome, String> {
    println!("binários: {}", bin.display());
    let port = 54300 + version;
    let server = match Server::start(version, bin, port, args.run_as.as_deref(), args.keep) {
        Ok(s) => s,
        Err(why) => {
            println!("o servidor não subiu: {why}");
            return Ok(Outcome::Failed {
                summary: format!("o servidor não subiu: {why}"),
                failures: Vec::new(),
            });
        }
    };

    let log_path = logs.join(format!("{version}.log"));
    let mut log = File::create(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
    let started = Instant::now();
    let mut child = cargo_test(root, &args.test_args)
        .env("PGSTUDIO_TEST_HOST", "127.0.0.1")
        .env("PGSTUDIO_TEST_PORT", port.to_string())
        .env("PGSTUDIO_TEST_USER", "postgres")
        .env("PGSTUDIO_TEST_PASSWORD", PASSWORD)
        .env("PGSTUDIO_PG_BIN", bin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cargo test: {e}"))?;

    // stderr num fio à parte, para nenhum dos dois canos encher e travar.
    let stderr = child.stderr.take().expect("stderr capturado");
    let err_thread = std::thread::spawn(move || {
        BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<_>>()
    });
    let mut tally = Tally::default();
    for line in BufReader::new(child.stdout.take().expect("stdout capturado"))
        .lines()
        .map_while(Result::ok)
    {
        let _ = writeln!(log, "{line}");
        tally.read(&line);
    }
    let status = child.wait().map_err(|e| format!("cargo test: {e}"))?;
    for line in err_thread.join().unwrap_or_default() {
        let _ = writeln!(log, "{line}");
    }
    drop(server);

    let summary = format!(
        "{} passaram, {} falharam, {} ignorados, {:.0} s",
        tally.passed,
        tally.failed,
        tally.ignored,
        started.elapsed().as_secs_f64()
    );
    println!("{summary}; log em {}", log_path.display());
    Ok(if status.success() {
        Outcome::Passed(summary)
    } else {
        Outcome::Failed {
            summary,
            failures: tally.failures,
        }
    })
}

/// Contagem das linhas "test result:" e nomes das falhas.
#[derive(Debug, Default, PartialEq)]
struct Tally {
    passed: u64,
    failed: u64,
    ignored: u64,
    failures: Vec<String>,
}

impl Tally {
    fn read(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix("test result: ") {
            for part in rest.split(';') {
                let mut words = part.split_whitespace();
                let (Some(first), Some(second)) = (words.next(), words.next()) else {
                    continue;
                };
                // "ok. 12 passed" começa com o veredito.
                let (n, what) = if first.ends_with('.') {
                    (second, words.next().unwrap_or(""))
                } else {
                    (first, second)
                };
                let n: u64 = n.parse().unwrap_or(0);
                match what {
                    "passed" => self.passed += n,
                    "failed" => self.failed += n,
                    "ignored" => self.ignored += n,
                    _ => {}
                }
            }
        } else if let Some(name) = line
            .strip_prefix("test ")
            .and_then(|l| l.strip_suffix(" ... FAILED"))
        {
            self.failures.push(name.to_string());
        }
    }
}

/// Um cluster descartável no ar; parado e apagado no `drop`.
struct Server {
    bin: PathBuf,
    dir: PathBuf,
    run_as: Option<String>,
    keep: bool,
}

impl Server {
    fn start(
        version: u32,
        bin: &Path,
        port: u32,
        run_as: Option<&str>,
        keep: bool,
    ) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("pgstudio-pgmatrix-{version}"));
        if dir.exists() {
            // Sobra de uma execução interrompida: para o servidor, se ainda
            // estiver no ar, antes de apagar.
            let _ = pg_command(bin, "pg_ctl", run_as)
                .arg("-D")
                .arg(dir.join("data"))
                .args(["-m", "immediate", "stop"])
                .output();
            std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let server = Server {
            bin: bin.to_path_buf(),
            dir: dir.clone(),
            run_as: run_as.map(str::to_string),
            keep,
        };
        let pwfile = dir.join("pwfile");
        std::fs::write(&pwfile, PASSWORD).map_err(|e| format!("{}: {e}", pwfile.display()))?;
        let tls = supports_tls(bin) && make_certificate(&dir);
        if let Some(user) = run_as {
            chown(&dir, user)?;
        }

        let data = dir.join("data");
        let out = pg_command(bin, "initdb", run_as)
            .arg("-D")
            .arg(&data)
            .args(["-U", "postgres", "-A", "scram-sha-256", "-E", "UTF8"])
            .args(["--locale=C", "--pwfile"])
            .arg(&pwfile)
            .output()
            .map_err(|e| format!("initdb: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "initdb: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }

        let mut options = format!("-p {port} -c listen_addresses=127.0.0.1 -c fsync=off");
        if !cfg!(windows) {
            options.push_str(&format!(" -k {}", dir.display()));
        }
        if tls {
            // Como no CI: TLS ligado, para os testes de Encryption cobrirem
            // Require e Verify Full.
            options.push_str(&format!(
                " -c ssl=on -c ssl_cert_file={0}/server.crt -c ssl_key_file={0}/server.key",
                dir.display()
            ));
        }
        let started = Instant::now();
        let out = pg_command(bin, "pg_ctl", run_as)
            .arg("-D")
            .arg(&data)
            .arg("-l")
            .arg(dir.join("server.log"))
            .args(["-o", &options, "-w", "-t", "60", "start"])
            .output()
            .map_err(|e| format!("pg_ctl: {e}"))?;
        if !out.status.success() {
            let log = std::fs::read_to_string(dir.join("server.log")).unwrap_or_default();
            return Err(format!(
                "pg_ctl start: {}\n{}",
                String::from_utf8_lossy(&out.stderr).trim(),
                log.trim()
            ));
        }
        println!(
            "servidor no ar na porta {port}{} ({:.1} s)",
            if tls { ", com TLS" } else { ", sem TLS" },
            started.elapsed().as_secs_f64()
        );
        Ok(server)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = pg_command(&self.bin, "pg_ctl", self.run_as.as_deref())
            .arg("-D")
            .arg(self.dir.join("data"))
            .args(["-m", "fast", "-w", "stop"])
            .stdout(Stdio::null())
            .status();
        if self.keep {
            println!("cluster mantido em {}", self.dir.display());
        } else {
            // O pg_ctl -w já esperou o servidor sair; uma folga para o
            // Windows soltar os arquivos.
            std::thread::sleep(Duration::from_millis(200));
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// `bin/<name>`, ou `runuser -u <user> -- bin/<name>` com `--run-as`.
fn pg_command(bin: &Path, name: &str, run_as: Option<&str>) -> Command {
    match run_as {
        Some(user) => {
            let mut cmd = Command::new("runuser");
            cmd.args(["-u", user, "--"]).arg(exe(bin, name));
            cmd
        }
        None => Command::new(exe(bin, name)),
    }
}

/// Se o servidor foi compilado com OpenSSL (`pg_config --configure`).
fn supports_tls(bin: &Path) -> bool {
    Command::new(exe(bin, "pg_config"))
        .arg("--configure")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("openssl"))
        .unwrap_or(false)
}

/// Certificado autoassinado em `dir/server.crt` e `dir/server.key`; falso se
/// não houver `openssl`.
fn make_certificate(dir: &Path) -> bool {
    let ok = Command::new("openssl")
        .args(["req", "-new", "-x509", "-days", "2", "-nodes"])
        .args(["-subj", "/CN=localhost", "-out"])
        .arg(dir.join("server.crt"))
        .arg("-keyout")
        .arg(dir.join("server.key"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    #[cfg(unix)]
    if ok {
        use std::os::unix::fs::PermissionsExt;
        let key = dir.join("server.key");
        let _ = std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600));
    }
    ok
}

fn chown(dir: &Path, user: &str) -> Result<(), String> {
    let status = Command::new("chown")
        .args(["-R", user])
        .arg(dir)
        .status()
        .map_err(|e| format!("chown: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("chown {user} {} falhou", dir.display()))
    }
}

fn is_root() -> bool {
    if cfg!(windows) {
        return false;
    }
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_cover_every_supported_version() {
        let a = parse(&[]).unwrap();
        assert_eq!(a.versions, vec![13, 14, 15, 16, 17]);
        assert!(!a.allow_missing && !a.keep && a.run_as.is_none());
    }

    #[test]
    fn parses_options_and_cargo_args() {
        let a = parse(&args(&[
            "--versions",
            "13, 17",
            "--pg-bin",
            "13=/opt/pg13/bin",
            "--run-as",
            "postgres",
            "--allow-missing",
            "--",
            "-p",
            "pgstudio-pg-metadata",
        ]))
        .unwrap();
        assert_eq!(a.versions, vec![13, 17]);
        assert_eq!(a.bins[&13], PathBuf::from("/opt/pg13/bin"));
        assert_eq!(a.run_as.as_deref(), Some("postgres"));
        assert!(a.allow_missing);
        assert_eq!(a.test_args, args(&["-p", "pgstudio-pg-metadata"]));
    }

    #[test]
    fn rejects_versions_outside_the_matrix() {
        assert!(parse(&args(&["--versions", "12"])).is_err());
        assert!(parse(&args(&["--pg-bin", "18=/x"])).is_err());
        assert!(parse(&args(&["--pg-bin", "/x"])).is_err());
    }

    #[test]
    fn reads_the_major_version() {
        assert_eq!(parse_major("postgres (PostgreSQL) 13.7\n"), Some(13));
        assert_eq!(parse_major("postgres (PostgreSQL) 17.10"), Some(17));
        assert_eq!(parse_major("postgres (PostgreSQL) 18beta1"), Some(18));
        assert_eq!(parse_major(""), None);
    }

    #[test]
    fn tallies_test_results_and_failures() {
        let mut t = Tally::default();
        for line in [
            "test catalog::tables ... ok",
            "test catalog::roles ... FAILED",
            "test result: FAILED. 10 passed; 1 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.2s",
            "test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s",
        ] {
            t.read(line);
        }
        assert_eq!((t.passed, t.failed, t.ignored), (15, 1, 2));
        assert_eq!(t.failures, vec!["catalog::roles".to_string()]);
    }
}
