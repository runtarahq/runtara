-- Add input column to instances table for persisting instance inputs
-- This allows the UI to display what input was provided when starting an instance

ALTER TABLE instances ADD COLUMN input BLOB;
