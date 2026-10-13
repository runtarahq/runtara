-- Where each checkpoint sits in its run: an address derived from a structured
-- (version 2) checkpoint key when the row is written, NULL for any other key.
-- Every row inside one composite step (a Split's iterations, an embedded
-- child's steps) shares a byte prefix of the address, so recording the step's
-- result can delete the rows inside it as one index range scan.
--
-- The "C" collation compares bytes, which is what a prefix range needs.
-- Rows written before this migration keep NULL and are never collapsed.
--
-- A plain transactional CREATE INDEX (SHARE lock: checkpoint writes wait
-- while it builds). It indexes no existing row, but still reads the table
-- once. sqlx runs each migration in a transaction, so CONCURRENTLY is not
-- available here.
SET LOCAL statement_timeout = 0;

ALTER TABLE checkpoints ADD COLUMN scope TEXT COLLATE "C";

CREATE INDEX idx_checkpoints_scope
    ON checkpoints (instance_id, scope)
    WHERE scope IS NOT NULL;
