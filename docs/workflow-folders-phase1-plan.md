# Workflow Folders → First-Class Entities (Phase 1)

**Status:** Planned — decisions locked
**Date:** 2026-07-14
**Goal:** Promote workflow "folders" from a derived `path` string into first-class entities with stable identity and real CRUD. This is the precursor to compiling a folder into an agent where each contained workflow is a capability (refines `agent-extraction-plan.md` §9.4 from *workflow-granular* to *folder-granular* — a folder = an agent package, a workflow = an exported capability).

---

## 1. Decisions (locked)

1. **Identity model — stable `folder_id` + synced `path`.** New `folders` table keyed by a stable generated `folder_id`, with `parent_folder_id` for the tree and a denormalized `path` kept in sync. Workflows keep their `path` column and gain a nullable `folder_id` FK. Additive, backward-compatible, rename-safe. The stable id is what Phase 2's compiled agent pins to (survives rename).
2. **Delete semantics — block if non-empty.** Deleting a folder that still contains workflows or subfolders is rejected; the caller must move/delete contents first. Replaces today's rename-to-root hack.

### Smaller decisions (recommended defaults, adjust on review)
- **Root folder `/`** is implicit, not a row (every tenant has it; `parent_folder_id = NULL` + `path = '/'` reserved). Workflows with `folder_id = NULL` are "at root."
- **`folder_id` generation** mirrors `workflow_id`'s scheme.
- **Name validation** reuses the existing segment rules from `validate_path` (no `/`, no `.`/`..`, non-empty, length bound).
- **Phase 1 folder metadata** stays minimal: `name`, `description`. Agent-manifest columns are deferred (§7).

---

## 2. Current state (verified, file:line)

A folder is a single `path TEXT NOT NULL DEFAULT '/'` column on `workflows`
(`crates/runtara-server/migrations/20260409000000_server_schema.sql:19`, index `idx_workflows_path`).
Everything else is derived from that string:

- **Repository** (`crates/runtara-server/src/api/repositories/workflows.rs`): `list_folders` = `SELECT DISTINCT path` (`:864`), `rename_folder` = prefix `UPDATE` (`:882`), `update_path` (`:840`), `create` omits path → DB default `/` (`:69`), list-with-path-filter (`:282`).
- **Service** (`crates/runtara-server/src/api/services/workflows.rs`): `validate_path` (`:34`), `move_workflow` (`:1041`), `list_folders` (`:1075`), `rename_folder` (`:1086`).
- **DTOs** (`crates/runtara-server/src/api/dto/workflows.rs`): `path` on create req (default `/`, `:757`); `FoldersResponse{folders: Vec<String>}`, `MoveWorkflowRequest{path}`, `RenameFolderRequest{oldPath,newPath}`, `RenameFolderResponse`.
- **Handlers** (`crates/runtara-server/src/api/handlers/workflows.rs`): `move_workflow_handler` (`:2750`), `list_folders_handler` (`:2785`), `rename_folder_handler` (`:2822`).
- **Routes** (`crates/runtara-server/src/server.rs:1731`): `GET /api/runtime/workflows/folders`, `PUT /api/runtime/workflows/folders/rename`, `PUT /api/runtime/workflows/{id}/move`.
- **MCP** (`crates/runtara-server/src/mcp/tools/workflows.rs`, `server.rs:181/201`): `list_workflow_folders`, `move_workflow`. No create/rename/delete folder tools.
- **Authz** (`crates/runtara-server/src/authz/mod.rs:306`): `workflow:folder_rename` (Owner/Admin); read-only role denied.
- **Frontend** (`crates/runtara-server/frontend/src/features/workflows/`): `hooks/useFolders.ts` `parseFolderPaths` synthesizes the tree (incl. ancestors) from a flat `string[]`; `components/FolderDialogs/index.tsx`; `queries/index.ts` `deleteFolder` = **rename-to-root hack** (`:701`); also `WorkflowsGrid`, `WorkflowCard`, `pages/Workflows`, `shared/queries/query-keys.ts`, `generated/RuntaraRuntimeApi.ts`.

**Blast radius is fully contained.** Nothing outside the above touches the workflow folder path — no trigger/report/execution/reference joins on it, no compiled artifact embeds it (grep-verified). Folder path is pure organizational metadata in one column.

### What today's model can't do (the reason for this change)
1. Empty folders can't exist (folder vanishes when its last workflow leaves).
2. Nowhere to hang metadata (no row → no future agent manifest).
3. No stable identity (rename mutates the key; pins break).
4. "Delete" is a rename hack.

---

## 3. Target schema

New migration `20260714000000_workflow_folders.sql` (idempotent, `IF NOT EXISTS` style):

```sql
CREATE TABLE IF NOT EXISTS folders (
    tenant_id         TEXT NOT NULL,
    folder_id         TEXT NOT NULL,
    parent_folder_id  TEXT,                       -- NULL = root-level
    name              TEXT NOT NULL,              -- last path segment
    path              TEXT NOT NULL,              -- denormalized full path, kept in sync
    description       TEXT,
    created_by        TEXT,
    deleted_at        TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, folder_id),
    FOREIGN KEY (tenant_id, parent_folder_id) REFERENCES folders(tenant_id, folder_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_folders_tenant_path ON folders(tenant_id, path) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_folders_parent ON folders(tenant_id, parent_folder_id);

ALTER TABLE workflows ADD COLUMN IF NOT EXISTS folder_id TEXT;
CREATE INDEX IF NOT EXISTS idx_workflows_folder_id ON workflows(tenant_id, folder_id);
-- FK optional here; can add after backfill guarantees referential integrity.
```

**Reserved for Phase 2 (not added now):** `agent_id`, `abi_version`, `uses_ai`, declared-connections, and a per-workflow `exported` flag. Adding them later is a pure column-add migration.

---

## 4. Migration / backfill

Within the same migration, after DDL:
1. Materialize a folder row for every distinct workflow `path` **and every synthesized ancestor** (same walk as `parseFolderPaths` in `useFolders.ts`), setting `name`, `path`, and `parent_folder_id` bottom-up. Generate `folder_id` per row.
2. `UPDATE workflows SET folder_id = (matching folder row's id)` by exact `path`.
3. Root (`path = '/'`) stays implicit (no row); root workflows keep `folder_id = NULL`.

No workflow row's `path` or behavior changes — the backfill is purely additive.

---

## 5. Work by layer

**Repository** — new `repositories/folders.rs` (or extend `workflows.rs`): `create_folder`, `get_folder`, `list_folders` (returns rows), `update_folder` (name/description), `rename_folder` (folder row(s) + descendant folder paths + workflow `path` prefixes, one txn — generalizes the existing prefix `UPDATE`), `delete_folder` (guarded, §6), `move_folder` (reparent + path recompute, cycle-checked). Keep legacy `update_path` / string `list_folders` during transition.

**Service** — folder CRUD; reuse `validate_path`/segment validation; non-empty guard for delete; cycle prevention for move/reparent; keep `move_workflow` (now also updates `folder_id`).

**DTOs** — `FolderDto{folderId, name, path, parentFolderId, description, workflowCount?}`; `CreateFolderRequest`, `UpdateFolderRequest`, `MoveFolderRequest`; reshape `FoldersResponse` → `Vec<FolderDto>` (keep a legacy `folders: Vec<String>` field, or version the route, for BC).

**Handlers + routes** — add `POST /workflows/folders`, `GET /workflows/folders/{id}`, `PATCH /workflows/folders/{id}`, `DELETE /workflows/folders/{id}`, `PUT /workflows/folders/{id}/move`; reshape `GET /workflows/folders`. Keep existing move/rename routes working.

**MCP** — add `create_folder`, `update_folder`, `delete_folder`, `get_folder`; reshape `list_workflow_folders` to return entities; keep `move_workflow`.

**Authz** — add `workflow:folder_create`, `workflow:folder_update`, `workflow:folder_delete` next to `workflow:folder_rename`; wire read-only denials (`authz/mod.rs`, `middleware/authorization.rs`).

**Frontend** — regen API client (`regen-frontend-api`); `useFolders` reads entities (keep `parseFolderPaths` only as fallback); `FolderDialogs` gains real create + real delete (drop the rename-to-root hack in `queries/index.ts:701`); wire new mutations; block-if-non-empty error surfaced in the delete dialog.

---

## 6. Delete spec (block if non-empty)

`DELETE /workflows/folders/{id}` returns 409 if the folder has any non-deleted workflows **or** any child folders. Error message directs the caller to move/delete contents first. Frontend delete dialog shows the block reason (counts) and offers "move contents to parent" as a manual pre-step (not automatic).

## 6a. Rename / move cascade spec

- **Rename** (`name` change): update this folder row's `name`+`path`, all descendant folder rows' `path` (prefix), and all descendant workflow `path`s (prefix) — one transaction. `folder_id`s unchanged.
- **Move** (reparent): set `parent_folder_id`, recompute `path` for the folder + descendants + their workflows; reject if the new parent is the folder itself or a descendant (cycle).

---

## 7. Deferred to Phase 2 (folder → agent)

The compile target and manifest: `invoke`-dispatch wrapper component, per-workflow `exported` flag, `agent_id`/`abi_version`/`uses_ai`/declared-connections columns, non-suspending validation gate, `meta.json` generation. Phase 1 intentionally ships only the entity + stable identity + CRUD, which is independently useful (real empty folders, metadata, real delete).

---

## 8. Test plan

- Repo/service unit tests: folder CRUD, rename cascade (folder + descendants + workflow paths), move/reparent cycle rejection, delete non-empty rejection.
- Migration test: backfill materializes ancestors, sets `workflow.folder_id`, root stays implicit.
- e2e (`e2e-verify`): create folder → create workflow in it → rename folder → assert workflow path follows and `folder_id` unchanged → attempt delete (blocked) → move workflow out → delete (succeeds).
