-- Durable launch queue.
--
-- A Core instance represents the durable workflow.  A row here represents one
-- physical attempt to hand that workflow to a runner.  Keeping the attempt in
-- its own table lets a full runner leave work durably queued without pinning a
-- request, trigger, or wake worker on an in-memory semaphore waiter.

-- Queue expiry is a distinct terminal cause, not a generic old-pending sweep.
-- The value is added here because Environment owns the queue while Core owns
-- the shared instance lifecycle enum.
ALTER TYPE termination_reason ADD VALUE IF NOT EXISTS 'launch_queue_timeout';

CREATE TABLE instance_launches (
    launch_id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL,
    image_id TEXT NOT NULL REFERENCES images(image_id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('start', 'resume', 'wake')),
    state TEXT NOT NULL CHECK (state IN (
        'queued', 'leased', 'starting', 'running',
        'suspended', 'completed', 'failed', 'cancelled'
    )),
    available_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deadline_at TIMESTAMPTZ NOT NULL,
    lease_owner TEXT,
    lease_expires_at TIMESTAMPTZ,
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT instance_launch_id_not_empty CHECK (length(launch_id) > 0)
);

-- One durable instance can have only one live launch generation.  Once a
-- generation parks or reaches a terminal state, a resume/wake may insert the
-- next one.  This is the database fence that makes manual resume and wake
-- races idempotent instead of two runner starts.
CREATE UNIQUE INDEX idx_instance_launches_one_active_per_instance
    ON instance_launches(instance_id)
    WHERE state IN ('queued', 'leased', 'starting', 'running');

-- The dispatcher claims a small ready batch using FOR UPDATE SKIP LOCKED.
CREATE INDEX idx_instance_launches_ready
    ON instance_launches(available_at, created_at)
    WHERE state = 'queued';

-- A dead dispatcher leaves only a lease to recover, never a task waiting in
-- process memory.  This index keeps its recovery scan bounded.
CREATE INDEX idx_instance_launches_expired_leases
    ON instance_launches(lease_expires_at)
    WHERE state = 'leased' AND lease_expires_at IS NOT NULL;

-- Queue expiry is a policy deadline, independent of runner execution timeout.
CREATE INDEX idx_instance_launches_deadlines
    ON instance_launches(deadline_at)
    WHERE state IN ('queued', 'leased');
