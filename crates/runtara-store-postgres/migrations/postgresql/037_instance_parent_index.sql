-- Children of one parent, in admission order: control:query(parent), the
-- parent-aware relation checks and the public parentInstanceId filter.
--
-- A plain transactional CREATE INDEX (SHARE lock: writes to instances wait
-- while it builds). Only child rows are indexed, and the instances table is
-- expected to hold thousands to tens of thousands of rows, so the build is
-- short. sqlx runs each migration in a transaction, so CONCURRENTLY is not
-- available here.
SET LOCAL statement_timeout = 0;

CREATE INDEX idx_instances_parent_admitted
    ON instances (tenant_id, parent_instance_id, admitted_at, instance_id)
    WHERE parent_instance_id IS NOT NULL;
