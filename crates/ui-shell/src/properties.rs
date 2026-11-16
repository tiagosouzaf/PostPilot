//! Janela de propriedades de segurança: "Role Properties" (páginas
//! General e Membership) e "Permissions" de banco, schema, tabela, view e
//! função, como as do SSMS. Script mostra o SQL antes; OK aplica tudo numa
//! transação.

use pgstudio_i18n::{tr, trf};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::{self, Align, Layout};
use pgstudio_core::connection::Engine;
use pgstudio_core::metadata::{PrincipalKind, Securable};
use pgstudio_core::query::{QueryEvent, QuerySession};

use crate::security::{self, GrantTarget, Grants, RoleSpec, Rows};
use crate::security_mssql::{
    self, PermState, PermTarget, Permissions, PrincipalLists, PrincipalSpec,
};

/// Página da janela.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    Membership,
    Permissions,
}

impl Page {
    fn label(self) -> &'static str {
        tr(match self {
            Page::General => "General",
            Page::Membership => "Membership",
            Page::Permissions => "Permissions",
        })
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
    /// Permissões de um objeto do SQL Server (com DENY).
    Permissions {
        target: PermTarget,
        before: Permissions,
        edit: Permissions,
        listed: BTreeSet<String>,
        selected: Option<String>,
        principals: Vec<String>,
    },
    /// Login, usuário ou role do SQL Server.
    Principal {
        kind: PrincipalKind,
        before: Option<PrincipalSpec>,
        edit: PrincipalSpec,
        confirm_password: String,
        lists: PrincipalLists,
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
            Securable::Principal { kind, name, .. } => {
                security_mssql::principal_read_sql(*kind, name.as_deref())
            }
            other if session.engine() == Engine::SqlServer => {
                security_mssql::permissions_read_sql(other).unwrap_or_default()
            }
            other => security::grants_read_sql(other).unwrap_or_default(),
        };
        read(&session, sql, tx.clone(), ctx);
        let page = match securable {
            Securable::Role(_) | Securable::NewRole | Securable::Principal { .. } => Page::General,
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
                trf!("Role Properties - {name}", name = b.name)
            }
            Some(Content::Grants { target, .. }) => {
                trf!(
                    "{kind} Properties - {name}",
                    kind = target.kind.label(),
                    name = target.name
                )
            }
            Some(Content::Permissions { target, .. }) => {
                trf!(
                    "{kind} Properties - {name}",
                    kind = target.kind,
                    name = target.name
                )
            }
            Some(Content::Principal {
                kind,
                before: Some(b),
                ..
            }) => {
                let name = &b.name;
                match kind {
                    PrincipalKind::Login => trf!("Login Properties - {name}", name = name),
                    PrincipalKind::ServerRole => {
                        trf!("Server Role Properties - {name}", name = name)
                    }
                    PrincipalKind::User => trf!("Database User - {name}", name = name),
                    PrincipalKind::DatabaseRole => {
                        trf!("Database Role Properties - {name}", name = name)
                    }
                }
            }
            _ => match self.securable {
                Securable::Principal { kind, .. } => tr(match kind {
                    PrincipalKind::Login => "Login - New",
                    PrincipalKind::ServerRole => "Server Role - New",
                    PrincipalKind::User => "Database User - New",
                    PrincipalKind::DatabaseRole => "Database Role - New",
                })
                .to_string(),
                Securable::NewRole => tr("Role - New").to_string(),
                Securable::Role(_) => tr("Role Properties").to_string(),
                _ => tr("Properties").to_string(),
            },
        }
    }

    fn pages(&self) -> &'static [Page] {
        match self.securable {
            Securable::Role(_) | Securable::NewRole | Securable::Principal { .. } => {
                &[Page::General, Page::Membership]
            }
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
            Some(Content::Permissions {
                target,
                before,
                edit,
                ..
            }) => security_mssql::permission_sql(target, before, edit),
            Some(Content::Principal {
                kind, before, edit, ..
            }) => security_mssql::principal_sql(*kind, before.as_ref(), edit, reveal_password),
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
                    Some(tr("The role needs a name."))
                } else if edit.password != *confirm_password {
                    Some(tr("The password and its confirmation do not match."))
                } else {
                    None
                }
            }
            Some(Content::Principal {
                kind,
                before,
                edit,
                confirm_password,
                ..
            }) => {
                let login = *kind == PrincipalKind::Login;
                if edit.name.trim().is_empty() {
                    Some(tr("The name is required."))
                } else if login && edit.password != *confirm_password {
                    Some(tr("The password and its confirmation do not match."))
                } else if login && before.is_none() && edit.password.is_empty() {
                    Some(tr("A new login needs a password."))
                } else {
                    None
                }
            }
            Some(_) => None,
            None => Some(tr("The properties are not loaded.")),
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
        let tsql = self.session.engine() == Engine::SqlServer;
        let (sql, rollback) = if tsql {
            (
                security_mssql::in_transaction(&statements),
                security_mssql::ROLLBACK,
            )
        } else {
            (security::in_transaction(&statements), "ROLLBACK")
        };
        self.session.execute(
            sql,
            Box::new(move |event| match event {
                QueryEvent::Error(e) => error = Some(e.message),
                QueryEvent::Finished { .. } => {
                    let result = match error.take() {
                        Some(e) => {
                            // O erro deixou a transação aberta.
                            session.execute(rollback.into(), Box::new(|_| {}));
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
            self.error = Some(tr("There are no changes to script.").to_string());
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
                    let mut log: Vec<String> =
                        vec![trf!("{title}: changes applied.", title = self.title())];
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
            Securable::Principal { kind, ref name, .. } => {
                let (before, lists) =
                    security_mssql::parse_principal(kind, results, name.is_some())?;
                let edit = before.clone().unwrap_or_else(|| PrincipalSpec::new(kind));
                Ok(Content::Principal {
                    kind,
                    before,
                    edit,
                    confirm_password: String::new(),
                    lists,
                })
            }
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
            _ if self.session.engine() == Engine::SqlServer => {
                let (target, before, principals) = security_mssql::parse_permissions(results)?;
                let listed: BTreeSet<String> = before.keys().map(|(w, _)| w.clone()).collect();
                let selected = listed.iter().next().cloned();
                Ok(Content::Permissions {
                    target,
                    edit: before.clone(),
                    before,
                    listed,
                    selected,
                    principals,
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
        let modal = crate::theme::modal(egui::Id::new("security_properties")).show(ctx, |ui| {
            crate::fit::dialog_width(ui, 660.0);
            ui.heading(title);
            ui.separator();
            // Altura fixa: com o separador vertical, a janela cresceria a
            // cada frame.
            let body = egui::vec2(ui.available_width(), 360.0);
            ui.allocate_ui_with_layout(body, Layout::left_to_right(Align::Min), |ui| {
                ui.set_height(360.0);
                ui.vertical(|ui| {
                    ui.set_width(120.0);
                    ui.weak(tr("Select a page"));
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
                if ui.button(tr("Cancel")).clicked() {
                    request = Some(Request::Closed);
                }
                if ui.add_enabled(ready, egui::Button::new(tr("OK"))).clicked() {
                    request = self.apply(ctx).or(request.take());
                }
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(format!(
                            "{} {}",
                            egui_phosphor::regular::SCROLL,
                            tr("Script")
                        )),
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
                    ui.label(tr("Role name:"));
                    ui.add(egui::TextEdit::singleline(&mut edit.name).desired_width(240.0));
                    ui.end_row();
                    ui.label(tr("Password:"));
                    ui.add(
                        egui::TextEdit::singleline(&mut edit.password)
                            .password(true)
                            .hint_text(tr("unchanged"))
                            .desired_width(240.0),
                    );
                    ui.end_row();
                    ui.label(tr("Confirm password:"));
                    ui.add(
                        egui::TextEdit::singleline(confirm_password)
                            .password(true)
                            .desired_width(240.0),
                    );
                    ui.end_row();
                    ui.label(tr("Valid until:"));
                    ui.add(
                        egui::TextEdit::singleline(&mut edit.valid_until)
                            .hint_text(tr("never"))
                            .desired_width(240.0),
                    );
                    ui.end_row();
                    ui.label(tr("Connection limit:"));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut edit.connection_limit).range(-1..=100_000),
                        );
                        ui.weak(tr("-1 means no limit"));
                    });
                    ui.end_row();
                });
            ui.add_space(8.0);
            ui.strong(tr("Attributes"));
            ui.checkbox(&mut edit.can_login, tr("Can log in (LOGIN)"));
            ui.checkbox(&mut edit.superuser, tr("Superuser"));
            ui.checkbox(&mut edit.create_db, tr("Create databases"));
            ui.checkbox(&mut edit.create_role, tr("Create roles"));
            ui.checkbox(
                &mut edit.inherit,
                tr("Inherit privileges of the roles it is a member of"),
            );
            ui.checkbox(&mut edit.replication, tr("Replication"));
            ui.checkbox(&mut edit.bypass_rls, tr("Bypass row-level security"));
        }
        (Page::Membership, Content::Role { edit, roles, .. }) => {
            let me = edit.name.clone();
            let (left, right) = (&mut edit.member_of, &mut edit.members);
            ui.columns(2, |cols| {
                let [a, b] = cols else { return };
                role_list(a, tr("Member of"), roles, &me, left);
                role_list(b, tr("Members"), roles, &me, right);
            });
        }
        (
            Page::General,
            Content::Principal {
                kind,
                before,
                edit,
                confirm_password,
                lists,
            },
        ) => principal_general(ui, *kind, before.is_some(), edit, confirm_password, lists),
        (
            Page::Membership,
            Content::Principal {
                kind, edit, lists, ..
            },
        ) => {
            let me = edit.name.clone();
            match kind {
                PrincipalKind::Login => {
                    role_list(
                        ui,
                        tr("Server roles"),
                        &lists.roles,
                        &me,
                        &mut edit.member_of,
                    );
                }
                PrincipalKind::User => {
                    role_list(
                        ui,
                        tr("Database roles"),
                        &lists.roles,
                        &me,
                        &mut edit.member_of,
                    );
                }
                PrincipalKind::ServerRole | PrincipalKind::DatabaseRole => {
                    let (left, right) = (&mut edit.member_of, &mut edit.members);
                    ui.columns(2, |cols| {
                        let [a, b] = cols else { return };
                        role_list(a, tr("Member of"), &lists.roles, &me, left);
                        role_list(b, tr("Members"), &lists.candidates, &me, right);
                    });
                }
            }
        }
        (
            Page::Permissions,
            Content::Permissions {
                target,
                edit,
                listed,
                selected,
                principals,
                ..
            },
        ) => permissions_page(ui, target, edit, listed, selected, principals),
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
            ui.label(trf!(
                "{kind}: {name}    Owner: {owner}",
                kind = target.kind.label(),
                name = target.name,
                owner = target.owner
            ));
            ui.add_space(4.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(180.0);
                    ui.strong(tr("Users or roles"));
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
                        .selected_text(tr("Add..."))
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
                        ui.weak(tr("Select or add a user or role."));
                        return;
                    };
                    ui.strong(trf!("Permissions for {who}", who = who));
                    egui::Grid::new("privileges")
                        .num_columns(3)
                        .striped(true)
                        .spacing([18.0, 4.0])
                        .show(ui, |ui| {
                            ui.strong(tr("Permission"));
                            ui.strong(tr("Grant"));
                            ui.strong(tr("With Grant"));
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
                                        trf!("Grant {privilege}", privilege = p),
                                    )
                                });
                                let w = ui.add(egui::Checkbox::without_text(&mut option));
                                w.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::WidgetType::Checkbox,
                                        true,
                                        option,
                                        trf!("With Grant {privilege}", privilege = p),
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

/// Página Permissions de um objeto do SQL Server: quem, e para cada
/// permissão as caixas Grant, With Grant e Deny do SSMS.
fn permissions_page(
    ui: &mut egui::Ui,
    target: &PermTarget,
    edit: &mut Permissions,
    listed: &mut BTreeSet<String>,
    selected: &mut Option<String>,
    principals: &[String],
) {
    ui.label(trf!(
        "{kind}: {name}    Owner: {owner}",
        kind = target.kind,
        name = target.name,
        owner = target.owner
    ));
    ui.add_space(4.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(180.0);
            ui.strong(tr("Users or roles"));
            egui::ScrollArea::vertical()
                .id_salt("grantees")
                .max_height(220.0)
                .show(ui, |ui| {
                    for who in listed.iter() {
                        if ui
                            .selectable_label(selected.as_deref() == Some(who), who.as_str())
                            .clicked()
                        {
                            *selected = Some(who.clone());
                        }
                    }
                });
            let mut add = None;
            egui::ComboBox::from_id_salt("grantee_add")
                .selected_text(tr("Add..."))
                .width(170.0)
                .show_ui(ui, |ui| {
                    for r in principals.iter().filter(|r| !listed.contains(*r)) {
                        if ui.selectable_label(false, r.as_str()).clicked() {
                            add = Some(r.clone());
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
                ui.weak(tr("Select or add a user or role."));
                return;
            };
            ui.strong(trf!("Permissions for {who}", who = who));
            egui::ScrollArea::vertical()
                .id_salt("permissions")
                .max_height(280.0)
                .show(ui, |ui| {
                    egui::Grid::new("permissions")
                        .num_columns(4)
                        .striped(true)
                        .spacing([18.0, 4.0])
                        .show(ui, |ui| {
                            ui.strong(tr("Permission"));
                            ui.strong(tr("Grant"));
                            ui.strong(tr("With Grant"));
                            ui.strong(tr("Deny"));
                            ui.end_row();
                            for p in &target.privileges {
                                let key = (who.clone(), p.clone());
                                let state = edit.get(&key).copied();
                                let mut grant =
                                    matches!(state, Some(PermState::Grant | PermState::WithGrant));
                                let mut option = state == Some(PermState::WithGrant);
                                let mut deny = state == Some(PermState::Deny);
                                ui.label(p.as_str());
                                let boxed = |ui: &mut egui::Ui, on: &mut bool, name: String| {
                                    let value = *on;
                                    let r = ui.add(egui::Checkbox::without_text(on));
                                    r.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::Checkbox,
                                            true,
                                            value,
                                            name.clone(),
                                        )
                                    });
                                    r.changed()
                                };
                                let g =
                                    boxed(ui, &mut grant, trf!("Grant {privilege}", privilege = p));
                                let w = boxed(
                                    ui,
                                    &mut option,
                                    trf!("With Grant {privilege}", privilege = p),
                                );
                                let d =
                                    boxed(ui, &mut deny, trf!("Deny {privilege}", privilege = p));
                                // "With Grant" implica "Grant"; "Deny" tira os dois.
                                let new = if d {
                                    Some(deny.then_some(PermState::Deny))
                                } else if w {
                                    Some(Some(if option {
                                        PermState::WithGrant
                                    } else {
                                        PermState::Grant
                                    }))
                                } else if g {
                                    Some(grant.then_some(PermState::Grant))
                                } else {
                                    None
                                };
                                match new {
                                    Some(Some(state)) => {
                                        edit.insert(key, state);
                                    }
                                    Some(None) => {
                                        edit.remove(&key);
                                    }
                                    None => {}
                                }
                                ui.end_row();
                            }
                        });
                });
        });
    });
}

/// Caixa de escolha de um nome entre `options`; `empty` é o texto de
/// "nenhum" (None: sempre há um escolhido).
fn name_combo(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut String,
    options: &[String],
    empty: Option<&str>,
) {
    let shown = if value.is_empty() {
        empty.unwrap_or_default().to_string()
    } else {
        value.clone()
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(240.0)
        .show_ui(ui, |ui| {
            if let Some(empty) = empty {
                ui.selectable_value(value, String::new(), empty);
            }
            for o in options {
                ui.selectable_value(value, o.clone(), o.as_str());
            }
        });
}

/// Página General de um login, usuário ou role do SQL Server.
fn principal_general(
    ui: &mut egui::Ui,
    kind: PrincipalKind,
    existing: bool,
    edit: &mut PrincipalSpec,
    confirm_password: &mut String,
    lists: &PrincipalLists,
) {
    egui::Grid::new("principal_general")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(tr(match kind {
                PrincipalKind::Login => "Login name:",
                PrincipalKind::User => "User name:",
                PrincipalKind::ServerRole | PrincipalKind::DatabaseRole => "Role name:",
            }));
            ui.add_enabled(
                !edit.fixed,
                egui::TextEdit::singleline(&mut edit.name).desired_width(240.0),
            );
            ui.end_row();
            match kind {
                PrincipalKind::Login => {
                    if edit.sql_login {
                        ui.label(tr("Password:"));
                        let mut field = egui::TextEdit::singleline(&mut edit.password)
                            .password(true)
                            .desired_width(240.0);
                        if existing {
                            field = field.hint_text(tr("unchanged"));
                        }
                        ui.add(field);
                        ui.end_row();
                        ui.label(tr("Confirm password:"));
                        ui.add(
                            egui::TextEdit::singleline(confirm_password)
                                .password(true)
                                .desired_width(240.0),
                        );
                        ui.end_row();
                    }
                    ui.label(tr("Default database:"));
                    name_combo(
                        ui,
                        "login_database",
                        &mut edit.default_database,
                        &lists.databases,
                        None,
                    );
                    ui.end_row();
                }
                PrincipalKind::User => {
                    ui.label(tr("Login name:"));
                    name_combo(
                        ui,
                        "user_login",
                        &mut edit.login,
                        &lists.logins,
                        Some(tr("(without login)")),
                    );
                    ui.end_row();
                    ui.label(tr("Default schema:"));
                    name_combo(
                        ui,
                        "user_schema",
                        &mut edit.default_schema,
                        &lists.schemas,
                        Some(tr("(server default)")),
                    );
                    ui.end_row();
                }
                PrincipalKind::ServerRole | PrincipalKind::DatabaseRole => {}
            }
        });
    ui.add_space(8.0);
    match kind {
        PrincipalKind::Login => {
            if edit.sql_login {
                ui.checkbox(&mut edit.check_policy, tr("Enforce password policy"));
            }
            ui.checkbox(&mut edit.disabled, tr("Login is disabled"));
        }
        PrincipalKind::ServerRole | PrincipalKind::DatabaseRole if edit.fixed => {
            ui.weak(tr("This is a fixed role: only its members can change."));
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
