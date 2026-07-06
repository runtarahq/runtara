-- Optimistic-concurrency guard for OAuth refresh-token rotation.
--
-- Rotating providers (QuickBooks/Intuit, Microsoft, Xero) issue a NEW refresh
-- token on every refresh and invalidate the previous one. When the rotated
-- token is persisted back, a stale write from a concurrent process (e.g. during
-- a rolling deploy that transiently runs two processes for one tenant) could
-- clobber the winner and orphan a live token. This column stores a hash — NOT
-- the token itself — of the current refresh token so the persist can be
-- conditional: it only lands when the stored hash is still NULL (never rotated /
-- legacy row) or equals the hash of the token we refreshed from.
ALTER TABLE connection_data_entity
    ADD COLUMN IF NOT EXISTS refresh_token_hash TEXT;
