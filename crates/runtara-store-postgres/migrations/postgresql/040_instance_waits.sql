-- Durable instance waits: a run (the waiter) waits for a fixed set of other
-- runs of its tenant (the targets) to finish, in mode `all` or `any`, with an
-- optional deadline. A wait is keyed by the waiter and its operation
-- (`wait_id` = the calling step's op_hash), so a replay of the step finds the
-- same wait. The host resolves a wait once, persisting the resolution and the
-- targets that had finished by then, so a replayed `any` never picks again.
--
-- Wakes: a target finishing stamps the parked waiter's wake in the same
-- commit when it can take the waiter's row without waiting, and otherwise
-- leaves a `wake_pending` nudge on the target row that the wake scheduler's
-- reconciler picks up. The park itself re-evaluates its waits, and the
-- reconciler re-reads parked waits from the rows every twelfth poll, so no
-- wake depends on the trigger alone.
SET LOCAL statement_timeout = 0;

-- Wake reasons are host metadata. Migration 024 declared them with an
-- unnamed column CHECK; replace it with a named one that also admits
-- `instances_terminal`. Added NOT VALID so this migration does not scan the
-- table under its lock; 041 validates it.
ALTER TABLE instances ADD CONSTRAINT instances_wake_reason_valid CHECK (
    wake_reason IS NULL
    OR wake_reason IN ('timer', 'custom_signal', 'manual_resume', 'recovery', 'instances_terminal')
) NOT VALID;

DO $$
DECLARE
    old_check RECORD;
BEGIN
    FOR old_check IN
        SELECT conname FROM pg_constraint
        WHERE conrelid = 'instances'::regclass
          AND contype = 'c'
          AND conname <> 'instances_wake_reason_valid'
          AND pg_get_constraintdef(oid) LIKE '%wake_reason%'
    LOOP
        EXECUTE format('ALTER TABLE instances DROP CONSTRAINT %I', old_check.conname);
    END LOOP;
END;
$$;

CREATE TABLE instance_waits (
    waiter_instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    wait_id TEXT NOT NULL CHECK (octet_length(wait_id) BETWEEN 1 AND 128),
    -- A fresh id per registration: target rows of a closed wait that was
    -- registered again never collide with the new ones.
    generation UUID NOT NULL DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('all', 'any')),
    -- Sorted, distinct target ids.
    targets TEXT[] NOT NULL CHECK (cardinality(targets) <= 1000),
    -- `v1:` sha256 of the mode and targets; the deadline is not part of it.
    fingerprint TEXT NOT NULL,
    -- Millisecond precision. The first registration's deadline stands.
    deadline TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    state TEXT NOT NULL CHECK (state IN ('pending', 'resolved', 'closed')),
    resolution TEXT CHECK (resolution IN ('satisfied', 'deadline', 'empty')),
    -- The targets that had finished when the wait resolved, in finish order.
    finished TEXT[],
    resolved_at TIMESTAMPTZ,
    closed_at TIMESTAMPTZ,
    -- Rotation cursor of the reconciler's full pass.
    last_reconciled_at TIMESTAMPTZ,
    PRIMARY KEY (waiter_instance_id, wait_id),
    CHECK ((resolution IS NULL) = (resolved_at IS NULL)),
    CHECK ((resolution IS NULL) = (finished IS NULL)),
    CHECK (state <> 'resolved' OR resolution IS NOT NULL),
    CHECK (state <> 'pending' OR resolution IS NULL),
    CHECK ((state = 'closed') = (closed_at IS NOT NULL))
);

-- The reconciler's full pass: pending waits, least recently reconciled first.
CREATE INDEX instance_waits_reconcile
    ON instance_waits (last_reconciled_at NULLS FIRST, waiter_instance_id, wait_id)
    WHERE state = 'pending';

-- Who waits on a target: one row per target of a pending wait, removed when
-- the wait resolves or closes, so a finishing run only ever finds live waits.
-- There is deliberately no foreign key: removing a wait never has to wait for
-- a row a finishing run is nudging. Rows left behind are pruned by the
-- reconciler.
CREATE TABLE instance_wait_targets (
    waiter_instance_id TEXT NOT NULL,
    wait_id TEXT NOT NULL,
    generation UUID NOT NULL,
    target_instance_id TEXT NOT NULL,
    wake_pending BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (waiter_instance_id, wait_id, generation, target_instance_id)
);
CREATE INDEX instance_wait_targets_target ON instance_wait_targets (target_instance_id);
CREATE INDEX instance_wait_targets_wake_pending
    ON instance_wait_targets (waiter_instance_id) WHERE wake_pending;

-- The waits a parked run can be woken by, beside its signal ids.
ALTER TABLE instance_input_parks ADD COLUMN wait_ids TEXT[] NOT NULL DEFAULT '{}';

-- Called for each run that just finished (or never-launched child whose
-- outcome was just published). For every pending wait on it: stamp the
-- waiter's wake when the wait's condition now holds and the waiter is parked
-- on it, taking the waiter's row only if nobody holds it (SKIP LOCKED: a
-- finishing run never waits on a waiter); otherwise leave a nudge for the
-- reconciler. The stamp is what the park and the reconciler would do; the
-- rule that resolves the wait stays in the host.
CREATE FUNCTION wake_instance_waiters() RETURNS trigger AS $$
DECLARE
    waiting RECORD;
    stamped BOOLEAN;
BEGIN
    FOR waiting IN
        SELECT t.waiter_instance_id, t.wait_id, t.generation, w.mode
        FROM instance_wait_targets AS t
        JOIN instance_waits AS w
          ON w.waiter_instance_id = t.waiter_instance_id
         AND w.wait_id = t.wait_id
         AND w.generation = t.generation
         AND w.state = 'pending'
        WHERE t.target_instance_id = NEW.instance_id
        ORDER BY t.waiter_instance_id, t.wait_id
    LOOP
        stamped := FALSE;
        -- The wait's condition, by the host's rule: a target finished when
        -- its instance row is terminal, or, with no row, when its outcome is
        -- published (the row wins).
        -- Parenthesized: PL/pgSQL ends an IF condition at its first
        -- top-level THEN.
        IF (CASE waiting.mode
            WHEN 'any' THEN EXISTS (
                SELECT 1
                FROM instance_wait_targets AS o
                LEFT JOIN instances AS i ON i.instance_id = o.target_instance_id
                WHERE o.waiter_instance_id = waiting.waiter_instance_id
                  AND o.wait_id = waiting.wait_id
                  AND o.generation = waiting.generation
                  AND COALESCE(
                      i.status IN ('completed', 'failed', 'cancelled'),
                      EXISTS (SELECT 1 FROM instance_external_outcomes AS e
                              WHERE e.instance_id = o.target_instance_id)))
            ELSE NOT EXISTS (
                SELECT 1
                FROM instance_wait_targets AS o
                LEFT JOIN instances AS i ON i.instance_id = o.target_instance_id
                WHERE o.waiter_instance_id = waiting.waiter_instance_id
                  AND o.wait_id = waiting.wait_id
                  AND o.generation = waiting.generation
                  AND NOT COALESCE(
                      i.status IN ('completed', 'failed', 'cancelled'),
                      EXISTS (SELECT 1 FROM instance_external_outcomes AS e
                              WHERE e.instance_id = o.target_instance_id)))
            END)
        THEN
            PERFORM 1 FROM instances
            WHERE instance_id = waiting.waiter_instance_id
              AND status = 'suspended'
              AND termination_reason = 'waiting_instances'
            FOR UPDATE SKIP LOCKED;
            IF FOUND THEN
                UPDATE instance_input_parks SET wake_scheduled = TRUE
                WHERE instance_id = waiting.waiter_instance_id
                  AND NOT wake_scheduled
                  AND waiting.wait_id = ANY (wait_ids);
                IF FOUND THEN
                    UPDATE instances
                    SET sleep_until = LEAST(sleep_until, clock_timestamp()),
                        wake_reason = 'instances_terminal'
                    WHERE instance_id = waiting.waiter_instance_id;
                    stamped := TRUE;
                END IF;
            END IF;
        END IF;
        IF NOT stamped THEN
            UPDATE instance_wait_targets SET wake_pending = TRUE
            WHERE waiter_instance_id = waiting.waiter_instance_id
              AND wait_id = waiting.wait_id
              AND generation = waiting.generation
              AND target_instance_id = NEW.instance_id
              AND NOT wake_pending;
        END IF;
    END LOOP;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- A run finishes: every writer of `instances.status`, raw SQL included.
CREATE TRIGGER instance_waits_wake_on_finish
    AFTER UPDATE OF status ON instances
    FOR EACH ROW
    WHEN (NEW.status IN ('completed', 'failed', 'cancelled')
          AND OLD.status NOT IN ('completed', 'failed', 'cancelled'))
    EXECUTE FUNCTION wake_instance_waiters();

-- A run written already finished.
CREATE TRIGGER instance_waits_wake_on_insert
    AFTER INSERT ON instances
    FOR EACH ROW
    WHEN (NEW.status IN ('completed', 'failed', 'cancelled'))
    EXECUTE FUNCTION wake_instance_waiters();

-- A child that never launched gets its fenced outcome.
CREATE TRIGGER instance_waits_wake_on_outcome
    AFTER INSERT ON instance_external_outcomes
    FOR EACH ROW
    EXECUTE FUNCTION wake_instance_waiters();
