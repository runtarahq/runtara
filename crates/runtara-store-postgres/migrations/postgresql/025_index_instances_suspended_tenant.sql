-- The System page samples how many of a tenant's instances are parked, and
-- parked means `suspended` — the value that dominates this table. A plain index
-- on `status` is no help for a table's most common value, so the planner
-- reasonably preferred a sequential scan and the count cost O(table) per sample.
--
-- Modelled on idx_instances_pending_tenant_created (019), which does the same
-- for pending starts. This one carries no second column: its reader counts and
-- never asks for the oldest, so tenant_id alone answers the query index-only.
--
-- Measured over 500k instances, 350k of them suspended, counting the largest
-- tenant's 306k: a parallel sequential scan of 15,622 buffers before, an
-- index-only scan of 261 with no heap fetches after.
--
-- One thing to know when checking whether this is working. An index-only scan
-- is only cheap once the visibility map says a page is all-visible, which
-- VACUUM sets. On a freshly bulk-loaded table the planner correctly costs this
-- index as needing a heap fetch per row and keeps the sequential scan; the plan
-- flips after the first (auto)vacuum. An index that appears to do nothing
-- immediately after a large import has probably just not been vacuumed yet.
CREATE INDEX IF NOT EXISTS idx_instances_suspended_tenant
    ON instances (tenant_id)
    WHERE status = 'suspended';
