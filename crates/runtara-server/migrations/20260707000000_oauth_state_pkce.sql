-- PKCE (RFC 7636) support: the authorize step stores the generated code_verifier
-- on the state row so the callback can send it in the token exchange. NULL for
-- providers/flows that don't use PKCE.
ALTER TABLE oauth_state
    ADD COLUMN IF NOT EXISTS code_verifier TEXT;
