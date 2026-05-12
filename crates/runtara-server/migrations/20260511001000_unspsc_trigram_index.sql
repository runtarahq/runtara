-- Operational index for UNSPSC title retrieval. The table is tenant data and
-- may not exist in every installation, so keep this migration conditional.

DO $$
BEGIN
    IF to_regclass('unspsc_node') IS NOT NULL
       AND EXISTS (
           SELECT 1 FROM information_schema.columns
           WHERE table_name = 'unspsc_node' AND column_name = 'commodity_title'
       )
       AND EXISTS (
           SELECT 1 FROM information_schema.columns
           WHERE table_name = 'unspsc_node' AND column_name = 'deleted'
       ) THEN
        EXECUTE '
            CREATE INDEX IF NOT EXISTS idx_unspsc_node_commodity_title_trgm
            ON unspsc_node USING gin (commodity_title gin_trgm_ops)
            WHERE deleted = false
        ';
    END IF;
END $$;
