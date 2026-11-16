# PostPilot plugin for Claude Code

This plugin lets Claude Code work in [PostPilot](https://github.com/tiagosouzaf/postpilot), the desktop PostgreSQL manager. With it, Claude can:

- see the servers connected in PostPilot, their databases, tables, columns, indexes and functions
- read and write the query windows (the text it writes is selected, and Ctrl+Z undoes it)
- run queries and read the results the user sees in the grid

Reading is free: read-only SQL runs inside a `READ ONLY` transaction. Anything that can change data or schema opens a confirmation dialog in PostPilot with the full SQL, and runs only if you click **Run**.

## Install

1. Install PostPilot. The `postpilot` command must be on your `PATH`, which the Arch, .deb and AppImage packages take care of.
2. In PostPilot, turn on **Tools > Allow Claude Code (MCP)**. The choice is saved.
3. In Claude Code, from the copy that came with the Linux packages:

   ```
   claude plugin marketplace add /usr/share/postpilot
   claude plugin install postpilot@postpilot
   ```

   or straight from GitHub: `/plugin marketplace add tiagosouzaf/postpilot`, then `/plugin install postpilot@postpilot`.

Without the plugin, you can add only the MCP server: `claude mcp add postpilot -- postpilot mcp`. Use one or the other, not both.

## What comes with it

| | |
| --- | --- |
| MCP server `postpilot` | `postpilot mcp`, the tools listed below |
| Skill `postpilot` | how to work in PostPilot: explore first, prefer the editor, PostgreSQL and JSON habits |
| `/postpilot:explain` | explain the query in the active window |
| `/postpilot:optimize` | rewrite the active query using its estimated plan |
| `/postpilot:fix` | fix the error from the last execution |

Tools: `list_servers`, `list_databases`, `list_objects`, `describe_table`, `describe_function`, `run_query`, `explain_query`, `get_activity`, `list_query_windows`, `get_query_window`, `edit_query_window`, `open_query_window`, `execute_query_window`, `get_query_results`.

## Privacy

What Claude reads (schema, query text, results) goes to Anthropic like the rest of the conversation. Results are capped at 200 rows per result by default (1000 at most) and 64 KB per call. Passwords never leave PostPilot.
