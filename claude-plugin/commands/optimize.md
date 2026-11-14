---
description: Make the query in PostPilot's active window faster, using its estimated plan
argument-hint: "[constraints, e.g. no new indexes]"
---

Read the active query window in PostPilot with `get_query_window` (the selection if there is one). Describe the tables involved with `describe_table`, including their indexes and estimated row counts. Get the estimated plan with `run_query` using `EXPLAIN (FORMAT JSON) <query>`. Do not use ANALYZE unless the user asks.

Find the expensive parts: sequential scans on large tables, bad row estimates, sorts and hash joins that spill, and functions on indexed columns. Rewrite the query to be faster with the same result, and put the rewrite in the editor with `edit_query_window` (`replace_selection` if the user had a selection, otherwise `replace_all`). If an index would help, write the `CREATE INDEX` as a comment below the query instead of running it. Then summarize what you changed and why, with the plan costs before and after (run `EXPLAIN` on the new version).

Constraints: $ARGUMENTS
