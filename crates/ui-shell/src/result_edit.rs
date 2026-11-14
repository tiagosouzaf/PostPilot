//! Edição direto na grade de resultados de uma consulta.
//!
//! Quando o lote é um comando só, que lê de uma tabela com chave primária
//! e traz a chave inteira, o resultado fica editável: dois cliques (ou F2)
//! numa célula editam ali mesmo, Enter grava (`UPDATE` pela chave), Esc
//! desiste; Delete nas linhas selecionadas pede confirmação e exclui. Para
//! saber de onde vem cada coluna, o servidor prepara o comando (sem rodar)
//! e diz a tabela e a coluna de origem; o catálogo diz a chave primária e
//! quais colunas o banco calcula. JOIN, agregação, expressões, views e
//! tabelas sem chave ficam só leitura, com o motivo.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::{self, Align, Layout, RichText};
use pgstudio_core::query::{ColumnSource, QuerySession};
use pgstudio_core::schema::quote_ident;
use pgstudio_result_grid::ResultStore;
use pgstudio_result_grid::grid::{Cell, GridOutput, GridState};

use crate::edit_rows::{literal, run};

/// Mais linhas que isso num Delete só: pede para excluir por SQL.
pub const MAX_DELETE_ROWS: u64 = 10_000;

/// Linhas mostradas na confirmação de exclusão.
const PREVIEW_ROWS: usize = 5;

/// Coluna da tabela de onde sai uma coluna do resultado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetColumn {
    pub name: String,
    pub attnum: i16,
    /// O banco calcula (gerada ou identidade `ALWAYS`).
    pub read_only: bool,
}

/// A tabela que o resultado edita.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// Nome qualificado e com aspas, para o SQL.
    pub table: String,
    /// `schema.tabela`, para mostrar.
    pub display: String,
    /// Para cada coluna do resultado, a coluna da tabela (ou `None` se é
    /// expressão).
    pub columns: Vec<Option<TargetColumn>>,
    /// Colunas do resultado com a chave primária, na ordem da chave.
    pub key: Vec<usize>,
}

impl Target {
    /// Colunas do resultado que dá para editar.
    pub fn editable(&self) -> Vec<bool> {
        self.columns
            .iter()
            .map(|c| c.as_ref().is_some_and(|c| !c.read_only))
            .collect()
    }

    /// `WHERE` que acha a linha pelos valores da chave em `row`.
    fn key_filter(&self, row: &[Option<String>]) -> String {
        self.key
            .iter()
            .map(|&i| {
                let name = self.columns[i].as_ref().map_or("", |c| &c.name);
                match row.get(i).cloned().flatten() {
                    Some(v) => format!("{} = {}", quote_ident(name), literal(Some(&v))),
                    None => format!("{} IS NULL", quote_ident(name)),
                }
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// UPDATE de uma célula: a coluna `col` do resultado recebe `value`
    /// na linha (achada pela chave em `row`). `None` se a coluna não é
    /// editável.
    pub fn update_sql(
        &self,
        row: &[Option<String>],
        col: usize,
        value: Option<&str>,
    ) -> Option<String> {
        let c = self.columns.get(col)?.as_ref().filter(|c| !c.read_only)?;
        let name = quote_ident(&c.name);
        Some(format!(
            "UPDATE {} SET {name} = {} WHERE {} RETURNING {name}",
            self.table,
            literal(value),
            self.key_filter(row)
        ))
    }

    /// DELETE das linhas (pela chave), devolvendo a chave das excluídas.
    pub fn delete_sql(&self, rows: &[Vec<Option<String>>]) -> String {
        let filter = if rows.len() == 1 {
            self.key_filter(&rows[0])
        } else {
            rows.iter()
                .map(|r| format!("({})", self.key_filter(r)))
                .collect::<Vec<_>>()
                .join("\n   OR ")
        };
        format!(
            "DELETE FROM {} WHERE {filter} RETURNING {}",
            self.table,
            self.key_columns()
        )
    }

    fn key_columns(&self) -> String {
        self.key
            .iter()
            .filter_map(|&i| self.columns[i].as_ref())
            .map(|c| quote_ident(&c.name))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Valores da chave numa linha do resultado.
    fn key_of(&self, row: &[Option<String>]) -> Vec<Option<String>> {
        self.key
            .iter()
            .map(|&i| row.get(i).cloned().flatten())
            .collect()
    }

    /// Colunas do resultado que vêm da mesma coluna da tabela que `col`
    /// (`SELECT nome, nome`): todas mudam juntas.
    fn same_column(&self, col: usize) -> Vec<usize> {
        let Some(attnum) = self
            .columns
            .get(col)
            .and_then(|c| c.as_ref())
            .map(|c| c.attnum)
        else {
            return vec![col];
        };
        (0..self.columns.len())
            .filter(|&i| self.columns[i].as_ref().is_some_and(|c| c.attnum == attnum))
            .collect()
    }
}

/// A tabela das colunas, se o resultado inteiro vem de uma tabela só; se
/// não, por que não dá para editar.
pub fn source_table(sources: &[Option<ColumnSource>]) -> Result<u32, String> {
    let tables: HashSet<u32> = sources.iter().flatten().map(|s| s.table).collect();
    match tables.len() {
        0 => Err(
            "The results are read-only: no column comes straight from a table \
                  (expressions, views or functions)."
                .into(),
        ),
        1 => Ok(*tables.iter().next().unwrap_or(&0)),
        _ => Err(
            "The results are read-only: they come from more than one table (a JOIN). \
                  Use Edit Top 200 Rows on the table instead."
                .into(),
        ),
    }
}

/// Colunas, chave e nome da tabela `oid`, com o tipo de relação.
pub fn table_sql(oid: u32) -> String {
    format!(
        "SELECT a.attnum, a.attname, \
                coalesce(a.attnum = ANY (i.indkey), false), \
                a.attgenerated <> '' OR a.attidentity = 'a', \
                format('%I.%I', n.nspname, c.relname), \
                n.nspname || '.' || c.relname, \
                c.relkind, \
                i.indexrelid IS NOT NULL \
           FROM pg_class c \
           JOIN pg_namespace n ON n.oid = c.relnamespace \
           JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
           LEFT JOIN pg_index i ON i.indrelid = c.oid AND i.indisprimary \
          WHERE c.oid = {oid} \
          ORDER BY a.attnum"
    )
}

/// Monta o alvo da edição com as linhas de [`table_sql`] e a origem de
/// cada coluna do resultado.
pub fn build_target(
    catalog: &[Vec<Option<String>>],
    sources: &[Option<ColumnSource>],
    table: u32,
) -> Result<Target, String> {
    let text = |r: &Vec<Option<String>>, i: usize| r.get(i).cloned().flatten().unwrap_or_default();
    let first = catalog
        .first()
        .ok_or_else(|| "The results are read-only: the table no longer exists.".to_string())?;
    let display = text(first, 5);
    if !matches!(text(first, 6).as_str(), "r" | "p") {
        return Err(format!(
            "The results are read-only: {display} is not a table."
        ));
    }
    if text(first, 7) != "t" {
        return Err(format!(
            "The results are read-only: {display} has no primary key, so its rows \
             cannot be identified."
        ));
    }
    struct Attr {
        attnum: i16,
        name: String,
        key: bool,
        read_only: bool,
    }
    let attrs: Vec<Attr> = catalog
        .iter()
        .map(|r| Attr {
            attnum: text(r, 0).parse().unwrap_or(0),
            name: text(r, 1),
            key: text(r, 2) == "t",
            read_only: text(r, 3) == "t",
        })
        .collect();
    let columns: Vec<Option<TargetColumn>> = sources
        .iter()
        .map(|s| {
            let s = s.filter(|s| s.table == table)?;
            let a = attrs.iter().find(|a| a.attnum == s.column)?;
            Some(TargetColumn {
                name: a.name.clone(),
                attnum: a.attnum,
                read_only: a.read_only,
            })
        })
        .collect();
    let mut key = Vec::new();
    let mut missing = Vec::new();
    for a in attrs.iter().filter(|a| a.key) {
        let at: Vec<usize> = (0..columns.len())
            .filter(|&i| columns[i].as_ref().is_some_and(|c| c.attnum == a.attnum))
            .collect();
        match at.as_slice() {
            [] => missing.push(a.name.clone()),
            [i] => key.push(*i),
            // A mesma chave duas vezes: pode ser a tabela com ela mesma.
            _ => {
                return Err(format!(
                    "The results are read-only: the primary key column {} appears more \
                     than once.",
                    a.name
                ));
            }
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "The results are read-only: they do not include the primary key of {display} \
             ({}). Add it to the SELECT list to edit.",
            missing.join(", ")
        ));
    }
    Ok(Target {
        table: text(first, 4),
        display,
        columns,
        key,
    })
}

/// Linhas esperando o "Yes" para excluir.
#[derive(Clone, Debug)]
struct ConfirmDelete {
    rows: Range<u64>,
    /// Algumas linhas, como `coluna = valor, ...`, para mostrar o que sai.
    preview: Vec<String>,
}

/// O que volta da sessão.
enum Reply {
    Sources(Result<Vec<Option<ColumnSource>>, String>),
    Catalog(Result<Vec<Vec<Option<String>>>, String>),
    Updated {
        cell: Cell,
        text: Option<String>,
        result: Result<Option<String>, String>,
    },
    Deleted {
        rows: Range<u64>,
        keys: Vec<Vec<Option<String>>>,
        result: Result<Vec<Vec<Option<String>>>, String>,
    },
}

/// Situação da edição do resultado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Perguntando ao servidor de onde vêm as colunas.
    Checking,
    Editable,
    ReadOnly(String),
}

/// A edição de um resultado: descobre se dá para editar e grava o que o
/// usuário muda na grade.
pub struct ResultEditor {
    session: Arc<dyn QuerySession>,
    ctx: egui::Context,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    status: Status,
    sources: Vec<Option<ColumnSource>>,
    table: u32,
    target: Option<Target>,
    /// Gravando ou excluindo.
    busy: bool,
    confirm: Option<ConfirmDelete>,
    /// O que aconteceu por último (ou por que não dá para editar, depois
    /// de uma tentativa).
    message: Option<String>,
    /// Mensagens para a aba Messages.
    log: Vec<String>,
}

impl ResultEditor {
    /// Começa a ver se o resultado de `sql` (um comando só, já executado
    /// na `session`) pode ser editado.
    pub fn new(session: Arc<dyn QuerySession>, sql: &str, ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let e = ResultEditor {
            session,
            ctx: ctx.clone(),
            tx,
            rx,
            status: Status::Checking,
            sources: Vec::new(),
            table: 0,
            target: None,
            busy: false,
            confirm: None,
            message: None,
            log: Vec::new(),
        };
        let tx = e.tx.clone();
        let repaint = ctx.clone();
        e.session.describe(
            sql.trim().trim_end_matches(';').to_string(),
            Box::new(move |r| {
                let _ = tx.send(Reply::Sources(r));
                repaint.request_repaint();
            }),
        );
        e
    }

    /// Resultado que nunca é editável, com o motivo.
    pub fn read_only(session: Arc<dyn QuerySession>, reason: &str, ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        ResultEditor {
            session,
            ctx: ctx.clone(),
            tx,
            rx,
            status: Status::ReadOnly(reason.to_string()),
            sources: Vec::new(),
            table: 0,
            target: None,
            busy: false,
            confirm: None,
            message: None,
            log: Vec::new(),
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn target(&self) -> Option<&Target> {
        self.target.as_ref()
    }

    pub fn is_busy(&self) -> bool {
        self.busy || self.status == Status::Checking
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn is_confirming(&self) -> bool {
        self.confirm.is_some()
    }

    /// Prévia das linhas da confirmação de exclusão.
    pub fn confirm_preview(&self) -> Option<(u64, &[String])> {
        self.confirm
            .as_ref()
            .map(|c| (c.rows.end - c.rows.start, c.preview.as_slice()))
    }

    /// Mensagens para a aba Messages desde a última chamada.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// Recebe as respostas do servidor e mantém a grade em dia (colunas
    /// editáveis, valores gravados, linhas excluídas).
    pub fn step(&mut self, store: &mut ResultStore, grid: &mut GridState) {
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Sources(Ok(sources)) => match source_table(&sources) {
                    Ok(table) => {
                        self.sources = sources;
                        self.table = table;
                        let tx = self.tx.clone();
                        run(&self.session, table_sql(table), &self.ctx, move |r, _| {
                            let _ = tx.send(Reply::Catalog(r));
                        });
                    }
                    Err(reason) => self.status = Status::ReadOnly(reason),
                },
                Reply::Sources(Err(e)) => {
                    self.status = Status::ReadOnly(format!(
                        "The results are read-only: the query could not be described ({e})."
                    ));
                }
                Reply::Catalog(Ok(rows)) => match build_target(&rows, &self.sources, self.table) {
                    Ok(target) => {
                        self.target = Some(target);
                        self.status = Status::Editable;
                    }
                    Err(reason) => self.status = Status::ReadOnly(reason),
                },
                Reply::Catalog(Err(e)) => {
                    self.status = Status::ReadOnly(format!("The results are read-only: {e}"));
                }
                Reply::Updated { cell, text, result } => {
                    self.busy = false;
                    match result {
                        Ok(value) => {
                            let cols = self
                                .target
                                .as_ref()
                                .map_or(vec![cell.col], |t| t.same_column(cell.col));
                            for c in cols {
                                store.set_cell(cell.row, c, value.clone());
                            }
                            self.message = Some("1 row updated.".into());
                            self.log.push("(1 row affected)".into());
                        }
                        Err(e) => {
                            self.log.push(format!("The value was not saved. {e}"));
                            self.message = Some(format!("The value was not saved: {e}"));
                            // Volta a editar com o que foi digitado, para
                            // corrigir (Esc desiste).
                            if let Some(text) = text {
                                grid.reopen_edit(cell, text);
                            }
                        }
                    }
                }
                Reply::Deleted { rows, keys, result } => {
                    self.busy = false;
                    match result {
                        Ok(deleted) => {
                            let deleted: HashSet<Vec<Option<String>>> =
                                deleted.into_iter().collect();
                            let mut removed = 0;
                            // De baixo para cima, para os índices valerem.
                            for (i, key) in keys.iter().enumerate().rev() {
                                if deleted.contains(key) {
                                    store.remove_row(rows.start + i as u64);
                                    removed += 1;
                                }
                            }
                            let asked = keys.len();
                            let text = if removed == asked {
                                plural_rows(removed, "deleted")
                            } else {
                                format!(
                                    "{} of {asked}: the others were not found; they may have \
                                     been changed or deleted by another user.",
                                    plural_rows(removed, "deleted").trim_end_matches('.')
                                )
                            };
                            self.log
                                .push(crate::execution::rows_affected(removed as u64));
                            self.message = Some(text);
                            grid.selection = None;
                        }
                        Err(e) => {
                            self.log.push(format!("The rows were not deleted. {e}"));
                            self.message = Some(format!("The rows were not deleted: {e}"));
                        }
                    }
                }
            }
        }
        grid.busy = self.is_busy();
        match &self.status {
            Status::Editable => {
                if let Some(t) = &self.target {
                    grid.editable = t.editable();
                    grid.read_only_reason = Some(
                        "This column is computed by the database (or is an expression) and \
                         cannot be edited."
                            .into(),
                    );
                }
            }
            Status::ReadOnly(reason) => {
                grid.editable.clear();
                grid.read_only_reason = Some(reason.clone());
            }
            Status::Checking => {
                grid.read_only_reason =
                    Some("Checking whether the results can be edited...".into());
            }
        }
    }

    /// Trata o que o usuário fez na grade neste frame.
    pub fn handle(&mut self, out: &GridOutput, store: &mut ResultStore, grid: &GridState) {
        if out.edit_refused {
            self.message = grid.read_only_reason.clone();
        }
        if let Some((cell, value)) = &out.commit {
            self.update(store, *cell, value.clone());
        }
        if let Some(rows) = out.delete_rows.clone() {
            self.request_delete(store, rows);
        }
    }

    /// Grava o valor de uma célula.
    pub fn update(&mut self, store: &mut ResultStore, cell: Cell, value: Option<String>) {
        if self.busy {
            return;
        }
        let Some(target) = &self.target else {
            return;
        };
        let row = store.row(cell.row);
        let Some(sql) = target.update_sql(&row, cell.col, value.as_deref()) else {
            return;
        };
        self.busy = true;
        self.message = Some("Saving...".into());
        let tx = self.tx.clone();
        run(&self.session, sql, &self.ctx, move |result, count| {
            let result = match result {
                Err(e) => Err(e),
                Ok(_) if count == 0 => Err(GONE.to_string()),
                Ok(rows) => Ok(rows
                    .into_iter()
                    .next()
                    .and_then(|r| r.into_iter().next().flatten())),
            };
            let _ = tx.send(Reply::Updated {
                cell,
                text: value.map(|v| v.to_string()),
                result,
            });
        });
    }

    /// Pede para excluir as linhas; só exclui depois do "Yes".
    pub fn request_delete(&mut self, store: &mut ResultStore, rows: Range<u64>) {
        if self.busy || rows.is_empty() {
            return;
        }
        let Some(target) = &self.target else {
            return;
        };
        let count = rows.end - rows.start;
        if count > MAX_DELETE_ROWS {
            self.message = Some(format!(
                "Select at most {MAX_DELETE_ROWS} rows to delete here; use a DELETE \
                 statement for more."
            ));
            return;
        }
        let preview = rows
            .clone()
            .take(PREVIEW_ROWS)
            .map(|r| {
                let row = store.row(r);
                target
                    .key
                    .iter()
                    .map(|&i| {
                        let name = target.columns[i].as_ref().map_or("", |c| &c.name);
                        let value = row[i].as_deref().unwrap_or("NULL");
                        format!("{name} = {value}")
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .collect();
        self.confirm = Some(ConfirmDelete { rows, preview });
    }

    /// Responde a confirmação de exclusão.
    pub fn answer_delete(&mut self, yes: bool, store: &mut ResultStore) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        if !yes || self.busy {
            return;
        }
        let Some(target) = &self.target else {
            return;
        };
        let rows: Vec<Vec<Option<String>>> = confirm.rows.clone().map(|r| store.row(r)).collect();
        let keys: Vec<Vec<Option<String>>> = rows.iter().map(|r| target.key_of(r)).collect();
        let sql = target.delete_sql(&rows);
        self.busy = true;
        self.message = Some("Deleting...".into());
        let tx = self.tx.clone();
        let range = confirm.rows;
        run(&self.session, sql, &self.ctx, move |result, _| {
            let _ = tx.send(Reply::Deleted {
                rows: range,
                keys,
                result,
            });
        });
    }

    /// A confirmação de exclusão, quando pedida.
    pub fn confirm_window(&mut self, ctx: &egui::Context, store: &mut ResultStore, id: egui::Id) {
        let Some(confirm) = &self.confirm else {
            return;
        };
        let count = confirm.rows.end - confirm.rows.start;
        let table = self.target.as_ref().map_or("", |t| t.display.as_str());
        let mut answer = None;
        let modal = egui::Modal::new(id.with("delete_rows")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(if count == 1 {
                "Delete Row"
            } else {
                "Delete Rows"
            });
            ui.label(format!(
                "You are about to delete {} from {table}. Are you sure?",
                if count == 1 {
                    "1 row".to_string()
                } else {
                    format!("{count} rows")
                }
            ));
            ui.add_space(4.0);
            for line in &confirm.preview {
                ui.label(RichText::new(line).monospace());
            }
            if count > confirm.preview.len() as u64 {
                ui.weak(format!(
                    "... and {} more",
                    count - confirm.preview.len() as u64
                ));
            }
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // "No" primeiro (à direita) e com o foco: Enter não exclui
                // sem querer.
                let no = ui.button("No");
                if no.clicked() {
                    answer = Some(false);
                }
                if ui.button("Yes").clicked() {
                    answer = Some(true);
                }
                if !ui.ctx().memory(|m| m.focused().is_some()) {
                    no.request_focus();
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(yes) = answer {
            self.answer_delete(yes, store);
        }
    }
}

fn plural_rows(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 row {what}.")
    } else {
        format!("{n} rows {what}.")
    }
}

/// A linha não estava mais lá.
const GONE: &str = "The row was not found; it may have been changed or deleted by another user.";

#[cfg(test)]
mod tests {
    use super::*;

    fn src(table: u32, column: i16) -> Option<ColumnSource> {
        Some(ColumnSource { table, column })
    }

    fn catalog(
        rows: &[(&str, &str, bool, bool)],
        kind: &str,
        has_pk: bool,
    ) -> Vec<Vec<Option<String>>> {
        rows.iter()
            .map(|(num, name, key, ro)| {
                vec![
                    Some(num.to_string()),
                    Some(name.to_string()),
                    Some(if *key { "t" } else { "f" }.into()),
                    Some(if *ro { "t" } else { "f" }.into()),
                    Some("public.\"Pedido\"".into()),
                    Some("public.Pedido".into()),
                    Some(kind.into()),
                    Some(if has_pk { "t" } else { "f" }.into()),
                ]
            })
            .collect()
    }

    fn pedido() -> Vec<Vec<Option<String>>> {
        catalog(
            &[
                ("1", "id", true, false),
                ("2", "Nome", false, false),
                ("3", "total", false, true),
            ],
            "r",
            true,
        )
    }

    #[test]
    fn one_table_or_a_reason() {
        assert_eq!(source_table(&[src(5, 1), None, src(5, 2)]), Ok(5));
        assert!(
            source_table(&[None, None])
                .unwrap_err()
                .contains("no column")
        );
        assert!(
            source_table(&[src(5, 1), src(6, 1)])
                .unwrap_err()
                .contains("JOIN")
        );
    }

    #[test]
    fn target_maps_result_columns_to_the_table() {
        // SELECT "Nome", id, total, now() FROM "Pedido"
        let sources = [src(9, 2), src(9, 1), src(9, 3), None];
        let t = build_target(&pedido(), &sources, 9).unwrap();
        assert_eq!(t.key, vec![1]);
        assert_eq!(t.editable(), vec![true, true, false, false]);
        let row = vec![
            Some("d'água".into()),
            Some("7".into()),
            Some("1".into()),
            None,
        ];
        assert_eq!(
            t.update_sql(&row, 0, Some("x")).unwrap(),
            "UPDATE public.\"Pedido\" SET \"Nome\" = 'x' WHERE id = '7' RETURNING \"Nome\""
        );
        assert_eq!(t.update_sql(&row, 2, Some("1")), None);
        assert_eq!(t.update_sql(&row, 3, Some("1")), None);
        assert_eq!(
            t.delete_sql(std::slice::from_ref(&row)),
            "DELETE FROM public.\"Pedido\" WHERE id = '7' RETURNING id"
        );
        let other = vec![None, Some("8".into()), None, None];
        assert_eq!(
            t.delete_sql(&[row, other]),
            "DELETE FROM public.\"Pedido\" WHERE (id = '7')\n   OR (id = '8') RETURNING id"
        );
    }

    #[test]
    fn read_only_reasons() {
        // Sem a chave no SELECT.
        let e = build_target(&pedido(), &[src(9, 2)], 9).unwrap_err();
        assert!(e.contains("primary key") && e.contains("(id)"), "{e}");
        // A chave duas vezes (a tabela com ela mesma).
        let e = build_target(&pedido(), &[src(9, 1), src(9, 1)], 9).unwrap_err();
        assert!(e.contains("more than once"), "{e}");
        // Sem chave primária.
        let no_pk = catalog(&[("1", "id", false, false)], "r", false);
        let e = build_target(&no_pk, &[src(9, 1)], 9).unwrap_err();
        assert!(e.contains("no primary key"), "{e}");
        // Não é tabela.
        let view = catalog(&[("1", "id", false, false)], "m", false);
        assert!(
            build_target(&view, &[src(9, 1)], 9)
                .unwrap_err()
                .contains("not a table")
        );
        assert!(build_target(&[], &[src(9, 1)], 9).is_err());
    }

    #[test]
    fn repeated_columns_change_together() {
        let sources = [src(9, 1), src(9, 2), src(9, 2)];
        let t = build_target(&pedido(), &sources, 9).unwrap();
        assert_eq!(t.same_column(1), vec![1, 2]);
        assert_eq!(t.same_column(0), vec![0]);
    }
}
