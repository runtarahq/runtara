-- Recovery polls pending cancellations independently of sleep deadlines.
-- Keep that scan bounded by pending work rather than retained command history.
CREATE INDEX idx_pending_cancellations ON pending_signals (instance_id)
    WHERE signal_type = 'cancel' AND acknowledged_at IS NULL;
