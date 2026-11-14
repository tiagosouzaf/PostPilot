//! Segurança: propriedades de roles e permissões de objetos, como as
//! páginas General, Membership e Permissions das janelas de propriedades
//! do SSMS.
//!
//! Aqui ficam as leituras (SQL e o que volta dele) e o SQL das mudanças,
//! sem interface: [`role_sql`] compara a role como estava com a editada, e
//! [`grant_sql`] faz o mesmo com as permissões, em GRANT e REVOKE.

use pgstudio_i18n::tr;
use std::collections::{BTreeMap, BTreeSet};

use pgstudio_core::metadata::Securable;
use pgstudio_core::schema::quote_ident;

/// Uma role como a janela mostra.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleSpec {
    pub name: String,
    /// LOGIN: a role entra no servidor (um "login" do SSMS).
    pub can_login: bool,
    pub superuser: bool,
    pub create_db: bool,
    pub create_role: bool,
    pub inherit: bool,
    pub replication: bool,
    pub bypass_rls: bool,
    /// -1 é sem limite.
    pub connection_limit: i32,
    /// `VALID UNTIL`, como o servidor escreve (`2027-01-01 00:00:00+00`),
    /// ou vazio.
    pub valid_until: String,
    /// Senha nova; vazia não muda.
    pub password: String,
    /// Roles de que esta é membro.
    pub member_of: BTreeSet<String>,
    /// Roles que são membros desta.
    pub members: BTreeSet<String>,
}

impl RoleSpec {
    /// Uma role nova, com os padrões do `CREATE ROLE` mais LOGIN (o caso
    /// comum, como o "New Login" do SSMS).
    pub fn new_login() -> Self {
        RoleSpec {
            can_login: true,
            inherit: true,
            connection_limit: -1,
            ..Default::default()
        }
    }
}

fn lit(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Leitura de uma role (pelo oid) e da lista de roles do servidor:
/// atributos, de quem ela é membro, quem é membro dela e todas as roles.
pub fn role_read_sql(oid: Option<u32>) -> String {
    let all = "SELECT rolname FROM pg_roles ORDER BY rolname";
    match oid {
        None => all.to_string(),
        Some(oid) => format!(
            "SELECT rolname, rolcanlogin, rolsuper, rolcreatedb, rolcreaterole, rolinherit, \
             rolreplication, rolbypassrls, rolconnlimit, rolvaliduntil::text \
             FROM pg_roles WHERE oid = {oid};\n\
             SELECT b.rolname FROM pg_auth_members m JOIN pg_roles b ON b.oid = m.roleid \
             WHERE m.member = {oid} ORDER BY 1;\n\
             SELECT r.rolname FROM pg_auth_members m JOIN pg_roles r ON r.oid = m.member \
             WHERE m.roleid = {oid} ORDER BY 1;\n\
             {all}"
        ),
    }
}

/// Linhas de um resultado, como texto.
pub type Rows = Vec<Vec<Option<String>>>;

fn text(r: &[Option<String>], i: usize) -> String {
    r.get(i).cloned().flatten().unwrap_or_default()
}

/// Lê o que [`role_read_sql`] devolveu: a role (None para uma nova) e
/// todas as roles do servidor.
pub fn parse_role(
    results: &[Rows],
    existing: bool,
) -> Result<(Option<RoleSpec>, Vec<String>), String> {
    let names = |rows: &Rows| rows.iter().map(|r| text(r, 0)).collect::<Vec<_>>();
    if !existing {
        let all = results.first().ok_or(tr("The roles could not be read."))?;
        return Ok((None, names(all)));
    }
    let [role, member_of, members, all] = results else {
        return Err(tr("The role could not be read.").into());
    };
    let r = role.first().ok_or(tr("The role no longer exists."))?;
    let flag = |i: usize| text(r, i) == "t";
    let spec = RoleSpec {
        name: text(r, 0),
        can_login: flag(1),
        superuser: flag(2),
        create_db: flag(3),
        create_role: flag(4),
        inherit: flag(5),
        replication: flag(6),
        bypass_rls: flag(7),
        connection_limit: text(r, 8).parse().unwrap_or(-1),
        valid_until: text(r, 9),
        password: String::new(),
        member_of: names(member_of).into_iter().collect(),
        members: names(members).into_iter().collect(),
    };
    Ok((Some(spec), names(all)))
}

/// Opções do `CREATE ROLE`/`ALTER ROLE` que mudaram de `before` (todas,
/// numa role nova) para `after`.
fn role_options(before: Option<&RoleSpec>, after: &RoleSpec, reveal_password: bool) -> Vec<String> {
    let mut out = Vec::new();
    let flags = [
        (|r: &RoleSpec| r.can_login) as fn(&RoleSpec) -> bool,
        |r| r.superuser,
        |r| r.create_db,
        |r| r.create_role,
        |r| r.inherit,
        |r| r.replication,
        |r| r.bypass_rls,
    ];
    let words = [
        "LOGIN",
        "SUPERUSER",
        "CREATEDB",
        "CREATEROLE",
        "INHERIT",
        "REPLICATION",
        "BYPASSRLS",
    ];
    for (get, word) in flags.iter().zip(words) {
        let value = get(after);
        if before.is_none_or(|b| get(b) != value) {
            out.push(if value {
                word.to_string()
            } else {
                format!("NO{word}")
            });
        }
    }
    if before.is_none_or(|b| b.connection_limit != after.connection_limit) {
        out.push(format!("CONNECTION LIMIT {}", after.connection_limit));
    }
    let until = after.valid_until.trim();
    if before.is_none_or(|b| b.valid_until.trim() != until)
        && (before.is_some() || !until.is_empty())
    {
        out.push(format!(
            "VALID UNTIL {}",
            if until.is_empty() {
                lit("infinity")
            } else {
                lit(until)
            }
        ));
    }
    if !after.password.is_empty() {
        out.push(if reveal_password {
            format!("PASSWORD {}", lit(&after.password))
        } else {
            // O script não leva a senha (ela iria parar num arquivo).
            "PASSWORD '********'".to_string()
        });
    }
    out
}

/// O SQL que leva a role de `before` (None: não existe) a `after`. Com
/// `reveal_password` falso (o Script), a senha sai mascarada.
pub fn role_sql(before: Option<&RoleSpec>, after: &RoleSpec, reveal_password: bool) -> Vec<String> {
    let mut out = Vec::new();
    let name = quote_ident(after.name.trim());
    match before {
        None => {
            let options = role_options(None, after, reveal_password);
            out.push(format!("CREATE ROLE {name} WITH {}", options.join(" ")));
        }
        Some(b) => {
            if b.name != after.name.trim() {
                out.push(format!(
                    "ALTER ROLE {} RENAME TO {name}",
                    quote_ident(&b.name)
                ));
            }
            let options = role_options(Some(b), after, reveal_password);
            if !options.is_empty() {
                out.push(format!("ALTER ROLE {name} WITH {}", options.join(" ")));
            }
        }
    }
    let empty = BTreeSet::new();
    let (old_of, old_members) = before.map_or((&empty, &empty), |b| (&b.member_of, &b.members));
    for r in after.member_of.difference(old_of) {
        out.push(format!("GRANT {} TO {name}", quote_ident(r)));
    }
    for r in old_of.difference(&after.member_of) {
        out.push(format!("REVOKE {} FROM {name}", quote_ident(r)));
    }
    for m in after.members.difference(old_members) {
        out.push(format!("GRANT {name} TO {}", quote_ident(m)));
    }
    for m in old_members.difference(&after.members) {
        out.push(format!("REVOKE {name} FROM {}", quote_ident(m)));
    }
    out
}

/// Tipo de objeto com permissões, com os privilégios que ele aceita.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Database,
    Schema,
    Table,
    View,
    Function,
}

impl ObjectKind {
    /// Privilégios do `GRANT ... ON` deste tipo, na ordem do SSMS.
    pub fn privileges(self) -> &'static [&'static str] {
        match self {
            ObjectKind::Database => &["CONNECT", "CREATE", "TEMPORARY"],
            ObjectKind::Schema => &["USAGE", "CREATE"],
            ObjectKind::Table | ObjectKind::View => &[
                "SELECT",
                "INSERT",
                "UPDATE",
                "DELETE",
                "TRUNCATE",
                "REFERENCES",
                "TRIGGER",
            ],
            ObjectKind::Function => &["EXECUTE"],
        }
    }

    /// A palavra do `GRANT ... ON <palavra>`.
    pub fn keyword(self) -> &'static str {
        match self {
            ObjectKind::Database => "DATABASE",
            ObjectKind::Schema => "SCHEMA",
            ObjectKind::Table | ObjectKind::View => "TABLE",
            ObjectKind::Function => "ROUTINE",
        }
    }

    /// Nome do tipo no título da janela.
    pub fn label(self) -> &'static str {
        pgstudio_i18n::tr(match self {
            ObjectKind::Database => "Database",
            ObjectKind::Schema => "Schema",
            ObjectKind::Table => "Table",
            ObjectKind::View => "View",
            ObjectKind::Function => "Function",
        })
    }
}

/// Leitura das permissões explícitas de um objeto: nome (já pronto para o
/// SQL), dono e tipo; os pares (quem, privilégio, com grant option); e
/// todas as roles do servidor.
pub fn grants_read_sql(target: &Securable) -> Option<String> {
    // (cabeça, FROM com o aclexplode, condição)
    let (head, from, cond) = match target {
        Securable::Database(db) => (
            "SELECT quote_ident(d.datname), pg_get_userbyid(d.datdba), 'd' FROM pg_database d",
            "pg_database d CROSS JOIN LATERAL aclexplode(d.datacl) a",
            format!("d.datname = {}", lit(db)),
        ),
        Securable::Schema { name, .. } => (
            "SELECT quote_ident(n.nspname), pg_get_userbyid(n.nspowner), 's' FROM pg_namespace n",
            "pg_namespace n CROSS JOIN LATERAL aclexplode(n.nspacl) a",
            format!("n.nspname = {}", lit(name)),
        ),
        Securable::Relation { oid, .. } => (
            "SELECT c.oid::regclass::text, pg_get_userbyid(c.relowner), c.relkind::text \
             FROM pg_class c",
            "pg_class c CROSS JOIN LATERAL aclexplode(c.relacl) a",
            format!("c.oid = {oid}"),
        ),
        Securable::Function { oid, .. } => (
            "SELECT p.oid::regprocedure::text, pg_get_userbyid(p.proowner), 'f' FROM pg_proc p",
            "pg_proc p CROSS JOIN LATERAL aclexplode(p.proacl) a",
            format!("p.oid = {oid}"),
        ),
        Securable::Role(_) | Securable::NewRole => return None,
    };
    Some(format!(
        "{head} WHERE {cond};\n\
         SELECT coalesce(g.rolname, 'PUBLIC'), a.privilege_type, a.is_grantable \
         FROM {from} LEFT JOIN pg_roles g ON g.oid = a.grantee WHERE {cond} ORDER BY 1, 2;\n\
         SELECT rolname FROM pg_roles ORDER BY rolname"
    ))
}

/// Quem tem o quê: (quem, privilégio) e se tem com grant option.
pub type Grants = BTreeMap<(String, String), bool>;

/// O objeto das permissões, como lido.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantTarget {
    /// Nome pronto para o SQL (`public.pedido`, `soma(integer,integer)`).
    pub name: String,
    pub owner: String,
    pub kind: ObjectKind,
}

/// Lê o que [`grants_read_sql`] devolveu.
pub fn parse_grants(results: &[Rows]) -> Result<(GrantTarget, Grants, Vec<String>), String> {
    let [head, acl, roles] = results else {
        return Err(tr("The permissions could not be read.").into());
    };
    let h = head.first().ok_or(tr("The object no longer exists."))?;
    let kind = match text(h, 2).as_str() {
        "d" => ObjectKind::Database,
        "s" => ObjectKind::Schema,
        "f" => ObjectKind::Function,
        "v" | "m" => ObjectKind::View,
        _ => ObjectKind::Table,
    };
    let target = GrantTarget {
        name: text(h, 0),
        owner: text(h, 1),
        kind,
    };
    let grants = acl
        .iter()
        .map(|r| ((text(r, 0), text(r, 1)), text(r, 2) == "t"))
        .collect();
    let roles = roles.iter().map(|r| text(r, 0)).collect();
    Ok((target, grants, roles))
}

fn grantee(name: &str) -> String {
    if name == "PUBLIC" {
        name.to_string()
    } else {
        quote_ident(name)
    }
}

/// GRANT e REVOKE que levam as permissões de `before` a `after`,
/// juntando os privilégios de cada pessoa num comando só.
pub fn grant_sql(target: &GrantTarget, before: &Grants, after: &Grants) -> Vec<String> {
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum Change {
        Grant,
        GrantWithOption,
        RevokeOption,
        Revoke,
    }
    let mut changes: BTreeMap<(String, Change), Vec<String>> = BTreeMap::new();
    let order = |p: &str| {
        target
            .kind
            .privileges()
            .iter()
            .position(|x| *x == p)
            .unwrap_or(usize::MAX)
    };
    for ((who, privilege), &option) in after {
        let change = match before.get(&(who.clone(), privilege.clone())) {
            None if option => Change::GrantWithOption,
            None => Change::Grant,
            Some(&old) if old == option => continue,
            Some(_) if option => Change::GrantWithOption,
            Some(_) => Change::RevokeOption,
        };
        changes
            .entry((who.clone(), change))
            .or_default()
            .push(privilege.clone());
    }
    for (who, privilege) in before.keys() {
        if !after.contains_key(&(who.clone(), privilege.clone())) {
            changes
                .entry((who.clone(), Change::Revoke))
                .or_default()
                .push(privilege.clone());
        }
    }
    let on = format!("{} {}", target.kind.keyword(), target.name);
    changes
        .into_iter()
        .map(|((who, change), mut privileges)| {
            privileges.sort_by_key(|p| order(p));
            let list = privileges.join(", ");
            let who = grantee(&who);
            match change {
                Change::Grant => format!("GRANT {list} ON {on} TO {who}"),
                Change::GrantWithOption => {
                    format!("GRANT {list} ON {on} TO {who} WITH GRANT OPTION")
                }
                Change::RevokeOption => {
                    format!("REVOKE GRANT OPTION FOR {list} ON {on} FROM {who}")
                }
                Change::Revoke => format!("REVOKE {list} ON {on} FROM {who}"),
            }
        })
        .collect()
}

/// Os comandos numa transação: tudo ou nada.
pub fn in_transaction(statements: &[String]) -> String {
    format!("BEGIN;\n{};\nCOMMIT", statements.join(";\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(values: &[&str]) -> Vec<Option<String>> {
        values
            .iter()
            .map(|v| (!v.is_empty()).then(|| v.to_string()))
            .collect()
    }

    #[test]
    fn a_new_role_is_created_with_every_option() {
        let mut r = RoleSpec::new_login();
        r.name = "Relatórios".into();
        r.password = "s'cret".into();
        r.member_of.insert("pg_read_all_data".into());
        let sql = role_sql(None, &r, true);
        assert_eq!(
            sql,
            vec![
                "CREATE ROLE \"Relatórios\" WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE \
                 INHERIT NOREPLICATION NOBYPASSRLS CONNECTION LIMIT -1 PASSWORD 's''cret'"
                    .to_string(),
                "GRANT pg_read_all_data TO \"Relatórios\"".to_string(),
            ]
        );
        assert!(role_sql(None, &r, false)[0].ends_with("PASSWORD '********'"));
    }

    #[test]
    fn an_existing_role_only_changes_what_was_edited() {
        let mut before = RoleSpec::new_login();
        before.name = "app".into();
        before.member_of.insert("leitura".into());
        before.members.insert("joao".into());
        assert!(role_sql(Some(&before), &before, true).is_empty());
        let mut after = before.clone();
        after.name = "app2".into();
        after.create_db = true;
        after.valid_until = "2030-01-01".into();
        after.member_of.clear();
        after.member_of.insert("escrita".into());
        after.members.insert("maria".into());
        assert_eq!(
            role_sql(Some(&before), &after, true),
            vec![
                "ALTER ROLE app RENAME TO app2",
                "ALTER ROLE app2 WITH CREATEDB VALID UNTIL '2030-01-01'",
                "GRANT escrita TO app2",
                "REVOKE leitura FROM app2",
                "GRANT app2 TO maria",
            ]
        );
        let mut forever = after.clone();
        forever.valid_until.clear();
        assert_eq!(
            role_sql(Some(&after), &forever, true),
            vec!["ALTER ROLE app2 WITH VALID UNTIL 'infinity'"]
        );
    }

    #[test]
    fn roles_are_read() {
        let results = vec![
            vec![row(&["app", "t", "f", "t", "f", "t", "f", "f", "5", ""])],
            vec![row(&["leitura"])],
            vec![],
            vec![row(&["app"]), row(&["leitura"])],
        ];
        let (spec, all) = parse_role(&results, true).unwrap();
        let spec = spec.unwrap();
        assert!(spec.can_login && spec.create_db && !spec.superuser);
        assert_eq!(spec.connection_limit, 5);
        assert!(spec.member_of.contains("leitura"));
        assert_eq!(all, ["app", "leitura"]);
        assert!(parse_role(&results[..2], true).is_err());
        let (none, all) = parse_role(&results[3..], false).unwrap();
        assert!(none.is_none() && all.len() == 2);
        assert!(role_read_sql(Some(10)).contains("WHERE oid = 10"));
    }

    #[test]
    fn grants_become_grouped_grant_and_revoke() {
        let target = GrantTarget {
            name: "public.pedido".into(),
            owner: "postgres".into(),
            kind: ObjectKind::Table,
        };
        let g = |items: &[(&str, &str, bool)]| -> Grants {
            items
                .iter()
                .map(|(w, p, o)| ((w.to_string(), p.to_string()), *o))
                .collect()
        };
        let before = g(&[
            ("app", "SELECT", false),
            ("app", "DELETE", false),
            ("PUBLIC", "SELECT", false),
            ("chefe", "UPDATE", true),
        ]);
        let after = g(&[
            ("app", "SELECT", false),
            ("app", "INSERT", false),
            ("app", "UPDATE", false),
            ("Leitor", "SELECT", true),
            ("chefe", "UPDATE", false),
        ]);
        assert_eq!(
            grant_sql(&target, &before, &after),
            vec![
                "GRANT SELECT ON TABLE public.pedido TO \"Leitor\" WITH GRANT OPTION",
                "REVOKE SELECT ON TABLE public.pedido FROM PUBLIC",
                "GRANT INSERT, UPDATE ON TABLE public.pedido TO app",
                "REVOKE DELETE ON TABLE public.pedido FROM app",
                "REVOKE GRANT OPTION FOR UPDATE ON TABLE public.pedido FROM chefe",
            ]
        );
        assert!(grant_sql(&target, &after, &after).is_empty());
    }

    #[test]
    fn grants_are_read() {
        let results = vec![
            vec![row(&["soma(integer,integer)", "postgres", "f"])],
            vec![
                row(&["PUBLIC", "EXECUTE", "f"]),
                row(&["app", "EXECUTE", "t"]),
            ],
            vec![row(&["app"])],
        ];
        let (t, grants, roles) = parse_grants(&results).unwrap();
        assert_eq!(t.kind, ObjectKind::Function);
        assert_eq!(t.kind.keyword(), "ROUTINE");
        assert!(grants[&("app".to_string(), "EXECUTE".to_string())]);
        assert_eq!(roles, ["app"]);
        let sql = grants_read_sql(&Securable::Relation {
            database: "loja".into(),
            oid: 42,
        })
        .unwrap();
        assert!(sql.contains("c.oid = 42"));
        assert!(grants_read_sql(&Securable::Role(1)).is_none());
        assert_eq!(
            in_transaction(&["A".into(), "B".into()]),
            "BEGIN;\nA;\nB;\nCOMMIT"
        );
    }
}
