-- Approved history of host-executed built-in artifacts (the control agent).
--
-- A workflow may reach `runtara:control` only through control bytes whose
-- `runtara:builtin-artifacts/control-h<wasm>-h<meta>@0.1.0` pin is approved
-- here and not revoked. The server approves the control bytes of its
-- component bundles at boot; an operator revokes a pin by setting
-- `revoked_at`, which takes effect at the next boot. Rows are never deleted
-- and a revocation is never undone, so parked runs keep resolving against a
-- stable history. Removal path: drop the table and its functions once no
-- server reads it.
CREATE TABLE approved_builtin_artifacts (
    pin TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    wasm_sha256 TEXT NOT NULL CHECK (wasm_sha256 ~ '^[0-9a-f]{64}$'),
    metadata_sha256 TEXT NOT NULL CHECK (metadata_sha256 ~ '^[0-9a-f]{64}$'),
    approved_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    revoked_reason TEXT,
    CHECK (pin = 'runtara:builtin-artifacts/' || agent_id || '-h' || wasm_sha256
                 || '-h' || metadata_sha256 || '@0.1.0')
);

CREATE FUNCTION approved_builtin_artifacts_append_only() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'approved built-in artifacts are revoked, never deleted';
    END IF;
    IF NEW.pin IS DISTINCT FROM OLD.pin
        OR NEW.agent_id IS DISTINCT FROM OLD.agent_id
        OR NEW.wasm_sha256 IS DISTINCT FROM OLD.wasm_sha256
        OR NEW.metadata_sha256 IS DISTINCT FROM OLD.metadata_sha256
        OR NEW.approved_at IS DISTINCT FROM OLD.approved_at
        OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS DISTINCT FROM OLD.revoked_at)
    THEN
        RAISE EXCEPTION 'an approved built-in artifact can only be revoked, once';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER approved_builtin_artifacts_append_only
    BEFORE UPDATE OR DELETE ON approved_builtin_artifacts
    FOR EACH ROW EXECUTE FUNCTION approved_builtin_artifacts_append_only();
