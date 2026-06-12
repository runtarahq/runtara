-- Bridge runtara-local (`rt_*`) API keys onto the user-management contract.
--
-- `jti` is the token identity used as the revocation-denylist key (`token:revoked:{jti}`
-- in the tenant Valkey). `issuing_user_id` is the Auth0 `sub` of the user who created the
-- key; at validation time runtara looks up `member:{issuing_user_id}` so the key inherits
-- that user's *current* role (demote/remove the user → the key follows on the next request).
--
-- Both columns are nullable: legacy rows predate this contract and keep their existing
-- behavior until rotated/expired. See docs/security/user-management-contracts.md.

ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS jti TEXT;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS issuing_user_id TEXT;

-- `jti` is unique when present; legacy NULLs are exempt via the partial index.
CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_jti ON api_keys(jti) WHERE jti IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_api_keys_issuing_user_id ON api_keys(issuing_user_id);
