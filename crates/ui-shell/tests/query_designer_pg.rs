//! Query Designer num PostgreSQL de verdade (com `PGSTUDIO_TEST_HOST`; sem
//! ele, avisa e passa), marco 6.1: cada SQL escrito à mão com LEFT, RIGHT
//! e FULL JOIN, DISTINCT, LIMIT, HAVING, OR, expressões e `count(*)` é lido
//! pelo designer, e o SQL que ele gera devolve as mesmas linhas.

use std::sync::Arc;
use std::time::Duration;

use pgstudio_core::connection::{ConnectionSettings, Connector, SslMode};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_ui_shell::query_designer::read::read;

fn connected() -> Option<pgstudio_core::connection::Connected> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let settings = ConnectionSettings {
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
    };
    let connector =
        pgstudio_pg_metadata::PgConnector::new(pgstudio_pg_client::DbRuntime::new().unwrap());
    Some(
        connector
            .connect(&settings, &var("PGSTUDIO_TEST_PASSWORD", ""))
            .unwrap(),
    )
}

/// As linhas do último resultado de `sql`.
fn rows(session: &Arc<dyn QuerySession>, sql: &str) -> Vec<Vec<Option<String>>> {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut out = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Columns(_) => out.clear(),
            QueryEvent::Row(r) => out.push(r),
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return out,
            _ => {}
        }
    }
}

const SCHEMA: &str = "
CREATE TABLE cliente (id int PRIMARY KEY, nome text, cidade text);
CREATE TABLE pedido (id int PRIMARY KEY, cliente_id int REFERENCES cliente, total numeric);
CREATE TABLE item (id int PRIMARY KEY, pedido_id int REFERENCES pedido, produto text,
    qtd int);
INSERT INTO cliente VALUES (1, 'Ana', 'Recife'), (2, 'Bia', 'Natal'), (3, 'Caio', NULL),
    (4, NULL, 'Recife'), (5, 'ana', 'Olinda');
INSERT INTO pedido VALUES (10, 1, 50), (11, 1, 150), (12, 2, 30), (13, NULL, 99),
    (14, 5, 200), (15, 5, 10);
INSERT INTO item VALUES (100, 10, 'caneta', 2), (101, 11, 'livro', 1), (102, 11, 'caneta', 5),
    (103, 14, 'mesa', 1), (104, NULL, NULL, NULL);
";

/// Consultas escritas à mão com o que o designer passou a desenhar no 6.1.
const QUERIES: &[&str] = &[
    "select c.nome, p.total from cliente c left join pedido p on p.cliente_id = c.id",
    "select c.nome, p.id from pedido p right outer join cliente c on c.id = p.cliente_id",
    "select c.id, p.id from cliente c full join pedido p on p.cliente_id = c.id",
    "select c.nome, p.id, i.produto from cliente c \
       left join pedido p on p.cliente_id = c.id \
       left join item i on i.pedido_id = p.id",
    "select c.nome, p.id from cliente c left join pedido p on p.cliente_id = c.id \
       where p.id = c.id or c.cidade = 'Recife'",
    "select distinct cidade from cliente",
    "select nome from cliente order by id limit 3",
    "select c.nome, count(*) as pedidos, sum(p.total) from cliente c \
       join pedido p on p.cliente_id = c.id where p.total > 0 \
       group by c.nome having count(*) > 1 and sum(p.total) >= 100 order by count(*) desc",
    "select count(*) from pedido",
    "select nome from cliente where nome = 'Ana' or nome = 'Bia' or id > 4",
    "select * from cliente where id > 1 and (nome like 'A%' or cidade is null)",
    "select lower(c.nome) as n, p.total * 2 from cliente c, pedido p \
       where p.cliente_id = c.id and lower(c.nome) like 'a%' order by lower(c.nome), p.total",
    "select cidade, sum(total) / count(*) as media, count(distinct p.id) from cliente \
       join pedido p on p.cliente_id = cliente.id group by cidade",
    "select lower(nome), count(id) from cliente group by lower(nome) having count(id) >= 1",
    "select cidade, count(*) from cliente where nome ~ '^[AB]' group by cidade \
       having count(*) = 1 or count(*) > 5",
];

#[test]
fn hand_written_queries_return_the_same_rows_after_the_designer() {
    let Some(connected) = connected() else {
        return;
    };
    let db = format!("pgstudio_qd_{}", std::process::id());
    let admin = connected.queries.open("postgres");
    rows(&admin, &format!("DROP DATABASE IF EXISTS {db}"));
    rows(&admin, &format!("CREATE DATABASE {db}"));
    let session = connected.queries.open(&db);
    rows(&session, SCHEMA);
    let model = connected.metadata.load_model(&db).unwrap();
    for sql in QUERIES {
        let query = read(sql, &model).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let designed = query.sql(&model);
        let back = read(&designed, &model).unwrap_or_else(|e| panic!("{designed}: {e}"));
        assert_eq!(back.sql(&model), designed, "round trip of {sql}");
        let mut expected = rows(&session, sql);
        let mut got = rows(&session, &designed);
        if !sql.contains("order by") {
            expected.sort();
            got.sort();
        }
        assert!(!expected.is_empty(), "{sql} returns nothing");
        assert_eq!(got, expected, "{sql}\n--- designed:\n{designed}");
    }
    drop(session);
    rows(&admin, &format!("DROP DATABASE {db} WITH (FORCE)"));
}
