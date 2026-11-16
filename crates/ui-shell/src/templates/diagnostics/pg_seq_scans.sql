-- Tables read row by row the most (sequential scans): on a large table
-- with a high share of sequential reads, a missing index is likely.
select t.schemaname                                           as schema,
       t.relname                                              as table_name,
       pg_size_pretty(pg_total_relation_size(t.relid))        as total_size,
       t.n_live_tup                                           as rows_estimate,
       t.seq_scan                                             as sequential_scans,
       t.seq_tup_read                                         as rows_read_sequentially,
       round(t.seq_tup_read::numeric / nullif(t.seq_scan, 0)) as rows_per_scan,
       coalesce(t.idx_scan, 0)                                as index_scans,
       round(100.0 * t.seq_scan
             / nullif(t.seq_scan + coalesce(t.idx_scan, 0), 0), 1) as sequential_pct
  from pg_stat_user_tables t
 where t.seq_scan > 0
 order by t.seq_tup_read desc
 limit 30;
