-- Every API key must belong to a user.
--
-- An `rt_*` key acts as its issuing user — it inherits that user's *current* role from
-- the tenant Valkey (`member:{issuing_user_id}`) on every request, and a user may read/revoke
-- only the keys they issued. That contract only holds if every row has an owner, so
-- `issuing_user_id` becomes NOT NULL and the legacy "owner-less key bypasses authorization" path
-- is removed in code.
--
-- Production keys are backfilled to real Auth0 `sub`s manually before this migration runs. The
-- backfill below is a safety net so the NOT NULL constraint can always apply: pre-contract rows
-- fall back to `created_by` (their historical creator), and any row with neither gets a sentinel
-- that fails closed at validation (no `member:{sub}` entry → denied under enforcement).

UPDATE api_keys
SET issuing_user_id = COALESCE(created_by, 'orphaned:' || id::text)
WHERE issuing_user_id IS NULL;

ALTER TABLE api_keys ALTER COLUMN issuing_user_id SET NOT NULL;
