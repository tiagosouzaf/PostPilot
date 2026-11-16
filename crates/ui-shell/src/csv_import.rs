//! Importar um arquivo CSV numa tabela (PostgreSQL e SQL Server).
//!
//! Este módulo é a parte sem interface: ler o arquivo, descobrir o
//! separador, casar as colunas do arquivo com as da tabela e carregar as
//! linhas. A janela fica em [`crate::csv_import_dialog`].
//!
//! - O arquivo é lido aos poucos ([`CsvReader`]), nunca inteiro na
//!   memória, e sempre fora da thread da interface.
//! - A carga vai por `INSERT` de várias linhas, pela sessão de consulta
//!   (serve para os dois motores), com os valores como texto: o banco
//!   converte para o tipo de cada coluna.
//! - Tudo numa transação só: um erro, ou o cancelamento, desfaz o que já
//!   tinha ido. A mensagem diz em que linhas do arquivo o erro aconteceu.
//! - Um campo vazio sem aspas é NULL e `""` é texto vazio, como o CSV que o
//!   PostPilot exporta (dá para trocar o texto que vale NULL).
//! - Arquivo que não é UTF-8 é lido como Windows-1252 (o CSV que o Excel
//!   grava em português).

use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use pgstudio_core::connection::Engine;
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_i18n::{tr, trf};

/// Linhas por `INSERT`. No SQL Server, um `VALUES` grande demora para
/// compilar (e o limite é 1000 linhas): com 100 linhas a carga foi mais
/// que o dobro mais rápida do que com 250 ou mais.
fn batch_rows(engine: Engine) -> usize {
    match engine {
        // O Redis não tem tabelas: a importação nem é oferecida.
        Engine::PostgreSql | Engine::Redis => 500,
        Engine::SqlServer => 100,
    }
}
/// Tamanho a partir do qual o `INSERT` é mandado mesmo com menos linhas.
const BATCH_BYTES: usize = 512 * 1024;
/// Registros lidos para a prévia.
pub const SAMPLE_ROWS: usize = 100;
/// Separadores que a detecção tenta, na ordem de preferência.
pub const DELIMITERS: [u8; 4] = *b",;\t|";

/// Como ler o arquivo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvOptions {
    pub delimiter: u8,
    /// A primeira linha tem os nomes das colunas.
    pub header: bool,
    /// Texto (sem aspas) que vale NULL; o padrão é o campo vazio.
    pub null_text: String,
}

impl Default for CsvOptions {
    fn default() -> Self {
        CsvOptions {
            delimiter: b',',
            header: true,
            null_text: String::new(),
        }
    }
}

impl CsvOptions {
    /// O valor do campo para o banco (`None` é NULL).
    pub fn value<'a>(&self, field: &'a Field) -> Option<&'a str> {
        (field.quoted || field.text != self.null_text).then_some(field.text.as_str())
    }
}

/// Um campo lido do arquivo.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Field {
    pub text: String,
    /// Veio entre aspas (aí o vazio é texto vazio, não NULL).
    pub quoted: bool,
}

/// Leitor de CSV: um registro por vez, com aspas (`""` é uma aspa),
/// quebras de linha dentro de aspas e fim de linha `\n`, `\r\n` ou `\r`.
/// Linhas em branco são puladas.
pub struct CsvReader<R: Read> {
    input: R,
    buf: Box<[u8]>,
    pos: usize,
    len: usize,
    delimiter: u8,
    /// Linha (a partir de 1) em que o próximo registro começa.
    line: u64,
    /// Bytes já lidos.
    bytes: u64,
    started: bool,
}

impl<R: Read> CsvReader<R> {
    pub fn new(input: R, delimiter: u8) -> Self {
        CsvReader {
            input,
            buf: vec![0; 64 * 1024].into_boxed_slice(),
            pos: 0,
            len: 0,
            delimiter,
            line: 1,
            bytes: 0,
            started: false,
        }
    }

    /// Bytes do arquivo já lidos (para o progresso).
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    fn fill(&mut self) -> io::Result<bool> {
        if self.pos < self.len {
            return Ok(true);
        }
        self.len = self.input.read(&mut self.buf)?;
        self.pos = 0;
        Ok(self.len > 0)
    }

    fn peek(&mut self) -> io::Result<Option<u8>> {
        Ok(self.fill()?.then(|| self.buf[self.pos]))
    }

    fn next(&mut self) -> io::Result<Option<u8>> {
        let b = self.peek()?;
        if b.is_some() {
            self.pos += 1;
            self.bytes += 1;
        }
        Ok(b)
    }

    /// Pula a marca de UTF-8 do começo do arquivo.
    fn skip_bom(&mut self) -> io::Result<()> {
        self.started = true;
        // A marca tem 3 bytes; o buffer recém-cheio tem todos (ou o
        // arquivo é menor que isso).
        if self.fill()? && self.buf[self.pos..self.len].starts_with(&[0xEF, 0xBB, 0xBF]) {
            self.pos += 3;
            self.bytes += 3;
        }
        Ok(())
    }

    /// Lê o próximo registro em `out`. Devolve a linha do arquivo em que
    /// ele começa, ou `None` no fim.
    pub fn record(&mut self, out: &mut Vec<Field>) -> io::Result<Option<u64>> {
        out.clear();
        if !self.started {
            self.skip_bom()?;
        }
        // Linhas em branco.
        loop {
            match self.peek()? {
                None => return Ok(None),
                Some(b'\n') => {
                    self.next()?;
                    self.line += 1;
                }
                Some(b'\r') => {
                    self.next()?;
                    if self.peek()? == Some(b'\n') {
                        self.next()?;
                    }
                    self.line += 1;
                }
                Some(_) => break,
            }
        }
        let start = self.line;
        let mut field: Vec<u8> = Vec::new();
        let mut quoted = false;
        // Dentro das aspas de um campo.
        let mut inside = false;
        // No começo de um campo (só aí uma aspa abre o campo).
        let mut fresh = true;
        loop {
            let b = self.next()?;
            if inside {
                match b {
                    // Aspas sem fechar: o campo vai até o fim do arquivo.
                    None => {
                        out.push(decode(field, quoted));
                        return Ok(Some(start));
                    }
                    Some(b'"') => {
                        if self.peek()? == Some(b'"') {
                            self.next()?;
                            field.push(b'"');
                        } else {
                            inside = false;
                        }
                    }
                    Some(b) => {
                        if b == b'\n' {
                            self.line += 1;
                        }
                        field.push(b);
                    }
                }
                continue;
            }
            match b {
                None => {
                    out.push(decode(field, quoted));
                    return Ok(Some(start));
                }
                Some(b'"') if fresh => {
                    quoted = true;
                    inside = true;
                    fresh = false;
                }
                Some(b) if b == self.delimiter => {
                    out.push(decode(std::mem::take(&mut field), quoted));
                    quoted = false;
                    fresh = true;
                }
                Some(b'\n') => {
                    self.line += 1;
                    out.push(decode(field, quoted));
                    return Ok(Some(start));
                }
                Some(b'\r') => {
                    if self.peek()? == Some(b'\n') {
                        self.next()?;
                    }
                    self.line += 1;
                    out.push(decode(field, quoted));
                    return Ok(Some(start));
                }
                Some(b) => {
                    field.push(b);
                    fresh = false;
                }
            }
        }
    }
}

/// Bytes de um campo como texto: UTF-8 ou, não sendo, Windows-1252.
fn decode(bytes: Vec<u8>, quoted: bool) -> Field {
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => e.into_bytes().iter().map(|&b| windows_1252(b)).collect(),
    };
    Field { text, quoted }
}

/// Um byte do Windows-1252 como caractere.
fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{81}', '\u{201A}', '\u{192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{2C6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8D}', '\u{17D}',
        '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}',
        '\u{2014}', '\u{2DC}', '\u{2122}', '\u{161}', '\u{203A}', '\u{153}', '\u{9D}', '\u{17E}',
        '\u{178}',
    ];
    match b {
        0x80..=0x9F => HIGH[usize::from(b - 0x80)],
        b => char::from(b),
    }
}

/// O separador mais provável para o começo de um arquivo: o que divide
/// as primeiras linhas no mesmo número de colunas (mais de uma), com mais
/// colunas. Sem nenhum assim, a vírgula.
pub fn sniff_delimiter(head: &[u8]) -> u8 {
    let mut best = (0usize, b',');
    for delimiter in DELIMITERS {
        let mut reader = CsvReader::new(head, delimiter);
        let mut record = Vec::new();
        let mut counts = Vec::new();
        while counts.len() < 20 && matches!(reader.record(&mut record), Ok(Some(_))) {
            counts.push(record.len());
        }
        // O último registro pode estar cortado pelo fim da amostra.
        if counts.len() > 2 && reader.bytes() as usize >= head.len() {
            counts.pop();
        }
        let Some(&first) = counts.first() else {
            continue;
        };
        let same = counts.iter().filter(|&&c| c == first).count();
        // Quase todas as linhas com o mesmo número de colunas.
        if first > 1 && same * 10 >= counts.len() * 9 && first > best.0 {
            best = (first, delimiter);
        }
    }
    best.1
}

/// O começo do arquivo, para a prévia e para casar as colunas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    /// Os primeiros registros (com a linha de nomes, se houver).
    pub records: Vec<Vec<Field>>,
    /// Tamanho do arquivo em bytes.
    pub size: u64,
}

impl Sample {
    /// Número de colunas do arquivo (o maior registro da amostra).
    pub fn width(&self) -> usize {
        self.records.iter().map(Vec::len).max().unwrap_or(0)
    }

    /// Nomes das colunas do arquivo: os da primeira linha ou "Column n".
    pub fn headers(&self, header: bool) -> Vec<String> {
        let first = self.records.first().filter(|_| header);
        (0..self.width())
            .map(|i| {
                first
                    .and_then(|r| r.get(i))
                    .map(|f| f.text.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| trf!("Column {n}", n = i + 1))
            })
            .collect()
    }

    /// Os registros de dados (sem a linha de nomes).
    pub fn rows(&self, header: bool) -> &[Vec<Field>] {
        let skip = usize::from(header && !self.records.is_empty());
        &self.records[skip..]
    }
}

/// Descobre o separador do arquivo `path`.
pub fn sniff(path: &Path) -> io::Result<u8> {
    let mut head = vec![0; 64 * 1024];
    let mut file = std::fs::File::open(path)?;
    let mut len = 0;
    while len < head.len() {
        match file.read(&mut head[len..])? {
            0 => break,
            n => len += n,
        }
    }
    Ok(sniff_delimiter(&head[..len]))
}

/// Lê os primeiros registros de `path`.
pub fn sample(path: &Path, delimiter: u8) -> io::Result<Sample> {
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut reader = CsvReader::new(file, delimiter);
    let mut records = Vec::new();
    let mut record = Vec::new();
    while records.len() < SAMPLE_ROWS && reader.record(&mut record)?.is_some() {
        records.push(std::mem::take(&mut record));
    }
    Ok(Sample { records, size })
}

/// O que dá para fazer com uma coluna da tabela.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnKind {
    Plain,
    /// Identidade que o banco só aceita receber pedindo (`OVERRIDING
    /// SYSTEM VALUE`, `SET IDENTITY_INSERT`).
    Identity,
    /// Calculada pelo banco: não recebe valor.
    Computed,
}

/// Uma coluna da tabela de destino.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableColumn {
    pub name: String,
    pub type_name: String,
    pub kind: ColumnKind,
}

/// A tabela de destino.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub engine: Engine,
    pub schema: String,
    pub table: String,
    pub columns: Vec<TableColumn>,
}

impl Target {
    /// `schema.tabela` como entra no SQL.
    pub fn qualified(&self) -> String {
        format!(
            "{}.{}",
            self.engine.quote_ident(&self.schema),
            self.engine.quote_ident(&self.table)
        )
    }

    /// `schema.tabela` para mostrar.
    pub fn display(&self) -> String {
        format!("{}.{}", self.schema, self.table)
    }
}

/// Consulta que lê o schema, o nome e as colunas da tabela com este id
/// (`oid` no PostgreSQL, `object_id` no SQL Server).
pub fn target_sql(engine: Engine, id: u64) -> String {
    match engine {
        Engine::PostgreSql | Engine::Redis => format!(
            "SELECT n.nspname, c.relname, a.attname, format_type(a.atttypid, a.atttypmod), \
                    CASE WHEN a.attgenerated <> '' THEN 'c' WHEN a.attidentity = 'a' THEN 'i' ELSE '' END \
               FROM pg_catalog.pg_class c \
               JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
               JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid \
              WHERE c.oid = {id} AND a.attnum > 0 AND NOT a.attisdropped \
              ORDER BY a.attnum"
        ),
        Engine::SqlServer => format!(
            "SELECT s.name, o.name, c.name, t.name, \
                    CASE WHEN c.is_computed = 1 OR t.name = 'timestamp' THEN 'c' \
                         WHEN c.is_identity = 1 THEN 'i' ELSE '' END \
               FROM sys.objects o \
               JOIN sys.schemas s ON s.schema_id = o.schema_id \
               JOIN sys.columns c ON c.object_id = o.object_id \
               JOIN sys.types t ON t.user_type_id = c.user_type_id \
              WHERE o.object_id = {id} \
              ORDER BY c.column_id"
        ),
    }
}

/// Roda `sql` e espera o fim. Devolve as linhas, ou o primeiro erro.
/// Bloqueia: só fora da thread da interface.
pub fn query(
    session: &Arc<dyn QuerySession>,
    sql: String,
) -> Result<Vec<Vec<Option<String>>>, String> {
    let (tx, rx) = mpsc::channel();
    session.execute(sql, Box::new(move |e| drop(tx.send(e))));
    let mut rows = Vec::new();
    let mut error: Option<String> = None;
    loop {
        match rx.recv() {
            Ok(QueryEvent::Row(row)) => rows.push(row),
            Ok(QueryEvent::Error(e)) => {
                error.get_or_insert(e.message);
            }
            Ok(QueryEvent::Finished { .. }) => break,
            Ok(_) => {}
            Err(_) => {
                error.get_or_insert_with(|| tr("The connection was lost.").to_string());
                break;
            }
        }
    }
    match error {
        Some(e) => Err(e),
        None => Ok(rows),
    }
}

/// Lê a tabela de destino. Bloqueia.
pub fn load_target(session: &Arc<dyn QuerySession>, id: u64) -> Result<Target, String> {
    let engine = session.engine();
    let rows = query(session, target_sql(engine, id))?;
    let text = |row: &[Option<String>], i: usize| row.get(i).cloned().flatten().unwrap_or_default();
    let first = rows
        .first()
        .ok_or_else(|| tr("The table was not found.").to_string())?;
    Ok(Target {
        engine,
        schema: text(first, 0),
        table: text(first, 1),
        columns: rows
            .iter()
            .map(|row| TableColumn {
                name: text(row, 2),
                type_name: text(row, 3),
                kind: match text(row, 4).as_str() {
                    "c" => ColumnKind::Computed,
                    "i" => ColumnKind::Identity,
                    _ => ColumnKind::Plain,
                },
            })
            .collect(),
    })
}

/// Nome para comparar: minúsculas, sem acentos, espaços, `_` ou `-`
/// ("Observação" casa com `observacao`, "Data Nasc" com `data_nasc`).
fn normalized(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '_' | '-'))
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            c => c,
        })
        .collect()
}

/// Casa as colunas da tabela com as do arquivo: para cada coluna da
/// tabela, o índice da coluna do arquivo (ou `None`: não importar). Com
/// nomes no arquivo, pelo nome; sem nomes, pela ordem. Colunas calculadas
/// ficam de fora.
pub fn auto_map(columns: &[TableColumn], headers: &[String], header: bool) -> Vec<Option<usize>> {
    if header {
        let names: Vec<String> = headers.iter().map(|h| normalized(h)).collect();
        return columns
            .iter()
            .map(|c| {
                let wanted = normalized(&c.name);
                (c.kind != ColumnKind::Computed)
                    .then(|| names.iter().position(|n| *n == wanted))
                    .flatten()
            })
            .collect();
    }
    let mut next = 0;
    columns
        .iter()
        .map(|c| {
            (c.kind != ColumnKind::Computed && next < headers.len()).then(|| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

/// O `INSERT` da carga, sem as linhas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    engine: Engine,
    /// `INSERT INTO t (a, b) VALUES `.
    prefix: String,
    /// Coluna do arquivo de cada coluna do `INSERT`.
    sources: Vec<usize>,
    /// Comandos antes e depois da carga, dentro da transação.
    before: Option<String>,
    after: Option<String>,
}

impl Plan {
    /// Monta o `INSERT` para as colunas casadas em `mapping` (veja
    /// [`auto_map`]). `None` se nenhuma coluna foi casada.
    pub fn new(target: &Target, mapping: &[Option<usize>]) -> Option<Plan> {
        let engine = target.engine;
        let mapped: Vec<(&TableColumn, usize)> = target
            .columns
            .iter()
            .zip(mapping)
            .filter_map(|(c, m)| Some((c, (*m)?)))
            .collect();
        if mapped.is_empty() {
            return None;
        }
        let names: Vec<String> = mapped
            .iter()
            .map(|(c, _)| engine.quote_ident(&c.name))
            .collect();
        let identity = mapped.iter().any(|(c, _)| c.kind == ColumnKind::Identity);
        let table = target.qualified();
        let overriding = match engine {
            Engine::PostgreSql if identity => " OVERRIDING SYSTEM VALUE",
            _ => "",
        };
        let switch = |on: &str| {
            (identity && engine == Engine::SqlServer)
                .then(|| format!("SET IDENTITY_INSERT {table} {on}"))
        };
        Some(Plan {
            engine,
            prefix: format!(
                "INSERT INTO {table} ({}){overriding} VALUES ",
                names.join(", ")
            ),
            sources: mapped.iter().map(|(_, i)| *i).collect(),
            before: switch("ON"),
            after: switch("OFF"),
        })
    }

    /// Acrescenta uma linha a `sql` (um `INSERT` começado ou vazio).
    fn push(&self, sql: &mut String, record: &[Field], options: &CsvOptions) {
        if sql.is_empty() {
            sql.push_str(&self.prefix);
        } else {
            sql.push_str(",\n");
        }
        sql.push('(');
        for (i, source) in self.sources.iter().enumerate() {
            if i > 0 {
                sql.push_str(", ");
            }
            let value = record.get(*source).and_then(|f| options.value(f));
            sql.push_str(&self.engine.literal(value));
        }
        sql.push(')');
    }

    /// O `INSERT` destas linhas (para a prévia e os testes).
    pub fn insert(&self, records: &[Vec<Field>], options: &CsvOptions) -> String {
        let mut sql = String::new();
        for r in records {
            self.push(&mut sql, r, options);
        }
        sql
    }

    fn begin(&self) -> &'static str {
        match self.engine {
            Engine::PostgreSql | Engine::Redis => "BEGIN",
            Engine::SqlServer => "SET XACT_ABORT ON; BEGIN TRANSACTION",
        }
    }

    fn commit(&self) -> &'static str {
        match self.engine {
            Engine::PostgreSql | Engine::Redis => "COMMIT",
            Engine::SqlServer => "COMMIT TRANSACTION",
        }
    }

    fn rollback(&self) -> &'static str {
        match self.engine {
            Engine::PostgreSql | Engine::Redis => "ROLLBACK",
            Engine::SqlServer => "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION",
        }
    }
}

/// O que a carga conta enquanto roda.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Linhas já mandadas e bytes do arquivo já lidos.
    Sent { rows: u64, bytes: u64 },
    /// Terminou: linhas gravadas.
    Done { rows: u64 },
    /// Falhou; nada foi gravado.
    Failed(String),
    /// Cancelada; nada foi gravado.
    Canceled,
}

/// Carrega `path` na tabela, numa transação. Bloqueia até o fim: rode
/// numa thread. `cancel` interrompe entre um `INSERT` e outro (quem
/// cancela também pode chamar [`QuerySession::cancel`] para interromper o
/// `INSERT` em andamento).
pub fn run(
    session: &Arc<dyn QuerySession>,
    path: &Path,
    options: &CsvOptions,
    plan: &Plan,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            progress(Progress::Failed(trf!(
                "Could not read {path}: {error}",
                path = path.display(),
                error = e
            )));
            return;
        }
    };
    if let Err(e) = query(session, plan.begin().to_string()) {
        progress(Progress::Failed(e));
        return;
    }
    let outcome = load(session, file, options, plan, cancel, progress);
    let outcome = match outcome {
        Ok(rows) => match query(session, plan.commit().to_string()) {
            Ok(_) => Progress::Done { rows },
            Err(e) => Progress::Failed(e),
        },
        Err(stop) => stop,
    };
    if !matches!(outcome, Progress::Done { .. }) {
        // Desfaz o que foi; no SQL Server, devolve também o IDENTITY_INSERT.
        let _ = query(session, plan.rollback().to_string());
        if let Some(after) = &plan.after {
            let _ = query(session, after.clone());
        }
    }
    progress(outcome);
}

/// A carga dentro da transação: devolve as linhas mandadas ou por que
/// parou.
fn load(
    session: &Arc<dyn QuerySession>,
    file: std::fs::File,
    options: &CsvOptions,
    plan: &Plan,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<u64, Progress> {
    let failed = |e: String| Progress::Failed(e);
    if let Some(before) = &plan.before {
        query(session, before.clone()).map_err(failed)?;
    }
    let mut reader = CsvReader::new(file, options.delimiter);
    let mut record = Vec::new();
    let read_error =
        |e: io::Error| Progress::Failed(trf!("Could not read the file: {error}", error = e));
    if options.header {
        reader.record(&mut record).map_err(read_error)?;
    }
    let mut rows = 0u64;
    let mut sql = String::new();
    let mut batch = 0usize;
    // Linhas do arquivo do lote: a primeira e a última.
    let mut lines = (0u64, 0u64);
    loop {
        let line = reader.record(&mut record).map_err(read_error)?;
        if let Some(line) = line {
            if batch == 0 {
                lines.0 = line;
            }
            lines.1 = line;
            plan.push(&mut sql, &record, options);
            batch += 1;
        }
        let full = batch >= batch_rows(plan.engine) || sql.len() >= BATCH_BYTES;
        if batch > 0 && (full || line.is_none()) {
            if cancel.load(Ordering::Relaxed) {
                return Err(Progress::Canceled);
            }
            query(session, std::mem::take(&mut sql)).map_err(|e| {
                if cancel.load(Ordering::Relaxed) {
                    return Progress::Canceled;
                }
                Progress::Failed(if lines.0 == lines.1 {
                    trf!(
                        "Line {line} of the file: {error}",
                        line = lines.0,
                        error = e
                    )
                } else {
                    trf!(
                        "Lines {from} to {to} of the file: {error}",
                        from = lines.0,
                        to = lines.1,
                        error = e
                    )
                })
            })?;
            rows += batch as u64;
            batch = 0;
            progress(Progress::Sent {
                rows,
                bytes: reader.bytes(),
            });
        }
        if line.is_none() {
            break;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Progress::Canceled);
    }
    if let Some(after) = &plan.after {
        query(session, after.clone()).map_err(failed)?;
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn records(text: &[u8], delimiter: u8) -> Vec<(u64, Vec<(String, bool)>)> {
        let mut reader = CsvReader::new(text, delimiter);
        let mut record = Vec::new();
        let mut out = Vec::new();
        while let Some(line) = reader.record(&mut record).unwrap() {
            out.push((
                line,
                record.iter().map(|f| (f.text.clone(), f.quoted)).collect(),
            ));
        }
        assert_eq!(reader.bytes(), text.len() as u64);
        out
    }

    fn plain(v: &str) -> (String, bool) {
        (v.to_string(), false)
    }

    fn quoted(v: &str) -> (String, bool) {
        (v.to_string(), true)
    }

    #[test]
    fn reads_quotes_line_breaks_and_blank_lines() {
        let text = b"\xEF\xBB\xBFid,nome,obs\r\n1,\"Ana, \"\"A\"\"\",\r\n\r\n2,\"\",\"linha\num\"\n3,sem fim de linha,x";
        assert_eq!(
            records(text, b','),
            vec![
                (1, vec![plain("id"), plain("nome"), plain("obs")]),
                (2, vec![plain("1"), quoted("Ana, \"A\""), plain("")]),
                (4, vec![plain("2"), quoted(""), quoted("linha\num")]),
                (6, vec![plain("3"), plain("sem fim de linha"), plain("x")]),
            ]
        );
    }

    #[test]
    fn reads_what_the_export_writes() {
        use pgstudio_result_grid::export::{Format, Writer};
        let rows = vec![
            vec![Some("1".to_string()), Some("Ana, \"A\"".to_string()), None],
            vec![
                Some("2".to_string()),
                Some(String::new()),
                Some(" linha\r\num\ttab \\ fim ".to_string()),
            ],
        ];
        let columns = vec!["id".into(), "nome".into(), "obs".into()];
        let mut w = Writer::new(Vec::new(), Format::Csv, columns, "t").unwrap();
        for r in &rows {
            w.row(r).unwrap();
        }
        let csv = w.finish().unwrap();
        let options = CsvOptions::default();
        let mut reader = CsvReader::new(csv.as_slice(), b',');
        let mut record = Vec::new();
        reader.record(&mut record).unwrap();
        for row in &rows {
            reader.record(&mut record).unwrap().unwrap();
            let back: Vec<Option<String>> = record
                .iter()
                .map(|f| options.value(f).map(str::to_string))
                .collect();
            assert_eq!(&back, row);
        }
        assert_eq!(reader.record(&mut record).unwrap(), None);
    }

    #[test]
    fn a_quote_in_the_middle_of_a_field_is_text() {
        assert_eq!(
            records(b"a;5\" tela;\"x\"y\n", b';'),
            vec![(1, vec![plain("a"), plain("5\" tela"), quoted("xy")])]
        );
    }

    #[test]
    fn a_file_that_is_not_utf8_is_read_as_windows_1252() {
        // "São João;R$ 10 – 20€" no Windows-1252.
        let text = b"S\xE3o Jo\xE3o;R$ 10 \x96 20\x80\n";
        assert_eq!(
            records(text, b';'),
            vec![(1, vec![plain("São João"), plain("R$ 10 – 20€")])]
        );
    }

    #[test]
    fn the_delimiter_is_the_one_that_splits_every_line_alike() {
        assert_eq!(sniff_delimiter(b"a,b,c\n1,2,3\n"), b',');
        assert_eq!(sniff_delimiter(b"nome;valor\nAna;1,50\nBia;2,75\n"), b';');
        assert_eq!(sniff_delimiter(b"a\tb\n1\t2\n"), b'\t');
        assert_eq!(sniff_delimiter(b"a|b|c\n1|2|3\n"), b'|');
        // Vírgulas dentro de aspas não contam.
        assert_eq!(
            sniff_delimiter(b"nome;obs\n\"Silva, Ana\";\"a, b, c\"\n"),
            b';'
        );
        assert_eq!(sniff_delimiter(b"uma coluna\nso\n"), b',');
        assert_eq!(sniff_delimiter(b""), b',');
    }

    fn column(name: &str, kind: ColumnKind) -> TableColumn {
        TableColumn {
            name: name.into(),
            type_name: "text".into(),
            kind,
        }
    }

    fn target(engine: Engine) -> Target {
        Target {
            engine,
            schema: "public".into(),
            table: "cliente".into(),
            columns: vec![
                column("id", ColumnKind::Identity),
                column("Nome Completo", ColumnKind::Plain),
                column("total", ColumnKind::Computed),
                column("obs", ColumnKind::Plain),
            ],
        }
    }

    #[test]
    fn columns_match_by_name_or_by_position() {
        let t = target(Engine::PostgreSql);
        let headers: Vec<String> = ["OBS", "nome_completo", "total", "outra"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            auto_map(&t.columns, &headers, true),
            vec![None, Some(1), None, Some(0)]
        );
        // Acentos não atrapalham.
        let accents = [column("observacao", ColumnKind::Plain)];
        assert_eq!(
            auto_map(&accents, &["Observação".to_string()], true),
            vec![Some(0)]
        );
        // Sem nomes: pela ordem, pulando a calculada.
        assert_eq!(
            auto_map(&t.columns, &headers[..2], false),
            vec![Some(0), Some(1), None, None]
        );
    }

    fn fields(values: &[&str]) -> Vec<Field> {
        values
            .iter()
            .map(|v| Field {
                text: v.to_string(),
                quoted: false,
            })
            .collect()
    }

    #[test]
    fn the_insert_quotes_names_and_values_for_each_engine() {
        let rows = vec![fields(&["1", "O'Neil", ""]), fields(&["2", "a\\b"])];
        let options = CsvOptions::default();
        let mapping = [Some(0), Some(1), None, Some(2)];
        let pg = Plan::new(&target(Engine::PostgreSql), &mapping).unwrap();
        assert_eq!(
            pg.insert(&rows, &options),
            "INSERT INTO public.cliente (id, \"Nome Completo\", obs) OVERRIDING SYSTEM VALUE VALUES \
             ('1', 'O''Neil', NULL),\n('2', E'a\\\\b', NULL)"
        );
        assert_eq!(pg.before, None);
        let ms = Plan::new(&target(Engine::SqlServer), &mapping).unwrap();
        assert_eq!(
            ms.insert(&rows, &options),
            "INSERT INTO [public].[cliente] ([id], [Nome Completo], [obs]) VALUES \
             (N'1', N'O''Neil', NULL),\n(N'2', N'a\\b', NULL)"
        );
        assert_eq!(
            ms.before.as_deref(),
            Some("SET IDENTITY_INSERT [public].[cliente] ON")
        );
        // Sem a identidade, nada de especial.
        let no_id = Plan::new(&target(Engine::SqlServer), &[None, Some(0), None, None]).unwrap();
        assert_eq!(no_id.before, None);
        assert!(Plan::new(&target(Engine::PostgreSql), &[None; 4]).is_none());
    }

    #[test]
    fn another_text_can_stand_for_null() {
        let options = CsvOptions {
            null_text: "\\N".into(),
            ..Default::default()
        };
        let plan = Plan::new(&target(Engine::PostgreSql), &[None, Some(0), None, Some(1)]).unwrap();
        assert!(
            plan.insert(&[fields(&["", "\\N"])], &options)
                .ends_with("VALUES ('', NULL)")
        );
    }

    /// Sessão que guarda o SQL e falha no comando que contém `fail`.
    struct Scripted {
        log: Arc<Mutex<Vec<String>>>,
        fail: Option<&'static str>,
    }

    impl QuerySession for Scripted {
        fn execute(&self, sql: String, mut sink: pgstudio_core::query::QuerySink) {
            let failed = self.fail.is_some_and(|f| sql.contains(f));
            self.log.lock().unwrap().push(sql);
            if failed {
                sink(QueryEvent::Error(
                    pgstudio_core::query::ServerMessage::error("valor inválido"),
                ));
            }
            sink(QueryEvent::Finished { canceled: false });
        }

        fn cancel(&self) {}

        fn database(&self) -> String {
            "loja".into()
        }
    }

    fn import(text: &str, fail: Option<&'static str>) -> (Vec<String>, Vec<Progress>) {
        let path = std::env::temp_dir().join(format!(
            "postpilot-csv-{}-{:?}.csv",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, text).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let session: Arc<dyn QuerySession> = Arc::new(Scripted {
            log: log.clone(),
            fail,
        });
        let plan = Plan::new(&target(Engine::PostgreSql), &[None, Some(0), None, Some(1)]).unwrap();
        let mut events = Vec::new();
        run(
            &session,
            &path,
            &CsvOptions::default(),
            &plan,
            &AtomicBool::new(false),
            &mut |p| events.push(p),
        );
        std::fs::remove_file(&path).unwrap();
        let log = log.lock().unwrap().clone();
        (log, events)
    }

    #[test]
    fn the_load_goes_in_batches_inside_one_transaction() {
        let mut text = String::from("nome,obs\n");
        for i in 0..1200 {
            text.push_str(&format!("cliente {i},\n"));
        }
        let (log, events) = import(&text, None);
        assert_eq!(log.len(), 5);
        assert_eq!(log[0], "BEGIN");
        assert!(log[1].starts_with(
            "INSERT INTO public.cliente (\"Nome Completo\", obs) VALUES ('cliente 0', NULL),\n"
        ));
        assert_eq!(log[1].lines().count(), 500);
        assert_eq!(log[3].lines().count(), 200);
        assert!(log[3].ends_with("('cliente 1199', NULL)"));
        assert_eq!(log[4], "COMMIT");
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], Progress::Sent { rows: 500, .. }));
        assert_eq!(events[3], Progress::Done { rows: 1200 });
    }

    #[test]
    fn an_error_rolls_everything_back_and_names_the_lines() {
        let mut text = String::from("nome,obs\n");
        for i in 0..700 {
            text.push_str(&format!("cliente {i},\n"));
        }
        let (log, events) = import(&text, Some("'cliente 600'"));
        assert_eq!(log.last().map(String::as_str), Some("ROLLBACK"));
        assert!(!log.iter().any(|s| s == "COMMIT"));
        assert_eq!(
            events.last(),
            Some(&Progress::Failed(
                "Lines 502 to 701 of the file: valor inválido".into()
            ))
        );
    }

    #[test]
    fn an_empty_file_imports_nothing() {
        let (log, events) = import("nome,obs\n", None);
        assert_eq!(log, ["BEGIN", "COMMIT"]);
        assert_eq!(events, [Progress::Done { rows: 0 }]);
    }
}
