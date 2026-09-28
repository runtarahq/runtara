-- Validate the wake-reason CHECK added NOT VALID by migration 040. VALIDATE
-- takes only a SHARE UPDATE EXCLUSIVE lock, so runs keep writing meanwhile.
SET LOCAL statement_timeout = 0;

ALTER TABLE instances VALIDATE CONSTRAINT instances_wake_reason_valid;
