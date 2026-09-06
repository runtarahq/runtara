-- Resume is a host launch operation. Retire persisted legacy commands without
-- changing instance state. Retain the old enum label for migration compatibility.
UPDATE pending_signals SET acknowledged_at = NOW()
WHERE signal_type = 'resume' AND acknowledged_at IS NULL;
