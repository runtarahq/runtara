-- Validate the parent-link CHECKs added NOT VALID by migration 035. VALIDATE
-- takes only a SHARE UPDATE EXCLUSIVE lock, so runs keep writing meanwhile.
SET LOCAL statement_timeout = 0;

ALTER TABLE instances VALIDATE CONSTRAINT instances_parent_close_policy_check;
ALTER TABLE instances VALIDATE CONSTRAINT instances_parent_link_check;
