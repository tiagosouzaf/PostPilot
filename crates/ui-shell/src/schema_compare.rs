//! "Schema Compare" (Tools > Schema Compare..., ou Tasks > Compare Schema...
//! num banco): compara dois bancos, mostra o que difere, gera o script que
//! deixa o destino como a origem e aplica numa transação.
//!
//! Tabelas, colunas, chaves, índices e checks vêm do `schema-model`
//! ([`Comparison`]); views e funções são comparadas pelo texto da definição
//! (`pg_get_viewdef` e `pg_get_functiondef`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui::{self, Align, Layout};
use pgstudio_core::metadata::MetadataProvider;
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_core::schema::compare::{Comparison, Name, Status};
use pgstudio_core::schema::quote_ident;

/// O tipo de objeto comparado.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Table,
    View,
    Function,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Table => "Table",
            Kind::View => "View",
            Kind::Function => "Function",
        }
    }
}

/// Um objeto: tipo, schema e nome (funções com os argumentos).
pub type Key = (Kind, String, String);

/// Uma view ou função de um banco.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    /// O texto comparado.
    pub text: String,
    pub create: String,
    pub drop: String,
}

/// Views de usuário (sem as de extensões).
pub const VIEWS_SQL: &str = "SELECT n.nspname, c.relname, pg_catalog.pg_get_viewdef(c.oid) \
    FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
    WHERE c.relkind = 'v' AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' \
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
        WHERE d.classid = 'pg_catalog.pg_class'::regclass AND d.objid = c.oid AND d.deptype = 'e') \
    ORDER BY 1, 2";

/// Funções e procedures de usuário (sem as de extensões).
pub const FUNCTIONS_SQL: &str = "SELECT n.nspname, p.proname, \
    pg_catalog.pg_get_function_identity_arguments(p.oid), pg_catalog.pg_get_functiondef(p.oid), \
    p.prokind::text \
    FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
    WHERE p.prokind IN ('f', 'p') AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' \
    AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
        WHERE d.classid = 'pg_catalog.pg_proc'::regclass AND d.objid = p.oid AND d.deptype = 'e') \
    ORDER BY 1, 2, 3";

/// As views de um banco, das linhas de [`VIEWS_SQL`].
pub fn views(rows: &[Vec<Option<String>>]) -> BTreeMap<Key, Definition> {
    rows.iter()
        .filter_map(|r| {
            let schema = r.first()?.clone()?;
            let name = r.get(1)?.clone()?;
            let def = r.get(2)?.clone()?;
            let body = def.trim().trim_end_matches(';').trim_end().to_string();
            let q = format!("{}.{}", quote_ident(&schema), quote_ident(&name));
            Some((
                (Kind::View, schema, name),
                Definition {
                    create: format!("CREATE VIEW {q} AS\n{body};"),
                    drop: format!("DROP VIEW {q};"),
                    text: body,
                },
            ))
        })
        .collect()
}

/// As funções de um banco, das linhas de [`FUNCTIONS_SQL`].
pub fn functions(rows: &[Vec<Option<String>>]) -> BTreeMap<Key, Definition> {
    rows.iter()
        .filter_map(|r| {
            let schema = r.first()?.clone()?;
            let name = r.get(1)?.clone()?;
            let args = r.get(2)?.clone().unwrap_or_default();
            let def = r.get(3)?.clone()?;
            let what = if r.get(4)?.as_deref() == Some("p") {
                "PROCEDURE"
            } else {
                "FUNCTION"
            };
            let text = def.trim().to_string();
            Some((
                (Kind::Function, schema.clone(), format!("{name}({args})")),
                Definition {
                    create: format!("{text};"),
                    drop: format!(
                        "DROP {what} {}.{}({args});",
                        quote_ident(&schema),
                        quote_ident(&name)
                    ),
                    text,
                },
            ))
        })
        .collect()
}

/// Uma diferença da lista.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub key: Key,
    pub status: Status,
    /// O DDL que só este objeto levaria.
    pub statements: Vec<String>,
}

/// O resultado de comparar.
pub struct Outcome {
    pub source: String,
    pub target: String,
    tables: Comparison,
    routines: BTreeMap<Key, (Option<Definition>, Option<Definition>)>,
    pub items: Vec<Item>,
    /// Quantos objetos são iguais.
    pub equal: usize,
}

impl Outcome {
    pub fn new(
        source: String,
        target: String,
        tables: Comparison,
        source_defs: BTreeMap<Key, Definition>,
        target_defs: BTreeMap<Key, Definition>,
    ) -> Self {
        let mut items: Vec<Item> = tables
            .tables
            .iter()
            .map(|t| Item {
                key: (Kind::Table, t.schema.clone(), t.name.clone()),
                status: t.status,
                statements: t.statements.clone(),
            })
            .collect();
        let mut equal = tables.equal;
        let keys: BTreeSet<Key> = source_defs
            .keys()
            .chain(target_defs.keys())
            .cloned()
            .collect();
        let mut routines = BTreeMap::new();
        for key in keys {
            let s = source_defs.get(&key).cloned();
            let t = target_defs.get(&key).cloned();
            let (status, statements) = match (&s, &t) {
                (Some(s), None) => (Status::OnlyInSource, vec![s.create.clone()]),
                (None, Some(t)) => (Status::OnlyInTarget, vec![t.drop.clone()]),
                (Some(s), Some(t)) if s.text != t.text => {
                    (Status::Different, vec![t.drop.clone(), s.create.clone()])
                }
                _ => {
                    equal += 1;
                    continue;
                }
            };
            items.push(Item {
                key: key.clone(),
                status,
                statements,
            });
            routines.insert(key, (s, t));
        }
        items.sort_by(|a, b| a.key.cmp(&b.key));
        Outcome {
            source,
            target,
            tables,
            routines,
            items,
            equal,
        }
    }

    /// O script para os objetos `chosen`, na ordem que o PostgreSQL aceita:
    /// apaga as views e funções que saem ou mudam, altera as tabelas, cria
    /// as funções e por fim as views.
    pub fn script(&self, chosen: &BTreeSet<Key>) -> Vec<String> {
        let pick = |kind: Kind, create: bool| -> Vec<String> {
            self.routines
                .iter()
                .filter(|(k, _)| k.0 == kind && chosen.contains(*k))
                .filter_map(|(_, (s, t))| match (create, s, t) {
                    (false, _, Some(t)) => Some(t.drop.clone()),
                    (true, Some(s), _) => Some(s.create.clone()),
                    _ => None,
                })
                .collect()
        };
        let tables: BTreeSet<Name> = chosen
            .iter()
            .filter(|k| k.0 == Kind::Table)
            .map(|k| (k.1.clone(), k.2.clone()))
            .collect();
        let tables = if tables.is_empty() {
            Vec::new()
        } else {
            self.tables.script(&tables)
        };
        [
            pick(Kind::View, false),
            pick(Kind::Function, false),
            tables,
            pick(Kind::Function, true),
            pick(Kind::View, true),
        ]
        .concat()
    }
}

/// As linhas de uma consulta, esperando nesta thread.
fn rows(session: &Arc<dyn QuerySession>, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut out = Vec::new();
    let mut error = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(120)) {
            Ok(QueryEvent::Row(r)) => out.push(r),
            Ok(QueryEvent::Error(e)) => error = Some(e.message),
            Ok(QueryEvent::Finished { .. }) => break,
            Ok(_) => {}
            Err(_) => return Err("The server did not answer.".into()),
        }
    }
    error.map_or(Ok(out), Err)
}

/// Compara `source` com `target` (roda numa thread).
fn compare(
    provider: &Arc<dyn MetadataProvider>,
    queries: &Arc<dyn QueryService>,
    source: &str,
    target: &str,
) -> Result<Outcome, String> {
    let model = |db: &str| {
        provider
            .load_model(db)
            .map_err(|e| format!("{db}: {}", e.0))
    };
    let defs = |db: &str| -> Result<BTreeMap<Key, Definition>, String> {
        let session = queries.open(db);
        let mut d = views(&rows(&session, VIEWS_SQL).map_err(|e| format!("{db}: {e}"))?);
        d.extend(functions(
            &rows(&session, FUNCTIONS_SQL).map_err(|e| format!("{db}: {e}"))?,
        ));
        Ok(d)
    };
    let tables = Comparison::new(&model(source)?, &model(target)?);
    Ok(Outcome::new(
        source.into(),
        target.into(),
        tables,
        defs(source)?,
        defs(target)?,
    ))
}

enum Phase {
    Idle,
    Comparing(Receiver<Result<Outcome, String>>),
    Applying(Receiver<Result<(), String>>, Vec<String>),
}

/// O que a janela pediu à casca.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// Abrir o script numa janela de consulta no banco destino.
    Script(String, String),
    Log(Vec<String>),
    /// Aplicou no banco destino.
    Applied(String, Vec<String>),
    Closed,
}

/// A janela "Schema Compare".
pub struct SchemaCompare {
    provider: Arc<dyn MetadataProvider>,
    queries: Arc<dyn QueryService>,
    pub source: String,
    pub target: String,
    databases: Option<Vec<String>>,
    databases_rx: Receiver<Vec<String>>,
    phase: Phase,
    outcome: Option<Outcome>,
    /// O que entra no script (tudo, depois de comparar).
    pub chosen: BTreeSet<Key>,
    /// O objeto cujo DDL aparece ao lado.
    pub selected: Option<Key>,
    confirming: bool,
    error: Option<String>,
    status: Option<String>,
}

impl SchemaCompare {
    /// Abre a janela; `source` é o banco de origem sugerido. `home` é o
    /// banco da conexão, de onde vem a lista de bancos.
    pub fn open(
        source: Option<String>,
        home: &str,
        provider: Arc<dyn MetadataProvider>,
        queries: Arc<dyn QueryService>,
        ctx: &egui::Context,
    ) -> Self {
        let (tx, databases_rx) = mpsc::channel();
        let ctx2 = ctx.clone();
        let mut names = Vec::new();
        queries.open(home).execute(
            "SELECT datname FROM pg_database WHERE datallowconn AND NOT datistemplate ORDER BY 1"
                .to_string(),
            Box::new(move |e| match e {
                QueryEvent::Row(r) => {
                    if let Some(Some(n)) = r.into_iter().next() {
                        names.push(n);
                    }
                }
                QueryEvent::Finished { .. } => {
                    let _ = tx.send(std::mem::take(&mut names));
                    ctx2.request_repaint();
                }
                _ => {}
            }),
        );
        Self {
            provider,
            queries,
            source: source.unwrap_or_default(),
            target: String::new(),
            databases: None,
            databases_rx,
            phase: Phase::Idle,
            outcome: None,
            chosen: BTreeSet::new(),
            selected: None,
            confirming: false,
            error: None,
            status: None,
        }
    }

    pub fn databases(&self) -> Option<&[String]> {
        self.databases.as_deref()
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Compara origem e destino.
    pub fn compare(&mut self, ctx: &egui::Context) {
        if self.source.is_empty() || self.target.is_empty() {
            self.error = Some("Choose the source and the target databases.".into());
            return;
        }
        if self.source == self.target {
            self.error = Some("The source and the target are the same database.".into());
            return;
        }
        self.error = None;
        self.status = None;
        self.confirming = false;
        let (tx, rx) = mpsc::channel();
        let (p, q) = (self.provider.clone(), self.queries.clone());
        let (s, t) = (self.source.clone(), self.target.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(compare(&p, &q, &s, &t));
            ctx.request_repaint();
        });
        self.phase = Phase::Comparing(rx);
    }

    /// O script dos objetos marcados.
    pub fn script(&self) -> Vec<String> {
        self.outcome
            .as_ref()
            .map(|o| o.script(&self.chosen))
            .unwrap_or_default()
    }

    /// Aplica o script no destino, numa transação.
    pub fn apply(&mut self, ctx: &egui::Context) {
        let Some(o) = &self.outcome else {
            return;
        };
        let script = o.script(&self.chosen);
        if script.is_empty() {
            self.error = Some("Select at least one difference.".into());
            return;
        }
        self.error = None;
        self.confirming = false;
        let (tx, rx) = mpsc::channel();
        let p = self.provider.clone();
        let db = o.target.clone();
        let stmts = script.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(p.apply_ddl(&db, &stmts).map_err(|e| e.0));
            ctx.request_repaint();
        });
        self.phase = Phase::Applying(rx, script);
    }

    /// Recebe o que as threads mandaram.
    pub fn step(&mut self, ctx: &egui::Context) -> Option<Request> {
        if let Ok(d) = self.databases_rx.try_recv() {
            if self.target.is_empty() {
                self.target = d
                    .iter()
                    .find(|n| **n != self.source)
                    .cloned()
                    .unwrap_or_default();
            }
            self.databases = Some(d);
        }
        match &self.phase {
            Phase::Idle => None,
            Phase::Comparing(rx) => {
                let r = rx.try_recv().ok()?;
                self.phase = Phase::Idle;
                match r {
                    Ok(o) => {
                        self.chosen = o.items.iter().map(|i| i.key.clone()).collect();
                        self.selected = o.items.first().map(|i| i.key.clone());
                        let line = format!(
                            "Schema Compare: {} to {}: {} differences, {} equal.",
                            o.source,
                            o.target,
                            o.items.len(),
                            o.equal
                        );
                        self.outcome = Some(o);
                        Some(Request::Log(vec![line]))
                    }
                    Err(e) => {
                        self.outcome = None;
                        self.error = Some(e.clone());
                        Some(Request::Log(vec![format!("Schema Compare: failed ({e}).")]))
                    }
                }
            }
            Phase::Applying(rx, script) => {
                let r = rx.try_recv().ok()?;
                let script = script.clone();
                self.phase = Phase::Idle;
                let target = self.target.clone();
                match r {
                    Ok(()) => {
                        let mut log = vec![format!(
                            "Schema Compare: applied {} statements to {target}.",
                            script.len()
                        )];
                        log.extend(script.iter().map(|s| format!("  {}", first_line(s))));
                        self.status = Some(format!("The changes were applied to {target}."));
                        // Compara de novo: o que entrou some da lista.
                        self.compare(ctx);
                        self.status = Some(format!("The changes were applied to {target}."));
                        Some(Request::Applied(target, log))
                    }
                    Err(e) => {
                        self.error = Some(format!("Nothing was changed: {e}"));
                        Some(Request::Log(vec![format!(
                            "Schema Compare: the changes were rolled back ({e})."
                        )]))
                    }
                }
            }
        }
    }

    /// Desenha a janela.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Request> {
        let mut request = self.step(ctx);
        let modal = egui::Modal::new(egui::Id::new("schema_compare")).show(ctx, |ui| {
            ui.set_width(940.0);
            ui.heading("Schema Compare");
            ui.separator();
            let idle = !self.is_busy();
            ui.add_enabled_ui(idle, |ui| {
                ui.horizontal(|ui| {
                    let dbs: Vec<String> = self.databases.clone().unwrap_or_default();
                    ui.label("Source:");
                    egui::ComboBox::from_id_salt("compare_source")
                        .width(200.0)
                        .selected_text(self.source.as_str())
                        .show_ui(ui, |ui| {
                            for d in &dbs {
                                ui.selectable_value(&mut self.source, d.clone(), d.as_str());
                            }
                        });
                    ui.label(egui_phosphor::regular::ARROW_RIGHT);
                    ui.label("Target:");
                    egui::ComboBox::from_id_salt("compare_target")
                        .width(200.0)
                        .selected_text(self.target.as_str())
                        .show_ui(ui, |ui| {
                            for d in &dbs {
                                ui.selectable_value(&mut self.target, d.clone(), d.as_str());
                            }
                        });
                    if ui
                        .button(format!("{} Compare", egui_phosphor::regular::GIT_DIFF))
                        .clicked()
                    {
                        self.compare(ctx);
                    }
                });
            });
            ui.separator();
            let body = egui::vec2(ui.available_width(), 380.0);
            ui.allocate_ui_with_layout(body, Layout::top_down(Align::Min), |ui| {
                ui.set_height(380.0);
                self.body(ui);
            });
            ui.separator();
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            } else if let Some(s) = &self.status {
                ui.label(format!("{} {s}", egui_phosphor::regular::CHECK_CIRCLE));
            }
            let bar = egui::vec2(ui.available_width(), 26.0);
            ui.allocate_ui_with_layout(bar, Layout::right_to_left(Align::Center), |ui| {
                if self.confirming {
                    if ui.button("No").clicked() {
                        self.confirming = false;
                    }
                    if ui.button("Yes").clicked() {
                        self.apply(ctx);
                    }
                    ui.label(format!(
                        "Apply {} statements to {} in one transaction?",
                        self.script().len(),
                        self.target
                    ));
                    return;
                }
                if ui.add_enabled(idle, egui::Button::new("Close")).clicked() {
                    request = Some(Request::Closed);
                }
                let ready = idle && self.outcome.is_some() && !self.chosen.is_empty();
                if ui.add_enabled(ready, egui::Button::new("Apply")).clicked() {
                    self.confirming = true;
                }
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(format!(
                            "{} Generate Script",
                            egui_phosphor::regular::SCROLL
                        )),
                    )
                    .clicked()
                {
                    let sql = self.script().join("\n\n");
                    let header = format!(
                        "-- Schema Compare: changes that make {} like {}.\n\n",
                        self.target, self.source
                    );
                    request = Some(Request::Script(header + &sql + "\n", self.target.clone()));
                }
            });
        });
        if modal.should_close() && request.is_none() && !self.is_busy() {
            request = Some(Request::Closed);
        }
        request
    }

    fn body(&mut self, ui: &mut egui::Ui) {
        if matches!(self.phase, Phase::Comparing(_)) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Comparing...");
            });
            return;
        }
        if matches!(self.phase, Phase::Applying(..)) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Applying...");
            });
            return;
        }
        let Some(o) = &self.outcome else {
            ui.label("Choose the databases and press Compare.");
            return;
        };
        if o.items.is_empty() {
            ui.label(format!(
                "{} The schemas are equal ({} objects compared).",
                egui_phosphor::regular::CHECK_CIRCLE,
                o.equal
            ));
            return;
        }
        ui.horizontal(|ui| {
            ui.strong(format!(
                "{} differences, {} selected ({} objects equal)",
                o.items.len(),
                o.items
                    .iter()
                    .filter(|i| self.chosen.contains(&i.key))
                    .count(),
                o.equal
            ));
            if ui.small_button("All").clicked() {
                self.chosen = o.items.iter().map(|i| i.key.clone()).collect();
            }
            if ui.small_button("None").clicked() {
                self.chosen.clear();
            }
        });
        let h = ui.available_height();
        ui.horizontal_top(|ui| {
            let list = egui::vec2(430.0, h);
            ui.allocate_ui_with_layout(list, Layout::top_down(Align::Min), |ui| {
                ui.set_min_size(list);
                egui::ScrollArea::vertical()
                    .id_salt("compare_items")
                    .max_height(h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("compare_grid")
                            .num_columns(3)
                            .striped(true)
                            .show(ui, |ui| {
                                for item in &o.items {
                                    let mut on = self.chosen.contains(&item.key);
                                    if ui.checkbox(&mut on, "").changed() {
                                        if on {
                                            self.chosen.insert(item.key.clone());
                                        } else {
                                            self.chosen.remove(&item.key);
                                        }
                                    }
                                    let (kind, schema, name) = &item.key;
                                    let label = format!("{} {schema}.{name}", kind.label());
                                    let sel = self.selected.as_ref() == Some(&item.key);
                                    if ui.selectable_label(sel, label).clicked() {
                                        self.selected = Some(item.key.clone());
                                    }
                                    let color = match item.status {
                                        Status::OnlyInSource => {
                                            egui::Color32::from_rgb(40, 150, 70)
                                        }
                                        Status::OnlyInTarget => ui.visuals().error_fg_color,
                                        Status::Different => ui.visuals().warn_fg_color,
                                    };
                                    ui.colored_label(color, item.status.label());
                                    ui.end_row();
                                }
                            });
                    });
            });
            let rest = egui::vec2(ui.available_width(), h);
            ui.allocate_ui_with_layout(rest, Layout::top_down(Align::Min), |ui| {
                ui.set_min_size(rest);
                let item = self
                    .selected
                    .as_ref()
                    .and_then(|k| o.items.iter().find(|i| &i.key == k));
                egui::ScrollArea::both()
                    .id_salt("compare_ddl")
                    .max_height(h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| match item {
                        Some(i) => {
                            for s in &i.statements {
                                ui.monospace(s);
                            }
                        }
                        None => {
                            ui.label("Select a difference to see its script.");
                        }
                    });
            });
        });
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgstudio_core::schema::Model;

    fn row(v: &[&str]) -> Vec<Option<String>> {
        v.iter().map(|s| Some(s.to_string())).collect()
    }

    #[test]
    fn views_and_functions_are_compared_by_definition() {
        let src_views = views(&[
            row(&[
                "public",
                "grandes",
                " SELECT id\n   FROM pedido\n  WHERE total > 100;",
            ]),
            row(&["public", "Nova", " SELECT 1;"]),
        ]);
        let dst_views = views(&[
            row(&[
                "public",
                "grandes",
                " SELECT id\n   FROM pedido\n  WHERE total > 50;",
            ]),
            row(&["public", "velha", " SELECT 2;"]),
        ]);
        let def = |body: &str| {
            format!(
                "CREATE OR REPLACE FUNCTION public.f(a integer)\n RETURNS integer\n LANGUAGE sql\nAS $function$select {body}$function$\n"
            )
        };
        let src_fns = functions(&[row(&["public", "f", "a integer", &def("a"), "f"])]);
        let dst_fns = functions(&[
            row(&["public", "f", "a integer", &def("a"), "f"]),
            row(&[
                "public",
                "p",
                "",
                "CREATE OR REPLACE PROCEDURE public.p() ...",
                "p",
            ]),
        ]);
        let mut s = src_views;
        s.extend(src_fns);
        let mut t = dst_views;
        t.extend(dst_fns);
        let o = Outcome::new(
            "a".into(),
            "b".into(),
            Comparison::new(&Model::default(), &Model::default()),
            s,
            t,
        );
        let summary: Vec<_> = o
            .items
            .iter()
            .map(|i| (i.key.0, i.key.2.as_str(), i.status))
            .collect();
        assert_eq!(
            summary,
            [
                (Kind::View, "Nova", Status::OnlyInSource),
                (Kind::View, "grandes", Status::Different),
                (Kind::View, "velha", Status::OnlyInTarget),
                (Kind::Function, "p()", Status::OnlyInTarget),
            ]
        );
        assert_eq!(o.equal, 1);
        assert_eq!(
            o.items[1].statements,
            [
                "DROP VIEW public.grandes;",
                "CREATE VIEW public.grandes AS\nSELECT id\n   FROM pedido\n  WHERE total > 100;"
            ]
        );
        assert_eq!(o.items[3].statements, ["DROP PROCEDURE public.p();"]);
        let all: BTreeSet<Key> = o.items.iter().map(|i| i.key.clone()).collect();
        let script = o.script(&all);
        assert_eq!(script[0], "DROP VIEW public.grandes;");
        assert_eq!(script[2], "DROP PROCEDURE public.p();");
        assert_eq!(script[3], "CREATE VIEW public.\"Nova\" AS\nSELECT 1;");
        assert_eq!(script.len(), 5);
    }
}
