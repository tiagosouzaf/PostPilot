//! Segurança do SQL Server: logins e server roles (do servidor), usuários
//! e database roles (de um banco), como as janelas "Login Properties",
//! "Database User" e "Database Role Properties" do SSMS.
//!
//! Como em [`crate::security`], aqui ficam as leituras e o SQL das
//! mudanças, sem interface: [`principal_sql`] compara o principal como
//! estava com o editado.

use pgstudio_i18n::tr;
use std::collections::{BTreeMap, BTreeSet};

use pgstudio_core::metadata::{PrincipalKind, Securable};

use crate::security::Rows;

/// Um login, usuário ou role como a janela mostra. Cada tipo usa só os
/// campos que tem.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrincipalSpec {
    pub name: String,
    /// Login: senha nova; vazia não muda.
    pub password: String,
    /// Login: banco padrão.
    pub default_database: String,
    /// Login: `CHECK_POLICY` (a política de senhas do Windows).
    pub check_policy: bool,
    /// Login: desabilitado.
    pub disabled: bool,
    /// Login: se é um login do SQL Server (com senha), e não do Windows
    /// ou do Entra.
    pub sql_login: bool,
    /// Usuário: o login dele; vazio é `WITHOUT LOGIN`.
    pub login: String,
    /// Usuário: schema padrão; vazio deixa o do servidor (`dbo`).
    pub default_schema: String,
    /// Role: fixa do servidor ou do banco (`sysadmin`, `db_owner`...): só
    /// os membros mudam.
    pub fixed: bool,
    /// Roles de que este principal é membro.
    pub member_of: BTreeSet<String>,
    /// Role: quem é membro dela.
    pub members: BTreeSet<String>,
}

impl PrincipalSpec {
    /// Um principal novo, com os padrões do SSMS.
    pub fn new(kind: PrincipalKind) -> Self {
        PrincipalSpec {
            default_database: match kind {
                PrincipalKind::Login => "master".into(),
                _ => String::new(),
            },
            check_policy: kind == PrincipalKind::Login,
            sql_login: kind == PrincipalKind::Login,
            ..Default::default()
        }
    }
}

/// As listas que a janela oferece.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrincipalLists {
    /// Roles de que o principal pode ser membro.
    pub roles: Vec<String>,
    /// Quem pode ser membro de uma role.
    pub candidates: Vec<String>,
    /// Login: os bancos (para o banco padrão).
    pub databases: Vec<String>,
    /// Usuário: os logins do servidor.
    pub logins: Vec<String>,
    /// Usuário: os schemas do banco.
    pub schemas: Vec<String>,
}

fn lit(text: &str) -> String {
    format!("N'{}'", text.replace('\'', "''"))
}

/// `[nome]`, com `]` dobrado.
fn bracket(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

fn text(r: &[Option<String>], i: usize) -> String {
    r.get(i).cloned().flatten().unwrap_or_default()
}

const NOTHING: &str = "SELECT CAST(NULL AS sysname) WHERE 1 = 0";
const LOGIN_TYPES: &str = "'S', 'U', 'G', 'E', 'X'";

/// Leitura de um principal (pelo nome; None para um novo) e das listas da
/// janela. São sempre sete resultados: o principal (uma linha de cinco
/// colunas, ou nenhuma), as roles dele, os membros dele, as roles que
/// existem, quem pode ser membro e duas listas do tipo (bancos, ou logins
/// e schemas).
pub fn principal_read_sql(kind: PrincipalKind, name: Option<&str>) -> String {
    let server = kind.server_level();
    let (principals, members) = if server {
        ("sys.server_principals", "sys.server_role_members")
    } else {
        ("sys.database_principals", "sys.database_role_members")
    };
    let cond = match name {
        Some(n) => format!("p.name = {}", lit(n)),
        None => "1 = 0".to_string(),
    };
    let head = match kind {
        PrincipalKind::Login => format!(
            "SELECT p.name, ISNULL(p.default_database_name, 'master'), \
             CAST(ISNULL(l.is_policy_checked, 0) AS int), CAST(p.is_disabled AS int), p.type \
             FROM sys.server_principals p \
             LEFT JOIN sys.sql_logins l ON l.principal_id = p.principal_id \
             WHERE p.type IN ({LOGIN_TYPES}) AND {cond}"
        ),
        PrincipalKind::User => format!(
            "SELECT p.name, ISNULL(SUSER_SNAME(p.sid), ''), ISNULL(p.default_schema_name, ''), \
             '', p.type FROM sys.database_principals p \
             WHERE p.type IN ({LOGIN_TYPES}) AND {cond}"
        ),
        PrincipalKind::ServerRole | PrincipalKind::DatabaseRole => format!(
            "SELECT p.name, '', '', CAST(p.is_fixed_role AS int), p.type \
             FROM {principals} p WHERE p.type = 'R' AND {cond}"
        ),
    };
    let member_of = format!(
        "SELECT r.name FROM {members} m \
         JOIN {principals} r ON r.principal_id = m.role_principal_id \
         JOIN {principals} p ON p.principal_id = m.member_principal_id \
         WHERE {cond} ORDER BY r.name"
    );
    let members_sql = format!(
        "SELECT r.name FROM {members} m \
         JOIN {principals} r ON r.principal_id = m.member_principal_id \
         JOIN {principals} p ON p.principal_id = m.role_principal_id \
         WHERE {cond} ORDER BY r.name"
    );
    let roles = format!(
        "SELECT name FROM {principals} WHERE type = 'R' AND name <> 'public' ORDER BY name"
    );
    let candidates = if server {
        format!(
            "SELECT name FROM sys.server_principals \
             WHERE (type IN ({LOGIN_TYPES}) AND name NOT LIKE '##%') \
                OR (type = 'R' AND is_fixed_role = 0 AND name <> 'public') ORDER BY name"
        )
    } else {
        format!(
            "SELECT name FROM sys.database_principals \
             WHERE (type IN ({LOGIN_TYPES}) \
                    AND name NOT IN ('dbo', 'guest', 'sys', 'INFORMATION_SCHEMA')) \
                OR (type = 'R' AND is_fixed_role = 0 AND name <> 'public') ORDER BY name"
        )
    };
    let (a, b) = match kind {
        PrincipalKind::Login => (
            "SELECT name FROM sys.databases ORDER BY name".to_string(),
            NOTHING.to_string(),
        ),
        PrincipalKind::User => (
            format!(
                "SELECT name FROM sys.server_principals \
                 WHERE type IN ({LOGIN_TYPES}) AND name NOT LIKE '##%' ORDER BY name"
            ),
            "SELECT name FROM sys.schemas WHERE schema_id < 16384 \
             AND name NOT IN ('guest', 'sys', 'INFORMATION_SCHEMA') ORDER BY name"
                .to_string(),
        ),
        _ => (NOTHING.to_string(), NOTHING.to_string()),
    };
    [head, member_of, members_sql, roles, candidates, a, b].join(";\n")
}

/// Lê o que [`principal_read_sql`] devolveu: o principal (None para um
/// novo) e as listas.
pub fn parse_principal(
    kind: PrincipalKind,
    results: &[Rows],
    existing: bool,
) -> Result<(Option<PrincipalSpec>, PrincipalLists), String> {
    let names = |rows: &Rows| rows.iter().map(|r| text(r, 0)).collect::<Vec<_>>();
    let [head, member_of, members, roles, candidates, a, b] = results else {
        return Err(tr("The login, user or role could not be read.").into());
    };
    let (a, b) = (names(a), names(b));
    let mut lists = PrincipalLists {
        roles: names(roles),
        candidates: names(candidates),
        ..Default::default()
    };
    match kind {
        PrincipalKind::Login => lists.databases = a,
        PrincipalKind::User => {
            lists.logins = a;
            lists.schemas = b;
        }
        _ => {}
    }
    if !existing {
        return Ok((None, lists));
    }
    let r = head
        .first()
        .ok_or(tr("The login, user or role no longer exists."))?;
    let mut spec = PrincipalSpec {
        name: text(r, 0),
        member_of: names(member_of).into_iter().collect(),
        members: names(members).into_iter().collect(),
        ..Default::default()
    };
    match kind {
        PrincipalKind::Login => {
            spec.default_database = text(r, 1);
            spec.check_policy = text(r, 2) == "1";
            spec.disabled = text(r, 3) == "1";
            spec.sql_login = text(r, 4).trim() == "S";
        }
        PrincipalKind::User => {
            spec.login = text(r, 1);
            spec.default_schema = text(r, 2);
        }
        PrincipalKind::ServerRole | PrincipalKind::DatabaseRole => {
            spec.fixed = text(r, 3) == "1";
        }
    }
    Ok((Some(spec), lists))
}

fn on_off(value: bool) -> &'static str {
    if value { "ON" } else { "OFF" }
}

/// O SQL que leva o principal de `before` (None: não existe) a `after`.
/// Com `reveal_password` falso (o Script), a senha sai mascarada.
pub fn principal_sql(
    kind: PrincipalKind,
    before: Option<&PrincipalSpec>,
    after: &PrincipalSpec,
    reveal_password: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let name = bracket(after.name.trim());
    let password = || {
        if reveal_password {
            lit(&after.password)
        } else {
            // O script não leva a senha (ela iria parar num arquivo).
            "N'********'".to_string()
        }
    };
    // ALTER SERVER ROLE para o servidor, ALTER ROLE para o banco.
    let role = if kind.server_level() {
        "SERVER ROLE"
    } else {
        "ROLE"
    };
    if let Some(b) = before
        && b.name != after.name.trim()
    {
        let what = match kind {
            PrincipalKind::Login => "LOGIN",
            PrincipalKind::User => "USER",
            _ => role,
        };
        out.push(format!(
            "ALTER {what} {} WITH NAME = {name}",
            bracket(&b.name)
        ));
    }
    match (kind, before) {
        (PrincipalKind::Login, None) => {
            out.push(format!(
                "CREATE LOGIN {name} WITH PASSWORD = {}, DEFAULT_DATABASE = {}, \
                 CHECK_EXPIRATION = OFF, CHECK_POLICY = {}",
                password(),
                bracket(after.default_database.trim()),
                on_off(after.check_policy)
            ));
            if after.disabled {
                out.push(format!("ALTER LOGIN {name} DISABLE"));
            }
        }
        (PrincipalKind::Login, Some(b)) => {
            // Uma opção por comando: o servidor recusa algumas combinações
            // (senha com CHECK_POLICY, por exemplo).
            if b.check_policy != after.check_policy && after.sql_login {
                out.push(format!(
                    "ALTER LOGIN {name} WITH CHECK_POLICY = {}",
                    on_off(after.check_policy)
                ));
            }
            if !after.password.is_empty() {
                out.push(format!("ALTER LOGIN {name} WITH PASSWORD = {}", password()));
            }
            if b.default_database != after.default_database.trim() {
                out.push(format!(
                    "ALTER LOGIN {name} WITH DEFAULT_DATABASE = {}",
                    bracket(after.default_database.trim())
                ));
            }
            if b.disabled != after.disabled {
                out.push(format!(
                    "ALTER LOGIN {name} {}",
                    if after.disabled { "DISABLE" } else { "ENABLE" }
                ));
            }
        }
        (PrincipalKind::User, None) => {
            let login = after.login.trim();
            let mut sql = format!("CREATE USER {name}");
            sql.push_str(&if login.is_empty() {
                " WITHOUT LOGIN".to_string()
            } else {
                format!(" FOR LOGIN {}", bracket(login))
            });
            if !after.default_schema.trim().is_empty() {
                sql.push_str(&format!(
                    " WITH DEFAULT_SCHEMA = {}",
                    bracket(after.default_schema.trim())
                ));
            }
            out.push(sql);
        }
        (PrincipalKind::User, Some(b)) => {
            let login = after.login.trim();
            if b.login != login && !login.is_empty() {
                out.push(format!("ALTER USER {name} WITH LOGIN = {}", bracket(login)));
            }
            let schema = after.default_schema.trim();
            if b.default_schema != schema && !schema.is_empty() {
                out.push(format!(
                    "ALTER USER {name} WITH DEFAULT_SCHEMA = {}",
                    bracket(schema)
                ));
            }
        }
        (PrincipalKind::ServerRole | PrincipalKind::DatabaseRole, None) => {
            out.push(format!("CREATE {role} {name}"));
        }
        (PrincipalKind::ServerRole | PrincipalKind::DatabaseRole, Some(_)) => {}
    }
    let empty = BTreeSet::new();
    let (old_of, old_members) = before.map_or((&empty, &empty), |b| (&b.member_of, &b.members));
    for r in after.member_of.difference(old_of) {
        out.push(format!("ALTER {role} {} ADD MEMBER {name}", bracket(r)));
    }
    for r in old_of.difference(&after.member_of) {
        out.push(format!("ALTER {role} {} DROP MEMBER {name}", bracket(r)));
    }
    for m in after.members.difference(old_members) {
        out.push(format!("ALTER {role} {name} ADD MEMBER {}", bracket(m)));
    }
    for m in old_members.difference(&after.members) {
        out.push(format!("ALTER {role} {name} DROP MEMBER {}", bracket(m)));
    }
    out
}

/// Os comandos numa transação que desfaz tudo no primeiro erro
/// (`XACT_ABORT`).
pub fn in_transaction(statements: &[String]) -> String {
    format!(
        "SET XACT_ABORT ON;\nBEGIN TRANSACTION;\n{};\nCOMMIT TRANSACTION;\nSET XACT_ABORT OFF",
        statements.join(";\n")
    )
}

/// Desfaz a transação que um erro deixou aberta.
pub const ROLLBACK: &str = "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION;\nSET XACT_ABORT OFF";

/// Como uma permissão está para alguém, como as três caixas do SSMS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermState {
    Grant,
    WithGrant,
    Deny,
}

/// Quem tem o quê: (quem, permissão) e o estado.
pub type Permissions = BTreeMap<(String, String), PermState>;

/// O objeto das permissões, como lido.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermTarget {
    /// Nome para mostrar (`[dbo].[pedidos]`, ou o do banco).
    pub name: String,
    pub owner: String,
    /// Nome do tipo no título, já traduzido.
    pub kind: &'static str,
    /// O ` ON OBJECT::[dbo].[pedidos]` dos comandos (vazio para o banco).
    pub on: String,
    /// As permissões que a janela lista.
    pub privileges: Vec<String>,
}

const DATABASE_PERMISSIONS: &[&str] = &[
    "CONNECT",
    "SELECT",
    "INSERT",
    "UPDATE",
    "DELETE",
    "EXECUTE",
    "REFERENCES",
    "CREATE TABLE",
    "CREATE VIEW",
    "CREATE PROCEDURE",
    "CREATE FUNCTION",
    "CREATE SCHEMA",
    "ALTER",
    "CONTROL",
    "VIEW DEFINITION",
    "BACKUP DATABASE",
    "SHOWPLAN",
];
const TABLE_PERMISSIONS: &[&str] = &[
    "SELECT",
    "INSERT",
    "UPDATE",
    "DELETE",
    "REFERENCES",
    "ALTER",
    "CONTROL",
    "VIEW DEFINITION",
];
const ROUTINE_PERMISSIONS: &[&str] = &["EXECUTE", "ALTER", "CONTROL", "VIEW DEFINITION"];
const TABLE_FUNCTION_PERMISSIONS: &[&str] = &[
    "SELECT",
    "REFERENCES",
    "ALTER",
    "CONTROL",
    "VIEW DEFINITION",
];

/// Leitura das permissões explícitas de um banco, tabela, view, procedure
/// ou função, na sessão do banco: o objeto (nome, dono, tipo); os trios
/// (quem, permissão, estado `G`, `W` ou `D`); e os usuários e roles do
/// banco.
pub fn permissions_read_sql(target: &Securable) -> Option<String> {
    let (head, cond) = match target {
        Securable::Database(_) => (
            "SELECT DB_NAME(), ISNULL(SUSER_SNAME(d.owner_sid), ''), 'DB', '' \
             FROM sys.databases d WHERE d.database_id = DB_ID()"
                .to_string(),
            "p.class = 0".to_string(),
        ),
        Securable::Relation { oid, .. } | Securable::Function { oid, .. } => (
            format!(
                "SELECT QUOTENAME(s.name) + '.' + QUOTENAME(o.name), \
                 ISNULL(USER_NAME(ISNULL(o.principal_id, s.principal_id)), ''), \
                 RTRIM(o.type), 'OBJECT' \
                 FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 WHERE o.object_id = {oid}"
            ),
            format!("p.class = 1 AND p.major_id = {oid} AND p.minor_id = 0"),
        ),
        _ => return None,
    };
    Some(format!(
        "{head};\n\
         SELECT USER_NAME(p.grantee_principal_id), p.permission_name, p.state \
         FROM sys.database_permissions p WHERE {cond} ORDER BY 1, 2;\n\
         SELECT name FROM sys.database_principals \
         WHERE (type IN ({LOGIN_TYPES}) AND name NOT IN ('dbo', 'sys', 'INFORMATION_SCHEMA')) \
            OR (type = 'R' AND is_fixed_role = 0) ORDER BY name"
    ))
}

/// Lê o que [`permissions_read_sql`] devolveu.
pub fn parse_permissions(
    results: &[Rows],
) -> Result<(PermTarget, Permissions, Vec<String>), String> {
    let [head, acl, principals] = results else {
        return Err(tr("The permissions could not be read.").into());
    };
    let h = head.first().ok_or(tr("The object no longer exists."))?;
    let name = text(h, 0);
    let (kind, defaults) = match text(h, 2).as_str() {
        "DB" => (tr("Database"), DATABASE_PERMISSIONS),
        "U" => (tr("Table"), TABLE_PERMISSIONS),
        "V" => (tr("View"), TABLE_PERMISSIONS),
        "P" | "PC" | "X" => (tr("Procedure"), ROUTINE_PERMISSIONS),
        "FN" | "FS" => (tr("Function"), ROUTINE_PERMISSIONS),
        "IF" | "TF" | "FT" => (tr("Function"), TABLE_FUNCTION_PERMISSIONS),
        _ => return Err(tr("This object has no permissions of its own.").into()),
    };
    let on = match text(h, 3).as_str() {
        "" => String::new(),
        class => format!(" ON {class}::{name}"),
    };
    let mut privileges: Vec<String> = defaults.iter().map(|p| p.to_string()).collect();
    let mut map = Permissions::new();
    for r in acl {
        let state = match text(r, 2).trim() {
            "G" => PermState::Grant,
            "W" => PermState::WithGrant,
            "D" => PermState::Deny,
            _ => continue,
        };
        let permission = text(r, 1);
        if !privileges.contains(&permission) {
            privileges.push(permission.clone());
        }
        map.insert((text(r, 0), permission), state);
    }
    let target = PermTarget {
        name,
        owner: text(h, 1),
        kind,
        on,
        privileges,
    };
    let names = principals.iter().map(|r| text(r, 0)).collect();
    Ok((target, map, names))
}

/// O GRANT, DENY e REVOKE que levam as permissões de `before` a `after`.
pub fn permission_sql(
    target: &PermTarget,
    before: &Permissions,
    after: &Permissions,
) -> Vec<String> {
    let keys: BTreeSet<&(String, String)> = before.keys().chain(after.keys()).collect();
    let on = &target.on;
    let mut out = Vec::new();
    for key in keys {
        let (who, p) = key;
        let who = bracket(who);
        let (old, new) = (before.get(key).copied(), after.get(key).copied());
        if old == new {
            continue;
        }
        // Tirar de quem podia repassar também tira de quem recebeu dele.
        let cascade = if old == Some(PermState::WithGrant) {
            " CASCADE"
        } else {
            ""
        };
        out.push(match new {
            None => format!("REVOKE {p}{on} FROM {who}{cascade}"),
            Some(PermState::Grant) if old == Some(PermState::WithGrant) => {
                format!("REVOKE GRANT OPTION FOR {p}{on} FROM {who} CASCADE")
            }
            Some(PermState::Grant) => format!("GRANT {p}{on} TO {who}"),
            Some(PermState::WithGrant) => format!("GRANT {p}{on} TO {who} WITH GRANT OPTION"),
            Some(PermState::Deny) => format!("DENY {p}{on} TO {who}{cascade}"),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_new_login_is_created_with_its_roles() {
        let mut login = PrincipalSpec::new(PrincipalKind::Login);
        login.name = "ana".into();
        login.password = "S3nha'forte".into();
        login.default_database = "vendas".into();
        login.member_of = set(&["dbcreator"]);
        assert_eq!(
            principal_sql(PrincipalKind::Login, None, &login, true),
            [
                "CREATE LOGIN [ana] WITH PASSWORD = N'S3nha''forte', \
                 DEFAULT_DATABASE = [vendas], CHECK_EXPIRATION = OFF, CHECK_POLICY = ON",
                "ALTER SERVER ROLE [dbcreator] ADD MEMBER [ana]",
            ]
        );
        // O Script esconde a senha.
        let script = principal_sql(PrincipalKind::Login, None, &login, false);
        assert!(script[0].contains("PASSWORD = N'********'"), "{script:?}");
    }

    #[test]
    fn a_login_changes_only_what_was_edited() {
        let mut before = PrincipalSpec::new(PrincipalKind::Login);
        before.name = "ana".into();
        before.member_of = set(&["dbcreator"]);
        assert!(principal_sql(PrincipalKind::Login, Some(&before), &before, true).is_empty());
        let mut after = before.clone();
        after.name = "ana.lima".into();
        after.disabled = true;
        after.check_policy = false;
        after.default_database = "vendas".into();
        after.member_of = set(&["sysadmin"]);
        assert_eq!(
            principal_sql(PrincipalKind::Login, Some(&before), &after, true),
            [
                "ALTER LOGIN [ana] WITH NAME = [ana.lima]",
                "ALTER LOGIN [ana.lima] WITH CHECK_POLICY = OFF",
                "ALTER LOGIN [ana.lima] WITH DEFAULT_DATABASE = [vendas]",
                "ALTER LOGIN [ana.lima] DISABLE",
                "ALTER SERVER ROLE [sysadmin] ADD MEMBER [ana.lima]",
                "ALTER SERVER ROLE [dbcreator] DROP MEMBER [ana.lima]",
            ]
        );
    }

    #[test]
    fn users_and_database_roles_use_the_database_commands() {
        let mut user = PrincipalSpec::new(PrincipalKind::User);
        user.name = "ana".into();
        user.login = "ana".into();
        user.default_schema = "dbo".into();
        user.member_of = set(&["db_datareader"]);
        assert_eq!(
            principal_sql(PrincipalKind::User, None, &user, true),
            [
                "CREATE USER [ana] FOR LOGIN [ana] WITH DEFAULT_SCHEMA = [dbo]",
                "ALTER ROLE [db_datareader] ADD MEMBER [ana]",
            ]
        );
        user.login.clear();
        user.default_schema.clear();
        user.member_of.clear();
        assert_eq!(
            principal_sql(PrincipalKind::User, None, &user, true),
            ["CREATE USER [ana] WITHOUT LOGIN"]
        );
        let mut role = PrincipalSpec::new(PrincipalKind::DatabaseRole);
        role.name = "vendedores".into();
        role.members = set(&["ana", "x]y"]);
        assert_eq!(
            principal_sql(PrincipalKind::DatabaseRole, None, &role, true),
            [
                "CREATE ROLE [vendedores]",
                "ALTER ROLE [vendedores] ADD MEMBER [ana]",
                "ALTER ROLE [vendedores] ADD MEMBER [x]]y]",
            ]
        );
        let mut server = PrincipalSpec::new(PrincipalKind::ServerRole);
        server.name = "auditores".into();
        assert_eq!(
            principal_sql(PrincipalKind::ServerRole, None, &server, true),
            ["CREATE SERVER ROLE [auditores]"]
        );
    }

    #[test]
    fn permissions_become_grant_deny_and_revoke() {
        let target = PermTarget {
            name: "[dbo].[pedidos]".into(),
            owner: "dbo".into(),
            kind: "Table",
            on: " ON OBJECT::[dbo].[pedidos]".into(),
            privileges: vec![],
        };
        let key = |who: &str, p: &str| (who.to_string(), p.to_string());
        let before: Permissions = [
            (key("ana", "SELECT"), PermState::Grant),
            (key("ana", "INSERT"), PermState::WithGrant),
            (key("ana", "UPDATE"), PermState::WithGrant),
            (key("ana", "DELETE"), PermState::Deny),
            (key("bia", "SELECT"), PermState::WithGrant),
            (key("bia", "INSERT"), PermState::Grant),
        ]
        .into();
        let after: Permissions = [
            (key("ana", "SELECT"), PermState::WithGrant),
            (key("ana", "INSERT"), PermState::Grant),
            (key("ana", "UPDATE"), PermState::Deny),
            (key("ana", "DELETE"), PermState::Grant),
            (key("bia", "INSERT"), PermState::Grant),
            (key("public", "SELECT"), PermState::Deny),
        ]
        .into();
        assert_eq!(
            permission_sql(&target, &before, &after),
            [
                "GRANT DELETE ON OBJECT::[dbo].[pedidos] TO [ana]",
                "REVOKE GRANT OPTION FOR INSERT ON OBJECT::[dbo].[pedidos] FROM [ana] CASCADE",
                "GRANT SELECT ON OBJECT::[dbo].[pedidos] TO [ana] WITH GRANT OPTION",
                "DENY UPDATE ON OBJECT::[dbo].[pedidos] TO [ana] CASCADE",
                "REVOKE SELECT ON OBJECT::[dbo].[pedidos] FROM [bia] CASCADE",
                "DENY SELECT ON OBJECT::[dbo].[pedidos] TO [public]",
            ]
        );
        let database = PermTarget {
            on: String::new(),
            ..target
        };
        let after: Permissions = [(key("ana", "CREATE TABLE"), PermState::Grant)].into();
        assert_eq!(
            permission_sql(&database, &Permissions::new(), &after),
            ["GRANT CREATE TABLE TO [ana]"]
        );
    }
}
