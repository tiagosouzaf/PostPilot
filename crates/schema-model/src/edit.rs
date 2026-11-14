//! Edições do modelo com desfazer e refazer.
//!
//! Cada edição de [`Document`] valida o pedido, aplica no modelo e guarda o
//! inverso no [`History`]. Por dentro, uma edição é uma lista de operações
//! sobre tabelas inteiras ([`Op`]): inserir, remover ou trocar. Assim o
//! desfazer não precisa conhecer cada tipo de edição, e uma edição que mexe
//! em várias tabelas (renomear uma tabela muda as FKs que apontam para ela)
//! se desfaz de uma vez.

use std::fmt;

use crate::{
    Check, Column, ForeignKey, Id, Index, Key, Model, Table, default_constraint_name,
    rename_in_expr,
};

/// Pedido de edição recusado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditError(pub String);

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EditError {}

fn err<T>(msg: impl Into<String>) -> Result<T, EditError> {
    Err(EditError(msg.into()))
}

/// Operação sobre uma tabela inteira.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Insere a tabela nesta posição da lista.
    Insert(usize, Table),
    /// Remove a tabela com este id.
    Remove(Id),
    /// Troca a tabela de mesmo id por esta.
    Replace(Table),
}

/// Aplica `op` e devolve a operação que desfaz.
fn apply(model: &mut Model, op: Op) -> Op {
    match op {
        Op::Insert(at, table) => {
            let id = table.id;
            model.tables.insert(at.min(model.tables.len()), table);
            Op::Remove(id)
        }
        Op::Remove(id) => {
            let at = model
                .tables
                .iter()
                .position(|t| t.id == id)
                .expect("a operação foi montada a partir do modelo");
            Op::Insert(at, model.tables.remove(at))
        }
        Op::Replace(table) => {
            let slot = model
                .table_mut(table.id)
                .expect("a operação foi montada a partir do modelo");
            Op::Replace(std::mem::replace(slot, table))
        }
    }
}

/// Uma edição: nome para o menu (Undo ...) e operações.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub label: String,
    pub ops: Vec<Op>,
}

impl Change {
    /// Aplica e devolve o inverso.
    fn apply(self, model: &mut Model) -> Change {
        let mut inverse: Vec<Op> = self.ops.into_iter().map(|op| apply(model, op)).collect();
        inverse.reverse();
        Change {
            label: self.label,
            ops: inverse,
        }
    }
}

/// Pilhas de desfazer e refazer.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Change>,
    redo: Vec<Change>,
}

impl History {
    /// Aplica uma edição nova; o refazer se perde, como em todo editor.
    pub fn apply(&mut self, model: &mut Model, change: Change) {
        if change.ops.is_empty() {
            return;
        }
        let inverse = change.apply(model);
        self.undo.push(inverse);
        self.redo.clear();
    }

    /// Desfaz a última edição e devolve o nome dela.
    pub fn undo(&mut self, model: &mut Model) -> Option<String> {
        let change = self.undo.pop()?;
        let redo = change.apply(model);
        let label = redo.label.clone();
        self.redo.push(redo);
        Some(label)
    }

    /// Refaz a última edição desfeita e devolve o nome dela.
    pub fn redo(&mut self, model: &mut Model) -> Option<String> {
        let change = self.redo.pop()?;
        let undo = change.apply(model);
        let label = undo.label.clone();
        self.undo.push(undo);
        Some(label)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|c| c.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|c| c.label.as_str())
    }

    /// Se há edições desde o último [`History::clear`] (para o `*` da aba).
    pub fn is_dirty(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Esquece as edições (depois de salvar no banco).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

/// O modelo e as edições feitas nele.
#[derive(Clone, Debug, Default)]
pub struct Document {
    pub model: Model,
    pub history: History,
}

impl Document {
    pub fn new(model: Model) -> Self {
        Document {
            model,
            history: History::default(),
        }
    }

    pub fn undo(&mut self) -> Option<String> {
        self.history.undo(&mut self.model)
    }

    pub fn redo(&mut self) -> Option<String> {
        self.history.redo(&mut self.model)
    }

    /// Roda `edit` numa cópia do modelo e grava as tabelas que mudaram como
    /// uma edição. Se `edit` recusar, nada muda.
    fn change<T>(
        &mut self,
        label: impl Into<String>,
        edit: impl FnOnce(&mut Model) -> Result<T, EditError>,
    ) -> Result<T, EditError> {
        let mut after = self.model.clone();
        let out = edit(&mut after)?;
        let before = &self.model;
        let mut ops = Vec::new();
        for t in &before.tables {
            match after.table(t.id) {
                None => ops.push(Op::Remove(t.id)),
                Some(new) if new != t => ops.push(Op::Replace(new.clone())),
                Some(_) => {}
            }
        }
        for (at, t) in after.tables.iter().enumerate() {
            if before.table(t.id).is_none() {
                ops.push(Op::Insert(at, t.clone()));
            }
        }
        // Os ids dados durante a edição continuam reservados mesmo se ela
        // for desfeita.
        self.model.next_id = after.next_id;
        let change = Change {
            label: label.into(),
            ops,
        };
        self.history.apply(&mut self.model, change);
        Ok(out)
    }

    /// Cria uma tabela com uma coluna `id` identity e chave primária, como o
    /// SSMS sugere, e devolve o id dela.
    pub fn add_table(&mut self, schema: &str, name: &str) -> Result<Id, EditError> {
        self.change(format!("Add Table {name}"), |m| {
            check_name(name)?;
            if m.find(schema, name).is_some() {
                return err(format!("A table named {schema}.{name} already exists."));
            }
            let id = m.new_id();
            let column = m.new_id();
            m.tables.push(Table {
                id,
                schema: schema.into(),
                name: name.into(),
                columns: vec![Column {
                    id: column,
                    name: "id".into(),
                    ty: "integer".into(),
                    nullable: false,
                    identity: Some(crate::Identity::Always),
                    ..Default::default()
                }],
                primary_key: Some(Key {
                    name: default_constraint_name(name, &[], "pkey"),
                    columns: vec!["id".into()],
                }),
                ..Default::default()
            });
            Ok(id)
        })
    }

    /// Renomeia ou muda o schema de uma tabela; as FKs que apontam para ela
    /// acompanham.
    pub fn rename_table(&mut self, table: Id, schema: &str, name: &str) -> Result<(), EditError> {
        self.change(format!("Rename Table {name}"), |m| {
            check_name(name)?;
            let t = get(m, table)?;
            let (old_schema, old_name) = (t.schema.clone(), t.name.clone());
            if (old_schema.as_str(), old_name.as_str()) == (schema, name) {
                return Ok(());
            }
            if m.find(schema, name).is_some() {
                return err(format!("A table named {schema}.{name} already exists."));
            }
            for t in &mut m.tables {
                for fk in &mut t.foreign_keys {
                    if fk.ref_schema == old_schema && fk.ref_table == old_name {
                        fk.ref_schema = schema.into();
                        fk.ref_table = name.into();
                    }
                }
            }
            let t = get_mut(m, table)?;
            t.schema = schema.into();
            t.name = name.into();
            Ok(())
        })
    }

    /// Apaga uma tabela e as FKs de outras tabelas que apontam para ela.
    pub fn drop_table(&mut self, table: Id) -> Result<(), EditError> {
        let name = get(&self.model, table)?.name.clone();
        self.change(format!("Delete Table {name}"), |m| {
            let t = get(m, table)?;
            let (schema, name) = (t.schema.clone(), t.name.clone());
            m.tables.retain(|t| t.id != table);
            for t in &mut m.tables {
                t.foreign_keys
                    .retain(|fk| !(fk.ref_schema == schema && fk.ref_table == name));
            }
            Ok(())
        })
    }

    /// Acrescenta uma coluna no fim da tabela e devolve o id dela.
    pub fn add_column(&mut self, table: Id, column: Column) -> Result<Id, EditError> {
        self.change(format!("Add Column {}", column.name), |m| {
            check_name(&column.name)?;
            check_type(&column.ty)?;
            let id = m.new_id();
            let t = get_mut(m, table)?;
            if t.column(&column.name).is_some() {
                return err(format!("Column {} already exists.", column.name));
            }
            t.columns.push(Column { id, ..column });
            Ok(id)
        })
    }

    /// Renomeia uma coluna nas chaves, índices e FKs (desta tabela e das que
    /// apontam para ela).
    pub fn rename_column(&mut self, table: Id, column: Id, name: &str) -> Result<(), EditError> {
        self.change(format!("Rename Column {name}"), |m| {
            check_name(name)?;
            let t = get(m, table)?;
            let old = column_of(t, column)?.name.clone();
            if old == name {
                return Ok(());
            }
            if t.column(name).is_some() {
                return err(format!("Column {name} already exists."));
            }
            let (schema, tname) = (t.schema.clone(), t.name.clone());
            let rename = |cols: &mut Vec<String>| {
                for c in cols.iter_mut().filter(|c| **c == old) {
                    *c = name.to_string();
                }
            };
            for other in &mut m.tables {
                for fk in &mut other.foreign_keys {
                    if fk.ref_schema == schema && fk.ref_table == tname {
                        rename(&mut fk.ref_columns);
                    }
                }
            }
            let t = get_mut(m, table)?;
            for key in t.primary_key.iter_mut().chain(t.uniques.iter_mut()) {
                rename(&mut key.columns);
            }
            for fk in &mut t.foreign_keys {
                rename(&mut fk.columns);
            }
            let expr = |e: &mut String| *e = rename_in_expr(e, &old, name);
            for index in &mut t.indexes {
                for c in &mut index.columns {
                    if *c == old {
                        *c = name.to_string();
                    } else if c.contains(['(', ' ', '"']) {
                        expr(c);
                    }
                }
                index.predicate.iter_mut().for_each(expr);
            }
            t.checks.iter_mut().for_each(|k| expr(&mut k.expr));
            for c in &mut t.columns {
                c.generated.iter_mut().for_each(expr);
            }
            column_of_mut(t, column)?.name = name.into();
            Ok(())
        })
    }

    /// Muda tipo, nulo, default, identity ou expressão gerada de uma coluna
    /// (o nome muda por [`Document::rename_column`]).
    pub fn update_column(
        &mut self,
        table: Id,
        column: Id,
        update: impl FnOnce(&mut Column),
    ) -> Result<(), EditError> {
        let name = column_of(get(&self.model, table)?, column)?.name.clone();
        self.change(format!("Change Column {name}"), |m| {
            let t = get_mut(m, table)?;
            let in_pk = t
                .primary_key
                .as_ref()
                .is_some_and(|k| k.columns.contains(&name));
            let c = column_of_mut(t, column)?;
            update(c);
            if c.name != name {
                return err("Use rename to change a column's name.");
            }
            check_type(&c.ty)?;
            if in_pk && c.nullable {
                return err(format!(
                    "Column {name} is in the primary key and cannot allow nulls."
                ));
            }
            Ok(())
        })
    }

    /// Apaga uma coluna e o que depende dela: chaves, índices e FKs (desta
    /// tabela e das que apontam para ela).
    pub fn drop_column(&mut self, table: Id, column: Id) -> Result<(), EditError> {
        let name = column_of(get(&self.model, table)?, column)?.name.clone();
        self.change(format!("Delete Column {name}"), |m| {
            let t = get(m, table)?;
            if t.columns.len() == 1 {
                return err("A table needs at least one column.");
            }
            let (schema, tname) = (t.schema.clone(), t.name.clone());
            for other in &mut m.tables {
                other.foreign_keys.retain(|fk| {
                    !(fk.ref_schema == schema
                        && fk.ref_table == tname
                        && fk.ref_columns.contains(&name))
                });
            }
            let t = get_mut(m, table)?;
            t.columns.retain(|c| c.id != column);
            if t.primary_key
                .as_ref()
                .is_some_and(|k| k.columns.contains(&name))
            {
                t.primary_key = None;
            }
            t.uniques.retain(|k| !k.columns.contains(&name));
            t.foreign_keys.retain(|fk| !fk.columns.contains(&name));
            t.indexes.retain(|i| !i.columns.contains(&name));
            Ok(())
        })
    }

    /// Define (ou tira, com uma lista vazia) a chave primária. As colunas
    /// dela passam a não aceitar nulos, como o PostgreSQL faz.
    pub fn set_primary_key(&mut self, table: Id, columns: Vec<String>) -> Result<(), EditError> {
        self.change("Set Primary Key", |m| {
            let t = get_mut(m, table)?;
            if columns.is_empty() {
                t.primary_key = None;
                return Ok(());
            }
            check_columns(t, &columns)?;
            let name = t
                .primary_key
                .as_ref()
                .map(|k| k.name.clone())
                .unwrap_or_else(|| default_constraint_name(&t.name, &[], "pkey"));
            for c in &mut t.columns {
                if columns.contains(&c.name) {
                    c.nullable = false;
                }
            }
            t.primary_key = Some(Key { name, columns });
            Ok(())
        })
    }

    /// Acrescenta uma constraint unique (nome padrão se vier vazio).
    pub fn add_unique(&mut self, table: Id, mut key: Key) -> Result<(), EditError> {
        self.change("Add Unique Key", |m| {
            let t = get_mut(m, table)?;
            check_columns(t, &key.columns)?;
            if key.name.is_empty() {
                key.name = default_constraint_name(&t.name, &key.columns, "key");
            }
            check_constraint_name(t, &key.name)?;
            t.uniques.push(key);
            Ok(())
        })
    }

    /// Acrescenta uma constraint check (nome padrão se vier vazio).
    pub fn add_check(&mut self, table: Id, mut check: Check) -> Result<(), EditError> {
        self.change("Add Check Constraint", |m| {
            if check.expr.trim().is_empty() {
                return err("The check expression is empty.");
            }
            let t = get_mut(m, table)?;
            if check.name.is_empty() {
                check.name = default_constraint_name(&t.name, &[], "check");
            }
            check_constraint_name(t, &check.name)?;
            t.checks.push(check);
            Ok(())
        })
    }

    /// Acrescenta uma chave estrangeira (nome padrão se vier vazio).
    pub fn add_foreign_key(&mut self, table: Id, mut fk: ForeignKey) -> Result<(), EditError> {
        self.change("Add Relationship", |m| {
            let target = m.find(&fk.ref_schema, &fk.ref_table).ok_or_else(|| {
                EditError(format!(
                    "Table {}.{} does not exist.",
                    fk.ref_schema, fk.ref_table
                ))
            })?;
            check_columns(target, &fk.ref_columns)?;
            let t = get_mut(m, table)?;
            check_columns(t, &fk.columns)?;
            if fk.columns.len() != fk.ref_columns.len() || fk.columns.is_empty() {
                return err("A relationship needs the same number of columns on both sides.");
            }
            if fk.name.is_empty() {
                fk.name = default_constraint_name(&t.name, &fk.columns, "fkey");
            }
            check_constraint_name(t, &fk.name)?;
            t.foreign_keys.push(fk);
            Ok(())
        })
    }

    /// Apaga uma constraint pelo nome (PK, unique, check ou FK).
    pub fn drop_constraint(&mut self, table: Id, name: &str) -> Result<(), EditError> {
        self.change(format!("Delete Constraint {name}"), |m| {
            let t = get_mut(m, table)?;
            let before = constraint_names(t).len();
            if t.primary_key.as_ref().is_some_and(|k| k.name == name) {
                t.primary_key = None;
            }
            t.uniques.retain(|k| k.name != name);
            t.checks.retain(|c| c.name != name);
            t.foreign_keys.retain(|fk| fk.name != name);
            if constraint_names(t).len() == before {
                return err(format!("Constraint {name} does not exist."));
            }
            Ok(())
        })
    }

    /// Acrescenta um índice.
    pub fn add_index(&mut self, table: Id, index: Index) -> Result<(), EditError> {
        self.change(format!("Add Index {}", index.name), |m| {
            check_name(&index.name)?;
            if index.columns.is_empty() {
                return err("An index needs at least one column.");
            }
            let schema = get(m, table)?.schema.clone();
            let taken = m.tables.iter().filter(|t| t.schema == schema).any(|t| {
                t.indexes.iter().any(|i| i.name == index.name)
                    || t.primary_key
                        .iter()
                        .chain(&t.uniques)
                        .any(|k| k.name == index.name)
            });
            if taken {
                return err(format!(
                    "An index named {} already exists in {schema}.",
                    index.name
                ));
            }
            get_mut(m, table)?.indexes.push(index);
            Ok(())
        })
    }

    /// Apaga um índice pelo nome.
    pub fn drop_index(&mut self, table: Id, name: &str) -> Result<(), EditError> {
        self.change(format!("Delete Index {name}"), |m| {
            let t = get_mut(m, table)?;
            let before = t.indexes.len();
            t.indexes.retain(|i| i.name != name);
            if t.indexes.len() == before {
                return err(format!("Index {name} does not exist."));
            }
            Ok(())
        })
    }
}

fn get(m: &Model, id: Id) -> Result<&Table, EditError> {
    m.table(id)
        .ok_or_else(|| EditError("The table no longer exists.".into()))
}

fn get_mut(m: &mut Model, id: Id) -> Result<&mut Table, EditError> {
    m.table_mut(id)
        .ok_or_else(|| EditError("The table no longer exists.".into()))
}

fn column_of(t: &Table, id: Id) -> Result<&Column, EditError> {
    t.column_by_id(id)
        .ok_or_else(|| EditError("The column no longer exists.".into()))
}

fn column_of_mut(t: &mut Table, id: Id) -> Result<&mut Column, EditError> {
    t.columns
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| EditError("The column no longer exists.".into()))
}

fn check_name(name: &str) -> Result<(), EditError> {
    if name.trim().is_empty() {
        return err("The name is empty.");
    }
    if name.len() > 63 {
        return err(format!("{name} is longer than 63 bytes."));
    }
    Ok(())
}

fn check_type(ty: &str) -> Result<(), EditError> {
    if ty.trim().is_empty() {
        return err("The data type is empty.");
    }
    Ok(())
}

fn check_columns(t: &Table, columns: &[String]) -> Result<(), EditError> {
    if columns.is_empty() {
        return err("Choose at least one column.");
    }
    match columns.iter().find(|c| t.column(c).is_none()) {
        Some(c) => err(format!("Column {c} does not exist in {}.", t.name)),
        None => Ok(()),
    }
}

fn constraint_names(t: &Table) -> Vec<&str> {
    t.primary_key
        .iter()
        .chain(&t.uniques)
        .map(|k| k.name.as_str())
        .chain(t.checks.iter().map(|c| c.name.as_str()))
        .chain(t.foreign_keys.iter().map(|f| f.name.as_str()))
        .collect()
}

fn check_constraint_name(t: &Table, name: &str) -> Result<(), EditError> {
    check_name(name)?;
    if constraint_names(t).contains(&name) {
        return err(format!("A constraint named {name} already exists."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;

    fn shop() -> Document {
        let mut d = Document::new(Model::new(Vec::new()));
        let cliente = d.add_table("public", "cliente").unwrap();
        d.add_column(
            cliente,
            Column {
                name: "nome".into(),
                ty: "text".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let pedido = d.add_table("public", "pedido").unwrap();
        d.add_column(
            pedido,
            Column {
                name: "cliente_id".into(),
                ty: "integer".into(),
                ..Default::default()
            },
        )
        .unwrap();
        d.add_foreign_key(
            pedido,
            ForeignKey {
                columns: vec!["cliente_id".into()],
                ref_schema: "public".into(),
                ref_table: "cliente".into(),
                ref_columns: vec!["id".into()],
                on_delete: Action::Cascade,
                ..Default::default()
            },
        )
        .unwrap();
        d.history.clear();
        d
    }

    fn id(d: &Document, name: &str) -> Id {
        d.model.find("public", name).unwrap().id
    }

    #[test]
    fn add_table_has_an_identity_primary_key() {
        let d = shop();
        let t = d.model.find("public", "cliente").unwrap();
        assert_eq!(t.primary_key.as_ref().unwrap().name, "cliente_pkey");
        assert_eq!(t.columns[0].identity, Some(crate::Identity::Always));
        let fk = &d.model.find("public", "pedido").unwrap().foreign_keys[0];
        assert_eq!(fk.name, "pedido_cliente_id_fkey");
    }

    #[test]
    fn renaming_a_table_moves_the_foreign_keys_and_undo_restores() {
        let mut d = shop();
        let before = d.model.clone();
        d.rename_table(id(&d, "cliente"), "vendas", "customer")
            .unwrap();
        let fk = &d.model.find("public", "pedido").unwrap().foreign_keys[0];
        assert_eq!(
            (fk.ref_schema.as_str(), fk.ref_table.as_str()),
            ("vendas", "customer")
        );
        assert_eq!(d.history.undo_label(), Some("Rename Table customer"));
        d.undo();
        assert_eq!(d.model.tables, before.tables);
        d.redo();
        assert!(d.model.find("vendas", "customer").is_some());
    }

    #[test]
    fn renaming_a_column_follows_keys_and_references() {
        let mut d = shop();
        let cliente = id(&d, "cliente");
        let col = d.model.table(cliente).unwrap().columns[0].id;
        d.rename_column(cliente, col, "cliente_id").unwrap();
        let t = d.model.table(cliente).unwrap();
        assert_eq!(t.primary_key.as_ref().unwrap().columns, ["cliente_id"]);
        let fk = &d.model.find("public", "pedido").unwrap().foreign_keys[0];
        assert_eq!(fk.ref_columns, ["cliente_id"]);
        assert!(d.rename_column(cliente, col, "nome").is_err());
    }

    #[test]
    fn dropping_takes_dependents_and_undo_brings_them_back() {
        let mut d = shop();
        let before = d.model.clone();
        let cliente = id(&d, "cliente");
        d.drop_table(cliente).unwrap();
        assert!(
            d.model
                .find("public", "pedido")
                .unwrap()
                .foreign_keys
                .is_empty()
        );
        d.undo();
        assert_eq!(d.model.tables, before.tables);

        let pk = d.model.table(cliente).unwrap().columns[0].id;
        d.drop_column(cliente, pk).unwrap();
        let t = d.model.table(cliente).unwrap();
        assert!(t.primary_key.is_none());
        assert!(
            d.model
                .find("public", "pedido")
                .unwrap()
                .foreign_keys
                .is_empty()
        );
        d.undo();
        assert_eq!(d.model.tables, before.tables);
    }

    #[test]
    fn refused_edits_change_nothing() {
        let mut d = shop();
        let before = d.model.clone();
        let pedido = id(&d, "pedido");
        assert!(d.add_table("public", "pedido").is_err());
        assert!(d.set_primary_key(pedido, vec!["nope".into()]).is_err());
        let pk = d.model.table(pedido).unwrap().columns[0].id;
        assert!(d.update_column(pedido, pk, |c| c.nullable = true).is_err());
        assert!(d.drop_constraint(pedido, "nope").is_err());
        assert_eq!(d.model, before);
        assert!(!d.history.is_dirty());
    }

    #[test]
    fn primary_key_columns_become_not_null() {
        let mut d = shop();
        let pedido = id(&d, "pedido");
        d.set_primary_key(pedido, vec!["id".into(), "cliente_id".into()])
            .unwrap();
        let t = d.model.table(pedido).unwrap();
        assert!(!t.column("cliente_id").unwrap().nullable);
        assert_eq!(t.primary_key.as_ref().unwrap().name, "pedido_pkey");
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut d = shop();
        let pedido = id(&d, "pedido");
        d.add_check(
            pedido,
            Check {
                name: String::new(),
                expr: "id > 0".into(),
            },
        )
        .unwrap();
        d.undo();
        assert_eq!(d.history.redo_label(), Some("Add Check Constraint"));
        d.add_unique(
            pedido,
            Key {
                name: String::new(),
                columns: vec!["cliente_id".into()],
            },
        )
        .unwrap();
        assert_eq!(d.history.redo_label(), None);
        let t = d.model.table(pedido).unwrap();
        assert_eq!(t.uniques[0].name, "pedido_cliente_id_key");
    }
}
