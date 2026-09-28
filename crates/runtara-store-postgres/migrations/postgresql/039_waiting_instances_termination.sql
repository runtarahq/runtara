-- A run parked on durable instance waits (control `wait`) carries this
-- termination reason while suspended. It is the discriminator the instance-
-- wait waker requires before stamping a wake, as `waiting_signal` is for
-- signal waits; a paused run has none and is never stamped.
--
-- Alone in its migration: a new enum value cannot be used in the transaction
-- that adds it, and 040 refers to it.
ALTER TYPE termination_reason ADD VALUE IF NOT EXISTS 'waiting_instances';
