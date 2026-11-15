//! As ferramentas que o Claude enxerga: nome, descrição e esquema JSON dos
//! argumentos, e a leitura dos argumentos de uma chamada.
//!
//! O servidor MCP publica esta lista em `tools/list` e o painel do Claude
//! usa a mesma; quem atende é a interface (`pgstudio-ui-shell`).

use serde::Deserialize;
use serde_json::{Value, json};

/// Uma ferramenta, como o MCP descreve (`name`, `description`,
/// `inputSchema`).
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    /// Só lê (anotação `readOnlyHint` do MCP).
    pub read_only: bool,
}

impl ToolSpec {
    /// A ferramenta no formato do `tools/list` do MCP.
    pub fn to_mcp(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": !self.read_only && self.name == "run_query",
                "openWorldHint": false,
            },
        })
    }
}

const SERVER: &str = "Server to use, as list_servers shows it (user@host:port). \
Default: the server of the active query window, or the active server in Object Explorer.";
const DATABASE: &str = "Database. Default: the database of the active query window, \
or the server's default database.";
const WINDOW: &str = "Query window id, as list_query_windows shows it. Default: the active window.";

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Todas as ferramentas, na ordem em que aparecem para o Claude.
pub fn all() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "list_servers",
            description: "List the database servers (PostgreSQL or SQL Server) connected in PostPilot's \
                Object Explorer, with the active one marked and the engine of each. Start \
                here to know what you can query, and write SQL in that engine's dialect \
                (T-SQL when engine is sqlserver).",
            input_schema: schema(json!({}), &[]),
            read_only: true,
        },
        ToolSpec {
            name: "list_databases",
            description: "List the databases of a server that the user can connect to, \
                with owner, encoding and size.",
            input_schema: schema(
                json!({ "server": { "type": "string", "description": SERVER } }),
                &[],
            ),
            read_only: true,
        },
        ToolSpec {
            name: "list_objects",
            description: "List tables, views, materialized views, sequences, functions and \
                procedures of a database (system schemas excluded), with estimated row counts \
                and comments.",
            input_schema: schema(
                json!({
                    "server": { "type": "string", "description": SERVER },
                    "database": { "type": "string", "description": DATABASE },
                    "schema": { "type": "string", "description": "Only this schema." },
                }),
                &[],
            ),
            read_only: true,
        },
        ToolSpec {
            name: "describe_table",
            description: "Describe a table or view: columns with types, nullability, defaults \
                and comments; constraints (primary, unique, foreign keys, checks); indexes; \
                foreign keys from other tables that reference it; and the view definition.",
            input_schema: schema(
                json!({
                    "table": { "type": "string", "description": "Table or view as written in SQL, e.g. orders or sales.\"Items\"." },
                    "server": { "type": "string", "description": SERVER },
                    "database": { "type": "string", "description": DATABASE },
                }),
                &["table"],
            ),
            read_only: true,
        },
        ToolSpec {
            name: "describe_function",
            description: "Show the CREATE statement of a function or procedure (all overloads).",
            input_schema: schema(
                json!({
                    "function": { "type": "string", "description": "Function name, optionally schema-qualified." },
                    "server": { "type": "string", "description": SERVER },
                    "database": { "type": "string", "description": DATABASE },
                }),
                &["function"],
            ),
            read_only: true,
        },
        ToolSpec {
            name: "run_query",
            description: "Run SQL in its own connection (not in a query window) and return the \
                rows as JSON. Read-only SQL (SELECT, WITH; on PostgreSQL also EXPLAIN without \
                ANALYZE and SHOW) runs right away: on PostgreSQL inside a READ ONLY \
                transaction with a 60 s timeout, on SQL Server inside a transaction that is \
                always rolled back. Anything that \
                can change data, schema or the session (INSERT, UPDATE, DELETE, DDL, EXPLAIN \
                ANALYZE of a change, ...) shows a confirmation dialog in PostPilot with the full \
                SQL, and runs only if the user clicks Run; if they decline, you get an error \
                saying so. Results are capped (max_rows per result, 64 KB in total). To show \
                SQL to the user instead of running it, use edit_query_window.",
            input_schema: schema(
                json!({
                    "sql": { "type": "string", "description": "One or more statements separated by semicolons." },
                    "server": { "type": "string", "description": SERVER },
                    "database": { "type": "string", "description": DATABASE },
                    "max_rows": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Rows per result to return (default 200)." },
                }),
                &["sql"],
            ),
            read_only: false,
        },
        ToolSpec {
            name: "list_query_windows",
            description: "List the open query windows (editor tabs): id, title, server, \
                database, whether it has unsaved changes, whether it is running, and which one \
                is active.",
            input_schema: schema(json!({}), &[]),
            read_only: true,
        },
        ToolSpec {
            name: "get_query_window",
            description: "Read a query window: its full text, the selected text and range \
                (characters, 0-based), the cursor line and column, server and database, and a \
                summary of its last execution.",
            input_schema: schema(
                json!({ "window": { "type": "integer", "description": WINDOW } }),
                &[],
            ),
            read_only: true,
        },
        ToolSpec {
            name: "edit_query_window",
            description: "Write SQL into a query window's editor. Does not run it. The user \
                sees the change selected and can undo it with Ctrl+Z. Modes: replace_all, \
                replace_selection (or insert at the cursor if nothing is selected), \
                insert_at_cursor, append.",
            input_schema: schema(
                json!({
                    "text": { "type": "string" },
                    "mode": { "type": "string", "enum": ["replace_all", "replace_selection", "insert_at_cursor", "append"], "description": "Default: replace_selection." },
                    "window": { "type": "integer", "description": WINDOW },
                }),
                &["text"],
            ),
            read_only: false,
        },
        ToolSpec {
            name: "open_query_window",
            description: "Open a new query window (like New Query), optionally with text, on a \
                server and database. Returns its id. Does not run anything.",
            input_schema: schema(
                json!({
                    "text": { "type": "string" },
                    "server": { "type": "string", "description": SERVER },
                    "database": { "type": "string", "description": DATABASE },
                }),
                &[],
            ),
            read_only: false,
        },
        ToolSpec {
            name: "execute_query_window",
            description: "Execute a query window like F5: the selection if there is one, \
                otherwise the whole text. Results appear in the window's grid for the user. \
                SQL that can change anything asks the user for confirmation first. Waits for \
                the end and returns the results (capped like run_query) and messages.",
            input_schema: schema(
                json!({
                    "window": { "type": "integer", "description": WINDOW },
                    "max_rows": { "type": "integer", "minimum": 1, "maximum": 1000 },
                }),
                &[],
            ),
            read_only: false,
        },
        ToolSpec {
            name: "get_query_results",
            description: "Read the results and messages of the last execution of a query \
                window (what the user sees in the Results and Messages tabs), capped like \
                run_query.",
            input_schema: schema(
                json!({
                    "window": { "type": "integer", "description": WINDOW },
                    "max_rows": { "type": "integer", "minimum": 1, "maximum": 1000 },
                }),
                &[],
            ),
            read_only: true,
        },
    ]
}

/// A ferramenta com esse nome.
pub fn find(name: &str) -> Option<ToolSpec> {
    all().into_iter().find(|t| t.name == name)
}

/// Como escrever no editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditMode {
    ReplaceAll,
    #[default]
    ReplaceSelection,
    InsertAtCursor,
    Append,
}

/// Uma chamada com os argumentos já lidos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCall {
    ListServers,
    ListDatabases {
        server: Option<String>,
    },
    ListObjects {
        server: Option<String>,
        database: Option<String>,
        schema: Option<String>,
    },
    DescribeTable {
        table: String,
        server: Option<String>,
        database: Option<String>,
    },
    DescribeFunction {
        function: String,
        server: Option<String>,
        database: Option<String>,
    },
    RunQuery {
        sql: String,
        server: Option<String>,
        database: Option<String>,
        max_rows: Option<usize>,
    },
    ListQueryWindows,
    GetQueryWindow {
        window: Option<u32>,
    },
    EditQueryWindow {
        text: String,
        mode: EditMode,
        window: Option<u32>,
    },
    OpenQueryWindow {
        text: Option<String>,
        server: Option<String>,
        database: Option<String>,
    },
    ExecuteQueryWindow {
        window: Option<u32>,
        max_rows: Option<usize>,
    },
    GetQueryResults {
        window: Option<u32>,
        max_rows: Option<usize>,
    },
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Args {
    server: Option<String>,
    database: Option<String>,
    schema: Option<String>,
    table: Option<String>,
    function: Option<String>,
    sql: Option<String>,
    text: Option<String>,
    mode: Option<EditMode>,
    window: Option<u32>,
    max_rows: Option<usize>,
}

impl ToolCall {
    /// Lê uma chamada `name(args)`. O erro é a mensagem para o Claude.
    pub fn parse(name: &str, args: &Value) -> Result<ToolCall, String> {
        let a: Args = match args {
            Value::Null => Args::default(),
            v => serde_json::from_value(v.clone())
                .map_err(|e| format!("Invalid arguments for {name}: {e}"))?,
        };
        let required = |v: Option<String>, field: &str| {
            v.filter(|s| !s.trim().is_empty())
                .ok_or_else(|| format!("{name} needs \"{field}\"."))
        };
        let blank = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
        Ok(match name {
            "list_servers" => ToolCall::ListServers,
            "list_databases" => ToolCall::ListDatabases {
                server: blank(a.server),
            },
            "list_objects" => ToolCall::ListObjects {
                server: blank(a.server),
                database: blank(a.database),
                schema: blank(a.schema),
            },
            "describe_table" => ToolCall::DescribeTable {
                table: required(a.table, "table")?,
                server: blank(a.server),
                database: blank(a.database),
            },
            "describe_function" => ToolCall::DescribeFunction {
                function: required(a.function, "function")?,
                server: blank(a.server),
                database: blank(a.database),
            },
            "run_query" => ToolCall::RunQuery {
                sql: required(a.sql, "sql")?,
                server: blank(a.server),
                database: blank(a.database),
                max_rows: a.max_rows,
            },
            "list_query_windows" => ToolCall::ListQueryWindows,
            "get_query_window" => ToolCall::GetQueryWindow { window: a.window },
            "edit_query_window" => ToolCall::EditQueryWindow {
                text: a.text.ok_or_else(|| format!("{name} needs \"text\"."))?,
                mode: a.mode.unwrap_or_default(),
                window: a.window,
            },
            "open_query_window" => ToolCall::OpenQueryWindow {
                text: a.text,
                server: blank(a.server),
                database: blank(a.database),
            },
            "execute_query_window" => ToolCall::ExecuteQueryWindow {
                window: a.window,
                max_rows: a.max_rows,
            },
            "get_query_results" => ToolCall::GetQueryResults {
                window: a.window,
                max_rows: a.max_rows,
            },
            other => return Err(format!("Unknown tool \"{other}\".")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_parses_with_its_required_fields() {
        for tool in all() {
            let required = tool.input_schema["required"].as_array().unwrap();
            let mut args = serde_json::Map::new();
            for r in required {
                args.insert(r.as_str().unwrap().into(), json!("x"));
            }
            ToolCall::parse(tool.name, &Value::Object(args))
                .unwrap_or_else(|e| panic!("{}: {e}", tool.name));
            assert!(tool.description.len() > 20);
        }
    }

    #[test]
    fn bad_arguments() {
        assert!(ToolCall::parse("run_query", &json!({})).is_err());
        assert!(ToolCall::parse("run_query", &json!({"sql": "  "})).is_err());
        assert!(ToolCall::parse("run_query", &json!({"sql": "x", "nope": 1})).is_err());
        assert!(ToolCall::parse("nope", &json!({})).is_err());
        assert_eq!(
            ToolCall::parse(
                "edit_query_window",
                &json!({"text": "a", "mode": "append", "window": 2})
            ),
            Ok(ToolCall::EditQueryWindow {
                text: "a".into(),
                mode: EditMode::Append,
                window: Some(2)
            })
        );
        assert_eq!(
            ToolCall::parse("list_servers", &Value::Null),
            Ok(ToolCall::ListServers)
        );
    }

    #[test]
    fn mcp_shape() {
        let t = find("run_query").unwrap().to_mcp();
        assert_eq!(t["inputSchema"]["required"], json!(["sql"]));
        assert_eq!(t["annotations"]["readOnlyHint"], false);
    }
}
