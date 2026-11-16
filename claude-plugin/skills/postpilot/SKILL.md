---
name: postpilot
description: Work in the user's PostPilot (desktop PostgreSQL manager) through its MCP tools. Use when the user mentions PostPilot, their query window or editor, a database or table open in PostPilot, or asks to write, explain, fix, optimize or run PostgreSQL queries against a server connected there.
---

# Working in PostPilot

PostPilot is the user's desktop PostgreSQL manager, in the style of SQL Server Management Studio. The `postpilot` MCP tools act on the PostPilot window open on their machine. If a tool answers that PostPilot is not open or that Claude Code access is off, tell the user to open PostPilot and turn on **Tools > Allow Claude Code (MCP)**, then try again. Do not work around it.

## Find your way first

1. `list_servers` shows the connected servers. A tool with no `server` uses the server of the active query window.
2. `list_query_windows` and `get_query_window` show what the user is looking at: the text, the selection, the server and the database. When the user says "this query" or "my query", read the active window first.
3. Before you write SQL, look at the real schema with `list_objects` and `describe_table`, and at function code with `describe_function`. Never guess column names.

## Reading is free, changing is the user's call

- `run_query` runs read-only SQL at once, inside a READ ONLY transaction with a 60 s timeout. Use it to explore: counts, samples with `LIMIT`, `EXPLAIN` (without ANALYZE).
- Any SQL that can change data or schema opens a confirmation dialog in PostPilot. The user sees the full SQL and clicks Run or Don't Run. If they decline, the tool says so. Accept it and do not retry the same statement.
- Results sent to you are capped: 200 rows per result by default, 1000 at most, 64 KB in all. Ask for aggregates and `LIMIT` instead of whole tables. A `truncated: true` result means there are more rows than you saw.

## Plans and activity

- For anything about speed, call `explain_query` before you suggest a change. It returns the plan as text on PostgreSQL and SQL Server. The default is the estimated plan and runs nothing. `analyze: true` executes the statement (inside a transaction that is rolled back) to get real rows and times, so use it only when the estimates are not enough or the user asks. SQL that can change anything asks the user first.
- When something is slow or stuck right now, call `get_activity`. It lists sessions with their current SQL, the blocking chains from the head blocker down, and the waiting locks. Say which session blocks which and what it is running. You cannot cancel or kill a session: tell the user which one to act on in Activity Monitor.

## Environments

`list_servers` shows the environment of each server and the user's rule for AI there (`ai_access`).

- `free`: the rule above (reading is free, changes ask).
- `confirm` (Production by default): reading the schema and estimated plans stays free, but anything that runs SQL or brings data (`run_query`, `execute_query_window`, `explain_query` with `analyze`, `get_activity`) asks the user first, even a `SELECT`. Ask for little and make each call count.
- `blocked`: no tool reaches that server. Do not retry or look for another way. Tell the user it is blocked in PostPilot.

## Prefer the editor

When the user asks you to write, fix or rewrite a query, put it in their editor with `edit_query_window`, then tell them what you changed. The new text is selected, and Ctrl+Z undoes it. Run it yourself (`execute_query_window`) only when they ask. Results then appear in their grid, and `get_query_results` reads them.

- `replace_selection` (the default) replaces the selected text, or inserts at the cursor.
- `replace_all` rewrites the whole window. `append` adds a statement at the end.
- `open_query_window` opens a new tab, optionally on another server or database.

## PostgreSQL habits

- Quote identifiers that have capitals or spaces (`"Itens"`). Qualify the schema when it is not `public`.
- JSON columns (`json`/`jsonb`): `->` returns JSON, `->>` returns text, `#>>'{a,b}'` reads a path, `@>` tests containment (and can use a GIN index), and `jsonb_array_elements` / `jsonb_each` turn arrays and objects into rows. Cast `->>` before you compare numbers: `(dados->>'total')::numeric > 100`.
- Suggest indexes as SQL in the editor. Do not create them without the user's go-ahead.
- Before a big `UPDATE` or `DELETE`, count the affected rows with a `SELECT` first and tell the user the number.
