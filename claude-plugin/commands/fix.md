---
description: Fix the error from the last execution of PostPilot's active query window
argument-hint: "[extra context]"
---

Read the active query window in PostPilot with `get_query_window` and its last results with `get_query_results`. Find the error message. Check the real schema with `describe_table` / `list_objects` for the objects involved, because wrong names are the most common cause.

Fix the SQL and put the fixed version in the editor with `edit_query_window` (`replace_selection` if the user had a selection, otherwise `replace_all`). Explain the cause in one or two sentences. Do not run it unless the user asks. If the fix needs a change to data or schema, say so instead of doing it.

Context: $ARGUMENTS
