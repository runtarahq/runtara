-- Migration: Change instance_id from UUID to TEXT
-- This allows arbitrary string identifiers for instances (not just UUIDs)

-- 1. Drop foreign key constraints that reference instances(instance_id)
ALTER TABLE checkpoints DROP CONSTRAINT checkpoints_instance_id_fkey;
ALTER TABLE wake_queue DROP CONSTRAINT wake_queue_instance_id_fkey;
ALTER TABLE instance_events DROP CONSTRAINT instance_events_instance_id_fkey;
ALTER TABLE pending_signals DROP CONSTRAINT pending_signals_instance_id_fkey;
ALTER TABLE containers DROP CONSTRAINT containers_instance_id_fkey;

-- 2. Change instance_id column type in instances table
ALTER TABLE instances ALTER COLUMN instance_id TYPE TEXT USING instance_id::TEXT;

-- 3. Change instance_id column type in related tables
ALTER TABLE checkpoints ALTER COLUMN instance_id TYPE TEXT USING instance_id::TEXT;
ALTER TABLE wake_queue ALTER COLUMN instance_id TYPE TEXT USING instance_id::TEXT;
ALTER TABLE instance_events ALTER COLUMN instance_id TYPE TEXT USING instance_id::TEXT;
ALTER TABLE pending_signals ALTER COLUMN instance_id TYPE TEXT USING instance_id::TEXT;
ALTER TABLE containers ALTER COLUMN instance_id TYPE TEXT USING instance_id::TEXT;

-- 4. Re-add foreign key constraints
ALTER TABLE checkpoints ADD CONSTRAINT checkpoints_instance_id_fkey
    FOREIGN KEY (instance_id) REFERENCES instances(instance_id) ON DELETE CASCADE;
ALTER TABLE wake_queue ADD CONSTRAINT wake_queue_instance_id_fkey
    FOREIGN KEY (instance_id) REFERENCES instances(instance_id) ON DELETE CASCADE;
ALTER TABLE instance_events ADD CONSTRAINT instance_events_instance_id_fkey
    FOREIGN KEY (instance_id) REFERENCES instances(instance_id) ON DELETE CASCADE;
ALTER TABLE pending_signals ADD CONSTRAINT pending_signals_instance_id_fkey
    FOREIGN KEY (instance_id) REFERENCES instances(instance_id) ON DELETE CASCADE;
ALTER TABLE containers ADD CONSTRAINT containers_instance_id_fkey
    FOREIGN KEY (instance_id) REFERENCES instances(instance_id) ON DELETE CASCADE;

-- 5. Add NOT NULL and length constraints
ALTER TABLE instances ADD CONSTRAINT instance_id_not_empty CHECK (length(instance_id) > 0);
