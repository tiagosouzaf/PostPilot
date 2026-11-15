//! Alt+F1 contra um SQL Server de verdade (com `MSSQL_TEST_HOST`; sem ele,
//! o teste só avisa; veja `docs/sqlserver.md`): os resultados de uma
//! tabela, de uma procedure e o erro de um nome que não existe.

use std::sync::mpsc;
use std::time::Duration;

use pgstudio_core::connection::{ConnectionSettings, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::object_info::info_tsql;

fn service() -> Option<pgstudio_mssql::MssqlQueryService> {
    let Ok(host) = std::env::var("MSSQL_TEST_HOST") else {
        eprintln!("MSSQL_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let mut settings = ConnectionSettings::for_engine(Engine::SqlServer);
    settings.host = host;
    settings.port = var("MSSQL_TEST_PORT", "1433").parse().unwrap();
    settings.user = var("MSSQL_TEST_USER", "sa");
    settings.ssl = SslMode::Require;
    Some(pgstudio_mssql::MssqlQueryService::new(
        settings,
        var("MSSQL_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ))
}

type Grid = (Vec<String>, Vec<Vec<String>>);

/// Os resultados (colunas e linhas) e os erros de `sql`.
fn run(session: &dyn QuerySession, sql: &str) -> (Vec<Grid>, Vec<String>) {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let (mut grids, mut errors): (Vec<Grid>, Vec<String>) = Default::default();
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Columns(c) => grids.push((c, Vec::new())),
            QueryEvent::Row(r) => grids
                .last_mut()
                .unwrap()
                .1
                .push(r.into_iter().map(Option::unwrap_or_default).collect()),
            QueryEvent::Error(e) => errors.push(e.message),
            QueryEvent::Finished { .. } => return (grids, errors),
            _ => {}
        }
    }
}

#[test]
fn alt_f1_describes_tables_and_procedures() {
    let Some(service) = service() else { return };
    let s = service.open("tempdb");
    let id = std::process::id();
    let (pai, filho, proc) = (
        format!("pp_info_pai_{id}"),
        format!("pp_info_filho_{id}"),
        format!("pp_info_proc_{id}"),
    );
    let (_, errors) = run(
        s.as_ref(),
        &format!(
            "CREATE TABLE dbo.[{pai}] (id int IDENTITY CONSTRAINT [pk_{pai}] PRIMARY KEY, \
               nome nvarchar(60) NOT NULL CONSTRAINT [uq_{pai}] UNIQUE, \
               valor decimal(10,2) NULL CONSTRAINT [df_{pai}] DEFAULT (0) \
                 CONSTRAINT [ck_{pai}] CHECK (valor >= 0), \
               criado datetime2(3) NULL);\n\
             CREATE INDEX [ix_{pai}] ON dbo.[{pai}] (criado, nome);\n\
             CREATE TABLE dbo.[{filho}] (id int PRIMARY KEY, \
               pai_id int CONSTRAINT [fk_{filho}] REFERENCES dbo.[{pai}] (id));\n\
             GO\n\
             CREATE PROCEDURE dbo.[{proc}] @nome nvarchar(60), @total int OUTPUT AS SET @total = 1"
        ),
    );
    assert_eq!(errors, Vec::<String>::new());

    let (grids, errors) = run(s.as_ref(), &info_tsql(&format!("[dbo].[{pai}]")));
    assert_eq!(errors, Vec::<String>::new());
    let heads: Vec<&str> = grids.iter().map(|g| g.0[0].as_str()).collect();
    assert_eq!(
        heads,
        [
            "Name",
            "Column_name",
            "Index_name",
            "Constraint_name",
            "Table is referenced by foreign key"
        ]
    );
    assert_eq!(
        grids[0].1[0][..4],
        [pai.as_str(), "dbo", "dbo", "user table"]
    );
    let columns: Vec<String> = grids[1].1.iter().map(|r| r[..5].join("|")).collect();
    assert_eq!(
        columns,
        [
            "id|int|no|yes|no",
            "nome|nvarchar(60)|no|no|no",
            "valor|decimal(10,2)|yes|no|no",
            "criado|datetime2(3)|yes|no|no",
        ]
    );
    assert_eq!(grids[1].1[2][5], "((0))");
    let indexes: Vec<String> = grids[2].1.iter().map(|r| r.join("|")).collect();
    assert_eq!(
        indexes,
        [
            format!("ix_{pai}|nonclustered|[criado], [nome]"),
            format!("pk_{pai}|clustered, unique, primary key|[id]"),
            format!("uq_{pai}|nonclustered, unique|[nome]"),
        ]
    );
    let constraints: Vec<String> = grids[3].1.iter().map(|r| r.join("|")).collect();
    assert_eq!(
        constraints,
        [
            format!("ck_{pai}|CHECK|([valor]>=(0))"),
            format!("df_{pai}|DEFAULT|((0)) FOR [valor]"),
            format!("pk_{pai}|PRIMARY KEY|[id]"),
            format!("uq_{pai}|UNIQUE|[nome]"),
        ]
    );
    assert_eq!(
        grids[4].1,
        [[format!("[dbo].[{filho}]"), format!("fk_{filho}")]]
    );

    // A tabela filha mostra a FK com a definição.
    let (grids, _) = run(s.as_ref(), &info_tsql(&filho));
    let fk = grids
        .iter()
        .find(|g| g.0[0] == "Constraint_name")
        .and_then(|g| g.1.iter().find(|r| r[1] == "FOREIGN KEY"))
        .expect("fk");
    assert_eq!(fk[2], format!("([pai_id]) REFERENCES [dbo].[{pai}] ([id])"));

    // Uma procedure: o objeto e os parâmetros.
    let (grids, errors) = run(s.as_ref(), &info_tsql(&format!("dbo.{proc}")));
    assert_eq!(errors, Vec::<String>::new());
    assert_eq!(grids.len(), 2);
    assert_eq!(grids[0].1[0][3], "sql stored procedure");
    let params: Vec<String> = grids[1].1.iter().map(|r| r.join("|")).collect();
    assert_eq!(params, ["@nome|nvarchar(60)|no", "@total|int|yes"]);

    // Um nome que não existe: o erro do sp_help, sem resultados.
    let (grids, errors) = run(s.as_ref(), &info_tsql("dbo.pp_nao_existe"));
    assert!(grids.is_empty());
    assert_eq!(
        errors,
        [
            "The object 'dbo.pp_nao_existe' does not exist in database 'tempdb' or is invalid \
          for this operation."
        ]
    );

    run(
        s.as_ref(),
        &format!("DROP PROCEDURE dbo.[{proc}]; DROP TABLE dbo.[{filho}]; DROP TABLE dbo.[{pai}]"),
    );
}
