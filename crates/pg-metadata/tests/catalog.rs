//! O Object Explorer contra um PostgreSQL de verdade.
//!
//! Usa as mesmas variáveis dos testes de `pg-client`
//! (`PGSTUDIO_TEST_HOST`, `PGSTUDIO_TEST_PORT`, `PGSTUDIO_TEST_USER`,
//! `PGSTUDIO_TEST_PASSWORD`); sem `PGSTUDIO_TEST_HOST` os testes só avisam.

use std::sync::Arc;

use pgstudio_core::connection::{ConnectionSettings, Connector, SslMode};
use pgstudio_core::metadata::{
    MetadataProvider, NodeId, NodeKind, ObjectNode, Property, ScriptAction,
};
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

/// A versão principal do servidor ("13.7" dá 13, "17beta1" dá 17).
fn server_major(version: &str) -> u32 {
    let digits: String = version.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().unwrap()
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
CREATE TYPE "sales/2024".mood AS ENUM ('sad', 'ok', 'it''s great');
CREATE DOMAIN "sales/2024".posint AS integer DEFAULT 1 NOT NULL
    CONSTRAINT posint_check CHECK (VALUE > 0);
CREATE TYPE "sales/2024".pair AS (a integer, "B" text);
CREATE TYPE "sales/2024".floatrange AS RANGE (subtype = float8, subtype_diff = float8mi);
CREATE EXTENSION citext;
CREATE TRIGGER orders_touch BEFORE INSERT OR UPDATE ON "sales/2024".orders
    FOR EACH ROW EXECUTE FUNCTION pg_catalog.suppress_redundant_updates_trigger();
CREATE TRIGGER orders_audit AFTER DELETE ON "sales/2024".orders
    FOR EACH STATEMENT EXECUTE FUNCTION pg_catalog.suppress_redundant_updates_trigger();
ALTER TABLE "sales/2024".orders DISABLE TRIGGER orders_audit;
"#;

impl TestDb {
    fn create(settings: &ConnectionSettings, password: &str, suffix: &str) -> TestDb {
        Self::create_with(settings, password, suffix, SCHEMA)
    }

    fn create_with(
        settings: &ConnectionSettings,
        password: &str,
        suffix: &str,
        schema: &str,
    ) -> TestDb {
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
        if !schema.is_empty() {
            rt.block_on(s.simple_query(schema)).unwrap();
        }
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
    assert_eq!(
        names(meta, &database),
        ["Database Diagrams", "Schemas", "Extensions"]
    );
    let extensions = names(meta, &child(meta, &database, "Extensions"));
    assert!(
        extensions.iter().any(|e| e.starts_with("citext (")),
        "{extensions:?}"
    );
    assert!(
        extensions.iter().any(|e| e.starts_with("plpgsql (")),
        "{extensions:?}"
    );
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
        [
            "Tables",
            "Views",
            "Materialized Views",
            "Functions",
            "Procedures",
            "Sequences",
            "Types"
        ]
    );

    let tables = child(meta, &schema, "Tables");
    assert_eq!(
        names(meta, &tables),
        ["customer", "events (partitioned)", "orders"]
    );
    let orders = child(meta, &tables, "orders");
    assert_eq!(
        names(meta, &orders),
        ["Columns", "Constraints", "Indexes", "Triggers"]
    );
    // As triggers internas da FK ficam de fora.
    assert_eq!(
        names(meta, &child(meta, &orders, "Triggers")),
        ["orders_audit (disabled)", "orders_touch"]
    );
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
    assert_eq!(names(meta, &views), ["big_orders"]);
    let matviews = child(meta, &schema, "Materialized Views");
    assert_eq!(names(meta, &matviews), ["order_count"]);
    let order_count = child(meta, &matviews, "order_count");
    assert_eq!(order_count.kind, NodeKind::MaterializedView);
    assert_eq!(names(meta, &order_count), ["Columns", "Indexes"]);
    assert_eq!(
        names(meta, &child(meta, &order_count, "Columns")),
        ["n (bigint, null)"]
    );
    let view = child(meta, &views, "big_orders");
    let view_columns = child(meta, &view, "Columns");
    assert_eq!(
        names(meta, &view_columns),
        ["id (bigint, null)", "amount (numeric(10,2), null)"]
    );

    let functions = child(meta, &schema, "Functions");
    assert_eq!(names(meta, &functions), ["add(a integer, b integer)"]);
    let procedures = child(meta, &schema, "Procedures");
    let noop = child(meta, &procedures, "noop()");
    assert_eq!(noop.kind, NodeKind::Procedure);
    assert_eq!(names(meta, &procedures), ["noop()"]);
    assert_eq!(
        names(meta, &child(meta, &schema, "Types")),
        [
            "floatrange (range)",
            "mood (enum)",
            "pair (composite)",
            "posint (domain)"
        ]
    );
    // Os tipos do citext (de extensão) não aparecem.
    let public = child(meta, &schemas, "public");
    assert!(names(meta, &child(meta, &public, "Types")).is_empty());
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

#[test]
fn reads_the_whole_catalog_for_intellisense() {
    let Some((settings, password)) = server() else {
        return;
    };
    let db = TestDb::create(&settings, &password, "intellisense");
    let connected = PgConnector::new(DbRuntime::new().unwrap())
        .connect(&settings, &password)
        .unwrap();
    let cat = connected.metadata.catalog(&db.name).unwrap();
    assert_eq!(cat.database, db.name);
    assert!(cat.search_path.iter().any(|s| s == "pg_catalog"));
    assert!(cat.search_path.iter().any(|s| s == "public"));

    let sales = cat.schema("\"sales/2024\"").unwrap();
    let names: Vec<&str> = sales.relations.iter().map(|r| r.name.as_str()).collect();
    // Partições ficam de fora, como no Object Explorer.
    assert_eq!(
        names,
        ["big_orders", "customer", "events", "order_count", "orders"]
    );
    let orders = cat.relation(Some("\"sales/2024\""), "orders").unwrap();
    let cols: Vec<(&str, &str)> = orders
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.ty.as_str()))
        .collect();
    assert_eq!(
        cols,
        [
            ("id", "bigint"),
            ("customer_id", "integer"),
            ("amount", "numeric(10,2)")
        ]
    );
    assert_eq!(
        cat.relation(Some("\"sales/2024\""), "big_orders")
            .unwrap()
            .kind,
        pgstudio_core::catalog::RelationKind::View
    );
    // Os construtores do range são funções de verdade para o IntelliSense.
    // O multirange (e o construtor dele) só existe a partir do PostgreSQL 14.
    let mut expected = vec!["add", "floatmultirange", "floatrange", "noop"];
    if server_major(&connected.server_version) < 14 {
        expected.retain(|f| *f != "floatmultirange");
    }
    assert_eq!(sales.functions, expected);
    let builtins = cat.schema("pg_catalog").unwrap();
    assert!(builtins.functions.iter().any(|f| f == "now"));
    assert!(builtins.relations.is_empty());
}

/// Objetos a mais para o teste de script: serial, coluna gerada, default,
/// índice comum e tabela unlogged com nome reservado.
const MORE: &str = r#"
CREATE TABLE "sales/2024"."user" (
    id serial PRIMARY KEY,
    "Name" text NOT NULL DEFAULT 'anon',
    price numeric(10, 2) NOT NULL,
    taxed numeric GENERATED ALWAYS AS (price * 1.1) STORED,
    tags text[]
);
CREATE INDEX user_lower_name ON "sales/2024"."user" (lower("Name")) WHERE price > 0;
CREATE UNLOGGED TABLE "sales/2024".scratch (k int);
"#;

/// Script de cada objeto, na ordem em que precisam ser criados.
fn scripts(meta: &dyn MetadataProvider, db: &str, action: ScriptAction) -> Vec<(String, String)> {
    let server = meta.roots().unwrap().remove(0);
    let databases = child(meta, &server, "Databases");
    let database = child(meta, &databases, db);
    let schema = child(meta, &child(meta, &database, "Schemas"), "sales/2024");
    let mut nodes = vec![schema.clone()];
    let extensions = child(meta, &database, "Extensions");
    let citext = meta
        .children(&extensions.id)
        .unwrap()
        .into_iter()
        .find(|n| n.name.starts_with("citext ("))
        .expect("citext");
    nodes.push(citext);
    let types = child(meta, &schema, "Types");
    for t in [
        "floatrange (range)",
        "mood (enum)",
        "pair (composite)",
        "posint (domain)",
    ] {
        nodes.push(child(meta, &types, t));
    }
    let seqs = child(meta, &schema, "Sequences");
    nodes.push(child(meta, &seqs, "invoice_no"));
    let tables = child(meta, &schema, "Tables");
    for t in [
        "customer",
        "events (partitioned)",
        "orders",
        "scratch",
        "user",
    ] {
        nodes.push(child(meta, &tables, t));
    }
    let functions = child(meta, &schema, "Functions");
    nodes.push(child(meta, &functions, "add(a integer, b integer)"));
    let procedures = child(meta, &schema, "Procedures");
    nodes.push(child(meta, &procedures, "noop()"));
    let triggers = child(meta, &child(meta, &tables, "orders"), "Triggers");
    for t in ["orders_audit (disabled)", "orders_touch"] {
        nodes.push(child(meta, &triggers, t));
    }
    let views = child(meta, &schema, "Views");
    nodes.push(child(meta, &views, "big_orders"));
    let matviews = child(meta, &schema, "Materialized Views");
    nodes.push(child(meta, &matviews, "order_count"));
    let mut out: Vec<(String, String)> = nodes
        .iter()
        .map(|n| (n.name.clone(), meta.script(&n.id, action).unwrap()))
        .collect();
    if action == ScriptAction::Drop {
        out.reverse();
    }
    out
}

#[test]
fn create_scripts_rebuild_the_objects_in_a_clean_database() {
    let Some((settings, password)) = server() else {
        return;
    };
    let src = TestDb::create_with(
        &settings,
        &password,
        "script_src",
        &format!("{SCHEMA}{MORE}"),
    );
    let dst = TestDb::create_with(&settings, &password, "script_dst", "");
    let connected = PgConnector::new(DbRuntime::new().unwrap())
        .connect(&settings, &password)
        .unwrap();
    let meta = connected.metadata.as_ref();

    let created = scripts(meta, &src.name, ScriptAction::Create);
    let user = &created.iter().find(|(n, _)| n == "user").unwrap().1;
    assert!(
        user.starts_with("CREATE TABLE \"sales/2024\".\"user\" (\n    id serial,\n"),
        "{user}"
    );
    assert!(
        user.contains("\"Name\" text NOT NULL DEFAULT 'anon'::text"),
        "{user}"
    );
    assert!(
        user.contains("GENERATED ALWAYS AS ((price * 1.1)) STORED"),
        "{user}"
    );
    assert!(user.contains("CREATE INDEX user_lower_name ON"), "{user}");
    let orders = &created.iter().find(|(n, _)| n == "orders").unwrap().1;
    assert!(
        orders.contains("id bigint GENERATED ALWAYS AS IDENTITY"),
        "{orders}"
    );
    assert!(
        orders.contains("REFERENCES \"sales/2024\".customer(id)"),
        "{orders}"
    );
    let scratch = &created.iter().find(|(n, _)| n == "scratch").unwrap().1;
    assert!(scratch.starts_with("CREATE UNLOGGED TABLE"), "{scratch}");
    let script = |name: &str| &created.iter().find(|(n, _)| n == name).unwrap().1;
    assert_eq!(
        script("mood (enum)"),
        "CREATE TYPE \"sales/2024\".mood AS ENUM (\n    'sad',\n    'ok',\n    'it''s great'\n);\n"
    );
    assert_eq!(
        script("posint (domain)"),
        "CREATE DOMAIN \"sales/2024\".posint\n    AS integer\n    DEFAULT 1\n    NOT NULL\n    \
         CONSTRAINT posint_check CHECK (VALUE > 0);\n"
    );
    assert_eq!(
        script("pair (composite)"),
        "CREATE TYPE \"sales/2024\".pair AS (\n    a integer,\n    \"B\" text\n);\n"
    );
    assert_eq!(
        script("floatrange (range)"),
        "CREATE TYPE \"sales/2024\".floatrange AS RANGE (\n    SUBTYPE = double precision,\n    \
         SUBTYPE_DIFF = float8mi\n);\n"
    );
    let audit = script("orders_audit (disabled)");
    assert!(
        audit.starts_with("CREATE TRIGGER orders_audit AFTER DELETE ON \"sales/2024\".orders"),
        "{audit}"
    );
    assert!(
        audit.ends_with("DISABLE TRIGGER orders_audit;\n"),
        "{audit}"
    );
    let citext = created
        .iter()
        .find(|(n, _)| n.starts_with("citext ("))
        .unwrap();
    assert!(
        citext
            .1
            .starts_with("CREATE EXTENSION IF NOT EXISTS citext\n    WITH SCHEMA public\n"),
        "{}",
        citext.1
    );
    assert!(
        script("order_count").starts_with("CREATE MATERIALIZED VIEW"),
        "{}",
        script("order_count")
    );

    // Rodar tudo num banco vazio e gerar de novo dá o mesmo texto.
    let target = ConnectionSettings {
        database: dst.name.clone(),
        ..settings.clone()
    };
    let rt = DbRuntime::new().unwrap();
    let s = rt.block_on(Session::connect(&target, &password)).unwrap();
    for (name, sql) in &created {
        if let Err(e) = rt.block_on(s.simple_query(sql)) {
            panic!("{name}: {e:?}\n{sql}");
        }
    }
    assert_eq!(scripts(meta, &dst.name, ScriptAction::Create), created);

    // E os DROP, na ordem inversa, apagam tudo.
    for (name, sql) in scripts(meta, &dst.name, ScriptAction::Drop) {
        if let Err(e) = rt.block_on(s.simple_query(&sql)) {
            panic!("{name}: {e:?}\n{sql}");
        }
    }
    let schemas = names(
        meta,
        &child(
            meta,
            &child(
                meta,
                &child(meta, &meta.roots().unwrap()[0], "Databases"),
                &dst.name,
            ),
            "Schemas",
        ),
    );
    assert!(!schemas.contains(&"sales/2024".to_string()), "{schemas:?}");
}

fn property<'a>(props: &'a [Property], name: &str) -> &'a str {
    props
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.value.as_str())
        .unwrap_or_else(|| panic!("no {name:?} in {props:?}"))
}

#[test]
fn properties_describe_each_kind_of_object() {
    let Some((settings, password)) = server() else {
        return;
    };
    let db = TestDb::create(&settings, &password, "props");
    let connected = PgConnector::new(DbRuntime::new().unwrap())
        .connect(&settings, &password)
        .unwrap();
    let meta = connected.metadata.as_ref();
    let server = meta.roots().unwrap().remove(0);
    let database = child(meta, &child(meta, &server, "Databases"), &db.name);
    let schema = child(meta, &child(meta, &database, "Schemas"), "sales/2024");
    let props = |n: &ObjectNode| meta.properties(&n.id).unwrap();

    let p = props(&database);
    assert_eq!(property(&p, "Owner"), settings.user);
    assert_eq!(property(&p, "Encoding"), "UTF8");

    let orders = child(meta, &child(meta, &schema, "Tables"), "orders");
    let p = props(&orders);
    assert_eq!(property(&p, "Schema"), "sales/2024");
    assert_eq!(property(&p, "Kind"), "table");
    assert!(!p.iter().any(|p| p.name == "Populated"), "{p:?}");

    let triggers = child(meta, &orders, "Triggers");
    let p = props(&child(meta, &triggers, "orders_touch"));
    assert_eq!(property(&p, "Table"), "orders");
    assert_eq!(property(&p, "Timing"), "BEFORE");
    assert_eq!(property(&p, "Events"), "INSERT OR UPDATE");
    assert_eq!(property(&p, "Level"), "row");
    assert_eq!(property(&p, "Enabled"), "yes");
    let p = props(&child(meta, &triggers, "orders_audit (disabled)"));
    assert_eq!(property(&p, "Events"), "DELETE");
    assert_eq!(property(&p, "Level"), "statement");
    assert_eq!(property(&p, "Enabled"), "no");

    let mv = child(
        meta,
        &child(meta, &schema, "Materialized Views"),
        "order_count",
    );
    let p = props(&mv);
    assert_eq!(property(&p, "Kind"), "materialized view");
    assert_eq!(property(&p, "Populated"), "yes");

    let noop = child(meta, &child(meta, &schema, "Procedures"), "noop()");
    let p = props(&noop);
    assert_eq!(property(&p, "Language"), "sql");
    assert!(!p.iter().any(|p| p.name == "Returns"), "{p:?}");
    let add = child(
        meta,
        &child(meta, &schema, "Functions"),
        "add(a integer, b integer)",
    );
    let p = props(&add);
    assert_eq!(property(&p, "Returns"), "integer");
    assert_eq!(property(&p, "Arguments"), "a integer, b integer");

    let types = child(meta, &schema, "Types");
    let p = props(&child(meta, &types, "mood (enum)"));
    assert_eq!(property(&p, "Kind"), "enum");
    assert_eq!(property(&p, "Values"), "sad, ok, it's great");
    let p = props(&child(meta, &types, "posint (domain)"));
    assert_eq!(property(&p, "Base type"), "integer");
    assert_eq!(property(&p, "Nullable"), "no");
    assert_eq!(property(&p, "Default"), "1");
    assert_eq!(property(&p, "Checks"), "CHECK (VALUE > 0)");
    let p = props(&child(meta, &types, "pair (composite)"));
    assert_eq!(property(&p, "Attributes"), "a integer, B text");
    let p = props(&child(meta, &types, "floatrange (range)"));
    assert_eq!(property(&p, "Base type"), "double precision");

    let extensions = child(meta, &database, "Extensions");
    let citext = meta
        .children(&extensions.id)
        .unwrap()
        .into_iter()
        .find(|n| n.name.starts_with("citext ("))
        .unwrap();
    assert_eq!(citext.kind, NodeKind::Extension);
    let p = props(&citext);
    assert_eq!(property(&p, "Schema"), "public");
    assert!(citext.name.contains(property(&p, "Version")), "{p:?}");

    let columns = child(meta, &orders, "Columns");
    let p = props(&child(meta, &columns, "id (PK, bigint, not null)"));
    assert_eq!(property(&p, "Identity"), "always");
    assert_eq!(property(&p, "Nullable"), "no");

    let roles = child(meta, &child(meta, &server, "Security"), "Roles");
    let me = child(meta, &roles, &settings.user);
    assert_eq!(property(&props(&me), "Login"), "yes");

    // Pastas e o servidor não têm propriedades além do nome.
    assert!(props(&types).is_empty());
    assert!(props(&server).is_empty());
}

#[test]
fn select_top_lists_the_columns() {
    let Some((settings, password)) = server() else {
        return;
    };
    let db = TestDb::create_with(
        &settings,
        &password,
        "select_top",
        &format!("{SCHEMA}{MORE}"),
    );
    let connected = PgConnector::new(DbRuntime::new().unwrap())
        .connect(&settings, &password)
        .unwrap();
    let meta = connected.metadata.as_ref();
    let server = meta.roots().unwrap().remove(0);
    let database = child(meta, &child(meta, &server, "Databases"), &db.name);
    let schema = child(meta, &child(meta, &database, "Schemas"), "sales/2024");
    let user = child(meta, &child(meta, &schema, "Tables"), "user");
    assert_eq!(
        meta.database_of(&user.id).as_deref(),
        Some(db.name.as_str())
    );
    assert_eq!(
        meta.script(&user.id, ScriptAction::SelectTop(1000))
            .unwrap(),
        "SELECT id,\n       \"Name\",\n       price,\n       taxed,\n       tags\n  FROM \"sales/2024\".\"user\"\n LIMIT 1000;\n"
    );
    let index = child(meta, &child(meta, &user, "Indexes"), "user_lower_name");
    assert!(meta.script(&index.id, ScriptAction::SelectTop(10)).is_err());
    assert_eq!(
        meta.script(&index.id, ScriptAction::Drop).unwrap(),
        "DROP INDEX \"sales/2024\".user_lower_name;\n"
    );
    let con = child(
        meta,
        &child(meta, &user, "Constraints"),
        "user_pkey (primary key)",
    );
    assert_eq!(
        meta.script(&con.id, ScriptAction::Create).unwrap(),
        "ALTER TABLE \"sales/2024\".\"user\"\n    ADD CONSTRAINT user_pkey PRIMARY KEY (id);\n"
    );
}
