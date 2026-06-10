# Timeline UI Parity Gaps

Audit date: 2026-06-10
Last updated: 2026-06-10

This document tracks parity between the workflow DSL/platform and the current timeline workflow editor. The legacy canvas editor is out of scope except where shared conversion code is used by the timeline.

## Scope

- Timeline view: `crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/TimelineView.tsx`
- Timeline inline forms: `crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/NodeForm`
- Shared graph conversion: `crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/CustomNodes/utils.tsx`
- Workflow settings/sidebar: `crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/EditorSidebar` and `crates/runtara-server/frontend/src/features/workflows/components/ValidationPanel`
- DSL source of truth: `crates/runtara-dsl/src/schema_types.rs`
- Runtime validation and behavior: `crates/runtara-workflows/src/validation.rs` and direct-wasm compiler/runtime modules

## Effort Scale

- S: focused frontend or metadata change, about 1-2 days including tests.
- M: touches multiple frontend modules or frontend plus light backend/DSL metadata, about 3-5 days including tests.
- L: requires new editing model, serializer changes, backend/API coordination, or larger UX design, about 1-2 weeks.
- XL: broad parity work with nested graph editing, migration concerns, and substantial test coverage, more than 2 weeks.

## Implementation Status

As of 2026-06-10, the audited timeline parity gaps are closed for DSL editability. Some complex fields use advanced JSON editors rather than dedicated visual builders; those are now considered UX polish, not parity blockers.

Implemented areas include graph/root metadata, wrapper import/export metadata, variable descriptions, entry point selection, edge condition/priority authoring, Delay authoring, Split advanced config, common breakpoint/durable controls, Agent/EmbedWorkflow retry and timeout fields with warning copy, Agent compensation JSON, AiAgent retry config, Log/Error context, WaitForSignal action and `onWait`, richer schema fields, and Split/Filter/GroupBy source `MappingValue` modes.

## Findings

### 1. Edge conditions and priorities needed timeline authoring

Status: Implemented.

Description:
The DSL supports `executionPlan[].condition` and `executionPlan[].priority`. Timeline routes now preserve and edit these fields for non-Conditional routes, including branch lanes, sequential transitions, and `onError` fanout. Conditional true/false branches intentionally do not expose edge-level condition/priority because the platform ignores those fields for `Conditional` outgoing edges.

Proposed change:
No remaining parity change. Future polish can replace JSON condition editing with a richer condition builder while keeping imported DSL lossless.

Complexity/effort: Implemented, L.

### 2. Delay step is supported by DSL/runtime but not by timeline creation/editing

Status: Implemented.

Description:
`Delay` is registered in step metadata, appears in timeline creation/editing, has a duration editor, and round-trips `durationMs`, `breakpoint`, and `durable`.

Proposed change:
No remaining parity change.

Complexity/effort: Implemented, M.

### 3. Workflow/root metadata was not fully editable

Status: Implemented.

Description:
Timeline settings now edit and preserve name, description, variables with descriptions, input/output schema, execution timeout, rate-limit budget, graph `durable`, `entryPoint`, `memoryTier`, and `trackEvents`. Wrapper-level `durable` import/export is mapped to graph durability because the update API persists the execution graph contract.

Proposed change:
No remaining parity change unless the backend update API adds a separate top-level workflow `durable` field distinct from `executionGraph.durable`.

Complexity/effort: Implemented, M.

### 4. WaitForSignal `onWait` and `action` needed advanced editing

Status: Implemented.

Description:
`action.key`, `action.correlation`, `action.context`, `timeoutMs`, `pollIntervalMs`, `responseSchema`, and `onWait` are editable and round-tripped. `onWait` is edited as advanced JSON rather than as a nested visual timeline.

Proposed change:
No remaining parity change. Optional UX polish: add a nested visual timeline editor for `onWait`.

Complexity/effort: Implemented, L.

### 5. Split advanced config was lossy

Status: Implemented.

Description:
Split now edits and preserves `value`, `parallelism`, `sequential`, `dontStopOnFailed`, `variables`, `maxRetries`, `retryDelay`, `timeout`, `allowNull`, `convertSingleValue`, and `batchSize`. The UI includes runtime warning copy where runtime behavior is warning-only.

Proposed change:
No remaining parity change.

Complexity/effort: Implemented, M.

### 6. Step-level breakpoint and durable controls were missing

Status: Implemented.

Description:
Common advanced controls expose `breakpoint` where supported and `durable` for Agent, Split, EmbedWorkflow, Delay, and AiAgent.

Proposed change:
No remaining parity change.

Complexity/effort: Implemented, M.

### 7. AiAgent retry settings were not round-tripped

Status: Implemented.

Description:
AiAgent now edits and preserves `maxRetries` and `retryDelay` in config.

Proposed change:
No remaining parity change.

Complexity/effort: Implemented, S to M.

### 8. Log and Error context mappings were missing

Status: Implemented.

Description:
Log and Error forms now expose context editors and the converter preserves context mapping values.

Proposed change:
No remaining parity change.

Complexity/effort: Implemented, S.

### 9. Schema field editing was narrower than the DSL

Status: Implemented for parity.

Description:
The shared schema parser/builder now preserves richer DSL fields including `default`, `example`, `items`, mixed JSON `enum` values, display metadata, validation metadata, nested `properties`, `visibleWhen`, and unknown extensions. `SchemaFieldsEditor` exposes common fields directly and an advanced JSON dialog for the remaining DSL fields. Split item/output schemas, AiAgent output schema, WaitForSignal response schema, and workflow input/output schemas now use or preserve the richer model.

Proposed change:
No remaining parity change. Optional UX polish: replace the advanced JSON dialog with dedicated controls for `items`, nested `properties`, `visibleWhen`, and validation metadata.

Complexity/effort: Implemented, L.

### 10. Variable descriptions were visible but dropped on load/save

Status: Implemented.

Description:
Variable descriptions now load, edit, save, export, and import.

Proposed change:
No remaining parity change.

Complexity/effort: Implemented, S.

### 11. Agent and EmbedWorkflow retry/timeout/compensation fields were unclear in UI

Status: Implemented.

Description:
Agent and EmbedWorkflow expose retry count, retry delay, and timeout. Timeout is labeled as warning-only where runtime validation reports it is not enforced. Agent compensation is editable as advanced JSON and is also labeled warning-only because the runtime accepts but does not enforce it.

Proposed change:
No remaining parity change. Optional UX polish: add a structured compensation form if compensation becomes enforced.

Complexity/effort: Implemented, M.

### 12. Import/export lost workflow metadata

Status: Implemented.

Description:
Import/export now use the same metadata contract as save for name, description, variables, schemas, execution timeout, rate-limit budget, graph durable, wrapper durable, entry point, memory tier, and track events.

Proposed change:
No remaining parity change.

Complexity/effort: Implemented, S to M.

### 13. Entry point was recomputed instead of preserved

Status: Implemented.

Description:
The composer preserves a valid imported/staged `entryPoint`, settings expose an Entry Point selector, Auto clears the explicit entry point, and the virtual start indicator now prefers the explicit entry point before falling back to topology.

Proposed change:
No remaining parity change.

Complexity/effort: Implemented, M.

### 14. Static HTTP step metadata fallback was stale

Status: Implemented.

Description:
The stale `/api/runtime/metadata/workflow/step-types` endpoint now derives from the same registry-backed metadata used by runtime step metadata paths.

Proposed change:
No remaining parity change.

Complexity/effort: Implemented, S to M.

### 15. Some config values used restricted reference-only UI despite DSL MappingValue support

Status: Implemented.

Description:
Split, Filter, and GroupBy source fields now use a shared source `MappingValue` editor with quick array-source suggestions plus immediate, reference, template, composite, and null-capable authoring. The serializer uses the generic mapping conversion path and preserves reference `type/default`, template values, composite arrays/objects, and immediate arrays.

Proposed change:
No remaining parity change.

Complexity/effort: Implemented, M.

## Recommended Follow-Up

No audited DSL parity blockers remain. Recommended follow-up is UX polish:

1. Replace advanced JSON areas with dedicated visual editors for nested `WaitForSignal.onWait`, schema `items/properties/visibleWhen`, and route conditions.
2. Add broader browser-driven coverage for the timeline settings popovers and advanced schema dialog.
3. Revisit Agent compensation UX if runtime enforcement is added later.
