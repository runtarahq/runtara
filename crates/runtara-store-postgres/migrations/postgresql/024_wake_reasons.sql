-- Wake metadata belongs to host scheduling, separate from guest commands.
ALTER TABLE instances ADD COLUMN wake_reason TEXT
    CHECK (wake_reason IN ('timer', 'custom_signal', 'manual_resume', 'recovery'));
UPDATE instances SET wake_reason = CASE
    WHEN termination_reason IN ('shutdown_requested', 'environment_restart') THEN 'recovery'
    ELSE 'timer' END
WHERE sleep_until IS NOT NULL;
