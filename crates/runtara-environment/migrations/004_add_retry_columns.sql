-- Add retry tracking columns to instances table
ALTER TABLE instances ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE instances ADD COLUMN max_retries INTEGER NOT NULL DEFAULT 3;
