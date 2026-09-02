-- Track the full compiler provenance that produced a workflow artifact.
--
-- `track_events` changes generated guest code, so a compilation row cannot be
-- considered ready solely because its workflow definition checksum matches.
-- The compiler template major and direct-lowering mode also alter generated
-- code without changing that definition.  Cache readers must reject artifacts
-- from an unknown compiler configuration rather than continuing to serve them
-- after a rollout changes either setting.
-- Keep this nullable: rows written before this provenance existed must miss
-- the cache and recompile rather than claiming an unknown compiler mode.

ALTER TABLE workflow_compilations
    ADD COLUMN IF NOT EXISTS track_events BOOLEAN,
    ADD COLUMN IF NOT EXISTS template_major TEXT,
    ADD COLUMN IF NOT EXISTS lowering_mode TEXT;
