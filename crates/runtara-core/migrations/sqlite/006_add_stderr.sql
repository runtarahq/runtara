-- Add stderr column to instances table for raw container stderr output.
-- This is separate from `error` to allow product to decide what to display to users.

ALTER TABLE instances ADD COLUMN stderr TEXT;
