-- Widen start labels to 1024 printable ASCII bytes. The instances table is
-- small, so this stays one transaction and must not be cut off by a
-- session-level statement timeout.
SET LOCAL statement_timeout = 0;

-- Dropping the CHECK first keeps the type change from rechecking every row;
-- migration 033 validates the replacement in its own transaction.
ALTER TABLE instances DROP CONSTRAINT valid_run_label;

-- No query searches labels through the trigram index, so it is not rebuilt.
DROP INDEX IF EXISTS idx_instances_run_label_search;

ALTER TABLE instances ALTER COLUMN run_label TYPE VARCHAR(1024);

ALTER TABLE instances ADD CONSTRAINT valid_run_label CHECK (
    run_label IS NULL OR (
        octet_length(run_label) BETWEEN 1 AND 1024
        AND run_label COLLATE "C" ~ '^[ -~]+$'
        AND run_label COLLATE "C" ~ '[!-~]'
    )
) NOT VALID;
