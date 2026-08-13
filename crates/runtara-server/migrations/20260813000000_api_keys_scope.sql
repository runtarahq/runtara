-- Per-key permission scope for `rt_*` API keys.
--
-- A key acts as its issuing user and inherits that user's live role on every request. `scope`
-- is an additional filter applied on top of that role: it can only narrow what the key may do,
-- never widen it.
--
-- The column holds a scope *name* (`read_only`), not a permission list, so a future scope is a
-- new value rather than a new migration. NULL means "no narrowing" — the inherit-everything
-- behavior every key had before this column existed, which is what every existing row keeps.

ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS scope TEXT;

COMMENT ON COLUMN api_keys.scope IS
    'Optional narrowing of the key''s inherited role (e.g. read_only). NULL = no narrowing.';
