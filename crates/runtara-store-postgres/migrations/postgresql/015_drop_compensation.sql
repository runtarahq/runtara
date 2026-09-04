-- Drop the compensation (saga) framework schema added by 007_compensation.sql.
--
-- Compensation was accepted by the DSL but never executed: the compiler never
-- emitted it, the SDK always wrote `compensation_step_id: None`, and the host
-- CompensationManager had no call sites. Nothing ever called
-- register_compensatable_checkpoint, so every column dropped below holds its
-- default and compensation_log is empty — this is not a lossy migration.
--
-- 007_compensation.sql is deliberately left in place: it has already been
-- applied everywhere, and sqlx::migrate! validates applied migrations by
-- checksum at startup. Removing or editing it would break boot.

-- Audit table (its indexes drop with it).
DROP TABLE IF EXISTS compensation_log;

-- Partial index over the checkpoint columns; must go before they do.
DROP INDEX IF EXISTS idx_checkpoints_compensatable;

ALTER TABLE checkpoints
    DROP COLUMN IF EXISTS is_compensatable,
    DROP COLUMN IF EXISTS compensation_step_id,
    DROP COLUMN IF EXISTS compensation_data,
    DROP COLUMN IF EXISTS compensation_state,
    DROP COLUMN IF EXISTS compensation_order,
    DROP COLUMN IF EXISTS compensated_at;

ALTER TABLE instances
    DROP COLUMN IF EXISTS compensation_state,
    DROP COLUMN IF EXISTS compensation_triggered_at,
    DROP COLUMN IF EXISTS compensation_reason;

-- Enum is only droppable once every column typed by it is gone.
DROP TYPE IF EXISTS compensation_state;
