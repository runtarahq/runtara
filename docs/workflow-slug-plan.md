# Workflow slug — plan (auto-generated, editable; capability id for workflow-as-agent)

_Produced via multi-agent mapping + synthesis, verified against the feature branch (2026-07-15)._

## Decisions from review (2026-07-15) — these OVERRIDE the sections below where they conflict

1. **Slug edits are ALWAYS allowed** (overrides §3's "block-if-referenced/force" and §7's parent-pin guard). Rationale from the owner: workflows are PUBLISHED to a central repo and referenced cross-tenant, so the publishing tenant has no local reference count to protect — a parent that composed `agent-<oldslug>` simply keeps that pin until it recompiles. Drop the `find_references`/dependents-409 machinery for slug edits entirely; the edit endpoint just re-validates (normalize + uniqueness + reserved) and writes the identity row.
   - Implication to carry forward: cross-tenant / central-repo capability ids will likely need namespacing/qualification (e.g. by publisher) at the MARKETPLACE layer (agents.runtara.com). The LOCAL `workflows.slug` stays per-tenant-unique; the registry can qualify it on publish. Note but don't build now.
2. **Length cap = 64 chars** (was 48 in §2). Re-trim a trailing `-` after truncation.
3. **Leading digits are ALLOWED** — do NOT prefix `w-` for a digit-led slug. `runtara:agent-<slug>` is WIT-valid because `agent-` is the leading part, so `agent-2fa` parses. PR1's unit tests MUST assert this against wit-parser for a digit-led slug (if it turns out invalid, add the `w-` prefix back). The empty/un-nameable fallback stays `wf-<8 hex of workflow_id>`.
4. **Slug is RESERVED on soft-delete** — a deleted workflow does NOT release its slug (a published capability id must not be silently reusable). Use a **full** `UNIQUE (tenant_id, slug)` constraint, NOT the partial `WHERE deleted_at IS NULL` index the reports precedent uses. (Deviates from §1/§4's report mirror — full-unique instead of partial.)
5. **Status: PLAN ONLY** — not yet implemented. PR1 (the shared `runtara-dsl` slug util) is the intended first step when greenlit.

---

# Implementation Plan: Workflow Slug as Workflow-as-Agent Capability Id

Verified the load-bearing claims directly: `CAPABILITIES_EXPORT_AGENT_ID = "workflow-agent"` const at `component.rs:245`, threaded into `emit_world_wit` at `:271`; reports' `slugify`/`validate_slug` (private) at `reports.rs:7662`/`:7679`; `canonical_agent_id = to_ascii_lowercase().replace('_',"-")` at `agent_meta.rs:1825`. The maps are consistent with the code.

---

## 1. Where the slug lives — **workflow row (`workflows.slug`), stable identity**

**Decision: a new `slug TEXT` column on the `workflows` table. Not in `workflow_definitions.definition` JSONB, not a new DSL field.**

Rationale, grounded in the maps:
- The capability id must be **invariant across versions**. `name` lives per-version inside `workflow_definitions.definition` JSONB (`ExecutionGraph.name`, `schema_types.rs:72-74`), copied wholesale into a new row on every save (`create_version`, `repositories/workflows.rs:611-652`). A slug stored there would silently fork per edit — exactly the drift that breaks a package id.
- `workflows` is the one-row-per-workflow identity table (PK `(tenant_id, workflow_id)`), the same home as `path` and the planned `folder_id`. This matches the folders philosophy (docs/workflow-folders-phase1-plan.md): stable pin lives on the identity row, human strings that can vary do not become the pin.
- `workflow_id` (UUID-v4 string, minted `services/workflows.rs:184`) stays the opaque immutable id; slug is a **separate** human/stable capability layer, never a replacement.

**Concrete changes:**
- Migration adds `slug TEXT` to `workflows` (post-rename name; the rename ran at `20260419000000`).
- `WorkflowDto` (`dto/workflows.rs:715-761`) gains `slug: String`.
- Read paths that assemble the DTO (`repositories/workflows.rs:438-447`, `:552-561`) must `SELECT w.slug` from the `workflows` join (slug comes from the identity row, unlike `name` which they extract from `definition`).

**Migration sketch** (`add-migration` skill, e.g. `20260716000000_workflow_slug.sql`):
```sql
ALTER TABLE workflows ADD COLUMN IF NOT EXISTS slug TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_workflows_tenant_slug
  ON workflows (tenant_id, slug) WHERE deleted_at IS NULL;
```
Column stays **nullable** at first (existing rows have no slug); backfill fills it, and only then could a later migration consider `NOT NULL` (see §7). This mirrors `idx_report_definitions_tenant_slug_active` (`20260429000000_report_definitions.sql:31-33`) — partial-unique so soft-deleted rows free their slug.

---

## 2. Normalization — one shared WIT-safe transform in `runtara-dsl`

Reports' `slugify` already lowercases, collapses non-alphanumeric runs to a single `-`, and trims edges (`reports.rs:7662-7677`) — so it never emits double/edge hyphens. Its gaps for a WIT segment: no leading-letter guarantee, no length bound, no non-empty fallback, and `validate_slug` doesn't reject a user-supplied `--`/leading-digit.

**Decision: add a shared pair in `crates/runtara-dsl/src/agent_meta.rs`, next to `canonical_agent_id`**, so the server service *and* the compiler crate (both depend on runtara-dsl) share one definition — mirroring how `canonical_agent_id` is the single shared fold. Do **not** reuse `is_valid_identifier` (`validation.rs:3`, allows uppercase/`_`/space) or `sanitize_path_segment` (`compile.rs:1277`, allows `_`).

```
pub fn generate_workflow_slug(name: &str, workflow_id: &str) -> String
pub fn validate_workflow_slug(slug: &str) -> Result<(), SlugError>
```

**`generate_workflow_slug` transform (name → slug):**
1. `slugify(name)` logic from reports (lowercase via `to_lowercase()`, non-`[a-z0-9]` run → single `-`, trim edge `-`).
2. Length cap: truncate to **48 chars**, re-trim a trailing `-` created by truncation. (WIT imposes no cap; the cap protects `runtara_agent_<snake>.wasm` / CAS filenames at `component.rs:236-238`.)
3. Leading-letter guarantee: if the result is empty **or** starts with a digit, prefix `w-` (e.g. `"2024 report"` → `w-2024-report`).
4. Non-empty fallback: if still empty (un-nameable graph), use `wf-<first 8 hex of workflow_id>` — always starts with `w`, effectively unique.

**`validate_workflow_slug` (author/edit-time gate):** non-empty; only `[a-z0-9-]`; starts with `[a-z]`; no leading/trailing/`--` (each hyphen-part non-empty — this is exactly wit-parser's part-non-empty rule); length ≤ 48. This makes both `agent-<slug>` WIT-valid (`wit-parser validate_id`) and `canonical_agent_id(slug) == slug` (idempotent, so reserved-set folding and snake round-trip `slug.replace('-','_')` at `component.rs:230` are stable).

Note: a leading digit is *technically* WIT-legal because `agent-` is prepended (`agent-2fa` passes), but I forbid it for canonical cleanliness and consistency with the reports precedent — a **deliberate choice**, not a WIT requirement.

---

## 3. Auto-generate + editable semantics

- **On create:** `slug = request.slug.map(normalize+validate).unwrap_or_else(|| generate_workflow_slug(name, workflow_id))`, exactly mirroring reports (`reports.rs:377-382`). Auto-derived when absent, overridable when supplied. Runs in `WorkflowService::create_workflow` (`services/workflows.rs:132`) right after `workflow_id` is minted (`:184`), before `repository.create`.
- **On later name edits: slug is never re-derived.** Because slug lives on the `workflows` row and name lives in per-version JSONB, the existing name-edit surfaces (`set_workflow_metadata` → PUT `/versions/{v}/graph`; `update_workflow`) simply never touch it. Stability is structural, not enforced by extra code. This is the folders lesson: the pin survives rename.
- **Explicit slug edit path:** a dedicated identity-level write (see §5), re-running validate + uniqueness + reserved checks. It must **not** ride the graph-JSON write path.
- **Editing a referenced workflow-agent's slug — guarded.** A deployed parent that composed this child has `agent-<oldslug>` baked into its WIT (`component.rs:262/271`); dependency rows are keyed by `child_workflow_id`, **not** slug, so the dependency graph won't auto-migrate the pinned string. **Decision: block slug edits by default when the workflow is referenced as a child agent by any parent** (look up dependents via the reference machinery — `workflow_dependencies` / `find_references`), returning a 409 that lists the dependents; allow only with an explicit `force` flag that signals "dependents must be recompiled/redeployed." *(The exact parent→child pin mechanism at recompile is flagged unconfirmed in §7 — the conservative block is chosen because of that.)*

---

## 4. Uniqueness + reserved

**Per-tenant uniqueness — mirror reports exactly:**
- DB: the partial-unique index from §1.
- App: add a `From<sqlx::Error>` arm (copy `reports.rs:63-76`) matching the constraint name `idx_workflows_tenant_slug` → reuse the existing `ServiceError::Conflict` (`services/workflows.rs:1131`). Handler maps `Conflict` → HTTP 409, as reports do (`handlers/reports.rs:503-506`).

**Collision resolution — split by intent (deliberate divergence from reports, justified):**
- **User-supplied slug that collides → hard 409 reject** (house precedent; no `-2` suffix helper exists in the codebase).
- **Auto-generated slug that collides → deterministic disambiguation:** append `-<first 4 hex of workflow_id>` and re-check. Auto-generation must never fail a create silently, and the tenant's names are not unique, so the derived slug *will* sometimes clash. This keeps create non-failing while explicit input still rejects.

**Reserved native agent ids — new check (does not exist today):**
- Reject if `canonical_agent_id(&slug)` matches any live catalog agent id **or** the `"workflow-agent"` placeholder. Source of truth = the **live `AgentCatalog`** already injected as `Arc<AgentCatalog>` in `WorkflowService`, via `AgentCatalog::has_agent(canonical_agent_id(&slug))` (`agent_meta.rs:1907`) — authoritative in production, versus the static `bundle-emit/src/main.rs:22-56` list which can drift. A collision would break `wac` composition (the world imports `runtara:agent-<realid>` and would export the same package) and clash on the `runtara_agent_<snake>.wasm` filename.
- Same split as uniqueness: user-supplied reserved slug → reject; auto-generated one → apply the `-<hex>` disambiguation.

---

## 5. API + FE — server-authoritative validation

**Server (authoritative; FE slugify is preview-only):**
- `CreateWorkflowRequest` (`handlers/workflows.rs:55`): add `slug: Option<String>`.
- `WorkflowService::create_workflow` (`services/workflows.rs:132`): normalize/validate/uniqueness/reserved, then `repository.create` (`repositories/workflows.rs:69-94`) writes the new column in its `INSERT`.
- **Slug edit:** add a dedicated `POST /api/runtime/workflows/{id}/slug` handler + `WorkflowService::update_workflow_slug`, writing only the `workflows` row (identity-level). Do **not** overload `set_workflow_metadata` (which mutates graph JSON in-place). Ownership-gated like the other update handlers.
- `WorkflowDto.slug` (`dto/workflows.rs:715`) surfaced in every get/list.

**MCP:**
- `CreateWorkflowParams` (`mcp/tools/workflows.rs:265`): add `slug: Option<String>`.
- New `set_workflow_slug` tool (register in `mcp/server.rs` alongside the metadata tools).
- Surface slug in `get_workflow_metadata` (`graph_mutations.rs:1308`) and document it in `workflow_authoring_schema` (`tools/workflows.rs:414`).

**Frontend (React `crates/runtara-server/frontend`):**
- `WorkflowForm` create schema (`components/WorkflowForm/index.tsx:25`): add a slug input auto-filled from name using the existing `shared/utils/string-utils.ts:6` `slugify`, editable, with a "leave blank to auto-generate" affordance.
- `SettingsContent` General section (`ValidationPanel/SettingsContent.tsx:71`): add an editable slug field wired to the **new slug endpoint** (not the executionGraph update), so name/desc and slug save through their correct paths.
- Thread through `queries/index.ts` (`createWorkflow` :91, new `updateWorkflowSlug`).
- **`regen-frontend-api`** after the DTO changes so `src/generated/RuntaraRuntimeApi.ts` stays in sync.

---

## 6. Tie-in to workflow-as-agent (this is slice b)

Replace the fixed const with the workflow's slug, threaded (not hard-coded) through the emitter, per the const's own doc-comment (`component.rs:242-244`):

- **`emit_world_wit`** (`component.rs:247`): add an `export_agent_id: &str` param; the `AgentCapabilities` arm interpolates it at `:271` instead of `CAPABILITIES_EXPORT_AGENT_ID`.
- **`build_direct_component_resolve_configured`** (`compile.rs:1129`): the `AgentCapabilities` arm uses the slug for `agent_wit_package(slug)` (`:1136-1139, :1197-1214`) and the world export line (`:1179-1189`).
- **`abi_json` `componentRunExport`** (`compile.rs:1012-1015`): embed `runtara:agent-<slug>/capabilities` in the `DIRECT_WORKFLOW_ABI` custom section.
- Delete/retire the `CAPABILITIES_EXPORT_AGENT_ID` const once all three consumers take the parameter.

**Plumbing the slug into compile:** the compile entry currently takes no id for this. The server deploy/compile caller loads `workflows.slug` and passes it down through `emit_direct_component_artifacts_configured` / `emit_direct_artifact`. For nested embeds (`runtara-compile --child`), the child's slug must be resolvable — for the server path via the `workflow_dependencies` → child `workflows.slug` join; for the standalone CLI this likely needs a **new `--slug`/child-slug arg** (flagged in §7).

**Compile-time failure mode:** an invalid slug reaching wit-parser is a hard `DirectCompileError::Component` (`compile.rs:1273-1275`) that bricks the workflow-as-agent *and every parent composing it*, with no fallback. Two mitigations: (a) validate at **save time** (§5) so bad slugs never persist; (b) add a **defensive re-validation at the compile entry** (`validate_workflow_slug` before the first `push_str`) so a legacy/corrupt row yields a friendly error rather than an opaque lexer failure.

---

## 7. Sequencing + risks

**PR order (each independently shippable):**
1. **Shared slug util in `runtara-dsl`** (`generate_workflow_slug` + `validate_workflow_slug`) with unit tests for WIT-safety (leading letter, no `--`/edge dashes, length cap, empty/reserved fallback) and idempotence under `canonical_agent_id`. Pure, no behavior change.
2. **Migration + backfill + read/write plumbing.** Add nullable `slug` + partial-unique index; `repository.create`/read paths write and surface it; `WorkflowDto.slug`. Slug not yet consumed by codegen — additive and safe.
3. **API + MCP + FE + validation/uniqueness/reserved/409.** Create override, new slug-edit endpoint + tool, `regen-frontend-api`.
4. **Codegen tie-in (slice b).** Thread slug through the three emitters, replace the const, plumb from the workflow record (+ `--child` resolution), defensive compile-time re-validation. **`e2e-verify`** a parent composing a child workflow-as-agent under a real `agent-<slug>` package.

**Backfill (in PR2):** derive from the current/latest version's `definition.name`. Because the logic needs the shared WIT-safe util + reserved-set check + per-tenant `-<hex>` disambiguation, do it as a **Rust startup backfill guarded by `slug IS NULL`** (idempotent, runs once), **not** raw SQL `regexp_replace` — SQL can't cleanly enforce leading-letter, reserved-set, or deterministic collision handling. Fallback for empty/reserved names: `wf-<8 hex of workflow_id>`. Keep the column nullable until backfill is confirmed complete across tenants; only a later migration should consider `NOT NULL`.

**Migration/rename hazards:**
- The new migration must target `workflows` (post-`20260419` rename) and use `ADD COLUMN IF NOT EXISTS` / `CREATE UNIQUE INDEX IF NOT EXISTS` — the table is created via two idempotent schemas (`20250101` VARCHAR-based, `20260409` TEXT `IF NOT EXISTS`); referencing the canonical `workflows` name after the rename is safe.
- Backfill collisions within a tenant are expected (names aren't unique) — handled by the `-<hex>` disambiguation; without it the partial-unique index would reject the second row.

**Could not confirm from the maps (verify before PR3/PR4):**
- **Exact parent→child pin at recompile:** maps state `workflow_dependencies` is keyed by `child_workflow_id`, not slug, but whether a parent's WIT pins the slug at compile time vs resolves it live from the child record is unconfirmed. This drives the §3 slug-edit guard — hence the conservative block-if-referenced default.
- **`runtara-compile` CLI access to the slug for `--child`:** the standalone CLI has no DB; a `--slug`/child-slug arg is likely required and its shape is unconfirmed.
- **Leading-digit and 48-char cap are chosen policies**, not WIT constraints — confirm they're acceptable product decisions.
- Whether slug should survive soft-delete + recreate (partial-unique frees it on delete, reports' behavior) when other workflows reference it as an agent — confirm this is desired for capability ids.

**Key files (all absolute):**
- `/Users/volodymyrrudyi/work/runtara/crates/runtara-dsl/src/agent_meta.rs:1825` — home for the shared slug util
- `/Users/volodymyrrudyi/work/runtara/crates/runtara-server/migrations/` — new `..._workflow_slug.sql` (model: `20260429000000_report_definitions.sql:31`)
- `/Users/volodymyrrudyi/work/runtara/crates/runtara-server/src/api/services/workflows.rs:132,184,1131` — create + slug gen + Conflict
- `/Users/volodymyrrudyi/work/runtara/crates/runtara-server/src/api/repositories/workflows.rs:69,438,552` — write + read paths
- `/Users/volodymyrrudyi/work/runtara/crates/runtara-server/src/api/dto/workflows.rs:715` and `handlers/workflows.rs:55` — DTO + request
- `/Users/volodymyrrudyi/work/runtara/crates/runtara-server/src/api/services/reports.rs:63,377,7662,7679` — precedent to copy
- `/Users/volodymyrrudyi/work/runtara/crates/runtara-server/src/mcp/tools/workflows.rs:265,414` and `graph_mutations.rs:1308` — MCP surface
- `/Users/volodymyrrudyi/work/runtara/crates/runtara-workflows/src/direct_wasm/component.rs:245,271` and `compile.rs:1012,1136,1179,1197,1273` — codegen tie-in
