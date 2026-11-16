//! O motor do Data Compare, sem interface: lê as tabelas e as chaves de um
//! banco, monta o `SELECT` ordenado pela chave, anda pelas duas tabelas ao
//! mesmo tempo (um merge pela chave, sem guardar as tabelas na memória) e
//! escreve o script que deixa o destino como a origem.
//!
//! # A ordem da chave
//!
//! O merge só funciona se os dois servidores devolverem as linhas numa
//! ordem que o app sabe repetir. A ordem de texto de um banco depende da
//! collation dele, então a chave é ordenada assim:
//!
//! - inteiros: pela coluna (usa o índice da chave primária) e comparados
//!   como números;
//! - o resto: pelo texto do valor numa collation binária (`"C"` no
//!   PostgreSQL, `Latin1_General_100_BIN2` no SQL Server), que o app
//!   compara byte a byte (ou unidade UTF-16 a unidade, no SQL Server).
//!
//! Por segurança o merge confere que cada lado chega em ordem estritamente
//! crescente; se não chegar, a tabela falha com um erro em vez de mostrar
//! diferenças que não existem.

use std::cmp::Ordering;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as Atomic};
use std::sync::mpsc;

use pgstudio_core::connection::Engine;
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_i18n::{tr, trf};
use serde::{Deserialize, Serialize};

/// Uma linha, como texto (`None` é NULL).
pub type Row = Vec<Option<String>>;

/// Linhas que esperam o merge entre o servidor e o app. Com a fila cheia
/// a leitura do servidor espera: a memória não cresce com a tabela.
const BUFFER: usize = 4096;

/// Diferenças de cada tabela que ficam na memória, para a grade. Todas
/// vão para o arquivo temporário de onde sai o script.
pub const SAMPLE: usize = 500;

/// Uma coluna de uma tabela, como o catálogo conta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// Nome do tipo base (`int4`, `nvarchar`...); vazio se não é um tipo
    /// base.
    pub type_name: String,
    /// Posição na chave primária, a partir de 1; 0 fora dela.
    pub key: u32,
    /// Coluna calculada pelo servidor (gerada, computada, `rowversion`):
    /// não se compara nem se grava.
    pub computed: bool,
    /// 0: não é identidade; 1: identidade; 2: `GENERATED ALWAYS` do
    /// PostgreSQL.
    pub identity: u8,
}

/// Uma tabela de um banco.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub schema: String,
    pub name: String,
    pub columns: Vec<Column>,
}

/// O SQL que lista as colunas de todas as tabelas do banco: schema,
/// tabela, coluna, tipo, posição na chave, calculada, identidade.
pub fn tables_sql(engine: Engine) -> &'static str {
    match engine {
        Engine::PostgreSql | Engine::Redis | Engine::MongoDb | Engine::Elasticsearch => {
            "SELECT n.nspname, c.relname, a.attname, \
                    CASE WHEN t.typtype = 'b' THEN t.typname ELSE '' END, \
                    COALESCE((SELECT p.ord FROM pg_catalog.pg_index i, \
                              unnest(i.indkey::int2[]) WITH ORDINALITY AS p(attnum, ord) \
                              WHERE i.indrelid = c.oid AND i.indisprimary \
                                AND p.attnum = a.attnum AND p.ord <= i.indnkeyatts), 0), \
                    CASE WHEN a.attgenerated <> '' THEN 1 ELSE 0 END, \
                    CASE a.attidentity WHEN 'a' THEN 2 WHEN 'd' THEN 1 ELSE 0 END \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 \
                  AND NOT a.attisdropped \
             JOIN pg_catalog.pg_type t ON t.oid = a.atttypid \
             WHERE c.relkind IN ('r', 'p') AND NOT c.relispartition \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND left(n.nspname, 8) <> 'pg_toast' AND left(n.nspname, 7) <> 'pg_temp' \
             ORDER BY n.nspname, c.relname, a.attnum"
        }
        Engine::SqlServer => {
            "SELECT s.name, t.name, c.name, ISNULL(TYPE_NAME(c.system_type_id), ''), \
                    ISNULL((SELECT ic.key_ordinal FROM sys.indexes i \
                            JOIN sys.index_columns ic ON ic.object_id = i.object_id \
                                 AND ic.index_id = i.index_id \
                            WHERE i.object_id = t.object_id AND i.is_primary_key = 1 \
                              AND ic.column_id = c.column_id \
                              AND ic.is_included_column = 0), 0), \
                    CASE WHEN c.is_computed = 1 OR TYPE_NAME(c.system_type_id) = 'timestamp' \
                         THEN 1 ELSE 0 END, \
                    CASE WHEN c.is_identity = 1 THEN 1 ELSE 0 END \
             FROM sys.tables t \
             JOIN sys.schemas s ON s.schema_id = t.schema_id \
             JOIN sys.columns c ON c.object_id = t.object_id \
             WHERE t.is_ms_shipped = 0 \
             ORDER BY s.name, t.name, c.column_id"
        }
    }
}

/// Roda `sql` até o fim e devolve as linhas. Bloqueia: chame fora da
/// thread da interface.
pub fn collect(session: &Arc<dyn QuerySession>, sql: &str) -> Result<Vec<Row>, String> {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.to_string(), Box::new(move |e| drop(tx.send(e))));
    let mut rows = Vec::new();
    let mut error = None;
    for e in rx {
        match e {
            QueryEvent::Row(r) => rows.push(r),
            QueryEvent::Error(e) => error = Some(e.message),
            QueryEvent::Finished { .. } => break,
            _ => {}
        }
    }
    match error {
        Some(e) => Err(e),
        None => Ok(rows),
    }
}

/// As tabelas do banco da sessão. Bloqueia.
pub fn read_tables(session: &Arc<dyn QuerySession>) -> Result<Vec<Table>, String> {
    Ok(tables_from_rows(collect(
        session,
        tables_sql(session.engine()),
    )?))
}

/// Junta as linhas de [`tables_sql`] em tabelas.
pub fn tables_from_rows(rows: Vec<Row>) -> Vec<Table> {
    let mut tables: Vec<Table> = Vec::new();
    for r in rows {
        let mut cell = r.into_iter().map(Option::unwrap_or_default);
        let mut next = || cell.next().unwrap_or_default();
        let (schema, name) = (next(), next());
        let column = Column {
            name: next(),
            type_name: next(),
            key: next().parse().unwrap_or(0),
            computed: next() == "1",
            identity: next().parse().unwrap_or(0),
        };
        match tables.last_mut() {
            Some(t) if t.schema == schema && t.name == name => t.columns.push(column),
            _ => tables.push(Table {
                schema,
                name,
                columns: vec![column],
            }),
        }
    }
    tables
}

/// Como uma coluna da chave é ordenada e comparada.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    /// Pela coluna, como número.
    Int,
    /// Pelo texto, em ordem binária (uma coluna a mais no `SELECT`).
    Text,
}

/// Uma coluna comparada.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanColumn {
    /// Nome na origem e no destino (diferem só na caixa, no SQL Server).
    pub source: String,
    pub target: String,
    pub type_name: String,
    /// Identidade no destino (veja [`Column::identity`]).
    pub identity: u8,
}

/// Como comparar um par de tabelas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub engine: Engine,
    pub source: (String, String),
    pub target: (String, String),
    pub columns: Vec<PlanColumn>,
    /// As colunas da chave (índices em `columns`), na ordem da chave.
    pub key: Vec<usize>,
    pub kinds: Vec<KeyKind>,
    /// Colunas fora da comparação (só de um lado ou calculadas).
    pub ignored: Vec<String>,
}

fn same_name(engine: Engine, a: &str, b: &str) -> bool {
    match engine {
        Engine::PostgreSql | Engine::Redis | Engine::MongoDb | Engine::Elasticsearch => a == b,
        Engine::SqlServer => a.to_lowercase() == b.to_lowercase(),
    }
}

/// A tabela de `tables` com o nome de `like`.
pub fn find_table<'a>(engine: Engine, tables: &'a [Table], like: &Table) -> Option<&'a Table> {
    tables.iter().find(|t| {
        same_name(engine, &t.schema, &like.schema) && same_name(engine, &t.name, &like.name)
    })
}

fn key_kind(engine: Engine, type_name: &str) -> KeyKind {
    let ints: &[&str] = match engine {
        Engine::PostgreSql | Engine::Redis | Engine::MongoDb | Engine::Elasticsearch => {
            &["int2", "int4", "int8"]
        }
        Engine::SqlServer => &["tinyint", "smallint", "int", "bigint"],
    };
    if ints.contains(&type_name) {
        KeyKind::Int
    } else {
        KeyKind::Text
    }
}

impl Plan {
    /// O plano para comparar `source` com `target`, ou por que não dá.
    pub fn new(engine: Engine, source: &Table, target: &Table) -> Result<Plan, String> {
        let mut key: Vec<&Column> = source.columns.iter().filter(|c| c.key > 0).collect();
        if key.is_empty() {
            return Err(tr("no primary key in the source").into());
        }
        key.sort_by_key(|c| c.key);
        let target_key: Vec<&Column> = target.columns.iter().filter(|c| c.key > 0).collect();
        if target_key.is_empty() {
            return Err(tr("no primary key in the target").into());
        }
        let same_key = key.len() == target_key.len()
            && key.iter().all(|k| {
                target_key
                    .iter()
                    .any(|t| same_name(engine, &t.name, &k.name))
            });
        if !same_key {
            return Err(tr("the primary keys are different").into());
        }
        let mut columns = Vec::new();
        let mut ignored = Vec::new();
        for c in &source.columns {
            let other = target
                .columns
                .iter()
                .find(|t| same_name(engine, &t.name, &c.name));
            match other {
                Some(t) if !c.computed && !t.computed => columns.push(PlanColumn {
                    source: c.name.clone(),
                    target: t.name.clone(),
                    type_name: t.type_name.clone(),
                    identity: t.identity,
                }),
                _ => ignored.push(c.name.clone()),
            }
        }
        for t in &target.columns {
            if !source
                .columns
                .iter()
                .any(|c| same_name(engine, &c.name, &t.name))
            {
                ignored.push(t.name.clone());
            }
        }
        let mut indexes = Vec::new();
        let mut kinds = Vec::new();
        for k in key {
            let Some(i) = columns.iter().position(|c| c.source == k.name) else {
                return Err(tr("a key column cannot be compared").into());
            };
            indexes.push(i);
            kinds.push(key_kind(engine, &k.type_name));
        }
        Ok(Plan {
            engine,
            source: (source.schema.clone(), source.name.clone()),
            target: (target.schema.clone(), target.name.clone()),
            columns,
            key: indexes,
            kinds,
            ignored,
        })
    }

    /// `schema.tabela` do destino, como aparece na janela.
    pub fn label(&self) -> String {
        format!("{}.{}", self.target.0, self.target.1)
    }

    fn qualified(&self, target: bool) -> String {
        let (schema, name) = if target { &self.target } else { &self.source };
        format!("{}.{}", self.engine.ident(schema), self.engine.ident(name))
    }

    /// O texto de uma coluna da chave que não é inteira, para ordenar.
    fn sort_text(&self, column: &PlanColumn, target: bool) -> String {
        let name = if target {
            &column.target
        } else {
            &column.source
        };
        let q = self.engine.quote_ident(name);
        match self.engine {
            Engine::PostgreSql | Engine::Redis | Engine::MongoDb | Engine::Elasticsearch => {
                format!("({q}::text)")
            }
            Engine::SqlServer => match column.type_name.as_str() {
                "uniqueidentifier" => format!("CONVERT(nchar(36), {q})"),
                "date" | "datetime" | "datetime2" | "smalldatetime" | "time" | "datetimeoffset" => {
                    format!("CONVERT(nvarchar(40), {q}, 121)")
                }
                "binary" | "varbinary" => format!("CONVERT(nvarchar(max), {q}, 1)"),
                "float" | "real" => format!("CONVERT(nvarchar(40), {q}, 3)"),
                "money" | "smallmoney" => format!("CONVERT(nvarchar(40), {q}, 2)"),
                _ => format!("CONVERT(nvarchar(4000), {q})"),
            },
        }
    }

    /// O `SELECT` de um lado: as colunas comparadas, depois o texto de
    /// cada coluna de chave [`KeyKind::Text`], na ordem da chave.
    pub fn select_sql(&self, target: bool) -> String {
        let mut list: Vec<String> = self
            .columns
            .iter()
            .map(|c| {
                self.engine
                    .quote_ident(if target { &c.target } else { &c.source })
            })
            .collect();
        let mut order = Vec::new();
        for (n, (&i, kind)) in self.key.iter().zip(&self.kinds).enumerate() {
            let column = &self.columns[i];
            match kind {
                KeyKind::Int => order.push(list[i].clone()),
                KeyKind::Text => {
                    let text = self.sort_text(column, target);
                    let alias = self.engine.quote_ident(&format!("pp_key_{}", n + 1));
                    list.push(format!("{text} AS {alias}"));
                    order.push(match self.engine {
                        Engine::PostgreSql
                        | Engine::Redis
                        | Engine::MongoDb
                        | Engine::Elasticsearch => {
                            format!("{text} COLLATE \"C\"")
                        }
                        Engine::SqlServer => format!("{text} COLLATE Latin1_General_100_BIN2"),
                    });
                }
            }
        }
        format!(
            "SELECT {} FROM {} ORDER BY {}",
            list.join(", "),
            self.qualified(target),
            order.join(", ")
        )
    }

    /// Compara as chaves de duas linhas vindas de [`Plan::select_sql`].
    fn cmp_keys(&self, a: &Row, b: &Row) -> Result<Ordering, String> {
        let mut text = self.columns.len();
        for (&i, kind) in self.key.iter().zip(&self.kinds) {
            let ord = match kind {
                KeyKind::Int => {
                    let n = |r: &Row| {
                        r.get(i)
                            .and_then(|v| v.as_deref())
                            .and_then(|v| v.trim().parse::<i128>().ok())
                            .ok_or_else(|| self.order_error())
                    };
                    n(a)?.cmp(&n(b)?)
                }
                KeyKind::Text => {
                    let t = |r: &'_ Row| -> Result<String, String> {
                        r.get(text)
                            .cloned()
                            .flatten()
                            .ok_or_else(|| self.order_error())
                    };
                    let (x, y) = (t(a)?, t(b)?);
                    text += 1;
                    match self.engine {
                        Engine::PostgreSql
                        | Engine::Redis
                        | Engine::MongoDb
                        | Engine::Elasticsearch => x.as_bytes().cmp(y.as_bytes()),
                        Engine::SqlServer => cmp_utf16_padded(&x, &y),
                    }
                }
            };
            if ord != Ordering::Equal {
                return Ok(ord);
            }
        }
        Ok(Ordering::Equal)
    }

    fn order_error(&self) -> String {
        trf!(
            "The key order of {table} could not be reproduced; the table was not compared.",
            table = self.label()
        )
    }
}

/// Ordem do SQL Server numa collation BIN2: unidade UTF-16 a unidade, com
/// o texto mais curto completado com espaços.
fn cmp_utf16_padded(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.encode_utf16(), b.encode_utf16());
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (p, q) => {
                let ord = p.unwrap_or(0x20).cmp(&q.unwrap_or(0x20));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// Uma diferença entre as duas tabelas (linhas só com as colunas
/// comparadas, na ordem de [`Plan::columns`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Diff {
    /// Só na origem: vira `INSERT`.
    OnlySource(Row),
    /// Só no destino: vira `DELETE`.
    OnlyTarget(Row),
    /// Nos dois, com valores diferentes (origem, destino): vira `UPDATE`.
    Different(Row, Row),
}

/// O que deu a comparação de uma tabela.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub only_source: u64,
    pub only_target: u64,
    pub different: u64,
    pub equal: u64,
}

impl Counts {
    pub fn differences(&self) -> u64 {
        self.only_source + self.only_target + self.different
    }
}

/// O andamento, lido pela interface enquanto a comparação roda.
#[derive(Default)]
pub struct Progress {
    /// Tabelas já comparadas.
    pub tables: AtomicU64,
    /// Linhas lidas da tabela atual, de cada lado.
    pub source_rows: AtomicU64,
    pub target_rows: AtomicU64,
    /// O usuário pediu para parar.
    pub cancel: AtomicBool,
}

/// Anda pelas duas tabelas ordenadas pela chave e entrega cada diferença
/// a `found`. Para com erro se um lado falhar, se a ordem não for a
/// esperada ou se `progress.cancel` ligar.
pub fn merge(
    plan: &Plan,
    source: &mut dyn Iterator<Item = Result<Row, String>>,
    target: &mut dyn Iterator<Item = Result<Row, String>>,
    progress: &Progress,
    found: &mut dyn FnMut(Diff) -> Result<(), String>,
) -> Result<Counts, String> {
    let n = plan.columns.len();
    let mut counts = Counts::default();
    // Lê a próxima linha de um lado, conferindo a ordem.
    let advance = |it: &mut dyn Iterator<Item = Result<Row, String>>,
                   last: &mut Option<Row>,
                   rows: &AtomicU64|
     -> Result<Option<Row>, String> {
        let Some(row) = it.next().transpose()? else {
            return Ok(None);
        };
        rows.fetch_add(1, Atomic::Relaxed);
        if let Some(previous) = last.as_ref()
            && plan.cmp_keys(previous, &row)? != Ordering::Less
        {
            return Err(plan.order_error());
        }
        // Só a chave precisa ficar para a próxima conferência, mas a linha
        // inteira é barata de guardar (é uma só).
        *last = Some(row.clone());
        Ok(Some(row))
    };
    let (mut last_s, mut last_t) = (None, None);
    let mut s = advance(source, &mut last_s, &progress.source_rows)?;
    let mut t = advance(target, &mut last_t, &progress.target_rows)?;
    let cut = |mut r: Row| {
        r.truncate(n);
        r
    };
    loop {
        if progress.cancel.load(Atomic::Relaxed) {
            return Err(tr("Canceled.").into());
        }
        let ord = match (&s, &t) {
            (None, None) => break,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(a), Some(b)) => plan.cmp_keys(a, b)?,
        };
        match ord {
            Ordering::Less => {
                counts.only_source += 1;
                found(Diff::OnlySource(cut(s.take().unwrap_or_default())))?;
                s = advance(source, &mut last_s, &progress.source_rows)?;
            }
            Ordering::Greater => {
                counts.only_target += 1;
                found(Diff::OnlyTarget(cut(t.take().unwrap_or_default())))?;
                t = advance(target, &mut last_t, &progress.target_rows)?;
            }
            Ordering::Equal => {
                let (a, b) = (
                    cut(s.take().unwrap_or_default()),
                    cut(t.take().unwrap_or_default()),
                );
                if a == b {
                    counts.equal += 1;
                } else {
                    counts.different += 1;
                    found(Diff::Different(a, b))?;
                }
                s = advance(source, &mut last_s, &progress.source_rows)?;
                t = advance(target, &mut last_t, &progress.target_rows)?;
            }
        }
    }
    Ok(counts)
}

/// As linhas de uma consulta, uma a uma, enquanto o servidor manda. A
/// fila é limitada: quando o merge está ocupado com o outro lado, a
/// leitura deste espera sem ocupar memória. Soltar antes do fim cancela a
/// consulta.
pub struct RowStream {
    rx: mpsc::Receiver<QueryEvent>,
    session: Arc<dyn QuerySession>,
    finished: bool,
}

impl RowStream {
    pub fn open(session: Arc<dyn QuerySession>, sql: String) -> Self {
        let (tx, rx) = mpsc::sync_channel(BUFFER);
        session.execute(
            sql,
            Box::new(move |e| {
                if !matches!(
                    e,
                    QueryEvent::Row(_) | QueryEvent::Error(_) | QueryEvent::Finished { .. }
                ) {
                    return;
                }
                // Fila cheia: espera o merge. O evento chega numa thread
                // da runtime das conexões, que avisamos para ela não
                // parar as outras consultas do app.
                if let Err(mpsc::TrySendError::Full(e)) = tx.try_send(e) {
                    let multi = tokio::runtime::Handle::try_current().is_ok_and(|h| {
                        h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
                    });
                    if multi {
                        tokio::task::block_in_place(|| drop(tx.send(e)));
                    } else {
                        drop(tx.send(e));
                    }
                }
            }),
        );
        Self {
            rx,
            session,
            finished: false,
        }
    }
}

impl Iterator for RowStream {
    type Item = Result<Row, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.rx.recv() {
            Ok(QueryEvent::Row(r)) => Some(Ok(r)),
            Ok(QueryEvent::Error(e)) => Some(Err(e.message)),
            Ok(QueryEvent::Finished { canceled: true }) => {
                self.finished = true;
                Some(Err(tr("Canceled.").into()))
            }
            Ok(_) | Err(_) => {
                self.finished = true;
                None
            }
        }
    }
}

impl Drop for RowStream {
    fn drop(&mut self) {
        if !self.finished {
            self.session.cancel();
        }
    }
}

/// O arquivo temporário com todas as diferenças de uma tabela, uma por
/// linha (JSON). Some com o resultado.
pub struct Spill(PathBuf);

impl Spill {
    pub fn path(&self) -> &Path {
        &self.0
    }

    fn create() -> std::io::Result<(Spill, BufWriter<std::fs::File>)> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "postpilot-data-compare-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Atomic::Relaxed)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        // São dados do banco: só o usuário lê.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(&path)?;
        Ok((Spill(path), BufWriter::new(file)))
    }
}

/// As diferenças gravadas num arquivo de [`Spill`], na ordem da chave.
pub fn read_diffs(path: &Path) -> std::io::Result<impl Iterator<Item = std::io::Result<Diff>>> {
    let file = BufReader::new(std::fs::File::open(path)?);
    Ok(file
        .lines()
        .map(|line| serde_json::from_str(&line?).map_err(std::io::Error::other)))
}

impl Drop for Spill {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// O resultado de uma tabela.
pub struct TableResult {
    pub plan: Plan,
    pub counts: Counts,
    /// As primeiras [`SAMPLE`] diferenças, para a grade.
    pub sample: Vec<Diff>,
    /// Todas as diferenças (sem arquivo quando não há nenhuma).
    pub spill: Option<Spill>,
    pub error: Option<String>,
}

/// Compara um par de tabelas. Bloqueia até o fim.
pub fn compare_table(
    plan: Plan,
    source: &Arc<dyn QuerySession>,
    target: &Arc<dyn QuerySession>,
    progress: &Progress,
) -> TableResult {
    progress.source_rows.store(0, Atomic::Relaxed);
    progress.target_rows.store(0, Atomic::Relaxed);
    let mut sample = Vec::new();
    let mut spill: Option<(Spill, BufWriter<std::fs::File>)> = None;
    let outcome = {
        let mut s = RowStream::open(source.clone(), plan.select_sql(false));
        let mut t = RowStream::open(target.clone(), plan.select_sql(true));
        let mut found = |d: Diff| -> Result<(), String> {
            if spill.is_none() {
                spill = Some(Spill::create().map_err(|e| e.to_string())?);
            }
            if let Some((_, w)) = &mut spill {
                serde_json::to_writer(&mut *w, &d).map_err(|e| e.to_string())?;
                w.write_all(b"\n").map_err(|e| e.to_string())?;
            }
            if sample.len() < SAMPLE {
                sample.push(d);
            }
            Ok(())
        };
        merge(&plan, &mut s, &mut t, progress, &mut found)
    };
    let flushed = match &mut spill {
        Some((_, w)) => w.flush().map_err(|e| e.to_string()),
        None => Ok(()),
    };
    let (counts, error) = match outcome.and_then(|c| flushed.map(|()| c)) {
        Ok(c) => (c, None),
        Err(e) => (Counts::default(), Some(e)),
    };
    let failed = error.is_some();
    TableResult {
        plan,
        counts,
        sample: if failed { Vec::new() } else { sample },
        spill: spill.map(|(s, _)| s).filter(|_| !failed),
        error,
    }
}

/// Quais comandos entram no script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub insert: bool,
    pub update: bool,
    pub delete: bool,
}

impl Default for Wanted {
    fn default() -> Self {
        Wanted {
            insert: true,
            update: true,
            delete: true,
        }
    }
}

impl Plan {
    fn literal(&self, column: &PlanColumn, value: Option<&str>) -> String {
        match (self.engine, value) {
            // O SQL Server não converte texto em binário sozinho.
            (Engine::SqlServer, Some(v))
                if matches!(column.type_name.as_str(), "binary" | "varbinary" | "image") =>
            {
                format!("CONVERT(varbinary(max), '{}', 1)", v.replace('\'', "''"))
            }
            _ => self.engine.literal(value),
        }
    }

    fn where_key(&self, row: &Row) -> String {
        self.key
            .iter()
            .map(|&i| {
                let c = &self.columns[i];
                format!(
                    "{} = {}",
                    self.engine.ident(&c.target),
                    self.literal(c, row.get(i).and_then(|v| v.as_deref()))
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// O `DELETE` de uma linha que só existe no destino.
    pub fn delete_sql(&self, row: &Row) -> String {
        format!(
            "DELETE FROM {} WHERE {};",
            self.qualified(true),
            self.where_key(row)
        )
    }

    /// O `UPDATE` que deixa a linha do destino como a da origem (só as
    /// colunas que diferem).
    pub fn update_sql(&self, source: &Row, target: &Row) -> String {
        let set: Vec<String> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(i, _)| source.get(*i) != target.get(*i) && !self.key.contains(i))
            .map(|(i, c)| {
                format!(
                    "{} = {}",
                    self.engine.ident(&c.target),
                    self.literal(c, source.get(i).and_then(|v| v.as_deref()))
                )
            })
            .collect();
        format!(
            "UPDATE {} SET {} WHERE {};",
            self.qualified(true),
            set.join(", "),
            self.where_key(target)
        )
    }

    /// O `INSERT` de uma linha que só existe na origem.
    pub fn insert_sql(&self, row: &Row) -> String {
        let names: Vec<String> = self
            .columns
            .iter()
            .map(|c| self.engine.ident(&c.target))
            .collect();
        let values: Vec<String> = self
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| self.literal(c, row.get(i).and_then(|v| v.as_deref())))
            .collect();
        let overriding =
            if self.engine == Engine::PostgreSql && self.columns.iter().any(|c| c.identity == 2) {
                " OVERRIDING SYSTEM VALUE"
            } else {
                ""
            };
        format!(
            "INSERT INTO {} ({}){overriding} VALUES ({});",
            self.qualified(true),
            names.join(", "),
            values.join(", ")
        )
    }

    /// A tabela tem coluna de identidade que o `INSERT` do SQL Server só
    /// aceita com `IDENTITY_INSERT`.
    fn identity_insert(&self) -> bool {
        self.engine == Engine::SqlServer && self.columns.iter().any(|c| c.identity > 0)
    }
}

/// Escreve o script que deixa cada destino como a origem: de cada tabela,
/// os `DELETE`, depois os `UPDATE`, depois os `INSERT`, tudo numa
/// transação. Devolve quantos comandos de dados escreveu.
pub fn write_script(
    out: &mut dyn Write,
    engine: Engine,
    header: &[String],
    tables: &[(Plan, PathBuf)],
    wanted: Wanted,
) -> std::io::Result<u64> {
    for line in header {
        writeln!(out, "-- {line}")?;
    }
    writeln!(out)?;
    match engine {
        Engine::PostgreSql | Engine::Redis | Engine::MongoDb | Engine::Elasticsearch => {
            writeln!(out, "BEGIN;")?
        }
        Engine::SqlServer => writeln!(
            out,
            "SET XACT_ABORT ON;\nSET NOCOUNT ON;\nSET DATEFORMAT ymd;\nBEGIN TRANSACTION;"
        )?,
    }
    let mut statements = 0;
    for (plan, spill) in tables {
        writeln!(out, "\n-- {}", plan.label())?;
        if wanted.delete {
            for d in read_diffs(spill)? {
                if let Diff::OnlyTarget(row) = d? {
                    writeln!(out, "{}", plan.delete_sql(&row))?;
                    statements += 1;
                }
            }
        }
        if wanted.update {
            for d in read_diffs(spill)? {
                if let Diff::Different(source, target) = d? {
                    writeln!(out, "{}", plan.update_sql(&source, &target))?;
                    statements += 1;
                }
            }
        }
        if wanted.insert {
            let mut identity = false;
            for d in read_diffs(spill)? {
                if let Diff::OnlySource(row) = d? {
                    if plan.identity_insert() && !identity {
                        identity = true;
                        writeln!(out, "SET IDENTITY_INSERT {} ON;", plan.qualified(true))?;
                    }
                    writeln!(out, "{}", plan.insert_sql(&row))?;
                    statements += 1;
                }
            }
            if identity {
                writeln!(out, "SET IDENTITY_INSERT {} OFF;", plan.qualified(true))?;
            }
        }
    }
    match engine {
        Engine::PostgreSql | Engine::Redis | Engine::MongoDb | Engine::Elasticsearch => {
            writeln!(out, "\nCOMMIT;")?
        }
        Engine::SqlServer => writeln!(out, "\nCOMMIT TRANSACTION;")?,
    }
    out.flush()?;
    Ok(statements)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, ty: &str, key: u32) -> Column {
        Column {
            name: name.into(),
            type_name: ty.into(),
            key,
            computed: false,
            identity: 0,
        }
    }

    fn table(columns: Vec<Column>) -> Table {
        Table {
            schema: "public".into(),
            name: "cliente".into(),
            columns,
        }
    }

    fn row(values: &[&str]) -> Result<Row, String> {
        Ok(values
            .iter()
            .map(|v| (*v != "NULL").then(|| v.to_string()))
            .collect())
    }

    fn run(
        plan: &Plan,
        s: Vec<Result<Row, String>>,
        t: Vec<Result<Row, String>>,
    ) -> (Result<Counts, String>, Vec<Diff>) {
        let mut found = Vec::new();
        let counts = merge(
            plan,
            &mut s.into_iter(),
            &mut t.into_iter(),
            &Progress::default(),
            &mut |d| {
                found.push(d);
                Ok(())
            },
        );
        (counts, found)
    }

    fn int_plan() -> Plan {
        let t = table(vec![column("id", "int4", 1), column("nome", "text", 0)]);
        Plan::new(Engine::PostgreSql, &t, &t).unwrap()
    }

    #[test]
    fn integer_keys_merge_as_numbers() {
        let plan = int_plan();
        assert_eq!(
            plan.select_sql(false),
            "SELECT id, nome FROM public.cliente ORDER BY id"
        );
        let (counts, found) = run(
            &plan,
            vec![
                row(&["2", "b"]),
                row(&["9", "i"]),
                row(&["10", "j"]),
                row(&["11", "NULL"]),
            ],
            vec![
                row(&["1", "a"]),
                row(&["2", "b"]),
                row(&["10", "J"]),
                row(&["11", "NULL"]),
            ],
        );
        assert_eq!(
            counts.unwrap(),
            Counts {
                only_source: 1,
                only_target: 1,
                different: 1,
                equal: 2
            }
        );
        assert_eq!(
            found,
            vec![
                Diff::OnlyTarget(row(&["1", "a"]).unwrap()),
                Diff::OnlySource(row(&["9", "i"]).unwrap()),
                Diff::Different(row(&["10", "j"]).unwrap(), row(&["10", "J"]).unwrap()),
            ]
        );
    }

    #[test]
    fn text_keys_sort_by_the_extra_column() {
        let t = table(vec![
            column("sigla", "varchar", 1),
            column("n", "int4", 2),
            column("v", "text", 0),
        ]);
        let plan = Plan::new(Engine::PostgreSql, &t, &t).unwrap();
        assert_eq!(
            plan.select_sql(true),
            "SELECT sigla, n, v, (sigla::text) AS pp_key_1 FROM public.cliente \
             ORDER BY (sigla::text) COLLATE \"C\", n"
        );
        // "Z" vem antes de "a" na ordem binária.
        let (counts, found) = run(
            &plan,
            vec![
                row(&["Z", "1", "x", "Z"]),
                row(&["a", "1", "x", "a"]),
                row(&["a", "2", "x", "a"]),
            ],
            vec![row(&["Z", "1", "x", "Z"]), row(&["a", "2", "y", "a"])],
        );
        assert_eq!(counts.unwrap().equal, 1);
        assert_eq!(
            found,
            vec![
                Diff::OnlySource(row(&["a", "1", "x"]).unwrap()),
                Diff::Different(
                    row(&["a", "2", "x"]).unwrap(),
                    row(&["a", "2", "y"]).unwrap()
                ),
            ]
        );
    }

    #[test]
    fn a_side_out_of_order_fails_instead_of_inventing_differences() {
        let (counts, _) = run(
            &int_plan(),
            vec![row(&["2", "b"]), row(&["1", "a"])],
            vec![row(&["1", "a"]), row(&["2", "b"])],
        );
        assert!(counts.unwrap_err().contains("public.cliente"));
        let (counts, _) = run(
            &int_plan(),
            vec![row(&["1", "a"]), Err("caiu".into())],
            vec![],
        );
        assert_eq!(counts.unwrap_err(), "caiu");
    }

    #[test]
    fn sql_server_text_order_pads_with_spaces() {
        assert_eq!(cmp_utf16_padded("a", "a  "), Ordering::Equal);
        assert_eq!(cmp_utf16_padded("a", "a\t"), Ordering::Greater);
        assert_eq!(cmp_utf16_padded("a", "ab"), Ordering::Less);
        assert_eq!(cmp_utf16_padded("Z", "a"), Ordering::Less);
    }

    #[test]
    fn tables_need_the_same_primary_key() {
        let with = table(vec![column("id", "int4", 1), column("nome", "text", 0)]);
        let without = table(vec![column("id", "int4", 0), column("nome", "text", 0)]);
        let other = table(vec![column("id", "int4", 0), column("nome", "text", 1)]);
        assert!(Plan::new(Engine::PostgreSql, &without, &with).is_err());
        assert!(Plan::new(Engine::PostgreSql, &with, &without).is_err());
        assert!(Plan::new(Engine::PostgreSql, &with, &other).is_err());
    }

    #[test]
    fn columns_of_one_side_and_computed_ones_are_left_out() {
        let mut total = column("total", "numeric", 0);
        total.computed = true;
        let s = table(vec![
            column("id", "int4", 1),
            column("nome", "text", 0),
            total.clone(),
            column("novo", "text", 0),
        ]);
        let t = table(vec![
            column("id", "int4", 1),
            column("nome", "text", 0),
            total,
            column("velho", "text", 0),
        ]);
        let plan = Plan::new(Engine::PostgreSql, &s, &t).unwrap();
        assert_eq!(plan.columns.len(), 2);
        assert_eq!(plan.ignored, ["total", "novo", "velho"]);
    }

    #[test]
    fn postgres_statements() {
        let mut id = column("id", "int4", 1);
        id.identity = 2;
        let t = table(vec![
            id,
            column("Nome", "text", 0),
            column("obs", "text", 0),
        ]);
        let plan = Plan::new(Engine::PostgreSql, &t, &t).unwrap();
        let a = row(&["7", "d'água", "NULL"]).unwrap();
        let b = row(&["7", "x", "y"]).unwrap();
        assert_eq!(
            plan.delete_sql(&b),
            "DELETE FROM public.cliente WHERE id = '7';"
        );
        assert_eq!(
            plan.update_sql(&a, &b),
            "UPDATE public.cliente SET \"Nome\" = 'd''água', obs = NULL WHERE id = '7';"
        );
        assert_eq!(
            plan.insert_sql(&a),
            "INSERT INTO public.cliente (id, \"Nome\", obs) OVERRIDING SYSTEM VALUE \
             VALUES ('7', 'd''água', NULL);"
        );
    }

    #[test]
    fn sql_server_statements_and_key_order() {
        let mut id = column("id", "uniqueidentifier", 1);
        id.identity = 0;
        let t = Table {
            schema: "dbo".into(),
            name: "Arquivo".into(),
            columns: vec![id, column("dados", "varbinary", 0)],
        };
        let plan = Plan::new(Engine::SqlServer, &t, &t).unwrap();
        assert_eq!(
            plan.select_sql(false),
            "SELECT [id], [dados], CONVERT(nchar(36), [id]) AS [pp_key_1] FROM dbo.Arquivo \
             ORDER BY CONVERT(nchar(36), [id]) COLLATE Latin1_General_100_BIN2"
        );
        assert_eq!(
            plan.insert_sql(&row(&["A-1", "0xFF"]).unwrap()),
            "INSERT INTO dbo.Arquivo (id, dados) VALUES (N'A-1', CONVERT(varbinary(max), '0xFF', 1));"
        );
    }
}
