-- Generalize the lease's `parked` flag into the transition its own execution
-- committed: a park, or the status its owned completion wrote. A retried
-- transition from the same lease is then recognised as already applied, for
-- completions as well as parks. Leaving `running` any other way leaves it NULL,
-- and every claim of a new epoch resets it.
ALTER TABLE invocation_root_leases
    ADD COLUMN released_by TEXT
        CHECK (released_by IS NULL
               OR released_by IN ('park', 'completed', 'failed', 'suspended', 'cancelled'));
UPDATE invocation_root_leases SET released_by = 'park' WHERE parked;
ALTER TABLE invocation_root_leases DROP COLUMN parked;
