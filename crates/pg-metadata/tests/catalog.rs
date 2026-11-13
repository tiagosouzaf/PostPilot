//! O Object Explorer contra um PostgreSQL de verdade.
//!
//! Usa as mesmas variáveis dos testes de `pg-client`
//! (`PGSTUDIO_TEST_HOST`, `PGSTUDIO_TEST_PORT`, `PGSTUDIO_TEST_USER`,
//! `PGSTUDIO_TEST_PASSWORD`); sem `PGSTUDIO_TEST_HOST` os testes só avisam.

use std::sync::Arc;

use pgstudio_core::connection::{ConnectionSettings, Connector, SslMode};
use pgstudio_core::metadata::{MetadataProvider, NodeId, NodeKind, ObjectNode};
use pgstudio_pg_client::{DbRuntime, Session};
use pgstudio_pg_metadata::PgConnector;

fn server() -> Option<(ConnectionSettings, String)> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    Some((
        ConnectionSettings {
            host,
            port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
            database: "postgres".into(),
            user: var("PGSTUDIO_TEST_USER", "postgres"),
            ssl: SslMode::Disable,
        },
        var("PGSTUDIO_TEST_PASSWORD", ""),
    ))
}

/// Banco de teste com um pouco de tudo, criado e apagado pelo teste.
struct TestDb {
    rt: DbRuntime,
    admin: Session,
    name: String,
}

const SCHEMA: &str = r#"
CREATE SCHEMA "sales/2024";
CREATE TABLE "sales/2024".customer (
    id integer PRIMARY KEY,
    email text NOT NULL UNIQUE,
    age integer CHECK (age >= 0)
);
CREATE TABLE "sales/2024".orders (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    customer_id integer REFERENCES "sales/2024".customer (id),
    total numeric(10, 2)
);
CREATE INDEX orders_customer_idx ON "sales/2024".orders (customer_id);
ALTER TABLE "sales/2024".orders DROP COLUMN total;
ALTER TABLE "sales/2024".orders ADD COLUMN amount numeric(10, 2);
CREATE VIEW "sales/2024".big_orders AS SELECT id, amount FROM "sales/2024".orders WHERE amount > 100;
CREATE MATERIALIZED VIEW "sales/2024".order_count AS SELECT count(*) AS n FROM "sales/2024".orders;
CREATE SEQUENCE "sales/2024".invoice_no;
CREATE FUNCTION "sales/2024".add(a integer, b integer) RETURNS integer LANGUAGE sql AS 'SELECT a + b';
CREATE PROCEDURE "sales/2024".noop() LANGUAGE sql AS 'SELECT 1';
CREATE TABLE "sales/2024".events (at date NOT NULL) PARTITION BY RANGE (at);
CREATE TABLE "sales/2024".events_2024 PARTITION OF "sales/2024".events
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
"#;

impl TestDb {
    fn create(settings: &ConnectionSettings, password: &str, suffix: &str) -> TestDb {
        let rt = DbRuntime::new().unwrap();
        let admin = rt.block_on(Session::connect(settings, password)).unwrap();
        let name = format!("pgstudio_meta_{}_{suffix}", std::process::id());
        rt.block_on(admin.simple_query(&format!("DROP DATABASE IF EXISTS {name}")))
            .unwrap();
        rt.block_on(admin.simple_query(&format!("CREATE DATABASE {name}")))
            .unwrap();
        let db = ConnectionSettings {
            database: name.clone(),
            ..settings.clone()
        };
        let s = rt.block_on(Session::connect(&db, password)).unwrap();
        rt.block_on(s.simple_query(SCHEMA)).unwrap();
        TestDb { rt, admin, name }
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let _ = self.rt.block_on(self.admin.simple_query(&format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        )));
    }
}

fn child(meta: &dyn MetadataProvider, parent: &ObjectNode, name: &str) -> ObjectNode {
    let children = meta.children(&parent.id).unwrap();
    children
        .iter()
        .find(|n| n.name == name)
        .cloned()
        .unwrap_or_else(|| {
            let names: Vec<_> = children.iter().map(|n| &n.name).collect();
            panic!("{name:?} not under {:?}: {names:?}", parent.name)
        })
}

fn names(meta: &dyn MetadataProvider, parent: &ObjectNode) -> Vec<String> {
    meta.children(&parent.id)
        .unwrap()
        .into_iter()
        .map(|n| n.name)
        .collect()
}

#[test]
fn walks_the_catalog_like_ssms() {
    let Some((settings, password)) = server() else {
        return;
    };
    let db = TestDb::create(&settings, &password, "walk");
    let connected = PgConnector::new(DbRuntime::new().unwrap())
        .connect(&settings, &password)
        .unwrap();
    let meta: Arc<dyn MetadataProvider> = connected.metadata.clone();
    let meta = meta.as_ref();

    let server = meta.roots().unwrap().remove(0);
    assert_eq!(server.kind, NodeKind::Server);
    assert_eq!(names(meta, &server), ["Databases", "Security"]);

    let databases = child(meta, &server, "Databases");
    let database = child(meta, &databases, &db.name);
    let schemas = child(meta, &database, "Schemas");
    let schema_names = names(meta, &schemas);
    assert!(schema_names.contains(&"public".to_string()));
    assert!(
        !schema_names.iter().any(|s| s.starts_with("pg_")),
        "{schema_names:?}"
    );
    assert!(!schema_names.contains(&"information_schema".to_string()));

    let schema = child(meta, &schemas, "sales/2024");
    assert_eq!(
        names(meta, &schema),
        ["Tables", "Views", "Functions", "Sequences"]
    );

    let tables = child(meta, &schema, "Tables");
    assert_eq!(
        names(meta, &tables),
        ["customer", "events (partitioned)", "orders"]
    );
    let orders = child(meta, &tables, "orders");
    assert_eq!(names(meta, &orders), ["Columns", "Constraints", "Indexes"]);
    let columns = child(meta, &orders, "Columns");
    assert_eq!(
        names(meta, &columns),
        [
            "id (PK, bigint, not null)",
            "customer_id (FK, integer, null)",
            "amount (numeric(10,2), null)"
        ]
    );
    let customer = child(meta, &tables, "customer");
    let constraints = names(meta, &child(meta, &customer, "Constraints"));
    assert!(
        constraints.contains(&"customer_pkey (primary key)".to_string()),
        "{constraints:?}"
    );
    assert!(constraints.contains(&"customer_email_key (unique)".to_string()));
    assert!(constraints.contains(&"customer_age_check (check)".to_string()));
    let indexes = names(meta, &child(meta, &orders, "Indexes"));
    assert_eq!(
        indexes,
        ["orders_customer_idx", "orders_pkey (primary key)"]
    );

    let views = child(meta, &schema, "Views");
    assert_eq!(
        names(meta, &views),
        ["big_orders", "order_count (materialized)"]
    );
    let view = child(meta, &views, "big_orders");
    let view_columns = child(meta, &view, "Columns");
    assert_eq!(
        names(meta, &view_columns),
        ["id (bigint, null)", "amount (numeric(10,2), null)"]
    );

    let functions = child(meta, &schema, "Functions");
    assert_eq!(
        names(meta, &functions),
        ["add(a integer, b integer)", "noop() (procedure)"]
    );
    let sequences = names(meta, &child(meta, &schema, "Sequences"));
    assert!(
        sequences.contains(&"invoice_no".to_string()),
        "{sequences:?}"
    );
    // A sequência da coluna identity também aparece, como no psql.
    assert!(
        sequences.contains(&"orders_id_seq".to_string()),
        "{sequences:?}"
    );

    let security = child(meta, &server, "Security");
    let roles = child(meta, &security, "Roles");
    let role_names = names(meta, &roles);
    assert!(role_names.contains(&settings.user), "{role_names:?}");
    assert!(
        !role_names.iter().any(|r| r.starts_with("pg_")),
        "{role_names:?}"
    );

    // Folhas e ids desconhecidos não têm filhos.
    assert!(meta.children(&columns.id).unwrap().len() == 3);
    let leaf = meta.children(&columns.id).unwrap().remove(0);
    assert!(!leaf.has_children);
    assert!(meta.children(&leaf.id).unwrap().is_empty());
    assert!(meta.children(&NodeId("nope".into())).unwrap().is_empty());
}

#[test]
fn refresh_sees_new_objects_and_missing_databases_fail_cleanly() {
    let Some((settings, password)) = server() else {
        return;
    };
    let db = TestDb::create(&settings, &password, "refresh");
    let connected = PgConnector::new(DbRuntime::new().unwrap())
        .connect(&settings, &password)
        .unwrap();
    let meta = connected.metadata.as_ref();
    let server = meta.roots().unwrap().remove(0);
    let databases = child(meta, &server, "Databases");
    let database = child(meta, &databases, &db.name);
    let schemas = child(meta, &database, "Schemas");
    let public = child(meta, &schemas, "public");
    let tables = child(meta, &public, "Tables");
    assert!(names(meta, &tables).is_empty());

    let target = ConnectionSettings {
        database: db.name.clone(),
        ..settings.clone()
    };
    let s = db
        .rt
        .block_on(Session::connect(&target, &password))
        .unwrap();
    db.rt
        .block_on(s.simple_query("CREATE TABLE public.added (x int)"))
        .unwrap();
    assert_eq!(names(meta, &tables), ["added"]);

    let gone = NodeId("db/pgstudio_no_such_db/Schemas".into());
    let err = meta.children(&gone).unwrap_err();
    assert!(err.0.contains("pgstudio_no_such_db"), "{err}");
}
