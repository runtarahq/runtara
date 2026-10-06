-- An execution that parks its own root records it on its lease row, in the
-- park's transaction. Leaving `running` revokes the lease either way; this is
-- what lets a retried park from the same lease tell its own committed park
-- (already applied) from a pause, cancel or replacement (superseded).
-- Every claim of a new epoch resets it.
ALTER TABLE invocation_root_leases
    ADD COLUMN parked BOOLEAN NOT NULL DEFAULT FALSE;
