//! As janelas do Table Designer para o que não cabe na grade de colunas,
//! como no SSMS: Foreign Key Relationships, Indexes/Keys e Check
//! Constraints.
//!
//! Cada janela edita um rascunho do item selecionado; "Apply" grava o
//! rascunho no modelo numa edição só (que se desfaz como as outras).

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::schema::{Action, Check, ForeignKey, Index, Key, Table};

use super::{Notice, TableDesigner};

/// Qual janela.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    Relationships,
    IndexesKeys,
    Checks,
}

impl DialogKind {
    pub fn title(self) -> &'static str {
        match self {
            DialogKind::Relationships => "Foreign Key Relationships",
            DialogKind::IndexesKeys => "Indexes/Keys",
            DialogKind::Checks => "Check Constraints",
        }
    }
}

/// Tipo de uma entrada do Indexes/Keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    PrimaryKey,
    UniqueKey,
    Index,
}

impl KeyKind {
    fn label(self) -> &'static str {
        match self {
            KeyKind::PrimaryKey => "Primary Key",
            KeyKind::UniqueKey => "Unique Key",
            KeyKind::Index => "Index",
        }
    }
}

/// Uma entrada do Indexes/Keys: chave primária, unique ou índice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexDraft {
    pub kind: KeyKind,
    pub name: String,
    /// Colunas (ou expressões, num índice).
    pub columns: Vec<String>,
    /// Só para índices.
    pub unique: bool,
    pub method: String,
    pub filter: String,
}

/// O rascunho sendo editado na janela.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Draft {
    Relationship(ForeignKey),
    IndexKey(IndexDraft),
    Check(Check),
}

/// Uma janela aberta.
#[derive(Debug)]
pub struct Dialog {
    pub kind: DialogKind,
    /// Nome do item selecionado no modelo; None para um item novo.
    selected: Option<String>,
    draft: Option<Draft>,
    /// O rascunho como estava ao selecionar (para saber se mudou).
    original: Option<Draft>,
    error: Option<String>,
}

const METHODS: [&str; 6] = ["btree", "hash", "gist", "gin", "brin", "spgist"];

impl Dialog {
    fn new(kind: DialogKind) -> Self {
        Dialog {
            kind,
            selected: None,
            draft: None,
            original: None,
            error: None,
        }
    }

    fn pick(&mut self, name: Option<String>, draft: Option<Draft>) {
        self.selected = name;
        self.original = if self.selected.is_some() {
            draft.clone()
        } else {
            None
        };
        self.draft = draft;
        self.error = None;
    }
}

/// Os itens que a janela lista, com o nome mostrado e o rascunho.
fn items(kind: DialogKind, t: &Table) -> Vec<(String, Draft)> {
    match kind {
        DialogKind::Relationships => t
            .foreign_keys
            .iter()
            .map(|fk| (fk.name.clone(), Draft::Relationship(fk.clone())))
            .collect(),
        DialogKind::Checks => t
            .checks
            .iter()
            .map(|c| (c.name.clone(), Draft::Check(c.clone())))
            .collect(),
        DialogKind::IndexesKeys => {
            let key = |kind, k: &Key| {
                (
                    k.name.clone(),
                    Draft::IndexKey(IndexDraft {
                        kind,
                        name: k.name.clone(),
                        columns: k.columns.clone(),
                        unique: true,
                        method: "btree".into(),
                        filter: String::new(),
                    }),
                )
            };
            t.primary_key
                .iter()
                .map(|k| key(KeyKind::PrimaryKey, k))
                .chain(t.uniques.iter().map(|k| key(KeyKind::UniqueKey, k)))
                .chain(t.indexes.iter().map(|i| {
                    (
                        i.name.clone(),
                        Draft::IndexKey(IndexDraft {
                            kind: KeyKind::Index,
                            name: i.name.clone(),
                            columns: i.columns.clone(),
                            unique: i.unique,
                            method: i.method.clone(),
                            filter: i.predicate.clone().unwrap_or_default(),
                        }),
                    )
                }))
                .collect()
        }
    }
}

impl TableDesigner {
    /// Abre uma das janelas, com o primeiro item selecionado.
    pub fn open_dialog(&mut self, kind: DialogKind) {
        let mut dialog = Dialog::new(kind);
        if let Some(t) = self.current_table()
            && let Some((name, draft)) = items(kind, t).into_iter().next()
        {
            dialog.pick(Some(name), Some(draft));
        }
        self.dialog = Some(dialog);
    }

    /// A janela aberta.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    /// Rascunho da janela aberta (para os testes editarem sem clicar em
    /// cada campo).
    pub fn dialog_draft_mut(&mut self) -> Option<&mut Draft> {
        self.dialog.as_mut()?.draft.as_mut()
    }

    /// Erro do último "Apply".
    pub fn dialog_error(&self) -> Option<&str> {
        self.dialog.as_ref()?.error.as_deref()
    }

    fn current_table(&self) -> Option<&Table> {
        self.table.and_then(|t| self.doc.model.table(t))
    }

    /// "Add": um rascunho novo com valores que já fazem sentido.
    pub fn dialog_add(&mut self) {
        let Some(kind) = self.dialog.as_ref().map(|d| d.kind) else {
            return;
        };
        let Some(t) = self.current_table() else {
            return;
        };
        let selected_column = self
            .selected
            .and_then(|c| t.column_by_id(c))
            .or(t.columns.first())
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let draft = match kind {
            DialogKind::Relationships => {
                // Aponta para a primeira outra tabela do schema, pela PK dela.
                let target = self
                    .doc
                    .model
                    .tables
                    .iter()
                    .find(|o| o.id != t.id && o.schema == t.schema && o.primary_key.is_some())
                    .map_or(t, |o| &**o);
                let ref_columns = target
                    .primary_key
                    .as_ref()
                    .map(|k| k.columns.clone())
                    .unwrap_or_default();
                Draft::Relationship(ForeignKey {
                    name: String::new(),
                    columns: ref_columns
                        .iter()
                        .map(|_| selected_column.clone())
                        .collect(),
                    ref_schema: target.schema.clone(),
                    ref_table: target.name.clone(),
                    ref_columns,
                    ..Default::default()
                })
            }
            DialogKind::IndexesKeys => Draft::IndexKey(IndexDraft {
                kind: KeyKind::Index,
                name: format!("{}_{}_idx", t.name, selected_column),
                columns: vec![selected_column],
                unique: false,
                method: "btree".into(),
                filter: String::new(),
            }),
            DialogKind::Checks => Draft::Check(Check {
                name: String::new(),
                expr: String::new(),
            }),
        };
        if let Some(d) = &mut self.dialog {
            d.pick(None, Some(draft));
        }
    }

    /// "Delete": apaga o item selecionado do modelo.
    pub fn dialog_delete(&mut self) {
        let Some(d) = &self.dialog else {
            return;
        };
        let (Some(name), Some(draft)) = (d.selected.clone(), d.original.clone()) else {
            // Um rascunho novo só some.
            if let Some(d) = &mut self.dialog {
                d.pick(None, None);
            }
            return;
        };
        let ok = match draft {
            Draft::IndexKey(IndexDraft {
                kind: KeyKind::PrimaryKey,
                ..
            }) => self.edit(|doc, t| doc.set_primary_key(t, Vec::new())),
            Draft::IndexKey(IndexDraft {
                kind: KeyKind::Index,
                ..
            }) => self.edit(|doc, t| doc.drop_index(t, &name)),
            _ => self.edit(|doc, t| doc.drop_constraint(t, &name)),
        }
        .is_some();
        if ok {
            let kind = self.dialog.as_ref().map(|d| d.kind);
            if let Some(kind) = kind {
                self.open_dialog(kind);
            }
        }
    }

    /// "Apply": grava o rascunho no modelo.
    pub fn dialog_apply(&mut self) {
        let Some(d) = &self.dialog else {
            return;
        };
        let (Some(draft), original, selected) =
            (d.draft.clone(), d.original.clone(), d.selected.clone())
        else {
            return;
        };
        let result = match (&draft, &selected) {
            (Draft::Relationship(fk), Some(name)) => {
                self.try_edit(|doc, t| doc.update_foreign_key(t, name, fk.clone()))
            }
            (Draft::Relationship(fk), None) => {
                self.try_edit(|doc, t| doc.add_foreign_key(t, fk.clone()))
            }
            (Draft::Check(c), Some(name)) => {
                self.try_edit(|doc, t| doc.update_check(t, name, c.clone()))
            }
            (Draft::Check(c), None) => self.try_edit(|doc, t| doc.add_check(t, c.clone())),
            (Draft::IndexKey(new), _) => {
                let old = match &original {
                    Some(Draft::IndexKey(o)) => Some(o.clone()),
                    _ => None,
                };
                self.apply_index_key(old, new.clone())
            }
        };
        let names = self
            .current_table()
            .map(|t| {
                let kind = self.dialog.as_ref().map_or(DialogKind::Checks, |d| d.kind);
                items(kind, t)
            })
            .unwrap_or_default();
        let Some(d) = &mut self.dialog else {
            return;
        };
        match result {
            Ok(()) => {
                // Seleciona o item gravado (o nome pode ter vindo do padrão).
                let wanted = draft_name(&draft);
                let saved = names
                    .iter()
                    .find(|(n, _)| *n == wanted)
                    .or_else(|| names.iter().find(|(n, _)| Some(n) == selected.as_ref()))
                    .or_else(|| names.last())
                    .cloned();
                match saved {
                    Some((n, dr)) => d.pick(Some(n), Some(dr)),
                    None => d.pick(None, None),
                }
            }
            Err(e) => d.error = Some(e),
        }
    }

    fn apply_index_key(&mut self, old: Option<IndexDraft>, new: IndexDraft) -> Result<(), String> {
        let index = Index {
            name: new.name.trim().to_string(),
            unique: new.unique,
            method: new.method.clone(),
            columns: new
                .columns
                .iter()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect(),
            predicate: (!new.filter.trim().is_empty()).then(|| new.filter.trim().to_string()),
        };
        let key = Key {
            name: index.name.clone(),
            columns: index.columns.clone(),
        };
        match (old, new.kind) {
            (Some(o), KeyKind::Index) if o.kind == KeyKind::Index => {
                self.try_edit(|doc, t| doc.update_index(t, &o.name, index))
            }
            (Some(o), KeyKind::UniqueKey) if o.kind == KeyKind::UniqueKey => {
                self.try_edit(|doc, t| doc.update_unique(t, &o.name, key))
            }
            (old, kind) => {
                // Tipo novo: tira o antigo e põe o novo (duas edições).
                if let Some(o) = &old {
                    match o.kind {
                        KeyKind::PrimaryKey => {
                            self.try_edit(|doc, t| doc.set_primary_key(t, Vec::new()))?;
                        }
                        KeyKind::UniqueKey => {
                            self.try_edit(|doc, t| doc.drop_constraint(t, &o.name))?;
                        }
                        KeyKind::Index => self.try_edit(|doc, t| doc.drop_index(t, &o.name))?,
                    }
                }
                let added = match kind {
                    KeyKind::PrimaryKey => {
                        self.try_edit(|doc, t| doc.set_primary_key(t, key.columns.clone()))
                    }
                    KeyKind::UniqueKey => self.try_edit(|doc, t| doc.add_unique(t, key)),
                    KeyKind::Index => self.try_edit(|doc, t| doc.add_index(t, index)),
                };
                if added.is_err() && old.is_some() {
                    // Volta o que foi tirado.
                    self.doc.undo();
                }
                added
            }
        }
    }

    /// Como [`TableDesigner::edit`], mas devolve o erro em vez de mostrá-lo.
    fn try_edit(
        &mut self,
        f: impl FnOnce(
            &mut pgstudio_core::schema::edit::Document,
            pgstudio_core::schema::Id,
        ) -> Result<(), pgstudio_core::schema::edit::EditError>,
    ) -> Result<(), String> {
        let table = self.table.ok_or("The table is not loaded.")?;
        f(&mut self.doc, table).map_err(|e| e.0)?;
        self.notice = None;
        Ok(())
    }

    pub(super) fn dialog_window(&mut self, ctx: &egui::Context, id: u32) {
        let Some(kind) = self.dialog.as_ref().map(|d| d.kind) else {
            return;
        };
        let Some(t) = self.current_table().cloned() else {
            return;
        };
        let tables: Vec<(String, String)> = self
            .doc
            .model
            .tables
            .iter()
            .map(|o| (o.schema.clone(), o.name.clone()))
            .collect();
        let list = items(kind, &t);
        let mut open = true;
        let mut action = None;
        let Some(d) = &mut self.dialog else {
            return;
        };
        let modal = crate::theme::modal(egui::Id::new(("designer_dialog", id))).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.heading(kind.title());
            ui.separator();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(200.0);
                    ui.label("Selected:");
                    egui::ScrollArea::vertical()
                        .id_salt("dialog_list")
                        .max_height(250.0)
                        .show(ui, |ui| {
                            for (name, draft) in &list {
                                let label = match draft {
                                    Draft::IndexKey(k) => {
                                        format!("{name} ({})", k.kind.label())
                                    }
                                    _ => name.clone(),
                                };
                                let selected = d.selected.as_ref() == Some(name);
                                if ui.selectable_label(selected, label).clicked() {
                                    d.pick(Some(name.clone()), Some(draft.clone()));
                                }
                            }
                            if d.selected.is_none() && d.draft.is_some() {
                                let _ = ui.selectable_label(true, "(new)");
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui.button(format!("{} Add", icon::PLUS)).clicked() {
                            action = Some(DialogAction::Add);
                        }
                        if ui
                            .add_enabled(
                                d.draft.is_some(),
                                egui::Button::new(format!("{} Delete", icon::TRASH)),
                            )
                            .clicked()
                        {
                            action = Some(DialogAction::Delete);
                        }
                    });
                });
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    let Some(draft) = &mut d.draft else {
                        ui.weak("Choose Add to create one.");
                        return;
                    };
                    match draft {
                        Draft::Relationship(fk) => relationship(ui, fk, &t, &tables),
                        Draft::IndexKey(k) => index_key(ui, k, &t),
                        Draft::Check(c) => check(ui, c),
                    }
                    ui.add_space(8.0);
                    if let Some(e) = &d.error {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        action = Some(DialogAction::Close);
                    }
                    let changed =
                        d.draft.is_some() && (d.selected.is_none() || d.original != d.draft);
                    if ui
                        .add_enabled(changed, egui::Button::new("Apply"))
                        .clicked()
                    {
                        action = Some(DialogAction::Apply);
                    }
                });
            });
        });
        match action {
            Some(DialogAction::Add) => self.dialog_add(),
            Some(DialogAction::Delete) => self.dialog_delete(),
            Some(DialogAction::Apply) => self.dialog_apply(),
            Some(DialogAction::Close) => self.dialog = None,
            None => {}
        }
        if modal.should_close() && matches!(action, None | Some(DialogAction::Close)) {
            open = false;
        }
        if !open {
            self.dialog = None;
        }
        if let Some(Notice::Error(_)) = &self.notice {
            // Erros do Delete aparecem na janela também.
            if let (Some(d), Some(Notice::Error(e))) = (&mut self.dialog, &self.notice) {
                d.error = Some(e.clone());
            }
        }
    }
}

enum DialogAction {
    Add,
    Delete,
    Apply,
    Close,
}

fn draft_name(draft: &Draft) -> String {
    match draft {
        Draft::Relationship(fk) => fk.name.clone(),
        Draft::IndexKey(k) => k.name.clone(),
        Draft::Check(c) => c.name.clone(),
    }
}

fn relationship(ui: &mut egui::Ui, fk: &mut ForeignKey, t: &Table, tables: &[(String, String)]) {
    egui::Grid::new("relationship_props")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            ui.label("(Name)");
            ui.add(egui::TextEdit::singleline(&mut fk.name).hint_text("(default name)"));
            ui.end_row();
            ui.label("Primary Key Table");
            egui::ComboBox::from_id_salt("fk_ref_table")
                .selected_text(format!("{}.{}", fk.ref_schema, fk.ref_table))
                .show_ui(ui, |ui| {
                    for (s, n) in tables {
                        let current = fk.ref_schema == *s && fk.ref_table == *n;
                        if ui.selectable_label(current, format!("{s}.{n}")).clicked() {
                            fk.ref_schema.clone_from(s);
                            fk.ref_table.clone_from(n);
                        }
                    }
                });
            ui.end_row();
            ui.label("Foreign Key Table");
            ui.label(format!("{}.{}", t.schema, t.name));
            ui.end_row();
            ui.label("On Update");
            action_combo(ui, "fk_on_update", &mut fk.on_update);
            ui.end_row();
            ui.label("On Delete");
            action_combo(ui, "fk_on_delete", &mut fk.on_delete);
            ui.end_row();
        });
    ui.label("Columns (foreign key → primary key):");
    let mut remove = None;
    let n = fk.columns.len().max(fk.ref_columns.len());
    fk.columns.resize(n, String::new());
    fk.ref_columns.resize(n, String::new());
    for i in 0..n {
        ui.horizontal(|ui| {
            column_combo(ui, ("fk_col", i), &mut fk.columns[i], t);
            ui.label(icon::ARROW_RIGHT);
            ui.add(egui::TextEdit::singleline(&mut fk.ref_columns[i]).desired_width(140.0));
            if ui.small_button(icon::X).on_hover_text("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        fk.columns.remove(i);
        fk.ref_columns.remove(i);
    }
    if ui
        .small_button(format!("{} Add column", icon::PLUS))
        .clicked()
    {
        fk.columns.push(String::new());
        fk.ref_columns.push(String::new());
    }
}

fn action_combo(ui: &mut egui::Ui, id: &str, value: &mut Action) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.sql())
        .show_ui(ui, |ui| {
            for a in Action::ALL {
                ui.selectable_value(value, a, a.sql());
            }
        });
}

fn column_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut String,
    t: &Table,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.as_str())
        .width(140.0)
        .show_ui(ui, |ui| {
            for c in &t.columns {
                ui.selectable_value(value, c.name.clone(), &c.name);
            }
        });
}

fn index_key(ui: &mut egui::Ui, k: &mut IndexDraft, t: &Table) {
    egui::Grid::new("index_key_props")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            ui.label("Type");
            egui::ComboBox::from_id_salt("index_kind")
                .selected_text(k.kind.label())
                .show_ui(ui, |ui| {
                    for kind in [KeyKind::PrimaryKey, KeyKind::UniqueKey, KeyKind::Index] {
                        ui.selectable_value(&mut k.kind, kind, kind.label());
                    }
                });
            ui.end_row();
            ui.label("(Name)");
            ui.add_enabled(
                k.kind != KeyKind::PrimaryKey,
                egui::TextEdit::singleline(&mut k.name),
            );
            ui.end_row();
            if k.kind == KeyKind::Index {
                ui.label("Is Unique");
                ui.checkbox(&mut k.unique, "");
                ui.end_row();
                ui.label("Method");
                egui::ComboBox::from_id_salt("index_method")
                    .selected_text(k.method.as_str())
                    .show_ui(ui, |ui| {
                        for m in METHODS {
                            ui.selectable_value(&mut k.method, m.to_string(), m);
                        }
                    });
                ui.end_row();
                ui.label("Filter (WHERE)");
                ui.add(egui::TextEdit::singleline(&mut k.filter));
                ui.end_row();
            }
        });
    ui.label(if k.kind == KeyKind::Index {
        "Columns (names or expressions):"
    } else {
        "Columns:"
    });
    let mut remove = None;
    for (i, c) in k.columns.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if k.kind == KeyKind::Index {
                ui.add(egui::TextEdit::singleline(c).desired_width(200.0));
            } else {
                column_combo(ui, ("key_col", i), c, t);
            }
            if ui.small_button(icon::X).on_hover_text("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        k.columns.remove(i);
    }
    ui.menu_button(format!("{} Add column", icon::PLUS), |ui| {
        for c in &t.columns {
            if ui.button(&c.name).clicked() {
                k.columns.push(c.name.clone());
                ui.close();
            }
        }
    });
}

fn check(ui: &mut egui::Ui, c: &mut Check) {
    egui::Grid::new("check_props")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            ui.label("(Name)");
            ui.add(egui::TextEdit::singleline(&mut c.name).hint_text("(default name)"));
            ui.end_row();
            ui.label("Expression");
            ui.add(
                egui::TextEdit::multiline(&mut c.expr)
                    .code_editor()
                    .desired_rows(3)
                    .hint_text("price > 0"),
            );
            ui.end_row();
        });
}
