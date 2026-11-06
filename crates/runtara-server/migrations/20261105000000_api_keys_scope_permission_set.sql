-- Granular API-key scopes (SYN-939).
--
-- The `api_keys.scope` column (added in 20260813000000_api_keys_scope) already holds an optional
-- narrowing of the key's inherited role. It previously held only a preset *name* (`read_only`).
-- It now also holds an explicit permission set: a JSON array of permission wire-names
-- (e.g. '["workflow:read","workflow:execute"]'), read back as `ApiKeyScope::Restricted`.
--
-- The column is already TEXT and already nullable, so no structural change is needed — a JSON
-- array fits, and NULL still means "no narrowing" (every pre-scope key keeps its behavior). This
-- migration only refreshes the column comment so the schema documents the new accepted forms.
--
-- Parsing stays fail-closed: an array naming a permission this build does not know is read as
-- `Unrecognized` (denies everything), never widened to full — so a rollback cannot broaden a key
-- that was created narrow.

COMMENT ON COLUMN api_keys.scope IS
    'Optional narrowing of the key''s inherited role. NULL = no narrowing; ''read_only'' = every '
    'read permission; a JSON array of permission wire-names = an explicit least-privilege set '
    '(SYN-939). Unknown values are treated as deny-all, never widened.';
