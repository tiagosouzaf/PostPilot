//! O resto da árvore do SSMS: o que fica além de tabelas, views,
//! procedures e funções. No banco: Synonyms, gatilhos de banco, assemblies,
//! rules, defaults, Service Broker, Storage e o resto de Security; no
//! servidor: credenciais, Server Objects e os jobs do SQL Server Agent.
//!
//! Tudo é lido só quando a pasta abre. Os ids destes nós:
//!
//! | Nó | Id |
//! | --- | --- |
//! | synonym, rule, default, função de agregação | `db/<banco>/p/<object_id>` |
//! | gatilho de banco | `db/<banco>/dt/<object_id>` |
//! | parâmetro | `db/<banco>/par/<object_id>/<parameter_id>` |
//! | estatística | `db/<banco>/st/<object_id>/<stats_id>` |
//! | objeto do banco só com nome | `db/<banco>/x/<tipo>/<nome>` |
//! | objeto do servidor | `srv/<tipo>/<nome>` |

use pgstudio_core::metadata::{
    Folder, MetadataError, NodeId, NodeKind, ObjectNode, Property, ScriptAction,
};
use pgstudio_i18n::{tr, trf};

use crate::explorer::{MssqlExplorer, bracket, dec, enc, folder, gone, node, not_yet, sql_type};

pub(crate) const CREDENTIALS: &str = "server/Security/Credentials";
pub(crate) const SERVER_OBJECTS: &str = "server/Objects";
pub(crate) const AGENT: &str = "server/Agent";

/// Um tipo de objeto que a árvore lista pelo nome e descreve na janela
/// Properties. Em `list` e `props`, `{d}` é o banco (`[banco].`, vazio no
/// servidor) e, em `props`, `{n}` é o nome como literal.
struct Named {
    /// O tipo no id do nó e a pasta dele.
    key: &'static str,
    folder: Folder,
    kind: NodeKind,
    /// `FROM ... WHERE ...` de onde sai a coluna `name`.
    list: &'static str,
    /// Consulta de uma linha; os apelidos são os nomes das propriedades.
    props: &'static str,
}

const CREATED: &str = "CONVERT(varchar(19), x.create_date, 120) AS [Created], \
                       CONVERT(varchar(19), x.modify_date, 120) AS [Last modified]";

/// Os objetos do banco listados pelo nome, na pasta `db/<banco>/f/<key>`.
const DATABASE_ITEMS: &[Named] = &[
    Named {
        key: "assemblies",
        folder: Folder::Assemblies,
        kind: NodeKind::Item,
        list: "FROM {d}sys.assemblies x WHERE x.is_user_defined = 1",
        props: "SELECT LOWER(REPLACE(x.permission_set_desc, '_', ' ')) AS [Security], \
                       x.clr_name AS [Version], {created} \
                  FROM {d}sys.assemblies x WHERE x.name = {n}",
    },
    Named {
        key: "filegroups",
        folder: Folder::Filegroups,
        kind: NodeKind::Item,
        list: "FROM {d}sys.filegroups x",
        props: "SELECT LOWER(REPLACE(x.type_desc, '_', ' ')) AS [Kind], \
                       CASE x.is_default WHEN 1 THEN 'yes' ELSE 'no' END AS [Default], \
                       CASE x.is_read_only WHEN 1 THEN 'yes' ELSE 'no' END AS [Read-only], \
                       (SELECT CAST(COUNT(*) AS varchar(10)) FROM {d}sys.database_files f \
                         WHERE f.data_space_id = x.data_space_id) AS [Files] \
                  FROM {d}sys.filegroups x WHERE x.name = {n}",
    },
    Named {
        key: "pschemes",
        folder: Folder::PartitionSchemes,
        kind: NodeKind::Item,
        list: "FROM {d}sys.partition_schemes x",
        props: "SELECT f.name AS [Function] FROM {d}sys.partition_schemes x \
                  JOIN {d}sys.partition_functions f ON f.function_id = x.function_id \
                 WHERE x.name = {n}",
    },
    Named {
        key: "pfuncs",
        folder: Folder::PartitionFunctions,
        kind: NodeKind::Item,
        list: "FROM {d}sys.partition_functions x",
        props: "SELECT CAST(x.fanout AS varchar(10)) AS [Partitions], \
                       CASE x.boundary_value_on_right WHEN 1 THEN 'right' ELSE 'left' END \
                           AS [Range], {created} \
                  FROM {d}sys.partition_functions x WHERE x.name = {n}",
    },
    Named {
        key: "ftcats",
        folder: Folder::FullTextCatalogs,
        kind: NodeKind::Item,
        list: "FROM {d}sys.fulltext_catalogs x",
        props: "SELECT CASE x.is_default WHEN 1 THEN 'yes' ELSE 'no' END AS [Default], \
                       USER_NAME(x.principal_id) AS [Owner] \
                  FROM {d}sys.fulltext_catalogs x WHERE x.name = {n}",
    },
    Named {
        key: "approles",
        folder: Folder::ApplicationRoles,
        kind: NodeKind::Item,
        list: "FROM {d}sys.database_principals x WHERE x.type = 'A'",
        props: "SELECT x.default_schema_name AS [Default schema], {created} \
                  FROM {d}sys.database_principals x WHERE x.type = 'A' AND x.name = {n}",
    },
    Named {
        key: "schemas",
        folder: Folder::Schemas,
        kind: NodeKind::Schema,
        list: "FROM {d}sys.schemas x",
        props: "SELECT p.name AS [Owner] FROM {d}sys.schemas x \
                  LEFT JOIN {d}sys.database_principals p ON p.principal_id = x.principal_id \
                 WHERE x.name = {n}",
    },
    Named {
        key: "certs",
        folder: Folder::Certificates,
        kind: NodeKind::Item,
        list: "FROM {d}sys.certificates x",
        props: "SELECT x.subject AS [Subject], \
                       CONVERT(varchar(19), x.start_date, 120) AS [Valid from], \
                       CONVERT(varchar(19), x.expiry_date, 120) AS [Valid until], \
                       LOWER(REPLACE(x.pvt_key_encryption_type_desc, '_', ' ')) AS [Private key] \
                  FROM {d}sys.certificates x WHERE x.name = {n}",
    },
    Named {
        key: "symkeys",
        folder: Folder::SymmetricKeys,
        kind: NodeKind::Item,
        list: "FROM {d}sys.symmetric_keys x",
        props: "SELECT x.algorithm_desc AS [Algorithm], \
                       CAST(x.key_length AS varchar(10)) AS [Key length], {created} \
                  FROM {d}sys.symmetric_keys x WHERE x.name = {n}",
    },
    Named {
        key: "asymkeys",
        folder: Folder::AsymmetricKeys,
        kind: NodeKind::Item,
        list: "FROM {d}sys.asymmetric_keys x",
        props: "SELECT x.algorithm_desc AS [Algorithm], \
                       CAST(x.key_length AS varchar(10)) AS [Key length] \
                  FROM {d}sys.asymmetric_keys x WHERE x.name = {n}",
    },
    Named {
        key: "msgtypes",
        folder: Folder::MessageTypes,
        kind: NodeKind::Item,
        list: "FROM {d}sys.service_message_types x WHERE x.message_type_id > 65535",
        props: "SELECT LOWER(REPLACE(x.validation_desc, '_', ' ')) AS [Validation], \
                       USER_NAME(x.principal_id) AS [Owner] \
                  FROM {d}sys.service_message_types x WHERE x.name = {n}",
    },
    Named {
        key: "contracts",
        folder: Folder::Contracts,
        kind: NodeKind::Item,
        list: "FROM {d}sys.service_contracts x WHERE x.service_contract_id > 65535",
        props: "SELECT USER_NAME(x.principal_id) AS [Owner] \
                  FROM {d}sys.service_contracts x WHERE x.name = {n}",
    },
    Named {
        key: "queues",
        folder: Folder::Queues,
        kind: NodeKind::Item,
        list: "FROM {d}sys.service_queues x WHERE x.is_ms_shipped = 0",
        props: "SELECT CASE x.is_receive_enabled WHEN 1 THEN 'yes' ELSE 'no' END AS [Enabled], \
                       x.activation_procedure AS [Activation procedure], {created} \
                  FROM {d}sys.service_queues x WHERE x.name = {n}",
    },
    Named {
        key: "services",
        folder: Folder::Services,
        kind: NodeKind::Item,
        list: "FROM {d}sys.services x WHERE x.service_id > 65535",
        props: "SELECT q.name AS [Queue], USER_NAME(x.principal_id) AS [Owner] \
                  FROM {d}sys.services x \
                  LEFT JOIN {d}sys.service_queues q ON q.object_id = x.service_queue_id \
                 WHERE x.name = {n}",
    },
];

/// Os objetos do servidor, na pasta `server/.../<key>`.
const SERVER_ITEMS: &[Named] = &[
    Named {
        key: "Credentials",
        folder: Folder::Credentials,
        kind: NodeKind::Item,
        list: "FROM sys.credentials x",
        props: "SELECT x.credential_identity AS [Identity], {created} \
                  FROM sys.credentials x WHERE x.name = {n}",
    },
    Named {
        key: "BackupDevices",
        folder: Folder::BackupDevices,
        kind: NodeKind::Item,
        list: "FROM sys.backup_devices x",
        props: "SELECT LOWER(x.type_desc) AS [Kind], x.physical_name AS [Physical name] \
                  FROM sys.backup_devices x WHERE x.name = {n}",
    },
    Named {
        key: "Endpoints",
        folder: Folder::Endpoints,
        kind: NodeKind::Item,
        list: "FROM sys.endpoints x",
        props: "SELECT LOWER(REPLACE(x.type_desc, '_', ' ')) AS [Kind], \
                       LOWER(REPLACE(x.protocol_desc, '_', ' ')) AS [Protocol], \
                       LOWER(x.state_desc) AS [State] \
                  FROM sys.endpoints x WHERE x.name = {n}",
    },
    Named {
        key: "LinkedServers",
        folder: Folder::LinkedServers,
        kind: NodeKind::LinkedServer,
        list: "FROM sys.servers x WHERE x.is_linked = 1",
        props: "SELECT x.product AS [Product], x.provider AS [Provider], \
                       x.data_source AS [Data source], x.catalog AS [Catalog], \
                       CONVERT(varchar(19), x.modify_date, 120) AS [Last modified] \
                  FROM sys.servers x WHERE x.name = {n}",
    },
    Named {
        key: "Triggers",
        folder: Folder::Triggers,
        kind: NodeKind::Trigger,
        list: "FROM sys.server_triggers x",
        props: "SELECT CASE x.is_disabled WHEN 1 THEN 'yes' ELSE 'no' END AS [Disabled], \
                       {created} \
                  FROM sys.server_triggers x WHERE x.name = {n}",
    },
    Named {
        key: "Jobs",
        folder: Folder::Jobs,
        kind: NodeKind::Job,
        list: "FROM msdb.dbo.sysjobs x",
        props: "SELECT CASE x.enabled WHEN 1 THEN 'yes' ELSE 'no' END AS [Enabled], \
                       NULLIF(x.description, '') AS [Description], \
                       SUSER_SNAME(x.owner_sid) AS [Owner], \
                       (SELECT CAST(COUNT(*) AS varchar(10)) FROM msdb.dbo.sysjobsteps s \
                         WHERE s.job_id = x.job_id) AS [Steps], \
                       (SELECT TOP (1) STUFF(STUFF(CAST(h.run_date AS varchar(8)), 5, 0, '-'), \
                                             8, 0, '-') + ' ' + \
                               STUFF(STUFF(RIGHT('000000' + CAST(h.run_time AS varchar(6)), 6), \
                                           3, 0, ':'), 6, 0, ':') + \
                               CASE h.run_status WHEN 0 THEN ' (failed)' WHEN 1 THEN ' (succeeded)' \
                                    WHEN 3 THEN ' (canceled)' ELSE '' END \
                          FROM msdb.dbo.sysjobhistory h \
                         WHERE h.job_id = x.job_id AND h.step_id = 0 \
                         ORDER BY h.instance_id DESC) AS [Last run], \
                       CONVERT(varchar(19), x.date_created, 120) AS [Created], \
                       CONVERT(varchar(19), x.date_modified, 120) AS [Last modified] \
                  FROM msdb.dbo.sysjobs x WHERE x.name = {n}",
    },
];

/// As pastas do banco que aceitam "Filter...": as listas que crescem.
const FILTERED: [&str; 10] = [
    "tables", "views", "synonyms", "procs", "tfuncs", "sfuncs", "afuncs", "dtypes", "ttypes",
    "seqs",
];

/// `N'texto'`.
fn lit(text: &str) -> String {
    format!("N'{}'", text.replace('\'', "''"))
}

/// O nome como vai no id do nó, já na consulta (`/` e `%` escapados).
const NAME_ID: &str = "REPLACE(REPLACE(x.name, '%', '%25'), '/', '%2F')";

impl Named {
    fn list_sql(&self, d: &str) -> String {
        format!(
            "SELECT {NAME_ID}, x.name {} ORDER BY x.name",
            self.list.replace("{d}", d)
        )
    }

    fn props_sql(&self, d: &str, name: &str) -> String {
        self.props
            .replace("{created}", CREATED)
            .replace("{d}", d)
            .replace("{n}", &lit(name))
    }
}

fn database_item(key: &str) -> Option<&'static Named> {
    DATABASE_ITEMS.iter().find(|i| i.key == key)
}

fn server_item(key: &str) -> Option<&'static Named> {
    SERVER_ITEMS.iter().find(|i| i.key == key)
}

/// A pasta do servidor com os itens de `key` (`server/Objects/Endpoints`).
fn server_folder_id(key: &str) -> String {
    match key {
        "Credentials" => CREDENTIALS.to_string(),
        "Jobs" => format!("{AGENT}/Jobs"),
        _ => format!("{SERVER_OBJECTS}/{key}"),
    }
}

fn server_folder(key: &str) -> Option<ObjectNode> {
    Some(folder(server_folder_id(key), server_item(key)?.folder))
}

impl MssqlExplorer {
    /// O filtro de nome de uma pasta, como condição do SQL sobre `name`
    /// (`s.name + '.' + o.name`). Vazio sem filtro.
    pub(crate) fn name_filter(&self, folder_id: &str, name: &str) -> String {
        match self.filter_text(folder_id) {
            Some(text) if !text.is_empty() => {
                let pattern = text
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
                    .replace('[', "\\[");
                format!(
                    " AND {name} LIKE {} ESCAPE '\\'",
                    lit(&format!("%{pattern}%"))
                )
            }
            _ => String::new(),
        }
    }

    /// O texto do filtro de uma pasta: `Some("")` quando ela aceita filtro
    /// e não tem nenhum.
    pub(crate) fn filter_text(&self, folder_id: &str) -> Option<String> {
        match folder_id.split('/').collect::<Vec<_>>().as_slice() {
            ["db", _, "f", tag] if FILTERED.contains(tag) => Some(
                self.filters
                    .lock()
                    .ok()
                    .and_then(|f| f.get(folder_id).cloned())
                    .unwrap_or_default(),
            ),
            _ => None,
        }
    }

    pub(crate) fn store_filter(&self, folder_id: &str, text: &str) {
        if self.filter_text(folder_id).is_none() {
            return;
        }
        if let Ok(mut filters) = self.filters.lock() {
            match text.trim() {
                "" => filters.remove(folder_id),
                t => filters.insert(folder_id.to_string(), t.to_string()),
            };
        }
    }

    /// Os filhos de uma pasta com filtro: avisa no começo da lista que ela
    /// está filtrada.
    pub(crate) fn with_filter_note(
        &self,
        folder_id: &str,
        mut nodes: Vec<ObjectNode>,
    ) -> Vec<ObjectNode> {
        if let Some(text) = self.filter_text(folder_id).filter(|t| !t.is_empty()) {
            nodes.insert(
                0,
                node(
                    format!("{folder_id}/note"),
                    trf!("Filter: {filter}", filter = text),
                    NodeKind::Note,
                    false,
                ),
            );
        }
        nodes
    }

    /// Filhos dos nós deste módulo; None quando o id é de outro lugar.
    pub(crate) fn more_children(&self, id: &str) -> Option<Result<Vec<ObjectNode>, MetadataError>> {
        match id {
            SERVER_OBJECTS => {
                return Some(Ok([
                    "BackupDevices",
                    "Endpoints",
                    "LinkedServers",
                    "Triggers",
                ]
                .into_iter()
                .filter_map(server_folder)
                .collect()));
            }
            AGENT => return Some(Ok(server_folder("Jobs").into_iter().collect())),
            _ => {}
        }
        if let Some(item) = SERVER_ITEMS.iter().find(|i| server_folder_id(i.key) == id) {
            let prefix = format!("srv/{}/", item.key);
            return Some(self.listed(&item.list_sql(""), &prefix, item.kind, false));
        }
        let parts: Vec<&str> = id.split('/').collect();
        match parts.as_slice() {
            ["db", db, "f", tag] => self.more_database_folder(id, &dec(db), tag),
            ["db", db, "o", obj] => {
                let _ = (db, obj);
                Some(Ok(vec![folder(
                    format!("{id}/f/params"),
                    Folder::Parameters,
                )]))
            }
            ["db", db, "o", obj, "f", "params"] => {
                let obj: i64 = match obj.parse() {
                    Ok(o) => o,
                    Err(_) => return Some(Err(gone())),
                };
                Some(self.parameters(&dec(db), obj))
            }
            ["db", db, "t" | "v", obj, "f", "stats"] => {
                let obj: i64 = match obj.parse() {
                    Ok(o) => o,
                    Err(_) => return Some(Err(gone())),
                };
                let db = dec(db);
                Some(self.listed(
                    &format!(
                        "SELECT x.stats_id, x.name FROM {}.sys.stats x \
                          WHERE x.object_id = {obj} ORDER BY x.name",
                        bracket(&db)
                    ),
                    &format!("db/{}/st/{obj}/", enc(&db)),
                    NodeKind::Statistic,
                    false,
                ))
            }
            _ => None,
        }
    }

    fn more_database_folder(
        &self,
        id: &str,
        db: &str,
        tag: &str,
    ) -> Option<Result<Vec<ObjectNode>, MetadataError>> {
        let d = bracket(db);
        let base = format!("db/{}/", enc(db));
        let folders = |list: &[(&str, Folder)]| {
            Some(Ok(list
                .iter()
                .map(|(tag, f)| folder(format!("{base}f/{tag}"), *f))
                .collect()))
        };
        // Objetos de `sys.objects` com schema, filtráveis pelo nome.
        let objects = |filter: &str, prefix: &str, kind: NodeKind, children: bool| {
            let like = self.name_filter(id, "s.name + '.' + o.name");
            Some(
                self.listed(
                    &format!(
                        "SELECT o.object_id, s.name + '.' + o.name FROM {d}.sys.objects o \
                         JOIN {d}.sys.schemas s ON s.schema_id = o.schema_id \
                         WHERE o.is_ms_shipped = 0 AND {filter}{like} ORDER BY s.name, o.name"
                    ),
                    &format!("{base}{prefix}/"),
                    kind,
                    children,
                )
                .map(|nodes| self.with_filter_note(id, nodes)),
            )
        };
        let types = |table: u8| {
            let like = self.name_filter(id, "s.name + '.' + t.name");
            Some(
                self.listed(
                    &format!(
                        "SELECT t.user_type_id, s.name + '.' + t.name FROM {d}.sys.types t \
                         JOIN {d}.sys.schemas s ON s.schema_id = t.schema_id \
                         WHERE t.is_user_defined = 1 AND t.is_table_type = {table}{like} \
                         ORDER BY s.name, t.name"
                    ),
                    &format!("{base}type/"),
                    NodeKind::Type,
                    false,
                )
                .map(|nodes| self.with_filter_note(id, nodes)),
            )
        };
        match tag {
            "prog" => folders(&[
                ("procs", Folder::Procedures),
                ("funcs", Folder::Functions),
                ("dbtrig", Folder::DatabaseTriggers),
                ("assemblies", Folder::Assemblies),
                ("types", Folder::Types),
                ("rules", Folder::Rules),
                ("defaults", Folder::Defaults),
                ("seqs", Folder::Sequences),
            ]),
            "funcs" => folders(&[
                ("tfuncs", Folder::TableFunctions),
                ("sfuncs", Folder::ScalarFunctions),
                ("afuncs", Folder::AggregateFunctions),
            ]),
            "types" => folders(&[
                ("dtypes", Folder::DataTypes),
                ("ttypes", Folder::TableTypes),
            ]),
            "broker" => folders(&[
                ("msgtypes", Folder::MessageTypes),
                ("contracts", Folder::Contracts),
                ("queues", Folder::Queues),
                ("services", Folder::Services),
            ]),
            "storage" => folders(&[
                ("filegroups", Folder::Filegroups),
                ("pschemes", Folder::PartitionSchemes),
                ("pfuncs", Folder::PartitionFunctions),
                ("ftcats", Folder::FullTextCatalogs),
            ]),
            "security" => folders(&[
                ("users", Folder::Users),
                ("dbroles", Folder::DatabaseRoles),
                ("approles", Folder::ApplicationRoles),
                ("schemas", Folder::Schemas),
                ("certs", Folder::Certificates),
                ("symkeys", Folder::SymmetricKeys),
                ("asymkeys", Folder::AsymmetricKeys),
            ]),
            "procs" => objects("o.type IN ('P', 'PC')", "o", NodeKind::Procedure, true),
            "tfuncs" => objects(
                "o.type IN ('IF', 'TF', 'FT')",
                "o",
                NodeKind::Function,
                true,
            ),
            "sfuncs" => objects("o.type IN ('FN', 'FS')", "o", NodeKind::Function, true),
            "afuncs" => objects("o.type = 'AF'", "p", NodeKind::Function, false),
            "seqs" => objects("o.type = 'SO'", "o", NodeKind::Sequence, false),
            "synonyms" => objects("o.type = 'SN'", "p", NodeKind::Synonym, false),
            "rules" => objects("o.type = 'R'", "p", NodeKind::Constraint, false),
            "defaults" => objects(
                "o.type = 'D' AND o.parent_object_id = 0",
                "p",
                NodeKind::Constraint,
                false,
            ),
            "dtypes" => types(0),
            "ttypes" => types(1),
            "dbtrig" => Some(self.listed(
                &format!(
                    "SELECT x.object_id, x.name FROM {d}.sys.triggers x \
                      WHERE x.parent_class = 0 ORDER BY x.name"
                ),
                &format!("{base}dt/"),
                NodeKind::Trigger,
                false,
            )),
            _ => {
                let item = database_item(tag)?;
                Some(self.listed(
                    &item.list_sql(&format!("{d}.")),
                    &format!("{base}x/{tag}/"),
                    item.kind,
                    false,
                ))
            }
        }
    }

    /// Parâmetros de uma procedure ou função, como o SSMS mostra:
    /// `@id (int, Input)`; o retorno de uma função escalar vem primeiro.
    fn parameters(&self, db: &str, obj: i64) -> Result<Vec<ObjectNode>, MetadataError> {
        let d = bracket(db);
        let base = format!("db/{}/par/{obj}/", enc(db));
        Ok(self
            .rows(&format!(
                "SELECT p.parameter_id, p.name, t.name, p.max_length, p.precision, p.scale, \
                        p.is_output, p.is_readonly \
                   FROM {d}.sys.parameters p \
                   JOIN {d}.sys.types t ON t.user_type_id = p.user_type_id \
                  WHERE p.object_id = {obj} ORDER BY p.parameter_id"
            ))?
            .into_iter()
            .filter_map(|r| {
                let text = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
                let number = |i: usize| text(i).parse::<i64>().unwrap_or(0);
                let id = r.first().cloned().flatten()?;
                let ty = crate::explorer::plain_type(&text(2), number(3), number(4), number(5));
                let label = if id == "0" {
                    trf!("Returns {type}", type = ty)
                } else {
                    let direction = if text(6) == "1" {
                        tr("Output")
                    } else if text(7) == "1" {
                        tr("Read-only")
                    } else {
                        tr("Input")
                    };
                    format!("{} ({ty}, {direction})", text(1))
                };
                Some(node(
                    format!("{base}{id}"),
                    label,
                    NodeKind::Parameter,
                    false,
                ))
            })
            .collect())
    }

    /// Scripts dos nós deste módulo; None quando o id é de outro lugar.
    pub(crate) fn more_script(
        &self,
        id: &str,
        action: ScriptAction,
    ) -> Option<Result<String, MetadataError>> {
        if matches!(action, ScriptAction::SelectTop(_)) {
            return None;
        }
        let drop = action == ScriptAction::Drop;
        let parts: Vec<&str> = id.split('/').collect();
        let number = |s: &str| s.parse::<i64>().map_err(|_| gone());
        Some(match parts.as_slice() {
            ["db", db, "p", obj] => {
                number(obj).and_then(|obj| self.script_plain(&dec(db), obj, drop))
            }
            ["db", db, "o", obj] => {
                // Só a sequence: o resto deste id o explorer já escreve.
                let db = dec(db);
                let obj = match number(obj) {
                    Ok(o) => o,
                    Err(e) => return Some(Err(e)),
                };
                match self.object_name(&db, obj) {
                    Ok((name, ty, _)) if ty == "SO" && !drop => {
                        self.create_sequence(&db, obj, &name)
                    }
                    Ok(_) => return None,
                    Err(e) => Err(e),
                }
            }
            ["db", db, "type", ty] => {
                number(ty).and_then(|ty| self.script_type(&dec(db), ty, drop))
            }
            ["db", db, "dt", obj] => number(obj).and_then(|obj| {
                let d = bracket(&dec(db));
                if drop {
                    let name = self.one(&format!(
                        "SELECT QUOTENAME(name) FROM {d}.sys.triggers WHERE object_id = {obj}"
                    ))?;
                    Ok(format!("DROP TRIGGER {name} ON DATABASE\nGO\n"))
                } else {
                    self.module(&format!(
                        "SELECT definition FROM {d}.sys.sql_modules WHERE object_id = {obj}"
                    ))
                }
            }),
            ["db", db, "x", "schemas", name] => {
                let (d, name) = (bracket(&dec(db)), dec(name));
                if drop {
                    Ok(format!("DROP SCHEMA {}\nGO\n", bracket(&name)))
                } else {
                    self.one(&format!(
                        "SELECT ISNULL(p.name, 'dbo') FROM {d}.sys.schemas x \
                           LEFT JOIN {d}.sys.database_principals p \
                             ON p.principal_id = x.principal_id WHERE x.name = {}",
                        lit(&name)
                    ))
                    .map(|owner| {
                        format!(
                            "CREATE SCHEMA {} AUTHORIZATION {}\nGO\n",
                            bracket(&name),
                            bracket(&owner)
                        )
                    })
                }
            }
            ["srv", "Triggers", name] => {
                let name = dec(name);
                if drop {
                    Ok(format!(
                        "DROP TRIGGER {} ON ALL SERVER\nGO\n",
                        bracket(&name)
                    ))
                } else {
                    self.module(&format!(
                        "SELECT m.definition FROM sys.server_sql_modules m \
                           JOIN sys.server_triggers t ON t.object_id = m.object_id \
                          WHERE t.name = {}",
                        lit(&name)
                    ))
                }
            }
            ["srv", "LinkedServers", name] => self.script_linked_server(&dec(name), drop),
            ["srv", "Jobs", name] => self.script_job(&dec(name), drop),
            ["db", _, "x" | "par" | "st", ..] | ["srv", ..] => Err(not_yet()),
            _ => return None,
        })
    }

    /// A primeira coluna da primeira linha.
    fn one(&self, sql: &str) -> Result<String, MetadataError> {
        self.rows(sql)?
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next().flatten())
            .ok_or_else(gone)
    }

    /// A definição de um módulo (`sys.sql_modules`), pronta para rodar.
    fn module(&self, sql: &str) -> Result<String, MetadataError> {
        let def = self
            .rows(sql)?
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next().flatten())
            .ok_or_else(|| {
                MetadataError(
                    tr("The definition is encrypted or not visible to this login.").to_string(),
                )
            })?;
        Ok(format!(
            "SET ANSI_NULLS ON\nGO\nSET QUOTED_IDENTIFIER ON\nGO\n{}\nGO\n",
            def.trim()
        ))
    }

    /// Synonym, rule, default e função de agregação.
    fn script_plain(&self, db: &str, obj: i64, drop: bool) -> Result<String, MetadataError> {
        let d = bracket(db);
        let (name, ty, _) = self.object_name(db, obj)?;
        let what = match ty.as_str() {
            "SN" => "SYNONYM",
            "R" => "RULE",
            "D" => "DEFAULT",
            "AF" => "AGGREGATE",
            _ => return Err(not_yet()),
        };
        if drop {
            return Ok(format!("DROP {what} {name}\nGO\n"));
        }
        match ty.as_str() {
            "SN" => {
                let base = self.one(&format!(
                    "SELECT base_object_name FROM {d}.sys.synonyms WHERE object_id = {obj}"
                ))?;
                Ok(format!("CREATE SYNONYM {name} FOR {base}\nGO\n"))
            }
            "R" | "D" => self.module(&format!(
                "SELECT definition FROM {d}.sys.sql_modules WHERE object_id = {obj}"
            )),
            // A função de agregação vem de um assembly: não tem texto.
            _ => Err(not_yet()),
        }
    }

    fn create_sequence(&self, db: &str, obj: i64, name: &str) -> Result<String, MetadataError> {
        let d = bracket(db);
        let r = self
            .rows(&format!(
                "SELECT t.name, CONVERT(varchar(40), q.start_value), \
                        CONVERT(varchar(40), q.increment), CONVERT(varchar(40), q.minimum_value), \
                        CONVERT(varchar(40), q.maximum_value), q.is_cycling, q.is_cached, \
                        q.cache_size \
                   FROM {d}.sys.sequences q \
                   JOIN {d}.sys.types t ON t.user_type_id = q.user_type_id \
                  WHERE q.object_id = {obj}"
            ))?
            .into_iter()
            .next()
            .ok_or_else(gone)?;
        let text = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        let cache = match (text(6).as_str(), text(7)) {
            ("1", size) if !size.is_empty() => format!("CACHE {size}"),
            ("1", _) => "CACHE".to_string(),
            _ => "NO CACHE".to_string(),
        };
        Ok(format!(
            "CREATE SEQUENCE {name}\n AS {}\n START WITH {}\n INCREMENT BY {}\n MINVALUE {}\n \
             MAXVALUE {}\n {}\n {cache}\nGO\n",
            bracket(&text(0)),
            text(1),
            text(2),
            text(3),
            text(4),
            if text(5) == "1" { "CYCLE" } else { "NO CYCLE" },
        ))
    }

    /// `CREATE TYPE ... FROM` (tipo de dados) ou `AS TABLE` (tipo de tabela).
    fn script_type(&self, db: &str, ty: i64, drop: bool) -> Result<String, MetadataError> {
        let d = bracket(db);
        let r = self
            .rows(&format!(
                "SELECT QUOTENAME(s.name) + '.' + QUOTENAME(t.name), b.name, t.max_length, \
                        t.precision, t.scale, t.is_nullable, tt.type_table_object_id \
                   FROM {d}.sys.types t \
                   JOIN {d}.sys.schemas s ON s.schema_id = t.schema_id \
                   LEFT JOIN {d}.sys.types b ON b.user_type_id = t.system_type_id \
                   LEFT JOIN {d}.sys.table_types tt ON tt.user_type_id = t.user_type_id \
                  WHERE t.user_type_id = {ty}"
            ))?
            .into_iter()
            .next()
            .ok_or_else(gone)?;
        let text = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        let number = |i: usize| text(i).parse::<i64>().unwrap_or(0);
        let name = text(0);
        if drop {
            return Ok(format!("DROP TYPE {name}\nGO\n"));
        }
        let null = |nullable: bool| if nullable { "NULL" } else { "NOT NULL" };
        let table = text(6);
        if table.is_empty() {
            return Ok(format!(
                "CREATE TYPE {name} FROM {} {}\nGO\n",
                sql_type(&text(1), false, "", number(2), number(3), number(4)),
                null(text(5) == "1")
            ));
        }
        let columns: Vec<String> = self
            .rows(&format!(
                "SELECT c.name, t.name, t.is_user_defined, s.name, c.max_length, c.precision, \
                        c.scale, c.is_nullable, c.is_identity \
                   FROM {d}.sys.columns c \
                   JOIN {d}.sys.types t ON t.user_type_id = c.user_type_id \
                   JOIN {d}.sys.schemas s ON s.schema_id = t.schema_id \
                  WHERE c.object_id = {table} ORDER BY c.column_id"
            ))?
            .into_iter()
            .map(|c| {
                let text = |i: usize| c.get(i).cloned().flatten().unwrap_or_default();
                let number = |i: usize| text(i).parse::<i64>().unwrap_or(0);
                format!(
                    "\t{} {}{} {}",
                    bracket(&text(0)),
                    sql_type(
                        &text(1),
                        text(2) == "1",
                        &text(3),
                        number(4),
                        number(5),
                        number(6)
                    ),
                    if text(8) == "1" { " IDENTITY(1,1)" } else { "" },
                    null(text(7) == "1")
                )
            })
            .collect();
        Ok(format!(
            "CREATE TYPE {name} AS TABLE(\n{}\n)\nGO\n",
            columns.join(",\n")
        ))
    }

    fn script_linked_server(&self, name: &str, drop: bool) -> Result<String, MetadataError> {
        if drop {
            return Ok(format!(
                "EXEC master.dbo.sp_dropserver @server = {}, @droplogins = 'droplogins'\nGO\n",
                lit(name)
            ));
        }
        let r = self
            .rows(&format!(
                "SELECT product, provider, data_source, catalog FROM sys.servers \
                  WHERE is_linked = 1 AND name = {}",
                lit(name)
            ))?
            .into_iter()
            .next()
            .ok_or_else(gone)?;
        let text = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        let mut sql = format!(
            "EXEC master.dbo.sp_addlinkedserver @server = {}, @srvproduct = {}",
            lit(name),
            lit(&text(0))
        );
        // Com o produto "SQL Server" o servidor não aceita os outros.
        if text(0) != "SQL Server" {
            for (arg, value) in [
                ("@provider", text(1)),
                ("@datasrc", text(2)),
                ("@catalog", text(3)),
            ] {
                if !value.is_empty() {
                    sql.push_str(&format!(", {arg} = {}", lit(&value)));
                }
            }
        }
        sql.push_str("\nGO\n");
        Ok(sql)
    }

    /// O job com os passos dele, pelas procedures do msdb.
    fn script_job(&self, name: &str, drop: bool) -> Result<String, MetadataError> {
        let job = lit(name);
        if drop {
            return Ok(format!(
                "EXEC msdb.dbo.sp_delete_job @job_name = {job}\nGO\n"
            ));
        }
        let head = self
            .rows(&format!(
                "SELECT enabled, description FROM msdb.dbo.sysjobs WHERE name = {job}"
            ))?
            .into_iter()
            .next()
            .ok_or_else(gone)?;
        let text = |r: &[Option<String>], i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        let mut sql = format!(
            "EXEC msdb.dbo.sp_add_job @job_name = {job}, @enabled = {}, @description = {}\nGO\n",
            text(&head, 0),
            lit(&text(&head, 1))
        );
        for step in self.rows(&format!(
            "SELECT s.step_name, s.subsystem, s.database_name, s.command, s.on_success_action, \
                    s.on_fail_action \
               FROM msdb.dbo.sysjobsteps s JOIN msdb.dbo.sysjobs j ON j.job_id = s.job_id \
              WHERE j.name = {job} ORDER BY s.step_id"
        ))? {
            sql.push_str(&format!(
                "EXEC msdb.dbo.sp_add_jobstep @job_name = {job}, @step_name = {}, \
                 @subsystem = {}, @database_name = {}, @on_success_action = {}, \
                 @on_fail_action = {},\n    @command = {}\nGO\n",
                lit(&text(&step, 0)),
                lit(&text(&step, 1)),
                lit(&text(&step, 2)),
                text(&step, 4),
                text(&step, 5),
                lit(&text(&step, 3)),
            ));
        }
        for schedule in self.rows(&format!(
            "SELECT c.name FROM msdb.dbo.sysjobschedules js \
               JOIN msdb.dbo.sysjobs j ON j.job_id = js.job_id \
               JOIN msdb.dbo.sysschedules c ON c.schedule_id = js.schedule_id \
              WHERE j.name = {job} ORDER BY c.name"
        ))? {
            sql.push_str(&format!(
                "EXEC msdb.dbo.sp_attach_schedule @job_name = {job}, @schedule_name = {}\nGO\n",
                lit(&text(&schedule, 0))
            ));
        }
        sql.push_str(&format!(
            "EXEC msdb.dbo.sp_add_jobserver @job_name = {job}\nGO\n"
        ));
        Ok(sql)
    }

    /// Propriedades dos nós deste módulo; None quando o id é de outro
    /// lugar.
    pub(crate) fn more_properties(
        &self,
        node: &NodeId,
    ) -> Option<Result<Vec<Property>, MetadataError>> {
        let parts: Vec<&str> = node.0.split('/').collect();
        let number = |s: &str| s.parse::<i64>().map_err(|_| gone());
        let sql = match parts.as_slice() {
            ["srv", key, name] => server_item(key)?.props_sql("", &dec(name)),
            ["db", db, "x", key, name] => {
                database_item(key)?.props_sql(&format!("{}.", bracket(&dec(db))), &dec(name))
            }
            ["db", db, "dt", obj] => {
                let obj = match number(obj) {
                    Ok(o) => o,
                    Err(e) => return Some(Err(e)),
                };
                format!(
                    "SELECT CASE x.is_disabled WHEN 1 THEN 'yes' ELSE 'no' END AS [Disabled], \
                            {CREATED} \
                       FROM {}.sys.triggers x WHERE x.object_id = {obj}",
                    bracket(&dec(db))
                )
            }
            ["db", db, "type", ty] => {
                let ty = match number(ty) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                let d = bracket(&dec(db));
                format!(
                    "SELECT s.name AS [Schema], \
                            CASE t.is_table_type WHEN 1 THEN NULL ELSE b.name END AS [Base type], \
                            CASE t.is_nullable WHEN 1 THEN 'yes' ELSE 'no' END AS [Nullable] \
                       FROM {d}.sys.types t \
                       JOIN {d}.sys.schemas s ON s.schema_id = t.schema_id \
                       LEFT JOIN {d}.sys.types b ON b.user_type_id = t.system_type_id \
                      WHERE t.user_type_id = {ty}"
                )
            }
            ["db", db, "par", obj, par] => {
                let (obj, par) = match (number(obj), number(par)) {
                    (Ok(o), Ok(p)) => (o, p),
                    _ => return Some(Err(gone())),
                };
                let d = bracket(&dec(db));
                format!(
                    "SELECT t.name AS [Data type], \
                            CASE p.is_output WHEN 1 THEN 'yes' ELSE 'no' END AS [Output], \
                            CASE p.has_default_value WHEN 1 \
                                 THEN CONVERT(nvarchar(200), p.default_value) END AS [Default] \
                       FROM {d}.sys.parameters p \
                       JOIN {d}.sys.types t ON t.user_type_id = p.user_type_id \
                      WHERE p.object_id = {obj} AND p.parameter_id = {par}"
                )
            }
            ["db", db, "st", obj, stat] => {
                let (obj, stat) = match (number(obj), number(stat)) {
                    (Ok(o), Ok(s)) => (o, s),
                    _ => return Some(Err(gone())),
                };
                let d = bracket(&dec(db));
                format!(
                    "SELECT STUFF((SELECT ', ' + c.name FROM {d}.sys.stats_columns sc \
                                     JOIN {d}.sys.columns c ON c.object_id = sc.object_id \
                                      AND c.column_id = sc.column_id \
                                    WHERE sc.object_id = x.object_id AND sc.stats_id = x.stats_id \
                                    ORDER BY sc.stats_column_id \
                                      FOR XML PATH(''), TYPE).value('.', 'nvarchar(max)'), \
                                  1, 2, '') AS [Columns], \
                            x.filter_definition AS [Condition], \
                            CASE x.auto_created WHEN 1 THEN 'yes' ELSE 'no' END AS [Auto created], \
                            (SELECT CONVERT(varchar(19), p.last_updated, 120) \
                               FROM {d}.sys.dm_db_stats_properties(x.object_id, x.stats_id) p) \
                                AS [Last updated] \
                       FROM {d}.sys.stats x WHERE x.object_id = {obj} AND x.stats_id = {stat}"
                )
            }
            _ => return None,
        };
        Some(self.properties_of(&sql))
    }
}

/// As pastas de primeiro nível do banco, na ordem do SSMS.
pub(crate) const DATABASE_FOLDERS: [(&str, Folder); 8] = [
    ("diagrams", Folder::Diagrams),
    ("tables", Folder::Tables),
    ("views", Folder::Views),
    ("synonyms", Folder::Synonyms),
    ("prog", Folder::Programmability),
    ("broker", Folder::ServiceBroker),
    ("storage", Folder::Storage),
    ("security", Folder::Security),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_folder_has_its_own_key() {
        let mut keys: Vec<&str> = DATABASE_ITEMS.iter().map(|i| i.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), DATABASE_ITEMS.len());
        for item in SERVER_ITEMS {
            assert!(server_folder(item.key).is_some());
        }
    }

    #[test]
    fn the_name_goes_into_the_property_query_as_a_literal() {
        let sql = server_item("Jobs").unwrap().props_sql("", "it's");
        assert!(sql.ends_with("WHERE x.name = N'it''s'"), "{sql}");
        let sql = database_item("schemas").unwrap().props_sql("[db].", "x");
        assert!(sql.contains("FROM [db].sys.schemas x"), "{sql}");
        assert!(!sql.contains('{'), "{sql}");
    }
}
