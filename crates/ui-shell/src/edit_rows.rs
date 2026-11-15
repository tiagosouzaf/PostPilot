//! Edit Top 200 Rows: a grade editável de uma tabela, como no SSMS.
//!
//! A janela tem uma sessão própria no banco da tabela. Ela lê as colunas e
//! a chave primária do catálogo, depois as primeiras linhas. Cada célula
//! alterada fica marcada até o usuário sair da linha (ou apertar Enter
//! fora da edição): aí a linha vai ao banco num comando só (UPDATE pela
//! chave primária, ou INSERT para a linha nova do fim), que é atômico; o
//! `RETURNING` traz os valores como o banco gravou (padrões, tipos). Erro
//! deixa a linha marcada, com a mensagem, e a seleção volta para ela; Esc
//! desfaz a linha.
//!
//! Sem chave primária não há como achar a linha de novo, então a grade
//! fica só leitura e diz por quê. Colunas geradas e de identidade
//! `ALWAYS` não são editáveis.

use pgstudio_i18n::{tr, tr_noop, trf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::{self, Align, Color32, Key, Layout, RichText, Sense, vec2};
use egui_phosphor::regular as icon;
use pgstudio_core::connection::Engine;
use pgstudio_core::query::{QueryEvent, QuerySession};

/// Altura de uma linha da grade.
const ROW_HEIGHT: f32 = 22.0;
/// Largura da coluna com o marcador da linha.
const MARKER_WIDTH: f32 = 26.0;

/// Uma coluna da tabela.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditColumn {
    pub name: String,
    /// Tipo como o `format_type` escreve.
    pub type_name: String,
    /// Faz parte da chave primária.
    pub key: bool,
    /// Gerada ou de identidade `ALWAYS`: o banco escolhe o valor.
    pub read_only: bool,
}

/// A tabela aberta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditTable {
    /// Motor do servidor, para o SQL.
    pub engine: Engine,
    /// Nome qualificado e com aspas, para o SQL.
    pub name: String,
    /// `schema.tabela`, para o título.
    pub display: String,
    pub columns: Vec<EditColumn>,
}

impl EditTable {
    /// Tem chave primária (dá para editar).
    pub fn has_key(&self) -> bool {
        self.columns.iter().any(|c| c.key)
    }

    /// Lista das colunas para SELECT e RETURNING.
    fn quote(&self, name: &str) -> String {
        self.engine.quote_ident(name)
    }

    fn column_list(&self) -> String {
        self.columns
            .iter()
            .map(|c| self.quote(&c.name))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// `WHERE` que acha a linha pelos valores originais da chave.
    fn key_filter(&self, original: &[Option<String>]) -> String {
        self.columns
            .iter()
            .zip(original)
            .filter(|(c, _)| c.key)
            .map(|(c, v)| match v {
                Some(v) => format!("{} = {}", self.quote(&c.name), self.engine.literal(Some(v))),
                None => format!("{} IS NULL", self.quote(&c.name)),
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// SQL que lê as primeiras `limit` linhas.
    pub fn select_sql(&self, limit: u32) -> String {
        match self.engine {
            Engine::PostgreSql => format!(
                "SELECT {} FROM {} LIMIT {limit}",
                self.column_list(),
                self.name
            ),
            Engine::SqlServer => format!(
                "SELECT TOP ({limit}) {} FROM {}",
                self.column_list(),
                self.name
            ),
        }
    }

    /// SQL Server: a chave da linha gravada vai para `@k` (`OUTPUT ...
    /// INTO`, que funciona com trigger) e a linha é lida de novo por ela.
    fn keyed(&self, statement: impl FnOnce(&str) -> String) -> String {
        let keys: Vec<&EditColumn> = self.columns.iter().filter(|c| c.key).collect();
        let declare = keys
            .iter()
            .map(|c| format!("{} {}", self.quote(&c.name), c.type_name))
            .collect::<Vec<_>>()
            .join(", ");
        let output = format!(
            "OUTPUT {} INTO @k",
            keys.iter()
                .map(|c| format!("inserted.{}", self.quote(&c.name)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let join = keys
            .iter()
            .map(|c| format!("k.{0} = t.{0}", self.quote(&c.name)))
            .collect::<Vec<_>>()
            .join(" AND ");
        format!(
            "DECLARE @k TABLE ({declare});\n{};\n\
             SELECT {} FROM {} AS t WHERE EXISTS (SELECT 1 FROM @k AS k WHERE {join});",
            statement(&output),
            self.column_list(),
            self.name
        )
    }

    /// UPDATE das colunas que mudaram, achando a linha pela chave
    /// original. None quando nada mudou.
    pub fn update_sql(
        &self,
        original: &[Option<String>],
        values: &[Option<String>],
    ) -> Option<String> {
        let sets: Vec<String> = self
            .columns
            .iter()
            .zip(original.iter().zip(values))
            .filter(|(c, (o, v))| !c.read_only && o != v)
            .map(|(c, (_, v))| {
                format!(
                    "{} = {}",
                    self.quote(&c.name),
                    self.engine.literal(v.as_deref())
                )
            })
            .collect();
        if sets.is_empty() {
            return None;
        }
        let filter = self.key_filter(original);
        Some(match self.engine {
            Engine::PostgreSql => format!(
                "UPDATE {} SET {} WHERE {filter} RETURNING {}",
                self.name,
                sets.join(", "),
                self.column_list()
            ),
            Engine::SqlServer => self.keyed(|output| {
                format!(
                    "UPDATE {} SET {} {output} WHERE {filter}",
                    self.name,
                    sets.join(", ")
                )
            }),
        })
    }

    /// INSERT das colunas preenchidas; as outras ficam com o padrão.
    pub fn insert_sql(&self, values: &[Option<String>], touched: &[bool]) -> String {
        let (names, literals): (Vec<String>, Vec<String>) = self
            .columns
            .iter()
            .zip(values.iter().zip(touched))
            .filter(|(c, (_, t))| !c.read_only && **t)
            .map(|(c, (v, _))| (self.quote(&c.name), self.engine.literal(v.as_deref())))
            .unzip();
        if self.engine == Engine::SqlServer {
            return self.keyed(|output| {
                if names.is_empty() {
                    format!("INSERT INTO {} {output} DEFAULT VALUES", self.name)
                } else {
                    format!(
                        "INSERT INTO {} ({}) {output} VALUES ({})",
                        self.name,
                        names.join(", "),
                        literals.join(", ")
                    )
                }
            });
        }
        if names.is_empty() {
            format!(
                "INSERT INTO {} DEFAULT VALUES RETURNING {}",
                self.name,
                self.column_list()
            )
        } else {
            format!(
                "INSERT INTO {} ({}) VALUES ({}) RETURNING {}",
                self.name,
                names.join(", "),
                literals.join(", "),
                self.column_list()
            )
        }
    }

    /// DELETE da linha pela chave original.
    pub fn delete_sql(&self, original: &[Option<String>]) -> String {
        format!(
            "DELETE FROM {} WHERE {}",
            self.name,
            self.key_filter(original)
        )
    }
}

/// Valor como literal de SQL: NULL, ou texto entre aspas que o banco
/// converte para o tipo da coluna. Com barra invertida, usa `E'...'`, que
/// vale com qualquer `standard_conforming_strings`.
pub fn literal(value: Option<&str>) -> String {
    pgstudio_core::connection::Engine::PostgreSql.literal(value)
}

/// Colunas, chave primária e nome de uma tabela, pelo oid.
fn columns_sql(engine: Engine, oid: u32) -> String {
    if engine == Engine::SqlServer {
        return mssql_columns_sql(oid);
    }
    format!(
        "SELECT a.attname, format_type(a.atttypid, a.atttypmod), \
                coalesce(a.attnum = ANY (i.indkey), false), \
                a.attgenerated <> '' OR a.attidentity = 'a', \
                format('%I.%I', n.nspname, c.relname), \
                n.nspname || '.' || c.relname \
           FROM pg_attribute a \
           JOIN pg_class c ON c.oid = a.attrelid \
           JOIN pg_namespace n ON n.oid = c.relnamespace \
           LEFT JOIN pg_index i ON i.indrelid = c.oid AND i.indisprimary \
          WHERE a.attrelid = {oid} AND a.attnum > 0 AND NOT a.attisdropped \
          ORDER BY a.attnum"
    )
}

/// [`columns_sql`] no SQL Server. O tipo vem completo (`nvarchar(60)`),
/// para declarar a tabela das chaves; calculadas, identidade, rowversion e
/// binárias ficam só leitura.
fn mssql_columns_sql(oid: u32) -> String {
    format!(
        "SELECT c.name, \
                CASE WHEN t.name IN ('varchar', 'char', 'varbinary', 'binary') \
                       THEN t.name + '(' + CASE WHEN c.max_length = -1 THEN 'max' \
                            ELSE CAST(c.max_length AS varchar(10)) END + ')' \
                     WHEN t.name IN ('nvarchar', 'nchar') \
                       THEN t.name + '(' + CASE WHEN c.max_length = -1 THEN 'max' \
                            ELSE CAST(c.max_length / 2 AS varchar(10)) END + ')' \
                     WHEN t.name IN ('decimal', 'numeric') \
                       THEN t.name + '(' + CAST(c.precision AS varchar(10)) + ',' \
                            + CAST(c.scale AS varchar(10)) + ')' \
                     WHEN t.name IN ('datetime2', 'time', 'datetimeoffset') \
                       THEN t.name + '(' + CAST(c.scale AS varchar(10)) + ')' \
                     ELSE t.name END, \
                CASE WHEN ic.column_id IS NULL THEN 'f' ELSE 't' END, \
                CASE WHEN c.is_computed = 1 OR c.is_identity = 1 \
                       OR t.name IN ('timestamp', 'binary', 'varbinary', 'image', 'sql_variant') \
                     THEN 't' ELSE 'f' END, \
                QUOTENAME(s.name) + '.' + QUOTENAME(o.name), \
                s.name + '.' + o.name \
           FROM sys.objects o \
           JOIN sys.schemas s ON s.schema_id = o.schema_id \
           JOIN sys.columns c ON c.object_id = o.object_id \
           JOIN sys.types t ON t.user_type_id = c.system_type_id \
           LEFT JOIN sys.indexes pk ON pk.object_id = o.object_id AND pk.is_primary_key = 1 \
           LEFT JOIN sys.index_columns ic ON ic.object_id = pk.object_id \
                                         AND ic.index_id = pk.index_id \
                                         AND ic.column_id = c.column_id \
          WHERE o.object_id = {oid} \
          ORDER BY c.column_id"
    )
}

fn parse_table(engine: Engine, rows: &[Vec<Option<String>>]) -> Result<EditTable, String> {
    let first = rows
        .first()
        .ok_or_else(|| tr("The table no longer exists.").to_string())?;
    let text = |r: &Vec<Option<String>>, i: usize| r.get(i).cloned().flatten().unwrap_or_default();
    Ok(EditTable {
        engine,
        name: text(first, 4),
        display: text(first, 5),
        columns: rows
            .iter()
            .map(|r| EditColumn {
                name: text(r, 0),
                type_name: text(r, 1),
                key: text(r, 2) == "t",
                read_only: text(r, 3) == "t",
            })
            .collect(),
    })
}

/// Estado de uma linha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    /// Igual ao banco.
    Clean,
    /// Alterada e ainda não gravada.
    Modified,
    /// Linha nova, ainda não gravada.
    New,
    /// A última gravação falhou.
    Error,
}

#[derive(Clone, Debug)]
struct Row {
    /// Como o banco tem (vazio numa linha nova).
    original: Vec<Option<String>>,
    values: Vec<Option<String>>,
    /// Células que o usuário mexeu (para o INSERT).
    touched: Vec<bool>,
    new: bool,
    error: Option<String>,
}

impl Row {
    fn blank(columns: usize) -> Self {
        Row {
            original: vec![None; columns],
            values: vec![None; columns],
            touched: vec![false; columns],
            new: true,
            error: None,
        }
    }

    fn loaded(values: Vec<Option<String>>) -> Self {
        let n = values.len();
        Row {
            original: values.clone(),
            values,
            touched: vec![false; n],
            new: false,
            error: None,
        }
    }

    fn is_dirty(&self) -> bool {
        if self.new {
            self.touched.iter().any(|t| *t)
        } else {
            self.original != self.values
        }
    }
}

/// O que volta da sessão.
enum Reply {
    Table(Result<EditTable, String>),
    Rows(Result<Vec<Vec<Option<String>>>, String>),
    /// Linha gravada (com os valores do RETURNING) ou erro.
    Saved {
        row: usize,
        result: Result<Vec<Option<String>>, String>,
    },
    Deleted {
        row: usize,
        result: Result<(), String>,
    },
}

/// Situação da janela.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    LoadingColumns,
    LoadingRows,
    Ready,
    Failed(String),
}

/// A janela Edit Top N Rows de uma tabela.
pub struct EditRows {
    session: Arc<dyn QuerySession>,
    oid: u32,
    limit: u32,
    table: Option<EditTable>,
    rows: Vec<Row>,
    phase: Phase,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    /// Linha sendo gravada ou excluída.
    busy: Option<usize>,
    /// Célula selecionada (linha, coluna).
    selected: Option<(usize, usize)>,
    /// Célula em edição e o texto dela.
    editing: Option<(usize, usize, String)>,
    /// Linha esperando o "Yes" para excluir.
    confirm_delete: Option<usize>,
    /// Última mensagem (erro ou o que foi feito).
    message: Option<String>,
    log: Vec<String>,
    /// Fecha quando a gravação pendente terminar bem.
    close_after_save: bool,
    closed: bool,
    /// Editor de JSON aberto numa célula (linha, coluna).
    json_editor: Option<(usize, usize, crate::json_viewer::JsonViewer)>,
    /// O editor de JSON pediu o JSON Query Guide.
    guide_requested: bool,
}

impl EditRows {
    /// Abre a tabela `oid` numa sessão do banco dela e começa a ler.
    pub fn new(session: Arc<dyn QuerySession>, oid: u32, limit: u32, ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut e = EditRows {
            session,
            oid,
            limit,
            table: None,
            rows: Vec::new(),
            phase: Phase::LoadingColumns,
            tx,
            rx,
            busy: None,
            selected: None,
            editing: None,
            confirm_delete: None,
            message: None,
            log: Vec::new(),
            close_after_save: false,
            closed: false,
            json_editor: None,
            guide_requested: false,
        };
        e.load_columns(ctx);
        e
    }

    /// Texto da aba, como no SSMS: `banco - schema.tabela`.
    pub fn title(&self) -> String {
        match &self.table {
            Some(t) => format!("{} - {}", self.session.database(), t.display),
            None => format!("{} - {}", self.session.database(), tr("Edit Rows")),
        }
    }

    pub fn table(&self) -> Option<&EditTable> {
        self.table.as_ref()
    }

    /// Lendo colunas ou linhas.
    pub fn is_loading(&self) -> bool {
        matches!(self.phase, Phase::LoadingColumns | Phase::LoadingRows)
    }

    /// Gravando ou excluindo uma linha.
    pub fn is_saving(&self) -> bool {
        self.busy.is_some()
    }

    /// Erro que impediu a leitura.
    pub fn load_error(&self) -> Option<&str> {
        match &self.phase {
            Phase::Failed(e) => Some(e),
            _ => None,
        }
    }

    /// Por que a grade é só leitura, se for.
    pub fn read_only_reason(&self) -> Option<&'static str> {
        match &self.table {
            Some(t) if !t.has_key() => Some(tr(
                "The table has no primary key, so its rows cannot be identified; \
                 the grid is read-only.",
            )),
            _ => None,
        }
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Linhas lidas ou inseridas (sem a linha em branco do fim).
    pub fn row_count(&self) -> usize {
        self.rows.iter().filter(|r| !r.new || r.is_dirty()).count()
    }

    /// Valor atual de uma célula.
    pub fn value(&self, row: usize, col: usize) -> Option<&str> {
        self.rows.get(row)?.values.get(col)?.as_deref()
    }

    pub fn row_state(&self, row: usize) -> RowState {
        match self.rows.get(row) {
            Some(r) if r.error.is_some() => RowState::Error,
            Some(r) if r.new => RowState::New,
            Some(r) if r.is_dirty() => RowState::Modified,
            _ => RowState::Clean,
        }
    }

    /// Erro da última gravação da linha.
    pub fn row_error(&self, row: usize) -> Option<&str> {
        self.rows.get(row)?.error.as_deref()
    }

    pub fn selected(&self) -> Option<(usize, usize)> {
        self.selected
    }

    /// Alguma linha alterada e não gravada.
    pub fn has_changes(&self) -> bool {
        self.rows.iter().any(Row::is_dirty)
    }

    pub fn is_confirming(&self) -> bool {
        self.confirm_delete.is_some()
    }

    /// Mensagens para o Output desde a última chamada.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// Pediu para fechar e já pode.
    pub fn take_closed(&mut self) -> bool {
        std::mem::take(&mut self.closed)
    }

    fn editable(&self) -> bool {
        self.phase == Phase::Ready && self.table.as_ref().is_some_and(EditTable::has_key)
    }

    fn column_read_only(&self, col: usize) -> bool {
        self.table
            .as_ref()
            .and_then(|t| t.columns.get(col))
            .is_none_or(|c| c.read_only)
    }

    /// Lê tudo de novo; alterações não gravadas se perdem.
    pub fn refresh(&mut self, ctx: &egui::Context) {
        if self.is_loading() || self.busy.is_some() {
            return;
        }
        self.editing = None;
        self.message = None;
        if self.table.is_some() {
            self.load_rows(ctx);
        } else {
            self.load_columns(ctx);
        }
    }

    fn load_columns(&mut self, ctx: &egui::Context) {
        self.phase = Phase::LoadingColumns;
        let tx = self.tx.clone();
        let engine = self.session.engine();
        run(
            &self.session,
            columns_sql(engine, self.oid),
            ctx,
            move |rows, _| {
                let _ = tx.send(Reply::Table(rows.and_then(|r| parse_table(engine, &r))));
            },
        );
    }

    fn load_rows(&mut self, ctx: &egui::Context) {
        let Some(table) = &self.table else {
            return;
        };
        self.phase = Phase::LoadingRows;
        let tx = self.tx.clone();
        run(
            &self.session,
            table.select_sql(self.limit),
            ctx,
            move |rows, _| {
                let _ = tx.send(Reply::Rows(rows));
            },
        );
    }

    /// Muda o valor de uma célula (na linha em branco do fim, ela vira uma
    /// linha nova). Não grava: isso acontece ao sair da linha.
    pub fn set_cell(&mut self, row: usize, col: usize, value: Option<String>) {
        if !self.editable() || self.column_read_only(col) || self.busy == Some(row) {
            return;
        }
        let columns = self.table.as_ref().map_or(0, |t| t.columns.len());
        let len = self.rows.len();
        let Some(r) = self.rows.get_mut(row) else {
            return;
        };
        if col >= columns {
            return;
        }
        r.values[col] = value;
        r.touched[col] = true;
        r.error = None;
        // A linha em branco virou linha nova: outra em branco no fim.
        if row + 1 == len && r.new {
            self.rows.push(Row::blank(columns));
        }
    }

    /// Seleciona uma célula; sair de uma linha alterada grava a linha.
    pub fn select(&mut self, row: usize, col: usize, ctx: &egui::Context) {
        if row >= self.rows.len() {
            return;
        }
        self.finish_edit();
        if let Some((current, _)) = self.selected
            && current != row
            && self.rows.get(current).is_some_and(Row::is_dirty)
        {
            self.save(current, ctx);
        }
        self.selected = Some((row, col));
    }

    /// Grava agora a linha selecionada, se mudou.
    pub fn commit(&mut self, ctx: &egui::Context) {
        self.finish_edit();
        if let Some((row, _)) = self.selected
            && self.rows.get(row).is_some_and(Row::is_dirty)
        {
            self.save(row, ctx);
        }
    }

    /// Desfaz as alterações não gravadas da linha.
    pub fn revert_row(&mut self, row: usize) {
        self.editing = None;
        let columns = self.table.as_ref().map_or(0, |t| t.columns.len());
        let last = self.rows.len().saturating_sub(1);
        let Some(r) = self.rows.get_mut(row) else {
            return;
        };
        if r.new {
            if row != last {
                self.rows.remove(row);
                self.selected = Some((row.min(self.rows.len() - 1), 0));
            } else {
                *r = Row::blank(columns);
            }
        } else {
            r.values = r.original.clone();
            r.touched.iter_mut().for_each(|t| *t = false);
            r.error = None;
        }
    }

    /// Pede para excluir a linha; só exclui depois do "Yes".
    pub fn request_delete(&mut self, row: usize) {
        if !self.editable() || self.busy.is_some() {
            return;
        }
        match self.rows.get(row) {
            // A linha em branco do fim não existe no banco.
            Some(r) if r.new && !r.is_dirty() => {}
            Some(_) => self.confirm_delete = Some(row),
            None => {}
        }
    }

    /// Responde a confirmação de exclusão.
    pub fn answer_delete(&mut self, yes: bool, ctx: &egui::Context) {
        let Some(row) = self.confirm_delete.take() else {
            return;
        };
        if !yes {
            return;
        }
        self.editing = None;
        let Some(r) = self.rows.get(row) else {
            return;
        };
        if r.new {
            self.rows.remove(row);
            self.message = Some(tr("1 row deleted.").to_string());
            return;
        }
        let Some(table) = &self.table else {
            return;
        };
        let sql = table.delete_sql(&r.original);
        self.busy = Some(row);
        let tx = self.tx.clone();
        run(&self.session, sql, ctx, move |result, count| {
            let result = match result {
                Err(e) => Err(e),
                Ok(_) if count == 0 => Err(tr(GONE).to_string()),
                Ok(_) => Ok(()),
            };
            let _ = tx.send(Reply::Deleted { row, result });
        });
    }

    /// Fecha a janela; com uma linha alterada, grava antes (no próximo
    /// [`EditRows::step`]) e só fecha se der certo. Devolve se já pode
    /// fechar.
    pub fn request_close(&mut self) -> bool {
        self.finish_edit();
        if self.editable() && self.has_changes() || self.busy.is_some() {
            self.close_after_save = true;
            false
        } else {
            true
        }
    }

    /// Fecha a edição da célula guardando o texto digitado.
    fn finish_edit(&mut self) {
        if let Some((row, col, text)) = self.editing.take() {
            let current = self.value(row, col).map(str::to_string);
            // Texto igual ao valor: nada muda (NULL continua NULL).
            if current.as_deref().unwrap_or("") != text || (current.is_none() && !text.is_empty()) {
                self.set_cell(row, col, Some(text));
            }
        }
    }

    /// Começa a editar a célula, com `text` (ou o valor atual).
    pub fn start_edit(&mut self, row: usize, col: usize, text: Option<String>) {
        if !self.editable() || self.column_read_only(col) || self.busy == Some(row) {
            return;
        }
        let text = text.unwrap_or_else(|| self.value(row, col).unwrap_or("").to_string());
        self.editing = Some((row, col, text));
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// Seleciona a célula sem gravar a linha anterior (screenshots).
    pub fn select_cell(&mut self, row: usize, col: usize) {
        self.selected = Some((row, col));
    }

    /// Tipo JSON da coluna, se ela for `json` ou `jsonb`.
    fn json_kind(&self, col: usize) -> Option<crate::json_viewer::JsonKind> {
        let column = self.table.as_ref()?.columns.get(col)?;
        crate::json_viewer::JsonKind::from_type_name(&column.type_name)
    }

    /// Abre o editor de JSON na célula (só leitura se a célula não for
    /// editável). Devolve se abriu.
    pub fn open_json_editor(&mut self, row: usize, col: usize) -> bool {
        let Some(kind) = self.json_kind(col) else {
            return false;
        };
        let Some(column) = self.table.as_ref().map(|t| t.columns[col].name.clone()) else {
            return false;
        };
        let value = self.value(row, col).unwrap_or("").to_string();
        let id = egui::Id::new(("edit_rows_json", self.oid));
        let editable = self.editable() && !self.column_read_only(col) && self.busy != Some(row);
        let viewer = if editable {
            crate::json_viewer::JsonViewer::editor(id, &column, row, &value, kind)
        } else {
            let Ok(node) = pgstudio_result_grid::json::parse(&value) else {
                return false;
            };
            let mut v = crate::json_viewer::JsonViewer::new(id, &column, row as u64, node);
            v.kind = kind;
            v.set_can_insert(false);
            v
        };
        self.finish_edit();
        self.json_editor = Some((row, col, viewer));
        true
    }

    /// O editor de JSON aberto.
    pub fn json_editor_mut(&mut self) -> Option<&mut crate::json_viewer::JsonViewer> {
        self.json_editor.as_mut().map(|(_, _, v)| v)
    }

    /// Se o editor de JSON pediu o guia (uma vez).
    pub fn take_guide_request(&mut self) -> bool {
        std::mem::take(&mut self.guide_requested)
    }

    /// OK no editor de JSON: grava o valor dele na célula, como se o
    /// usuário tivesse digitado. Devolve se havia um valor válido.
    pub fn json_editor_ok(&mut self) -> bool {
        let Some((row, col, value)) = self
            .json_editor
            .as_ref()
            .and_then(|(r, c, v)| Some((*r, *c, v.ok_value()?)))
        else {
            return false;
        };
        self.save_json(row, col, value);
        true
    }

    fn save_json(&mut self, row: usize, col: usize, text: String) {
        self.json_editor = None;
        let value = (!text.is_empty()).then_some(text);
        if value.as_deref() != self.value(row, col) {
            self.set_cell(row, col, value);
        }
    }

    fn json_window(&mut self, ctx: &egui::Context) {
        let Some((row, col, viewer)) = &mut self.json_editor else {
            return;
        };
        let dark = ctx.global_style().visuals.dark_mode;
        let colors = crate::theme::editor_colors(if dark {
            pgstudio_core::Theme::Dark
        } else {
            pgstudio_core::Theme::Light
        });
        let (row, col) = (*row, *col);
        match viewer.show(ctx, &colors) {
            Some(crate::json_viewer::ViewerAction::Save(text)) => self.save_json(row, col, text),
            Some(crate::json_viewer::ViewerAction::Close) => self.json_editor = None,
            Some(crate::json_viewer::ViewerAction::Guide) => self.guide_requested = true,
            Some(crate::json_viewer::ViewerAction::Insert(_)) | None => {}
        }
    }

    fn save(&mut self, row: usize, ctx: &egui::Context) {
        if self.busy.is_some() {
            return;
        }
        let (Some(table), Some(r)) = (&self.table, self.rows.get(row)) else {
            return;
        };
        let sql = if r.new {
            Some(table.insert_sql(&r.values, &r.touched))
        } else {
            table.update_sql(&r.original, &r.values)
        };
        let Some(sql) = sql else {
            return;
        };
        self.busy = Some(row);
        let tx = self.tx.clone();
        run(&self.session, sql, ctx, move |result, _| {
            let result =
                result.and_then(|rows| rows.into_iter().next().ok_or(tr(GONE).to_string()));
            let _ = tx.send(Reply::Saved { row, result });
        });
    }

    /// Recebe as respostas da sessão.
    pub fn step(&mut self, ctx: &egui::Context) {
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Table(Ok(table)) => {
                    self.table = Some(table);
                    self.load_rows(ctx);
                }
                Reply::Table(Err(e)) | Reply::Rows(Err(e)) => {
                    self.log.push(trf!("Edit Rows: {error}", error = e));
                    self.phase = Phase::Failed(e);
                }
                Reply::Rows(Ok(rows)) => {
                    let columns = self.table.as_ref().map_or(0, |t| t.columns.len());
                    self.rows = rows.into_iter().map(Row::loaded).collect();
                    if self.table.as_ref().is_some_and(EditTable::has_key) {
                        self.rows.push(Row::blank(columns));
                    }
                    self.phase = Phase::Ready;
                    self.selected = self.selected.filter(|(r, _)| *r < self.rows.len());
                    if let Some(reason) = self.read_only_reason() {
                        self.log.push(trf!("Edit Rows: {error}", error = reason));
                    }
                }
                Reply::Saved { row, result } => {
                    self.busy = None;
                    let columns = self.table.as_ref().map_or(0, |t| t.columns.len());
                    let last = self.rows.len().saturating_sub(1);
                    let Some(r) = self.rows.get_mut(row) else {
                        continue;
                    };
                    match result {
                        Ok(values) => {
                            let inserted = r.new;
                            *r = Row::loaded(values);
                            if row == last {
                                self.rows.push(Row::blank(columns));
                            }
                            self.message = Some(
                                if inserted {
                                    tr("1 row inserted.")
                                } else {
                                    tr("1 row updated.")
                                }
                                .to_string(),
                            );
                        }
                        Err(e) => {
                            r.error = Some(e.clone());
                            self.close_after_save = false;
                            self.log.push(trf!(
                                "Edit Rows: the row was not committed. {error} \
                                 Correct the values or press Esc to cancel the changes.",
                                error = e
                            ));
                            self.message =
                                Some(trf!("The row was not committed: {error}", error = e));
                            let col = self.selected.map_or(0, |(_, c)| c);
                            self.selected = Some((row, col));
                        }
                    }
                }
                Reply::Deleted { row, result } => {
                    self.busy = None;
                    match result {
                        Ok(()) => {
                            if row < self.rows.len() {
                                self.rows.remove(row);
                            }
                            self.message = Some(tr("1 row deleted.").to_string());
                            self.selected = self
                                .selected
                                .map(|(r, c)| (r.min(self.rows.len().saturating_sub(1)), c));
                        }
                        Err(e) => {
                            self.log.push(trf!(
                                "Edit Rows: the row was not deleted. {error}",
                                error = e
                            ));
                            self.message =
                                Some(trf!("The row was not deleted: {error}", error = e));
                        }
                    }
                }
            }
        }
        // Fechando: grava a próxima linha alterada; sem nenhuma, fecha.
        if self.close_after_save && self.busy.is_none() {
            match self.rows.iter().position(Row::is_dirty) {
                Some(row) if self.editable() => self.save(row, ctx),
                _ => {
                    self.close_after_save = false;
                    self.closed = true;
                }
            }
        }
    }

    /// Desenha a janela.
    pub fn show(&mut self, ui: &mut egui::Ui, id: u32) {
        let ctx = ui.ctx().clone();
        self.step(&ctx);
        ui.horizontal(|ui| {
            let refresh =
                egui::Button::new(format!("{} {}", icon::ARROWS_CLOCKWISE, tr("Refresh")));
            if ui
                .add_enabled(!self.is_loading() && self.busy.is_none(), refresh)
                .on_hover_text(tr("Read the rows again; unsaved changes are lost"))
                .clicked()
            {
                self.refresh(&ctx);
            }
            if self.is_loading() {
                ui.spinner();
                ui.weak(tr("Loading..."));
            } else if self.busy.is_some() {
                ui.spinner();
                ui.weak(tr("Saving..."));
            } else if let Some(m) = &self.message {
                ui.label(m);
            }
        });
        if let Some(reason) = self.read_only_reason() {
            ui.colored_label(Color32::from_rgb(0xc0, 0x80, 0x00), reason);
        }
        ui.separator();
        if let Some(e) = self.load_error() {
            ui.colored_label(ui.visuals().error_fg_color, e.to_string());
            return;
        }
        if self.table.is_none() {
            return;
        }
        egui::Panel::bottom(egui::Id::new(("edit_rows_status", id)))
            .resizable(false)
            .show(ui, |ui| {
                let n = self.row_count();
                let rows = if n == 1 {
                    tr("1 row").to_string()
                } else {
                    trf!("{rows} rows", rows = n)
                };
                ui.weak(if self.has_changes() {
                    format!("{rows} · {}", tr("pending changes"))
                } else {
                    rows
                });
            });
        self.keys(ui, &ctx);
        self.grid(ui, id, &ctx);
        self.confirm_window(&ctx);
        self.json_window(&ctx);
    }

    /// Teclado fora da edição: setas, F2/Enter, Esc, Ctrl+0, Delete e
    /// texto digitado (começa a editar).
    fn keys(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.confirm_delete.is_some() {
            return;
        }
        let rows = self.rows.len();
        let cols = self.table.as_ref().map_or(0, |t| t.columns.len());
        let Some((row, col)) = self.selected else {
            return;
        };
        if self.editing.is_some() {
            return;
        }
        // Só quando nenhum outro widget tem o foco (o editor de SQL, por
        // exemplo).
        if ui.ctx().memory(|m| m.focused().is_some()) {
            return;
        }
        let (moves, text) = ui.input_mut(|i| {
            let mut moves = Vec::new();
            for (key, m) in [
                (Key::ArrowUp, (-1, 0)),
                (Key::ArrowDown, (1, 0)),
                (Key::ArrowLeft, (0, -1)),
                (Key::ArrowRight, (0, 1)),
            ] {
                if i.consume_key(egui::Modifiers::NONE, key) {
                    moves.push(m);
                }
            }
            let text: String = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect();
            (moves, text)
        });
        for (dr, dc) in moves {
            let r = (row as isize + dr).clamp(0, rows as isize - 1) as usize;
            let c = (col as isize + dc).clamp(0, cols as isize - 1) as usize;
            self.select(r, c, ctx);
        }
        let Some((row, col)) = self.selected else {
            return;
        };
        let (edit, escape, null, delete, enter) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::F2),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
                i.consume_key(egui::Modifiers::COMMAND, Key::Num0),
                i.consume_key(egui::Modifiers::NONE, Key::Delete),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
            )
        });
        if !text.is_empty() {
            self.start_edit(row, col, Some(text));
        } else if edit {
            self.start_edit(row, col, None);
        } else if escape {
            self.revert_row(row);
        } else if null {
            self.set_cell(row, col, None);
        } else if delete {
            self.request_delete(row);
        } else if enter {
            self.commit(ctx);
        }
    }

    fn widths(&self, ui: &egui::Ui) -> Vec<f32> {
        let Some(table) = &self.table else {
            return Vec::new();
        };
        let char_w =
            ui.fonts_mut(|f| f.glyph_width(&egui::TextStyle::Body.resolve(ui.style()), '0'));
        table
            .columns
            .iter()
            .enumerate()
            .map(|(c, col)| {
                let longest = self
                    .rows
                    .iter()
                    .take(200)
                    .map(|r| r.values[c].as_deref().map_or(4, |v| v.chars().count()))
                    .max()
                    .unwrap_or(0)
                    .max(col.name.chars().count() + 2);
                (longest as f32 * char_w + 16.0).clamp(60.0, 320.0)
            })
            .collect()
    }

    fn grid(&mut self, ui: &mut egui::Ui, id: u32, ctx: &egui::Context) {
        let widths = self.widths(ui);
        let Some(table) = self.table.clone() else {
            return;
        };
        let mut clicked: Option<(usize, usize, bool)> = None;
        let mut delete: Option<usize> = None;
        let mut edited: Option<(usize, usize, String, bool)> = None;
        let mut cancel_edit = false;
        let mut json_menu: Option<(usize, usize)> = None;
        let json_cols: Vec<bool> = (0..table.columns.len())
            .map(|c| self.json_kind(c).is_some())
            .collect();
        let editable = self.editable();
        egui::ScrollArea::horizontal()
            .id_salt(("edit_rows_h", id))
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    ui.horizontal(|ui| {
                        ui.add_sized(vec2(MARKER_WIDTH, ROW_HEIGHT), egui::Label::new(""));
                        for (c, col) in table.columns.iter().enumerate() {
                            let text = if col.key {
                                format!("{} {}", icon::KEY, col.name)
                            } else {
                                col.name.clone()
                            };
                            ui.add_sized(
                                vec2(widths[c], ROW_HEIGHT),
                                egui::Label::new(RichText::new(text).strong()).truncate(),
                            )
                            .on_hover_text(&col.type_name);
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt(("edit_rows_v", id))
                        .auto_shrink(false)
                        .show_rows(ui, ROW_HEIGHT, self.rows.len(), |ui, range| {
                            for r in range {
                                ui.horizontal(|ui| {
                                    let state = self.row_state(r);
                                    let current = self.selected.is_some_and(|(s, _)| s == r);
                                    let last_blank = r + 1 == self.rows.len()
                                        && self.rows[r].new
                                        && !self.rows[r].is_dirty();
                                    let marker = match state {
                                        RowState::Error => icon::WARNING_CIRCLE,
                                        _ if last_blank => icon::ASTERISK,
                                        RowState::Modified | RowState::New => icon::PENCIL_SIMPLE,
                                        RowState::Clean if current => icon::CARET_RIGHT,
                                        RowState::Clean => "",
                                    };
                                    let m = ui.add_sized(
                                        vec2(MARKER_WIDTH, ROW_HEIGHT),
                                        egui::Button::new(marker).frame_when_inactive(false),
                                    );
                                    let m = match &self.rows[r].error {
                                        Some(e) => m.on_hover_text(e),
                                        None => m,
                                    };
                                    if editable && !last_blank {
                                        m.context_menu(|ui| {
                                            if ui
                                                .button(format!("{} {}", icon::TRASH, tr("Delete")))
                                                .clicked()
                                            {
                                                delete = Some(r);
                                                ui.close();
                                            }
                                        });
                                    }
                                    for (c, col) in table.columns.iter().enumerate() {
                                        let w = widths[c];
                                        if let Some((er, ec, text)) = &mut self.editing
                                            && *er == r
                                            && *ec == c
                                        {
                                            let edit_id = egui::Id::new(("edit_rows_cell", id));
                                            let resp = ui.add_sized(
                                                vec2(w, ROW_HEIGHT),
                                                egui::TextEdit::singleline(text)
                                                    .id(edit_id)
                                                    .margin(vec2(4.0, 2.0)),
                                            );
                                            if !resp.has_focus() && !resp.lost_focus() {
                                                resp.request_focus();
                                                cursor_to_end(ui.ctx(), edit_id, text);
                                            }
                                            if resp.lost_focus() {
                                                let (enter, esc) = ui.input(|i| {
                                                    (
                                                        i.key_pressed(Key::Enter),
                                                        i.key_pressed(Key::Escape),
                                                    )
                                                });
                                                if esc {
                                                    cancel_edit = true;
                                                } else {
                                                    edited = Some((r, c, text.clone(), enter));
                                                }
                                            }
                                            continue;
                                        }
                                        let value = self.rows[r].values[c].as_deref();
                                        let selected = self.selected == Some((r, c));
                                        let text = match value {
                                            Some(v) => RichText::new(v.replace('\n', " ")),
                                            None if last_blank => RichText::new(""),
                                            None => RichText::new(tr("NULL")).italics().weak(),
                                        };
                                        let text = if col.read_only { text.weak() } else { text };
                                        let (rect, resp) = ui.allocate_exact_size(
                                            vec2(w, ROW_HEIGHT),
                                            Sense::click(),
                                        );
                                        let v = ui.visuals();
                                        if selected {
                                            ui.painter().rect_filled(
                                                rect,
                                                0.0,
                                                v.selection.bg_fill.gamma_multiply(0.6),
                                            );
                                        } else if matches!(
                                            state,
                                            RowState::Modified | RowState::New
                                        ) && self.rows[r].touched[c]
                                        {
                                            ui.painter().rect_filled(
                                                rect,
                                                0.0,
                                                Color32::from_rgba_unmultiplied(
                                                    0xff, 0xc8, 0x00, 40,
                                                ),
                                            );
                                        }
                                        ui.painter().rect_stroke(
                                            rect,
                                            0.0,
                                            egui::Stroke::new(
                                                1.0,
                                                v.widgets.noninteractive.bg_stroke.color,
                                            ),
                                            egui::StrokeKind::Inside,
                                        );
                                        let galley = egui::WidgetText::from(text).into_galley(
                                            ui,
                                            Some(egui::TextWrapMode::Truncate),
                                            w - 8.0,
                                            egui::TextStyle::Body,
                                        );
                                        let pos = egui::pos2(
                                            rect.left() + 4.0,
                                            rect.center().y - galley.size().y / 2.0,
                                        );
                                        ui.painter().galley(pos, galley, v.text_color());
                                        let label = value.unwrap_or("NULL").to_string();
                                        resp.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::WidgetType::Other,
                                                true,
                                                &label,
                                            )
                                        });
                                        if json_cols[c] && !last_blank {
                                            resp.context_menu(|ui| {
                                                let label = if editable && !col.read_only {
                                                    "Edit JSON..."
                                                } else {
                                                    "View JSON..."
                                                };
                                                if ui
                                                    .button(format!(
                                                        "{} {}",
                                                        icon::BRACKETS_CURLY,
                                                        pgstudio_i18n::tr(label)
                                                    ))
                                                    .clicked()
                                                {
                                                    json_menu = Some((r, c));
                                                    ui.close();
                                                }
                                            });
                                        }
                                        if resp.double_clicked() {
                                            clicked = Some((r, c, true));
                                        } else if resp.clicked() {
                                            clicked = Some((r, c, false));
                                        }
                                    }
                                });
                            }
                        });
                });
            });
        if cancel_edit {
            self.editing = None;
        }
        if let Some((r, c, text, enter)) = edited {
            self.editing = Some((r, c, text));
            self.finish_edit();
            if enter {
                // Enter na edição fecha a célula e desce uma linha, como
                // no SSMS (o que grava a linha alterada).
                let next = (r + 1).min(self.rows.len().saturating_sub(1));
                if next != r {
                    self.select(next, c, ctx);
                }
            }
        }
        if let Some((r, c, edit)) = clicked {
            self.select(r, c, ctx);
            // Nas colunas json/jsonb o duplo clique abre o editor de JSON.
            if edit && !self.open_json_editor(r, c) {
                self.start_edit(r, c, None);
            }
        }
        if let Some((r, c)) = json_menu {
            self.select(r, c, ctx);
            self.open_json_editor(r, c);
        }
        if let Some(r) = delete {
            self.request_delete(r);
        }
    }

    /// A linha a excluir como `chave = valor` (a linha nova, que não está
    /// no banco, não tem o que mostrar).
    pub fn confirm_row_text(&self, row: usize) -> Option<String> {
        let table = self.table.as_ref()?;
        let r = self.rows.get(row).filter(|r| !r.new)?;
        Some(
            table
                .columns
                .iter()
                .zip(&r.original)
                .filter(|(c, _)| c.key)
                .map(|(c, v)| format!("{} = {}", c.name, v.as_deref().unwrap_or("NULL")))
                .collect::<Vec<_>>()
                .join(", "),
        )
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        if self.confirm_delete.is_none() {
            return;
        }
        let mut answer = None;
        let row = self.confirm_delete.and_then(|r| self.confirm_row_text(r));
        let modal = crate::theme::modal(egui::Id::new("edit_rows_delete")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(tr("Delete Row"));
            ui.label(tr("You are about to delete 1 row. Are you sure?"));
            if let Some(row) = &row {
                ui.add_space(4.0);
                ui.label(RichText::new(row).monospace());
            }
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(tr("No")).clicked() {
                    answer = Some(false);
                }
                if ui.button(tr("Yes")).clicked() {
                    answer = Some(true);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(yes) = answer {
            self.answer_delete(yes, ctx);
        }
    }
}

/// Põe o cursor da edição no fim do texto (digitar numa célula começa a
/// edição com a letra digitada).
fn cursor_to_end(ctx: &egui::Context, id: egui::Id, text: &str) {
    use egui::text::{CCursor, CCursorRange};
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let end = CCursor::new(text.chars().count());
    state.cursor.set_char_range(Some(CCursorRange::one(end)));
    state.store(ctx, id);
}

/// A linha não estava mais lá (mudou a chave ou foi excluída por outra
/// sessão).
const GONE: &str =
    tr_noop("The row was not found; it may have been changed or deleted by another user.");

/// Roda `sql` e chama `done` uma vez com as linhas do último resultado
/// (ou o erro) e o número de linhas que o último comando leu ou alterou.
pub(crate) fn run(
    session: &Arc<dyn QuerySession>,
    sql: String,
    ctx: &egui::Context,
    done: impl FnOnce(Result<Vec<Vec<Option<String>>>, String>, u64) + Send + 'static,
) {
    let ctx = ctx.clone();
    let mut rows = Vec::new();
    let mut count = 0;
    let mut error: Option<String> = None;
    let mut done = Some(done);
    session.execute(
        sql,
        Box::new(move |event| match event {
            QueryEvent::Columns(_) => rows.clear(),
            QueryEvent::Row(r) => rows.push(r),
            QueryEvent::CommandComplete { rows: n } => count = n,
            QueryEvent::Error(e) => {
                let mut text = e.message;
                if let Some(d) = e.detail {
                    text = format!("{text} ({d})");
                }
                error = Some(text);
            }
            QueryEvent::Notice(_) => {}
            QueryEvent::Finished { canceled } => {
                let result = match error.take() {
                    Some(e) => Err(e),
                    None if canceled => Err(tr("The command was canceled.").to_string()),
                    None => Ok(std::mem::take(&mut rows)),
                };
                if let Some(done) = done.take() {
                    done(result, count);
                }
                ctx.request_repaint();
            }
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> EditTable {
        EditTable {
            engine: Engine::PostgreSql,
            name: "public.\"Pedido\"".into(),
            display: "public.Pedido".into(),
            columns: vec![
                EditColumn {
                    name: "id".into(),
                    type_name: "integer".into(),
                    key: true,
                    read_only: false,
                },
                EditColumn {
                    name: "Nome".into(),
                    type_name: "text".into(),
                    key: false,
                    read_only: false,
                },
                EditColumn {
                    name: "total".into(),
                    type_name: "numeric".into(),
                    key: false,
                    read_only: true,
                },
            ],
        }
    }

    fn v(values: &[Option<&str>]) -> Vec<Option<String>> {
        values.iter().map(|v| v.map(str::to_string)).collect()
    }

    #[test]
    fn literals_escape_quotes_and_backslashes() {
        assert_eq!(literal(None), "NULL");
        assert_eq!(literal(Some("d'água")), "'d''água'");
        assert_eq!(literal(Some(r"c:\temp")), r"E'c:\\temp'");
    }

    #[test]
    fn update_sets_only_changed_editable_columns() {
        let t = table();
        let original = v(&[Some("1"), Some("a"), Some("10")]);
        assert_eq!(t.update_sql(&original, &original), None);
        let values = v(&[Some("1"), None, Some("99")]);
        assert_eq!(
            t.update_sql(&original, &values).unwrap(),
            "UPDATE public.\"Pedido\" SET \"Nome\" = NULL WHERE id = '1' \
             RETURNING id, \"Nome\", total"
        );
    }

    #[test]
    fn a_changed_key_is_found_by_its_original_value() {
        let t = table();
        let original = v(&[Some("1"), Some("a"), None]);
        let values = v(&[Some("2"), Some("a"), None]);
        let sql = t.update_sql(&original, &values).unwrap();
        assert!(sql.contains("SET id = '2' WHERE id = '1'"), "{sql}");
    }

    #[test]
    fn insert_names_only_touched_columns() {
        let t = table();
        let values = v(&[None, Some("b"), Some("5")]);
        assert_eq!(
            t.insert_sql(&values, &[false, true, true]),
            "INSERT INTO public.\"Pedido\" (\"Nome\") VALUES ('b') RETURNING id, \"Nome\", total"
        );
        assert_eq!(
            t.insert_sql(&values, &[false, false, false]),
            "INSERT INTO public.\"Pedido\" DEFAULT VALUES RETURNING id, \"Nome\", total"
        );
    }

    #[test]
    fn delete_uses_the_key() {
        let t = table();
        assert_eq!(
            t.delete_sql(&v(&[Some("7"), Some("x"), None])),
            "DELETE FROM public.\"Pedido\" WHERE id = '7'"
        );
    }

    #[test]
    fn catalog_rows_become_a_table() {
        let row = |name: &str, key: &str, ro: &str| {
            v(&[
                Some(name),
                Some("integer"),
                Some(key),
                Some(ro),
                Some("public.t"),
                Some("public.t"),
            ])
        };
        let t = parse_table(
            Engine::PostgreSql,
            &[row("id", "t", "f"), row("g", "f", "t")],
        )
        .unwrap();
        assert!(t.has_key());
        assert!(t.columns[1].read_only);
        assert_eq!(t.select_sql(200), "SELECT id, g FROM public.t LIMIT 200");
        assert!(parse_table(Engine::PostgreSql, &[]).is_err());
    }

    #[test]
    fn sql_server_reads_the_saved_row_back_by_its_key() {
        let t = EditTable {
            engine: Engine::SqlServer,
            name: "[dbo].[t]".into(),
            display: "dbo.t".into(),
            columns: vec![
                EditColumn {
                    name: "id".into(),
                    type_name: "int".into(),
                    key: true,
                    read_only: true,
                },
                EditColumn {
                    name: "nome".into(),
                    type_name: "nvarchar(40)".into(),
                    key: false,
                    read_only: false,
                },
            ],
        };
        assert_eq!(
            t.select_sql(200),
            "SELECT TOP (200) [id], [nome] FROM [dbo].[t]"
        );
        let tail = "SELECT [id], [nome] FROM [dbo].[t] AS t \
                    WHERE EXISTS (SELECT 1 FROM @k AS k WHERE k.[id] = t.[id]);";
        let o = |v: &[Option<&str>]| v.iter().map(|x| x.map(str::to_string)).collect::<Vec<_>>();
        assert_eq!(
            t.update_sql(&o(&[Some("7"), Some("a")]), &o(&[Some("7"), Some("b")]))
                .unwrap(),
            format!(
                "DECLARE @k TABLE ([id] int);\n\
                 UPDATE [dbo].[t] SET [nome] = N'b' OUTPUT inserted.[id] INTO @k \
                 WHERE [id] = N'7';\n{tail}"
            )
        );
        assert_eq!(
            t.insert_sql(&o(&[None, Some("x")]), &[false, true]),
            format!(
                "DECLARE @k TABLE ([id] int);\n\
                 INSERT INTO [dbo].[t] ([nome]) OUTPUT inserted.[id] INTO @k VALUES (N'x');\n{tail}"
            )
        );
        assert!(
            t.insert_sql(&o(&[None, None]), &[false, false])
                .contains("OUTPUT inserted.[id] INTO @k DEFAULT VALUES")
        );
    }
}
