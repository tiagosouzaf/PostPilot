//! O DDL do Table Designer num SQL Server de verdade (com
//! `MSSQL_TEST_HOST`; sem ele, os testes só avisam): as edições feitas no
//! modelo viram T-SQL, são aplicadas numa transação e o banco recarregado
//! fica igual ao modelo editado.

use std::sync::Arc;

use pgstudio_core::connection::{ConnectionSettings, Connector, Engine, SslMode};
use pgstudio_core::metadata::MetadataProvider;
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_core::schema::diff::{data_loss_warnings, diff, unsupported};
use pgstudio_core::schema::edit::Document;
use pgstudio_core::schema::{Action, Check, Column, ForeignKey, Identity, Index, Model, Table};
use pgstudio_mssql::MssqlConnector;
use pgstudio_pg_client::DbRuntime;

struct TestDb {
    name: String,
    explorer: Arc<dyn MetadataProvider>,
    admin: Arc<dyn QuerySession>,
}

/// Roda `sql` até o fim; devolve a mensagem do erro, se houve.
fn execute(session: &Arc<dyn QuerySession>, sql: String) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql, Box::new(move |e| drop(tx.send(e))));
    let mut error = None;
    while let Ok(e) = rx.recv_timeout(std::time::Duration::from_secs(60)) {
        match e {
            QueryEvent::Error(e) => error = Some(e.message),
            QueryEvent::Finished { .. } => break,
            _ => {}
        }
    }
    error
}

const SCHEMA: &[&str] = &[
    "CREATE TABLE dbo.cliente (id int NOT NULL CONSTRAINT pk_cliente PRIMARY KEY, \
        nome nvarchar(60) NOT NULL CONSTRAINT uq_cliente_nome UNIQUE, \
        email varchar(80) NULL, nota varchar(10) NULL DEFAULT 'a');",
    "CREATE TABLE dbo.pedido (id int IDENTITY NOT NULL CONSTRAINT pk_pedido PRIMARY KEY, \
        cliente_id int NOT NULL CONSTRAINT fk_pedido_cliente REFERENCES dbo.cliente (id), \
        total decimal(10,2) NULL CONSTRAINT ck_total CHECK (total >= 0) DEFAULT 0, \
        dobro AS (total * 2));",
    "CREATE INDEX ix_pedido_total ON dbo.pedido (total DESC) WHERE total > 0;",
    "CREATE TABLE dbo.velha (a int NULL);",
    "INSERT INTO dbo.cliente (id, nome) VALUES (1, N'Ana'), (2, N'Bia');",
    "INSERT INTO dbo.pedido (cliente_id, total) VALUES (1, 50), (2, 80);",
];

impl TestDb {
    fn create(tag: &str) -> Option<TestDb> {
        let Ok(host) = std::env::var("MSSQL_TEST_HOST") else {
            eprintln!("MSSQL_TEST_HOST not set; skipping");
            return None;
        };
        let var =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
        let mut s = ConnectionSettings::for_engine(Engine::SqlServer);
        s.host = host;
        s.port = var("MSSQL_TEST_PORT", "1433").parse().unwrap();
        s.user = var("MSSQL_TEST_USER", "sa");
        s.ssl = SslMode::Require;
        let connected = MssqlConnector::new(DbRuntime::new().unwrap())
            .connect(&s, &var("MSSQL_TEST_PASSWORD", ""))
            .expect("connect");
        let name = format!("pp_designer_{tag}_{}", std::process::id());
        let explorer = connected.metadata;
        // CREATE DATABASE não roda em transação: vai pela sessão de consulta.
        let admin = connected.queries.open("master");
        if let Some(e) = execute(&admin, format!("CREATE DATABASE [{name}]")) {
            panic!("{e}");
        }
        let schema: Vec<String> = SCHEMA.iter().map(|s| (*s).to_string()).collect();
        explorer.apply_ddl(&name, &schema).expect("schema");
        Some(TestDb {
            name,
            explorer,
            admin,
        })
    }

    fn load(&self) -> Model {
        self.explorer.load_model(&self.name).unwrap()
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let db = &self.name;
        execute(
            &self.admin,
            format!(
                "ALTER DATABASE [{db}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{db}]"
            ),
        );
    }
}

/// O modelo sem os ids (os de objetos novos mudam ao recarregar) e com as
/// listas em ordem de nome.
fn shape(m: &Model) -> Vec<Table> {
    let mut tables: Vec<Table> = m
        .tables
        .iter()
        .map(|t| (**t).clone())
        .map(|mut t| {
            t.id = 0;
            for c in &mut t.columns {
                c.id = 0;
            }
            t.uniques.sort_by(|a, b| a.name.cmp(&b.name));
            t.checks.sort_by(|a, b| a.name.cmp(&b.name));
            t.foreign_keys.sort_by(|a, b| a.name.cmp(&b.name));
            t.indexes.sort_by(|a, b| a.name.cmp(&b.name));
            t
        })
        .collect();
    tables.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
    tables
}

fn id(m: &Model, name: &str) -> u64 {
    m.find("dbo", name).unwrap().id
}

fn col(m: &Model, table: &str, name: &str) -> u64 {
    m.find("dbo", table).unwrap().column(name).unwrap().id
}

#[test]
fn edits_applied_to_the_database_come_back_when_reloaded() {
    let Some(db) = TestDb::create("edit") else {
        return;
    };
    let saved = db.load();
    assert!(diff(&saved, &saved).is_empty());
    let mut d = Document::new(saved.clone());
    let cliente = id(&d.model, "cliente");
    let pedido = id(&d.model, "pedido");

    // Cliente: nome novo, coluna renomeada, chave que muda de tipo (a PK e
    // a FK de pedido saem e voltam), default novo e coluna com default
    // apagada.
    d.rename_table(cliente, "dbo", "customer").unwrap();
    d.rename_column(cliente, col(&d.model, "customer", "email"), "e-mail")
        .unwrap();
    d.update_column(cliente, col(&d.model, "customer", "id"), |c| {
        c.ty = "bigint".into();
    })
    .unwrap();
    d.update_column(cliente, col(&d.model, "customer", "nome"), |c| {
        c.ty = "nvarchar(200)".into();
        c.default = Some("N'sem nome'".into());
    })
    .unwrap();
    d.drop_column(cliente, col(&d.model, "customer", "nota"))
        .unwrap();
    // Pedido: a coluna da FK acompanha o tipo; `total` muda de nome com
    // check, índice filtrado, default e coluna calculada em cima dela.
    d.update_column(pedido, col(&d.model, "pedido", "cliente_id"), |c| {
        c.ty = "bigint".into();
    })
    .unwrap();
    d.rename_column(pedido, col(&d.model, "pedido", "total"), "valor")
        .unwrap();
    d.add_column(
        pedido,
        Column {
            name: "obs".into(),
            ty: "nvarchar(50)".into(),
            nullable: true,
            ..Default::default()
        },
    )
    .unwrap();
    d.add_index(
        pedido,
        Index {
            name: "ix_pedido_obs".into(),
            unique: false,
            method: "nonclustered".into(),
            columns: vec!["obs".into(), "valor DESC".into()],
            predicate: None,
        },
    )
    .unwrap();
    // Tabela nova com nome reservado, FK com ação e check.
    let item = d.add_table("dbo", "order").unwrap();
    d.add_column(
        item,
        Column {
            name: "pedido_id".into(),
            ty: "int".into(),
            nullable: false,
            ..Default::default()
        },
    )
    .unwrap();
    d.add_foreign_key(
        item,
        ForeignKey {
            columns: vec!["pedido_id".into()],
            ref_schema: "dbo".into(),
            ref_table: "pedido".into(),
            ref_columns: vec!["id".into()],
            on_delete: Action::Cascade,
            ..Default::default()
        },
    )
    .unwrap();
    d.add_check(
        item,
        Check {
            name: "ck_order_pedido".into(),
            expr: "[pedido_id]>(0)".into(),
        },
    )
    .unwrap();
    d.drop_table(id(&d.model, "velha")).unwrap();

    assert!(unsupported(&saved, &d.model).is_empty());
    // Só apagar avisa: os tipos só alargaram.
    assert_eq!(data_loss_warnings(&saved, &d.model).len(), 2);
    let ddl = diff(&saved, &d.model);
    db.explorer
        .apply_ddl(&db.name, &ddl)
        .unwrap_or_else(|e| panic!("{e}\n{}", ddl.join("\n")));
    let reloaded = db.load();
    assert_eq!(shape(&reloaded), shape(&d.model), "{}", ddl.join("\n"));
    let order = reloaded.find("dbo", "order").unwrap();
    assert_eq!(order.columns[0].identity, Some(Identity::Always));
    assert!(
        ddl.iter()
            .any(|s| s == "EXEC sp_rename N'[dbo].[cliente]', N'customer';"),
        "{}",
        ddl.join("\n")
    );
}

#[test]
fn a_failed_statement_rolls_everything_back() {
    let Some(db) = TestDb::create("rollback") else {
        return;
    };
    let before = db.load();
    let ddl = vec![
        "CREATE TABLE dbo.temp (a int);".to_string(),
        "ALTER TABLE dbo.nope ADD b int;".to_string(),
    ];
    let err = db.explorer.apply_ddl(&db.name, &ddl).unwrap_err();
    assert!(err.0.contains("statement 2 of 2"), "{err}");
    assert!(err.0.contains("dbo.nope"), "{err}");
    assert_eq!(db.load(), before);
    // A sessão continua usável, e fora da transação, depois do ROLLBACK.
    db.explorer
        .apply_ddl(&db.name, &["CREATE TABLE dbo.ok (a int);".to_string()])
        .unwrap();
    assert!(db.load().find("dbo", "ok").is_some());
    let types = db.explorer.type_names(&db.name).unwrap();
    assert!(types.iter().any(|t| t == "nvarchar(max)"), "{types:?}");
}
