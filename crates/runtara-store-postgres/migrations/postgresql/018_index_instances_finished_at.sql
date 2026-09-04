-- Support tenant metrics aggregation, which filters instances by tenant and a
-- finished_at range. Nothing indexed finished_at, so that query's cost tracked
-- total execution history rather than the window asked for: measured on 2M
-- instances, "last hour" took 76ms - the same seq scan "last 90 days" pays for.
-- With this index that request is 0.9ms.
--
-- Partial on finished_at IS NOT NULL because pending, running and suspended
-- instances are never aggregated, and a row only enters the index when it
-- reaches a terminal state.
--
-- status is INCLUDEd rather than left to a heap lookup for one specific
-- reason: without it, the planner finds this index attractive enough at wide
-- windows to abandon a parallel sequential scan for idx_instances_status,
-- which is a worse plan - measured at 16% slower over 90 days. Carrying status
-- lets it choose correctly at both ends. It costs no extra write on the hot
-- path, since status is set by the same UPDATE that sets finished_at.
-- memory_peak_bytes is deliberately NOT included: it is written by a later,
-- separate UPDATE, so indexing it would add an index write per instance.

CREATE INDEX IF NOT EXISTS idx_instances_tenant_finished
    ON instances (tenant_id, finished_at)
    INCLUDE (status)
    WHERE finished_at IS NOT NULL;
