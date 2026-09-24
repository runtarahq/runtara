WITH claimed AS (
    SELECT id FROM usage_pending ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED
), facts AS (
    DELETE FROM usage_pending p USING claimed c WHERE p.id = c.id RETURNING p.*
), retained AS (
    SELECT * FROM facts WHERE finished_at >= $2
), aggregated AS (
    INSERT INTO usage_minutes AS u
        (tenant_id, bucket_time, invocation_count, success_count, failure_count, cancelled_count,
         duration_count, duration_sum_ms, duration_min_ms, duration_max_ms,
         memory_count, memory_sum_bytes, memory_max_bytes, cpu_count, cpu_sum_usec, cpu_max_usec)
    SELECT tenant_id, to_timestamp(floor(extract(epoch FROM finished_at) / 60) * 60),
           count(*) FILTER (WHERE completion),
           count(*) FILTER (WHERE completion AND status = 'completed'),
           count(*) FILTER (WHERE completion AND status = 'failed'),
           count(*) FILTER (WHERE completion AND status = 'cancelled'),
           count(duration_ms), coalesce(sum(duration_ms), 0), min(duration_ms), max(duration_ms),
           count(memory_bytes), coalesce(sum(memory_bytes), 0), max(memory_bytes),
           count(cpu_usec), coalesce(sum(cpu_usec), 0), max(cpu_usec)
    FROM retained GROUP BY 1, 2 ORDER BY 1, 2
    ON CONFLICT (tenant_id, bucket_time) DO UPDATE SET
        invocation_count = u.invocation_count + excluded.invocation_count,
        success_count = u.success_count + excluded.success_count,
        failure_count = u.failure_count + excluded.failure_count,
        cancelled_count = u.cancelled_count + excluded.cancelled_count,
        duration_count = u.duration_count + excluded.duration_count,
        duration_sum_ms = u.duration_sum_ms + excluded.duration_sum_ms,
        duration_min_ms = least(u.duration_min_ms, excluded.duration_min_ms),
        duration_max_ms = greatest(u.duration_max_ms, excluded.duration_max_ms),
        memory_count = u.memory_count + excluded.memory_count,
        memory_sum_bytes = u.memory_sum_bytes + excluded.memory_sum_bytes,
        memory_max_bytes = greatest(u.memory_max_bytes, excluded.memory_max_bytes),
        cpu_count = u.cpu_count + excluded.cpu_count,
        cpu_sum_usec = u.cpu_sum_usec + excluded.cpu_sum_usec,
        cpu_max_usec = greatest(u.cpu_max_usec, excluded.cpu_max_usec)
)
