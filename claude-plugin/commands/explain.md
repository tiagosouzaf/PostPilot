---
description: Explain the query in PostPilot's active query window (or its selection) in plain words
argument-hint: "[what to focus on]"
---

Read the active query window in PostPilot with `get_query_window`. If there is a selection, explain only the selection. Otherwise explain the whole text.

Look up the tables and columns it uses with `describe_table`, so the explanation matches the real schema. Explain what the query returns or changes, step by step, in the user's language, and point out anything risky or surprising: missing join conditions, `NULL` handling, implicit casts, a full-table `UPDATE` or `DELETE`. Do not run it and do not change the editor.

Focus: $ARGUMENTS
