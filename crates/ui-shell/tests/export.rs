//! Results to File contra um PostgreSQL de verdade (com
//! `PGSTUDIO_TEST_HOST`): 1 milhão de linhas vão para o arquivo sem ficar
//! na memória, e o CSV e o TSV voltam iguais com `COPY`. O `COPY FROM`
//! lê o arquivo do lado do servidor, então essa parte só roda com o
//! servidor na mesma máquina (`127.0.0.1` ou `localhost`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::execution::{Execution, Outcome};

fn service() -> Option<(pgstudio_pg_client::PgQueryService, bool)> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let local = host == "127.0.0.1" || host == "localhost";
    let settings = ConnectionSettings {
        engine: Default::default(),
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
    };
    Some((
        pgstudio_pg_client::PgQueryService::new(
            settings,
            var("PGSTUDIO_TEST_PASSWORD", ""),
            pgstudio_pg_client::DbRuntime::new().unwrap(),
        ),
        local,
    ))
}

fn value(session: &Arc<dyn QuerySession>, sql: &str) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut first = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(120)).unwrap() {
            QueryEvent::Columns(_) => first = None,
            QueryEvent::Row(r) if first.is_none() => first = r.into_iter().next().flatten(),
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return first,
            _ => {}
        }
    }
}

/// Valores difíceis: NULL, texto vazio, aspas, vírgula, tab, quebra de
/// linha e barra invertida.
const SOURCE: &str = "SELECT g AS id, \
       CASE WHEN g % 7 = 0 THEN NULL WHEN g % 11 = 0 THEN '' \
            ELSE 'linha ' || g || ', \"x\"' || E'\\t\\\\' || CASE WHEN g % 5 = 0 THEN E'\\n' ELSE '' END \
       END AS texto, \
       (g * 1.5)::numeric AS valor \
  FROM generate_series(1, 1000000) g";

/// Tira a primeira linha (o cabeçalho) do arquivo.
fn drop_first_line(path: &std::path::Path) {
    use std::io::{BufRead, BufReader, BufWriter, Write};
    let headerless = path.with_extension("body");
    let mut reader = BufReader::new(std::fs::File::open(path).unwrap());
    let mut header = Vec::new();
    reader.read_until(b'\n', &mut header).unwrap();
    let mut out = BufWriter::new(std::fs::File::create(&headerless).unwrap());
    std::io::copy(&mut reader, &mut out).unwrap();
    out.flush().unwrap();
    drop(out);
    std::fs::rename(&headerless, path).unwrap();
}

fn to_file(session: Arc<dyn QuerySession>, path: &std::path::Path) -> Execution {
    let ctx = egui::Context::default();
    let mut exec = Execution::start_to_file(session, SOURCE.into(), 0, path, &ctx).unwrap();
    let start = Instant::now();
    while !exec.poll() {
        assert!(start.elapsed() < Duration::from_secs(120), "never finished");
        std::thread::sleep(Duration::from_millis(10));
    }
    exec
}

#[test]
fn a_million_rows_go_to_the_file_and_come_back_with_copy() {
    let Some((service, local)) = service() else {
        return;
    };
    let admin = service.open("postgres");
    let pid = std::process::id();
    for ext in ["csv", "tsv"] {
        let path = std::env::temp_dir().join(format!("pgstudio-export-{pid}.{ext}"));
        let exec = to_file(service.open("postgres"), &path);
        assert_eq!(exec.outcome(), Outcome::Succeeded, "{:?}", exec.messages);
        assert!(exec.results.is_empty(), "rows must not be kept");
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 30_000_000, "{size}");
        if local {
            // O servidor (outro usuário) precisa ler o arquivo.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            let t = format!("export_back_{pid}_{ext}");
            // HEADER no formato texto só existe a partir do PostgreSQL 15;
            // antes disso, o cabeçalho sai do arquivo antes do COPY.
            let version: u32 = value(&admin, "SHOW server_version_num")
                .unwrap()
                .parse()
                .unwrap();
            let options = if ext == "csv" {
                "FORMAT csv, HEADER"
            } else if version >= 150_000 {
                "FORMAT text, HEADER"
            } else {
                drop_first_line(&path);
                "FORMAT text"
            };
            value(
                &admin,
                &format!(
                    "DROP TABLE IF EXISTS {t}; CREATE TABLE {t} (id int, texto text, valor numeric); \
                     COPY {t} FROM '{}' ({options})",
                    path.display()
                ),
            );
            let diff = value(
                &admin,
                &format!("SELECT count(*) FROM (({SOURCE}) EXCEPT ALL SELECT * FROM {t}) d"),
            );
            assert_eq!(diff, Some("0".into()), "{ext}");
            assert_eq!(
                value(&admin, &format!("SELECT count(*) FROM {t}")),
                Some("1000000".into())
            );
            value(&admin, &format!("DROP TABLE {t}"));
        }
        std::fs::remove_file(&path).unwrap();
    }
}
