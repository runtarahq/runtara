-- Per-workflow memory as a number of MiB; the S/M/L/XL tiers become editor
-- presets. Existing versions keep the memory their tier gave them.
ALTER TABLE workflow_definitions ADD COLUMN IF NOT EXISTS memory_mb INTEGER NOT NULL DEFAULT 256;
UPDATE workflow_definitions SET memory_mb = CASE upper(memory_tier)
    WHEN 'S' THEN 8
    WHEN 'M' THEN 64
    WHEN 'L' THEN 128
    ELSE 256
END;
