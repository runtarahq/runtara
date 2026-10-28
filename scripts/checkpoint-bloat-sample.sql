-- One row describing the `checkpoints` table and the WAL position, for
-- sampling a runtime database over time (once a day for a week, say).
--
--   psql "$RUNTARA_DATABASE_URL" -X --csv -f scripts/checkpoint-bloat-sample.sql
--
-- Read-only: it reads statistics views, the catalog and relation sizes, and
-- scans no table. Keep every row. Between two samples:
--
-- - the WAL rate is pg_wal_lsn_diff(later.wal_lsn, earlier.wal_lsn) over the
--   seconds between them (the whole cluster, not just this table);
-- - autovacuum_count says how many times autovacuum vacuumed the table.
--
-- vacuum_trigger_dead and vacuum_trigger_inserts are the n_dead_tup and
-- n_ins_since_vacuum at which autovacuum next vacuums the table, from the
-- table's own settings where it has them. Postgres 18 scales the insert
-- fraction by the share of pages not all-frozen (relallfrozen of relpages),
-- so the more of the table is frozen, the fewer inserts start a vacuum.
WITH t AS (
    SELECT c.oid, c.reltoastrelid, c.reloptions, greatest(c.reltuples, 0) AS reltuples,
           c.relpages,
           -- New in 18; read through to_jsonb, it is NULL before.
           (to_jsonb(c) ->> 'relallfrozen')::bigint AS relallfrozen,
           (SELECT option_value FROM pg_options_to_table(c.reloptions)
            WHERE option_name = 'autovacuum_vacuum_threshold') AS threshold,
           (SELECT option_value FROM pg_options_to_table(c.reloptions)
            WHERE option_name = 'autovacuum_vacuum_scale_factor') AS scale_factor,
           (SELECT option_value FROM pg_options_to_table(c.reloptions)
            WHERE option_name = 'autovacuum_vacuum_insert_threshold') AS insert_threshold,
           (SELECT option_value FROM pg_options_to_table(c.reloptions)
            WHERE option_name = 'autovacuum_vacuum_insert_scale_factor') AS insert_scale_factor
    FROM pg_class c
    WHERE c.oid = 'checkpoints'::regclass
)
SELECT
    now() AS sampled_at,
    pg_current_wal_lsn() AS wal_lsn,
    w.wal_bytes AS wal_bytes_since_stats_reset,
    w.stats_reset AS wal_stats_reset,
    s.n_live_tup,
    s.n_dead_tup,
    round(100.0 * s.n_dead_tup / nullif(s.n_live_tup + s.n_dead_tup, 0), 1) AS dead_pct,
    s.n_ins_since_vacuum,
    s.n_tup_ins,
    s.n_tup_del,
    s.autovacuum_count,
    s.last_autovacuum,
    s.autoanalyze_count,
    s.last_autoanalyze,
    t.reltuples::bigint AS reltuples,
    t.relpages,
    t.relallfrozen,
    -- least() ignores the NULL a server without the setting (before 18) returns.
    least(
        coalesce(t.threshold, current_setting('autovacuum_vacuum_threshold'))::float8
          + coalesce(t.scale_factor, current_setting('autovacuum_vacuum_scale_factor'))::float8 * t.reltuples,
        current_setting('autovacuum_vacuum_max_threshold', true)::float8
    )::bigint AS vacuum_trigger_dead,
    (coalesce(t.insert_threshold, current_setting('autovacuum_vacuum_insert_threshold'))::float8
      + coalesce(t.insert_scale_factor, current_setting('autovacuum_vacuum_insert_scale_factor'))::float8
        * t.reltuples
        * CASE WHEN t.relpages > 0 AND t.relallfrozen > 0
               THEN 1 - least(t.relallfrozen, t.relpages)::float8 / t.relpages
               ELSE 1 END)::bigint AS vacuum_trigger_inserts,
    pg_relation_size(t.oid) AS heap_bytes,
    CASE WHEN t.reltoastrelid = 0 THEN 0 ELSE pg_total_relation_size(t.reltoastrelid) END AS toast_bytes,
    pg_indexes_size(t.oid) AS index_bytes,
    pg_total_relation_size(t.oid) AS total_bytes,
    ts.n_dead_tup AS toast_n_dead_tup,
    ts.autovacuum_count AS toast_autovacuum_count,
    t.reloptions
FROM t
JOIN pg_stat_user_tables s ON s.relid = t.oid
LEFT JOIN pg_stat_all_tables ts ON ts.relid = t.reltoastrelid
CROSS JOIN pg_stat_wal w;
