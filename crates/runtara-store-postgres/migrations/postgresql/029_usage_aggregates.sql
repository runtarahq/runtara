-- Usage is product data: capture it in the lifecycle transaction, before raw
-- instances can expire. A bounded worker folds these facts into minute buckets.
-- No FK to instances: pending facts must survive instance cleanup.
CREATE TABLE usage_pending (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    finished_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('completed', 'failed', 'cancelled')),
    termination_reason TEXT,
    completion BOOLEAN NOT NULL,
    export BOOLEAN NOT NULL,
    duration_ms DOUBLE PRECISION,
    memory_bytes BIGINT,
    cpu_usec BIGINT
);

CREATE TABLE usage_minutes (
    tenant_id TEXT NOT NULL,
    bucket_time TIMESTAMPTZ NOT NULL,
    invocation_count BIGINT NOT NULL,
    success_count BIGINT NOT NULL,
    failure_count BIGINT NOT NULL,
    cancelled_count BIGINT NOT NULL,
    duration_count BIGINT NOT NULL,
    duration_sum_ms DOUBLE PRECISION NOT NULL,
    duration_min_ms DOUBLE PRECISION,
    duration_max_ms DOUBLE PRECISION,
    memory_count BIGINT NOT NULL,
    memory_sum_bytes NUMERIC NOT NULL,
    memory_max_bytes BIGINT,
    cpu_count BIGINT NOT NULL,
    cpu_sum_usec NUMERIC NOT NULL,
    cpu_max_usec BIGINT,
    PRIMARY KEY (tenant_id, bucket_time)
);
CREATE INDEX usage_minutes_retention ON usage_minutes (bucket_time);

-- First terminal outcome owns the invocation. Repeated completion writes and
-- late resource reports must not move it into a different time bucket or count
-- it again. These markers disappear with the raw instance, not its history.
ALTER TABLE instances
    ADD COLUMN usage_finished_at TIMESTAMPTZ,
    ADD COLUMN usage_status TEXT,
    ADD COLUMN usage_reason TEXT,
    ADD COLUMN usage_memory_recorded BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN usage_cpu_recorded BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX instances_usage_backfill ON instances (instance_id)
    WHERE usage_finished_at IS NULL AND finished_at IS NOT NULL
      AND status IN ('completed', 'failed', 'cancelled');

CREATE FUNCTION capture_instance_usage() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    first_completion BOOLEAN;
    live_completion BOOLEAN := FALSE;
    duration DOUBLE PRECISION;
    memory BIGINT;
    cpu BIGINT;
BEGIN
    -- Deletion captures any pre-upgrade row that backfill has not reached yet.
    IF TG_OP = 'DELETE' THEN
        NEW := OLD;
    END IF;
    IF NEW.usage_finished_at IS NULL AND
       (NEW.finished_at IS NULL OR NEW.status NOT IN ('completed', 'failed', 'cancelled')) THEN
        RETURN NEW;
    END IF;

    first_completion := NEW.usage_finished_at IS NULL;
    IF first_completion THEN
        NEW.usage_finished_at := NEW.finished_at;
        NEW.usage_status := NEW.status::TEXT;
        NEW.usage_reason := NEW.termination_reason::TEXT;
        IF NEW.started_at <= NEW.finished_at THEN
            duration := EXTRACT(EPOCH FROM (NEW.finished_at - NEW.started_at)) * 1000;
        END IF;
        IF TG_OP = 'INSERT' THEN
            live_completion := TRUE;
        ELSIF TG_OP = 'UPDATE' THEN
            -- Backfill never replays old invocations as current OTEL traffic.
            live_completion := OLD.status NOT IN ('completed', 'failed', 'cancelled')
                               OR OLD.finished_at IS NULL;
        END IF;
    END IF;

    IF NOT NEW.usage_memory_recorded AND NEW.memory_peak_bytes >= 0 THEN
        memory := NEW.memory_peak_bytes;
        NEW.usage_memory_recorded := TRUE;
    END IF;
    IF NOT NEW.usage_cpu_recorded AND NEW.cpu_usage_usec >= 0 THEN
        cpu := NEW.cpu_usage_usec;
        NEW.usage_cpu_recorded := TRUE;
    END IF;
    IF first_completion OR memory IS NOT NULL OR cpu IS NOT NULL THEN
        INSERT INTO usage_pending
            (tenant_id, finished_at, status, termination_reason, completion, export,
             duration_ms, memory_bytes, cpu_usec)
        VALUES
            (NEW.tenant_id, NEW.usage_finished_at, NEW.usage_status, NEW.usage_reason,
             first_completion, live_completion OR (NOT first_completion AND TG_OP = 'UPDATE'),
             duration, memory, cpu);
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER instance_usage
    BEFORE INSERT OR UPDATE OF status, finished_at, memory_peak_bytes, cpu_usage_usec OR DELETE
    ON instances FOR EACH ROW EXECUTE FUNCTION capture_instance_usage();
