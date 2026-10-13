-- The index a scope collapse deletes through: an instance's rows by address.
-- Rows without an address are left out.
--
-- Built in its own migration so that the previous one's ACCESS EXCLUSIVE
-- lock is already released. A plain CREATE INDEX holds a SHARE lock while it
-- reads the whole table once: checkpoint writes wait, reads do not. It is not
-- built CONCURRENTLY, because a failed concurrent build leaves an invalid
-- index that a retried IF NOT EXISTS would then keep.
SET LOCAL statement_timeout = 0;

CREATE INDEX idx_checkpoints_scope
    ON checkpoints (instance_id, scope)
    WHERE scope IS NOT NULL;
