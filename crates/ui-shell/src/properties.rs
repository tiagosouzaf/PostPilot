//! Janela de propriedades de segurança: "Role Properties" (páginas
//! General e Membership) e "Permissions" de banco, schema, tabela, view e
//! função, como as do SSMS. Script mostra o SQL antes; OK aplica tudo numa
//! transação.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::{self, Align, Layout};
use pgstudio_core::metadata::Securable;
use pgstudio_core::query::{QueryEvent, QuerySession};

use crate::security::{self, GrantTarget, Grants, RoleSpec, Rows};

/// Página da janela.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    Membership,
    Permissions,
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Membership => "Membership",
            Page::Permissions => "Permissions",
        }
    }
}

/// O que a janela mostra, depois de lido.
pub enum Content {
    Role {
        before: Option<RoleSpec>,
        edit: RoleSpec,
        confirm_password: String,
        roles: Vec<String>,
    },
    Grants {
        target: GrantTarget,
        before: Grants,
        edit: Grants,
        /// Quem aparece na lista (com ou sem privilégios).
        listed: BTreeSet<String>,
        selected: Option<String>,
        roles: Vec<String>,
    },
}

enum State {
    Loading,
    Ready(Box<Content>),
    Failed(String),
}

enum Reply {
    Read(Result<Vec<Rows>, String>),
    Applied(Result<(), String>),
}

/// O que a janela pediu à casca.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// Abrir o SQL numa janela de consulta (no banco, se houver).
    Script(String, Option<String>),
    /// As mudanças foram aplicadas: linhas para o Output.
    Applied(Vec<String>),
    /// Fechou sem aplicar.
    Closed,
}

/// A janela aberta.
pub struct SecurityDialog {
    securable: Securable,
    session: Arc<dyn QuerySession>,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    state: State,
    pub page: Page,
    applying: bool,
    error: Option<String>,
    /// Comandos enviados no OK, para o Output.
    sent: Vec<String>,
}

/// Roda `sql` e manda os resultados (ou o erro) por `tx`.
fn read(session: &Arc<dyn QuerySession>, sql: String, tx: Sender<Reply>, ctx: &egui::Context) {
    let ctx = ctx.clone();
    let mut results: Vec<Rows> = Vec::new();
    let mut current: Option<Rows> = None;
    let mut error = None;
    session.execute(
        sql,
        Box::new(move |event| match event {
            QueryEvent::Columns(_) => current = Some(Vec::new()),
            QueryEvent::Row(r) => {
                if let Some(rows) = &mut current {
                    rows.push(r);
                }
            }
            QueryEvent::CommandComplete { .. } => results.push(current.take().unwrap_or_default()),
            QueryEvent::Error(e) => error = Some(e.message),
            QueryEvent::Finished { .. } => {
                let reply = match error.take() {
                    Some(e) => Err(e),
                    None => Ok(std::mem::take(&mut results)),
                };
                let _ = tx.send(Reply::Read(reply));
                ctx.request_repaint();
            }
            QueryEvent::Notice(_) => {}
        }),
    );
}

impl SecurityDialog {
    /// Abre a janela e começa a leitura. `session` é do servidor (roles) ou
    /// do banco do objeto.
    pub fn open(securable: Securable, session: Arc<dyn QuerySession>, ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let sql = match &securable {
            Securable::Role(oid) => security::role_read_sql(Some(*oid)),
            Securable::NewRole => security::role_read_sql(None),
            other => security::grants_read_sql(other).unwrap_or_default(),
        };
        read(&session, sql, tx.clone(), ctx);
        let page = match securable {
            Securable::Role(_) | Securable::NewRole => Page::General,
            _ => Page::Permissions,
        };
        Self {
            securable,
            session,
            tx,
            rx,
            state: State::Loading,
            page,
            applying: false,
            error: None,
            sent: Vec::new(),
        }
    }

    pub fn securable(&self) -> &Securable {
        &self.securable
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, State::Loading)
    }

    pub fn is_applying(&self) -> bool {
        self.applying
    }

    pub fn error(&self) -> Option<&str> {
        match &self.state {
            State::Failed(e) => Some(e),
            _ => self.error.as_deref(),
        }
    }

    /// O conteúdo lido, para editar (testes).
    pub fn content_mut(&mut self) -> Option<&mut Content> {
        match &mut self.state {
            State::Ready(c) => Some(c),
            _ => None,
        }
    }

    fn content(&self) -> Option<&Content> {
        match &self.state {
            State::Ready(c) => Some(c),
            _ => None,
        }
    }

    pub fn title(&self) -> String {
        match self.content() {
            Some(Content::Role {
                before: Some(b), ..
            }) => {
                format!("Role Properties - {}", b.name)
            }
            Some(Content::Grants { target, .. }) => {
                format!("{} Properties - {}", target.kind.label(), target.name)
            }
            _ => match self.securable {
                Securable::NewRole => "Role - New".to_string(),
                Securable::Role(_) => "Role Properties".to_string(),
                _ => "Properties".to_string(),
            },
        }
    }

    fn pages(&self) -> &'static [Page] {
        match self.securable {
            Securable::Role(_) | Securable::NewRole => &[Page::General, Page::Membership],
            _ => &[Page::Permissions],
        }
    }

    /// O SQL das mudanças; `reveal_password` falso no Script.
    pub fn statements(&self, reveal_password: bool) -> Vec<String> {
        match self.content() {
            Some(Content::Role { before, edit, .. }) => {
                security::role_sql(before.as_ref(), edit, reveal_password)
            }
            Some(Content::Grants {
                target,
                before,
                edit,
                ..
            }) => security::grant_sql(target, before, edit),
            _ => Vec::new(),
        }
    }

    /// Por que o OK não pode seguir, se for o caso.
    fn invalid(&self) -> Option<&'static str> {
        match self.content() {
            Some(Content::Role {
                edit,
                confirm_password,
                ..
            }) => {
                if edit.name.trim().is_empty() {
                    Some("The role needs a name.")
                } else if edit.password != *confirm_password {
                    Some("The password and its confirmation do not match.")
                } else {
                    None
                }
            }
            Some(_) => None,
            None => Some("The properties are not loaded."),
        }
    }

    /// Aplica as mudanças numa transação. Sem mudanças, só fecha.
    pub fn apply(&mut self, ctx: &egui::Context) -> Option<Request> {
        if let Some(why) = self.invalid() {
            self.error = Some(why.to_string());
            return None;
        }
        let statements = self.statements(true);
        if statements.is_empty() {
            return Some(Request::Closed);
        }
        self.applying = true;
        self.error = None;
        self.sent = self.statements(false);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let session = self.session.clone();
        let mut error = None;
        self.session.execute(
            security::in_transaction(&statements),
            Box::new(move |event| match event {
                QueryEvent::Error(e) => error = Some(e.message),
                QueryEvent::Finished { .. } => {
                    let result = match error.take() {
                        Some(e) => {
                            // O erro deixou a transação aberta.
                            session.execute("ROLLBACK".into(), Box::new(|_| {}));
                            Err(e)
                        }
                        None => Ok(()),
                    };
                    let _ = tx.send(Reply::Applied(result));
                    ctx.request_repaint();
                }
                _ => {}
            }),
        );
        None
    }

    /// O pedido de Script: o SQL (com a senha mascarada) para uma janela de
    /// consulta.
    pub fn script(&mut self) -> Option<Request> {
        if let Some(why) = self.invalid() {
            self.error = Some(why.to_string());
            return None;
        }
        let statements = self.statements(false);
        if statements.is_empty() {
            self.error = Some("There are no changes to script.".to_string());
            return None;
        }
        let mut sql = statements.join(";\n");
        sql.push_str(";\n");
        Some(Request::Script(
            sql,
            self.securable.database().map(str::to_string),
        ))
    }

    /// Recebe as respostas.
    pub fn step(&mut self) -> Option<Request> {
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Read(Ok(results)) => {
                    self.state = match self.load(&results) {
                        Ok(c) => State::Ready(Box::new(c)),
                        Err(e) => State::Failed(e),
                    };
                }
                Reply::Read(Err(e)) => self.state = State::Failed(e),
                Reply::Applied(Ok(())) => {
                    self.applying = false;
                    let mut log: Vec<String> = vec![format!("{}: changes applied.", self.title())];
                    log.extend(self.sent.iter().map(|s| format!("  {s};")));
                    return Some(Request::Applied(log));
                }
                Reply::Applied(Err(e)) => {
                    self.applying = false;
                    self.error = Some(e);
                }
            }
        }
        None
    }

    fn load(&self, results: &[Rows]) -> Result<Content, String> {
        match self.securable {
            Securable::Role(_) | Securable::NewRole => {
                let existing = matches!(self.securable, Securable::Role(_));
                let (before, roles) = security::parse_role(results, existing)?;
                let edit = before.clone().unwrap_or_else(RoleSpec::new_login);
                Ok(Content::Role {
                    before,
                    edit,
                    confirm_password: String::new(),
                    roles,
                })
            }
            _ => {
                let (target, before, roles) = security::parse_grants(results)?;
                let listed: BTreeSet<String> = before.keys().map(|(w, _)| w.clone()).collect();
                let selected = listed.iter().next().cloned();
                Ok(Content::Grants {
                    target,
                    edit: before.clone(),
                    before,
                    listed,
                    selected,
                    roles,
                })
            }
        }
    }

    /// Desenha a janela; devolve o que ela pediu.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Request> {
        let mut request = self.step();
        let title = self.title();
        // Modal, como as janelas de propriedades do SSMS.
        let modal = egui::Modal::new(egui::Id::new("security_properties")).show(ctx, |ui| {
            ui.set_width(660.0);
            ui.heading(title);
            ui.separator();
            // Altura fixa: com o separador vertical, a janela cresceria a
            // cada frame.
            let body = egui::vec2(ui.available_width(), 360.0);
            ui.allocate_ui_with_layout(body, Layout::left_to_right(Align::Min), |ui| {
                ui.set_height(360.0);
                ui.vertical(|ui| {
                    ui.set_width(120.0);
                    ui.weak("Select a page");
                    for &p in self.pages() {
                        if ui.selectable_label(self.page == p, p.label()).clicked() {
                            self.page = p;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| match &mut self.state {
                    State::Loading => {
                        ui.spinner();
                    }
                    State::Failed(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                    }
                    State::Ready(content) => page(ui, self.page, content),
                });
            });
            ui.separator();
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            let bar = egui::vec2(ui.available_width(), 26.0);
            ui.allocate_ui_with_layout(bar, Layout::right_to_left(Align::Center), |ui| {
                let ready = matches!(self.state, State::Ready(_)) && !self.applying;
                if ui.button("Cancel").clicked() {
                    request = Some(Request::Closed);
                }
                if ui.add_enabled(ready, egui::Button::new("OK")).clicked() {
                    request = self.apply(ctx).or(request.take());
                }
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(format!("{} Script", egui_phosphor::regular::SCROLL)),
                    )
                    .clicked()
                {
                    request = self.script().or(request.take());
                }
                if self.applying {
                    ui.spinner();
                }
            });
        });
        if modal.should_close() && request.is_none() && !self.applying {
            request = Some(Request::Closed);
        }
        request
    }
}

fn page(ui: &mut egui::Ui, page: Page, content: &mut Content) {
    match (page, content) {
        (
            Page::General,
            Content::Role {
                edit,
                confirm_password,
                ..
            },
        ) => {
            egui::Grid::new("role_general")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Role name:");
                    ui.add(egui::TextEdit::singleline(&mut edit.name).desired_width(240.0));
                    ui.end_row();
                    ui.label("Password:");
                    ui.add(
                        egui::TextEdit::singleline(&mut edit.password)
                            .password(true)
                            .hint_text("unchanged")
                            .desired_width(240.0),
                    );
                    ui.end_row();
                    ui.label("Confirm password:");
                    ui.add(
                        egui::TextEdit::singleline(confirm_password)
                            .password(true)
                            .desired_width(240.0),
                    );
                    ui.end_row();
                    ui.label("Valid until:");
                    ui.add(
                        egui::TextEdit::singleline(&mut edit.valid_until)
                            .hint_text("never")
                            .desired_width(240.0),
                    );
                    ui.end_row();
                    ui.label("Connection limit:");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut edit.connection_limit).range(-1..=100_000),
                        );
                        ui.weak("-1 means no limit");
                    });
                    ui.end_row();
                });
            ui.add_space(8.0);
            ui.strong("Attributes");
            ui.checkbox(&mut edit.can_login, "Can log in (LOGIN)");
            ui.checkbox(&mut edit.superuser, "Superuser");
            ui.checkbox(&mut edit.create_db, "Create databases");
            ui.checkbox(&mut edit.create_role, "Create roles");
            ui.checkbox(
                &mut edit.inherit,
                "Inherit privileges of the roles it is a member of",
            );
            ui.checkbox(&mut edit.replication, "Replication");
            ui.checkbox(&mut edit.bypass_rls, "Bypass row-level security");
        }
        (Page::Membership, Content::Role { edit, roles, .. }) => {
            let me = edit.name.clone();
            let (left, right) = (&mut edit.member_of, &mut edit.members);
            ui.columns(2, |cols| {
                let [a, b] = cols else { return };
                role_list(a, "Member of", roles, &me, left);
                role_list(b, "Members", roles, &me, right);
            });
        }
        (
            Page::Permissions,
            Content::Grants {
                target,
                edit,
                listed,
                selected,
                roles,
                ..
            },
        ) => {
            ui.label(format!(
                "{}: {}    Owner: {}",
                target.kind.label(),
                target.name,
                target.owner
            ));
            ui.add_space(4.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(180.0);
                    ui.strong("Users or roles");
                    egui::ScrollArea::vertical()
                        .id_salt("grantees")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for who in listed.iter() {
                                if ui
                                    .selectable_label(
                                        selected.as_deref() == Some(who),
                                        who.as_str(),
                                    )
                                    .clicked()
                                {
                                    *selected = Some(who.clone());
                                }
                            }
                        });
                    let mut add = None;
                    egui::ComboBox::from_id_salt("grantee_add")
                        .selected_text("Add...")
                        .width(170.0)
                        .show_ui(ui, |ui| {
                            let candidates = std::iter::once("PUBLIC".to_string())
                                .chain(roles.iter().cloned())
                                .filter(|r| !listed.contains(r));
                            for r in candidates {
                                if ui.selectable_label(false, r.as_str()).clicked() {
                                    add = Some(r);
                                }
                            }
                        });
                    if let Some(r) = add {
                        listed.insert(r.clone());
                        *selected = Some(r);
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    let Some(who) = selected.clone() else {
                        ui.weak("Select or add a user or role.");
                        return;
                    };
                    ui.strong(format!("Permissions for {who}"));
                    egui::Grid::new("privileges")
                        .num_columns(3)
                        .striped(true)
                        .spacing([18.0, 4.0])
                        .show(ui, |ui| {
                            ui.strong("Permission");
                            ui.strong("Grant");
                            ui.strong("With Grant");
                            ui.end_row();
                            for &p in target.kind.privileges() {
                                let key = (who.clone(), p.to_string());
                                let mut grant = edit.contains_key(&key);
                                let mut option = edit.get(&key).copied().unwrap_or(false);
                                ui.label(p);
                                let g = ui.add(egui::Checkbox::without_text(&mut grant));
                                g.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::WidgetType::Checkbox,
                                        true,
                                        grant,
                                        format!("Grant {p}"),
                                    )
                                });
                                let w = ui.add(egui::Checkbox::without_text(&mut option));
                                w.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::WidgetType::Checkbox,
                                        true,
                                        option,
                                        format!("With Grant {p}"),
                                    )
                                });
                                if g.changed() {
                                    if grant {
                                        edit.insert(key, false);
                                    } else {
                                        edit.remove(&key);
                                    }
                                } else if w.changed() {
                                    // "With Grant" implica "Grant".
                                    edit.insert(key, option);
                                }
                                ui.end_row();
                            }
                        });
                });
            });
        }
        _ => {}
    }
}

/// Lista de roles com caixas de marcar (página Membership).
fn role_list(
    ui: &mut egui::Ui,
    title: &str,
    roles: &[String],
    me: &str,
    set: &mut BTreeSet<String>,
) {
    ui.strong(title);
    egui::ScrollArea::vertical()
        .id_salt(title)
        .max_height(300.0)
        .show(ui, |ui| {
            for r in roles.iter().filter(|r| *r != me) {
                let mut on = set.contains(r);
                if ui.checkbox(&mut on, r.as_str()).changed() {
                    if on {
                        set.insert(r.clone());
                    } else {
                        set.remove(r);
                    }
                }
            }
        });
}
