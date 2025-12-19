-- Remove definition_id from instances; core no longer tracks image/workflow identifiers.

-- Drop index that referenced definition_id.
DROP INDEX IF EXISTS idx_instances_definition;

-- Drop the column itself.
ALTER TABLE instances
    DROP COLUMN IF EXISTS definition_id;

-- Ensure definition_version has a default to keep inserts simple.
ALTER TABLE instances
    ALTER COLUMN definition_version SET DEFAULT 1;
