# Bulk object ops + Split batching — quick reference

What changed and how to use it. Everything here is additive — existing code is untouched.

## 1. Object model bulk operations

### Store primitives (`runtara-object-store`)

| Method | Semantics |
| --- | --- |
| `create_instances(schema, instances)` | Existing strict insert; errors on any unique conflict or validation failure. |
| `create_instances_extended(schema, instances, opts)` | **New.** Same as above but with opt-in conflict + validation handling. |
| `update_instances(schema, properties, condition)` | Existing — same values applied to every row matching condition. |
| `update_instances_by_ids(schema, Vec<(id, properties)>)` | **New.** Per-row values, atomic (N `UPDATE`s in one tx). |
| `delete_instances(schema, condition)` | Existing — condition-based. |
| `upsert_instances(schema, instances, conflict_columns)` | Existing. |

**`BulkCreateOptions`:**

```rust
BulkCreateOptions {
    conflict_mode: ConflictMode::Error                           // default
                 | ConflictMode::Skip { conflict_columns: Vec<String> }
                 | ConflictMode::Upsert { conflict_columns: Vec<String> },
    validation_mode: ValidationMode::Stop   // default; first invalid row aborts
                   | ValidationMode::Skip,  // invalid rows go into `errors`, valid ones insert
}
```

Returns `BulkCreateResult { created_count, skipped_count, errors: Vec<{ index, reason }> }`.

**Size cap:** every bulk primitive that takes a `Vec` enforces
`StoreConfig.bulk_request_limit` (default **10 000**, env
`OBJECT_MODEL_BULK_REQUEST_LIMIT`). Over the cap → `ObjectStoreError::validation`.
Condition-based bulk update/delete are not capped (SQL decides row count).

### Public HTTP API (`/api/runtime/object-model`)

All routes JWT-authenticated; `connectionId` query param optional.

```
POST   /instances/{schema_id}/bulk          → bulk create
PATCH  /instances/{schema_id}/bulk          → bulk update (byCondition | byIds)
DELETE /instances/{schema_id}/bulk          → bulk delete by ids (existed; now transactional)
```

#### POST — bulk create

Two accepted shapes — pick whichever fits your caller. Provide exactly one.

**Object form** — one JSON object per record:

```jsonc
{
  "instances": [ {...}, {...} ],
  "onConflict":       "error" | "skip" | "upsert",   // default "error"
  "onError":          "stop"  | "skip",              // default "stop"
  "conflictColumns":  ["sku"]                        // REQUIRED when onConflict = skip | upsert
}
```

**Columnar form** — column names once, rows as arrays. Cuts ~2–3× on wire size
for large uniform payloads (snapshots, CSV-style writes). `constants` are merged
into every row; row values win on key overlap. `nullifyEmptyStrings` (default
false) converts `""` in non-string columns to `null` before validation — handy
when sources (CSV, SFTP) deliver missing cells as empty strings.

```jsonc
{
  "columns": ["sku", "warehouse_id", "qty", "available_date"],
  "rows": [
    ["ABC", "1818", "5", "2026-04-18T08:00:00+00:00"],
    ["DEF", "1818", "2", ""]
  ],
  "constants":          { "snapshot_date": "2026-04-18" },
  "nullifyEmptyStrings": true,
  "onConflict":          "skip",
  "conflictColumns":     ["sku", "warehouse_id", "snapshot_date"]
}
```

Response (same for both forms):

```jsonc
{
  "success": true,
  "createdCount": 2,
  "skippedCount": 1,
  "errors": [ { "index": 1, "reason": "Required column 'qty' is missing" } ],
  "message": "2 created, 1 skipped"
}
```

- `onConflict: "skip"` without `conflictColumns` → 400.
- In `Skip` conflict mode, `skippedCount` includes rows dropped by `ON CONFLICT DO NOTHING`; we know the count but not which rows.
- Supplying both `instances` and `columns`/`rows` → 400.
- Row length must equal `columns.len()` → 400 with row index.
- `conflictColumns` may reference keys provided via `constants` (e.g., `snapshot_date` in the example).

#### PATCH — bulk update

Two shapes selected by `mode`:

```jsonc
// Apply the same values to every matching row
{
  "mode": "byCondition",
  "properties": { "status": "archived" },
  "condition": { "op": "IN", "arguments": ["id", ["uuid1", "uuid2"]] }
}

// Per-row values
{
  "mode": "byIds",
  "updates": [
    { "id": "uuid1", "properties": { "qty": 100, "label": "a" } },
    { "id": "uuid2", "properties": { "qty": 200 } }
  ]
}
```

Response: `{ success, updatedCount, message }`.

#### DELETE — bulk delete

Unchanged contract: `{ "instanceIds": [...] }`. The service method is now one
transactional `delete_instances(IN("id", ids))` (was a per-ID loop).

### Internal API (what agents call) — `/api/internal/object-model`

Same capabilities exposed for workflow binaries. Tenant comes from `X-Org-Id`
header, no JWT.

```
POST /instances                              → single create (existed)
PUT  /instances/{schema_name}/{id}           → single update (existed)
POST /instances/delete                       → single delete (NEW)
POST /instances/bulk-create                  → NEW
POST /instances/bulk-update                  → NEW
POST /instances/bulk-delete                  → NEW
```

Payloads use `snake_case` keys (`schema_name`, `on_conflict`, `conflict_columns`,
`on_error`) — consistent with existing internal endpoints.

### Agent SDK capabilities (module `object_model`)

| Capability id | Use |
| --- | --- |
| `delete-instance` | Single delete by `instance_id`. |
| `bulk-create-instances` | Insert many. Accepts either `instances` (object form) or `columns` + `rows` (+ optional `constants`, `nullify_empty_strings`). Supports `on_conflict`, `on_error`, `conflict_columns`. |
| `bulk-update-instances` | Either `{ condition, properties }` OR `{ updates: [{id, properties}] }`. |
| `bulk-delete-instances` | Either `{ ids }` OR `{ condition }`. |

Input/output schemas are live at `GET /api/v1/agents` under the `object_model` agent.

Workflow step example:

```jsonc
{
  "stepType": "Agent",
  "id": "insert",
  "agentId": "object_model",
  "capabilityId": "bulk-create-instances",
  "inputMapping": {
    "schema_name":     { "valueType": "immediate", "value": "Product" },
    "instances":       { "valueType": "reference", "value": "data.input.instances" },
    "on_conflict":     { "valueType": "immediate", "value": "skip" },
    "conflict_columns":{ "valueType": "immediate", "value": ["sku"] },
    "on_error":        { "valueType": "immediate", "value": "skip" }
  }
}
```

Outputs: `success`, `created_count`, `skipped_count`, `errors: [{index, reason}]`.

### Frontend UI

On any object-type detail page (`/objects/:typeName`):

- Top-right toolbar now has **Bulk Insert** (next to Import CSV).
- When rows are selected, the action strip shows **Edit N selected** +
  **Delete N selected**.

`BulkInsertDialog` — paste a JSON array of records, pick `On conflict` (error /
skip / upsert), `On validation error` (stop / skip), and conflict columns when
needed. Results show `createdCount`, `skippedCount`, and a collapsible list of
per-row errors.

`BulkEditDialog` — checkbox-pick which fields to set, enter new values, applied
to every selected row via `PATCH` with `mode: byCondition`, `condition: IN("id",
selected_ids)`.

### When to use which layer

- **Writing a workflow / agent integration** → agent capability
  (`object_model::bulk-create-instances` etc.).
- **Calling from an external app** → public HTTP API (`/api/runtime/...`).
- **Writing Rust host code (tests, migrations, admin)** → store primitive
  directly.

## 2. SplitStep batching

New optional field on `SplitConfig`: `batchSize` (serialized from
`batch_size: Option<u32>`).

| Value | Behavior |
| --- | --- |
| unset / 0 | **Default** — one iteration per element. `[1,2,3,4,5]` → 5 iterations with scalar items `1, 2, 3, 4, 5`. |
| `n > 0` | Chunk the array into sub-arrays of size `n` (last chunk may be shorter). `[1,2,3,4,5]` with `batchSize=2` → 3 iterations with items `[1,2]`, `[3,4]`, `[5]`. |

When batching is on, each iteration's subgraph receives an **array** as its
input item (not a scalar), so downstream steps should use an array-aware path
(e.g. a nested `Split` over that sub-array, or an agent that accepts a list).

### Use

```jsonc
{
  "stepType": "Split",
  "id": "process_in_batches",
  "config": {
    "value":     { "valueType": "reference", "value": "data.input.items" },
    "batchSize": 100,
    "parallelism": 4
  },
  "subgraph": { /* ...subgraph that expects `data` to be a 100-element array... */ }
}
```

Useful for:

- Chunking inputs for the new bulk insert/update endpoints (stay under the
  10 000-item cap without changing caller code).
- Feeding batch-style APIs that accept arrays (e.g. Shopify GraphQL batch
  mutations, bulk webhook delivery).
- Reducing overhead when each iteration has fixed per-call cost.

## 3. Verification status

- Store: 6 new integration tests + 31 existing all passing (`TEST_DATABASE_URL=...
  cargo test -p runtara-object-store --test integration`).
- Split emitter: 34 tests including 2 new `batch_size` tests.
- HTTP: all three endpoints smoke-tested end-to-end via curl against a running
  server (bulk create in every mode, bulk update in both shapes, bulk delete).
- Agent WASM: compiled a real workflow that calls `object_model::bulk-create-instances`
  and ran it via wasmtime. Confirmed `created_count` / `skipped_count` /
  conflict-skip behavior against the DB.

## 4. Gotchas observed during verification

1. **Dispatch table is manually maintained.** If you add a new `#[capability]`
   to `runtara-agents`, you must also add a match arm in
   [runtara-workflow-stdlib/src/dispatch.rs](../crates/runtara-workflow-stdlib/src/dispatch.rs).
   `cargo test -p runtara-workflow-stdlib` has
   `test_dispatch_table_completeness` that catches omissions.
2. **`runtara-ctl register` defaults to `runner_type=oci`.** For WASM
   workflows you currently have to either set it in DB or use a different
   registration path. Not ideal, noted as follow-up.
3. **Workflow input wrapping:** `runtara-ctl start --input X` stores X verbatim;
   the generated workflow reads `input_json.get("data")`, so wrap your payload
   as `{"data": {...}}`. Schema references like `data.input.instances` resolve
   against that.
