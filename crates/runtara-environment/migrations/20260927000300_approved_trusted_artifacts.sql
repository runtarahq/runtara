-- Record trusted built-ins (S3, Azure) in the approved history as well.
--
-- The server approves the installed `runtara:trusted-artifacts/<agent>-h<wasm>
-- -h<meta>@0.1.0` pins at boot, next to the control pins. An earlier trusted
-- pin that is approved and not revoked lets a parked run pinned to it keep
-- calling its agent on wake or resume (the installed bytes run); a start
-- never uses the history. Rows keep the append-only, revoke-once rules.
-- Removal path: restore the builtin-only check once no trusted rows exist.
ALTER TABLE approved_builtin_artifacts
    DROP CONSTRAINT approved_builtin_artifacts_check,
    ADD CONSTRAINT approved_builtin_artifacts_pin_check CHECK (
        pin IN (
            'runtara:builtin-artifacts/' || agent_id || '-h' || wasm_sha256
                || '-h' || metadata_sha256 || '@0.1.0',
            'runtara:trusted-artifacts/' || agent_id || '-h' || wasm_sha256
                || '-h' || metadata_sha256 || '@0.1.0'
        )
    );
