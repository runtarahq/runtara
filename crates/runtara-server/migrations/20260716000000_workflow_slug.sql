-- Workflow slug: the stable, human-editable capability id of a
-- workflow-as-agent (`runtara:agent-<slug>/capabilities`).
--
-- Lives on the `workflows` identity row (one row per workflow), NOT in the
-- per-version `workflow_definitions.definition` JSONB — the capability id must
-- be invariant across versions and survive renames (same philosophy as `path`
-- / `folder_id`: the pin lives on the identity row).
--
-- Nullable at first: existing rows are backfilled at server startup by
-- `backfill_workflow_slugs` (Rust, not SQL — the WIT-safe transform, reserved
-- native-agent-id checks, and per-tenant collision disambiguation live in
-- `runtara_dsl::agent_meta::generate_workflow_slug`).
ALTER TABLE workflows ADD COLUMN IF NOT EXISTS slug TEXT;

-- FULL unique index (not partial on deleted_at IS NULL): a soft-deleted
-- workflow RESERVES its slug — a published capability id must never be
-- silently reusable by a different workflow (docs/workflow-slug-plan.md,
-- decision 4). Multiple NULLs are fine (not-yet-backfilled / legacy-deleted
-- rows don't collide).
CREATE UNIQUE INDEX IF NOT EXISTS idx_workflows_tenant_slug
  ON workflows (tenant_id, slug);
