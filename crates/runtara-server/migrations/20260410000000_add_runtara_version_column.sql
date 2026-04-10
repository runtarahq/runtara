-- Add runtara_version column to scenario_compilations if it doesn't exist.
-- The column was present in the schema definition but missing from databases
-- created before it was added.
ALTER TABLE scenario_compilations ADD COLUMN IF NOT EXISTS runtara_version VARCHAR(255);
