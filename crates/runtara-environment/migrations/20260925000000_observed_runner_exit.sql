-- Retain a physical exit until Core's lifecycle transition (including managed
-- input closure) commits. Restart recovery must not relaunch a known crash as
-- though only its Environment process had disappeared.
ALTER TABLE container_registry ADD COLUMN observed_exit JSONB;
