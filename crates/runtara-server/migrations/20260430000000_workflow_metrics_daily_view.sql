-- workflow_metrics_daily: daily roll-up of workflow_metrics_hourly.
-- Read-only view; no separate population job. The hourly composite index
-- (tenant_id, workflow_id, version, hour_bucket DESC) backs the GROUP BY.

CREATE OR REPLACE VIEW workflow_metrics_daily AS
SELECT
    tenant_id,
    workflow_id,
    version,
    date_trunc('day', hour_bucket) AS day_bucket,
    SUM(invocation_count)::bigint AS invocation_count,
    SUM(success_count)::bigint     AS success_count,
    SUM(failure_count)::bigint     AS failure_count,
    SUM(timeout_count)::bigint     AS timeout_count,
    SUM(total_duration_seconds) / NULLIF(SUM(invocation_count), 0)
        AS avg_duration_seconds,
    MIN(min_duration_seconds) AS min_duration_seconds,
    MAX(max_duration_seconds) AS max_duration_seconds,
    SUM(total_memory_mb) / NULLIF(SUM(invocation_count), 0)
        AS avg_memory_mb,
    MIN(min_memory_mb) AS min_memory_mb,
    MAX(max_memory_mb) AS max_memory_mb,
    SUM(total_queue_duration_seconds) / NULLIF(SUM(invocation_count), 0)
        AS avg_queue_duration_seconds,
    MIN(min_queue_duration_seconds) AS min_queue_duration_seconds,
    MAX(max_queue_duration_seconds) AS max_queue_duration_seconds,
    SUM(total_processing_overhead_seconds) / NULLIF(SUM(invocation_count), 0)
        AS avg_processing_overhead_seconds,
    MIN(min_processing_overhead_seconds) AS min_processing_overhead_seconds,
    MAX(max_processing_overhead_seconds) AS max_processing_overhead_seconds,
    (SUM(success_count)::numeric / NULLIF(SUM(invocation_count), 0)) * 100
        AS success_rate_percent
FROM workflow_metrics_hourly
GROUP BY tenant_id, workflow_id, version, date_trunc('day', hour_bucket);
