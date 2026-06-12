-- Resource ownership: record the creating user (Auth0 sub) so the role map's `own`-scoped
-- permissions (Member update/delete on own resources) can be enforced for workflows and
-- triggers. `report_definitions` and `api_keys` already carry `created_by`. database and
-- connection have no enforceable per-row owner and are intentionally excluded — their
-- update/delete are flat Allow, never `own`.
--
-- Nullable: existing rows predate ownership tracking and stay NULL. A NULL owner is treated as
-- unowned — only Owner/Admin (who bypass the ownership check) can manage those rows until they
-- are recreated. ADD COLUMN of a nullable column with no default is metadata-only (no rewrite).

ALTER TABLE workflows ADD COLUMN IF NOT EXISTS created_by TEXT;

ALTER TABLE invocation_trigger ADD COLUMN IF NOT EXISTS created_by TEXT;
