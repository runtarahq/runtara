# Reports: `file_upload` block — plan

A new report block type: a drop zone / file picker that runs a workflow with the
selected file as input. Two trigger modes: run automatically on file selection,
or run via a button with a configurable label.

## Design summary

The block is a thin new shell around three things that already exist end-to-end:

1. **`FileData`** — the canonical file shape `{content: base64, filename, mimeType}`
   (`crates/runtara-agents/src/types.rs:19`), already produced by the shared
   frontend `FileInput` component + `fileToFileData`
   (`frontend/src/shared/components/ui/file-input.tsx`,
   `frontend/src/shared/utils/file-utils.ts`), already accepted by the workflow
   input validator (`file` field type → object,
   `crates/runtara-workflows/src/input_validation.rs:149`), and already consumed
   by agents (compression, s3, xlsx, sftp). Frontend cap 50 MB
   (`shared/types/file.ts:30`); tenant routes accept 96 MB bodies specifically to
   leave headroom for base64-encoded 50 MB file inputs (`server.rs:723`, SYN-457).

2. **`ReportWorkflowActionConfig`** (`crates/runtara-report-dsl/src/types.rs:691`)
   — already carries `workflowId`, `version`, `label` (the configurable button
   label), `runningLabel`, `successMessage`, `reloadBlock`, and
   `context {mode, inputKey}`. With `context.mode = "value"`, the execute
   pipeline takes `trigger.value` verbatim and wraps it under `inputKey`
   (`services/reports.rs:5318` `resolve_report_workflow_action_context`,
   `:5331`). So the block sends `trigger: {value: <FileData>}` and the workflow
   receives `{"data": {"<inputKey>": {content, filename, mimeType}}, "variables": {}}`.

3. **The report workflow-action execute pipeline** —
   `POST /api/runtime/reports/{report_id}/blocks/{block_id}/workflow-actions/{action_id}/execute`
   (route `server.rs:1564`, handler `handlers/reports.rs:306`, service
   `services/reports.rs:524`). It already provides Idempotency-Key handling with
   deterministic instance ids, a bounded wait (default 2 s, max 5 s) with
   in-place re-render on completion, and the frontend orchestration hook
   `useReportWorkflowAction` with its status-polling fallback (10 min) and
   react-query render-cache write (`reportActionRender.ts`).

Because the post-completion response re-renders the report's data blocks, a
"drop CSV → workflow imports rows → table below updates" flow works with zero
extra machinery.

**No new HTTP routes, no new trigger fields, no edit-op changes.** The only
execute-path backend change is teaching the per-block action lookup about the
new config location.

## Block config (DSL)

New variant `ReportBlockType::FileUpload` (`file_upload`) and:

```rust
// crates/runtara-report-dsl/src/types.rs
pub struct ReportFileUploadConfig {
    #[serde(rename = "workflowAction")]
    pub workflow_action: ReportWorkflowActionConfig,
    /// How the workflow is started once a file is chosen.
    #[serde(default)]
    pub trigger: ReportFileUploadTrigger,          // automatic | button (default button)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Helper text rendered inside the drop zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Accepted file types, passed to the input's `accept` attr and checked on
    /// drop (extensions ".csv" or MIME types "text/csv"). Empty = any file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accept: Vec<String>,
    /// Per-file cap; clamped to the platform's 50 MB limit.
    #[serde(default, rename = "maxSizeBytes", skip_serializing_if = "Option::is_none")]
    pub max_size_bytes: Option<u64>,
}

#[serde(rename_all = "snake_case")]
pub enum ReportFileUploadTrigger { Automatic, #[default] Button }
```

Deliberate reuse instead of new fields:
- **Button label** = `workflowAction.label` (default "Run workflow"), spinner
  text = `runningLabel`, completion toast/inline message = `successMessage`.
- **Input key** = `workflowAction.context.inputKey` (authoring default `"file"`).
- **Post-run refresh** = the existing render-in-place response + `reloadBlock`.

Example definition fragment:

```json
{
  "id": "csv_import",
  "type": "file_upload",
  "file_upload": {
    "title": "Import price list",
    "description": "Drop a CSV here — rows are upserted into Products.",
    "accept": [".csv", "text/csv"],
    "trigger": "button",
    "workflowAction": {
      "id": "upload",
      "workflowId": "8f2c…",
      "label": "Import",
      "runningLabel": "Importing…",
      "successMessage": "Price list imported.",
      "context": { "mode": "value", "inputKey": "file" }
    }
  }
}
```

## Backend changes

`crates/runtara-report-dsl`:
1. `types.rs:804` — add `FileUpload` to `ReportBlockType`; add
   `ReportFileUploadConfig` + `ReportFileUploadTrigger` (both with the `utoipa`
   and `schemars` derive attrs); add `file_upload: Option<ReportFileUploadConfig>`
   to `ReportBlockDefinition` (`types.rs:609`).
2. `lint.rs:61` — add `file_upload` to `ALLOWED_BLOCK_KEYS`.
3. Edit ops: none (`ReportEditOp` is block-type-agnostic).

`crates/runtara-server`:
4. `api/services/reports/renderers.rs:164` — `FileUploadRenderer` impl + branch
   in `renderer_for`. Like markdown it has no data source; `render_file_upload_block`
   echoes the display config (title, description, accept, trigger mode, action
   id/labels) so full-report renders include the block. It must NOT echo nothing
   for unknown reasons silently — mirror the markdown renderer's shape.
5. `api/services/reports.rs`:
   - `locate_report_workflow_action` (`:5177`) — new branch: if
     `block.file_upload` is set and `workflowAction.id` (fallback `"upload"`)
     matches `action_id`, return it with `fallback_field: "upload"`.
   - `validate_report_block_workflow_action_ids` (`:5212`) — include the
     file_upload action id in the duplicate check.
   - `validate_definition` (`:1603` area) — new `FileUpload` branch:
     * `file_upload` config required for the type (and forbidden on other types
       — the lint key list handles authoring, this is the hard check);
     * delegate to `validate_report_workflow_action_config` (context string
       `"file_upload block"`);
     * require `context.mode == value` (E: "file_upload workflowAction.context.mode
       must be 'value'");
     * reject `visibleWhen`/`hiddenWhen`/`disabledWhen` — there is no row to
       evaluate them against (`ensure_report_workflow_action_enabled` would
       evaluate against `null`);
     * `accept` entries must be non-empty; `maxSizeBytes` must be > 0 and
       ≤ 52 428 800 (50 MB);
     * no `source`/dataset requirement (mirror markdown).
6. `server.rs:374-435` — register `ReportFileUploadConfig` and
   `ReportFileUploadTrigger` in the OpenAPI `components(schemas(...))` list.
7. MCP authoring: `mcp/tools/reports.rs` — extend the `"type"` union string
   (`:690`) and add a `blockShape` entry (`:687` section) documenting the config,
   the `context.mode: value` + `inputKey` requirement, both trigger modes, and
   the example above.

Execute path, idempotency, wait/poll, and render-in-place need **zero changes**
— the request body is the existing `ExecuteReportWorkflowActionRequest` with
`trigger.value` carrying the `FileData` object. The SHA-256 payload fingerprint
and the 96 MB body limit both handle a base64 50 MB file.

## Frontend changes

1. Regenerate the API client (`regen-frontend-api` skill /
   `npm run generate-api-runtime-local`).
2. `features/reports/types.ts` — re-export `ReportFileUploadConfig`.
3. `components/ReportBlockHost.tsx` — `RenderedBlock` branch → `FileUploadBlock`;
   `BlockSkeleton` branch; **skip the block-data fetch** for this type (markdown
   precedent — it's a control, not a data block).
4. New `components/blocks/FileUploadBlock.tsx`:
   - Drop zone built on the shared `FileInput`
     (`shared/components/ui/file-input.tsx` — drag-and-drop + click already
     implemented, emits `FileData`); pass `accept`, enforce
     `min(maxSizeBytes, MAX_FILE_SIZE_BYTES)` via `validateFileSize` before
     encoding.
   - Run through `useReportWorkflowAction` with
     `trigger: { value: fileData }` — phases (`submitting → running →
     refreshing`), polling fallback, and render-cache write come for free.
   - **Button mode**: selection shows a filename + size chip with a clear
     control; the button (label = `workflowAction.label`) starts the run.
   - **Automatic mode**: run starts immediately on select/drop; no button.
   - While running: input disabled, `runningLabel` shown. On success:
     `successMessage`, input reset (clear the native input's `value` so
     re-selecting the same file fires again). On failure: inline error with the
     workflow error message + retry keeping the selected file.
   - Each run gets a fresh idempotency key (the hook already does
     `crypto.randomUUID()`), so intentionally re-uploading the same file is a
     new run — correct.
5. Wizard editor:
   - `wizard-v2/blocks/FileUploadBlockEditor.tsx` — workflow picker + labels
     (reuse the workflow-action editor pieces from `tableActionEditors.tsx`),
     trigger-mode toggle, accept list, max size, input key (default `"file"`,
     surfaced next to a hint about the workflow's `file`-typed input field).
   - Register in `BlockEditor.tsx` (`:136` dispatch).
   - `changeBlockType.ts` — compiler-forced: `TYPE_SPECIFIC_FIELDS`,
     `blockTypeLabel`, `BLOCK_TYPES`, plus `defaultConfigForType` (default:
     button mode, `context {mode: "value", inputKey: "file"}`).

## Testing

- **DSL**: serde round-trip for the new config; lint accepts the `file_upload`
  key and still flags unknown keys.
- **Server**: `validate_definition` cases (missing config, wrong context mode,
  row-condition rejection, duplicate action ids across table/card/file_upload);
  `locate_report_workflow_action` finds the upload action (mirror the existing
  test at `services/reports.rs:10734`); corpus fixture with a `file_upload`
  block (`tests/fixtures/reports` + `reports_corpus.rs` /
  `reports_runtime_corpus.rs`).
- **Frontend mocked e2e** (`frontend/e2e/tests/mocked/reports/`): render, drop a
  file, assert the execute request carries base64 `trigger.value` and the
  Idempotency-Key header; button vs automatic mode; error + retry path.
- **Live e2e-verify** (required before done): isolated server per
  `reference_e2e_server_isolation`; workflow with a `file`-typed input that
  decodes the upload (e.g. compression or text agent) and writes an object
  instance; report with the block + a table over that object; POST the execute
  route with a real base64 payload and assert (a) instance completes, (b) the
  returned in-place render shows the new row.

## Out of scope (possible follow-ups)

- **Multiple files** — config `multiple`; either an array under `inputKey` or
  one execution per file. Deferred to keep v1's run/result UX simple.
- **Files > 50 MB** — would need presigned direct-to-S3 upload against the
  tenant's default file-storage connection (`services/file_storage.rs`) and a
  by-reference file shape; today everything is inline base64.
- **Upload progress bar** — base64 + single POST gives no meaningful progress;
  only worth it with the presigned path.
- **Row-condition visibility** — rejected in v1 (no row context); could later
  support filter-context conditions if a need appears.

## Sequencing

1. DSL + server (types, lint, validation, renderer, action lookup, OpenAPI, MCP
   authoring schema) — server-side complete and testable via MCP/HTTP alone.
2. Frontend renderer + wizard editor (after client regen).
3. Tests + live e2e-verify.
