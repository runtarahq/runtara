# Timeline UI Parity Gaps

Audit date: 2026-06-10

This document captures gaps between the workflow DSL/platform and what users can edit through the current timeline workflow editor. The canvas editor is treated as legacy. Shared graph loading/saving code is included only where timeline uses it for persistence.

## Scope

- Timeline view: `crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/TimelineView.tsx`
- Timeline inline forms: `crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/NodeForm`
- Shared graph conversion: `crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/CustomNodes/utils.tsx`
- Workflow settings/sidebar: `crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/EditorSidebar`
- DSL source of truth: `crates/runtara-dsl/src/schema_types.rs`
- Runtime validation and behavior: `crates/runtara-workflows/src/validation.rs` and direct-wasm compiler/runtime modules

## Effort Scale

- S: focused frontend or metadata change, about 1-2 days including tests.
- M: touches multiple frontend modules or frontend plus light backend/DSL metadata, about 3-5 days including tests.
- L: requires new editing model, serializer changes, backend/API coordination, or larger UX design, about 1-2 weeks.
- XL: broad parity work with nested graph editing, migration concerns, and substantial test coverage, more than 2 weeks.

## Implementation Status

Follow-up implementation on 2026-06-10 addressed the highest-risk timeline save/load losses: graph/root metadata, variable descriptions, entry point preservation, edge condition/priority preservation, Delay authoring, Split advanced config, common breakpoint/durable controls, AiAgent retry config, Log/Error context, WaitForSignal action metadata, and richer schema columns.

Second follow-up implementation on 2026-06-10 added timeline route editing for execution-plan edge `condition` and `priority`, added advanced JSON editing for `WaitForSignal.onWait`, and replaced the stale workflow step metadata endpoint with registry-derived metadata. Remaining larger parity work is primarily full visual nested `onWait` graph editing, full advanced schema modeling, more complete MappingValue authoring across specialized source fields, and clearer UX for accepted-but-warning Agent/EmbedWorkflow timeout and compensation fields.

## Findings

### 1. Edge conditions and priorities needed timeline authoring

Status: Implemented for non-Conditional timeline routes on 2026-06-10. Conditions and priorities are preserved in React Flow edge data, exposed through route settings on branch lanes and sequential transitions, and serialized back to `executionPlan`. Conditional true/false branches intentionally do not expose edge-level condition/priority because the platform ignores those fields for `Conditional` step outgoing edges.

Description:
The DSL supports `executionPlan[].condition` and `executionPlan[].priority` for conditional routing, default fallback routing, prioritized edges, and conditional `onError` recovery. The timeline originally exposed only edge labels such as `next`, `true`, `false`, `default`, `onError`, switch route labels, and AI tool labels. Workflows authored through DSL or MCP with edge conditions/priorities therefore needed an explicit timeline authoring surface, not just passive serializer preservation.

Proposed change:
Add a timeline edge/route configuration model that supports optional condition and priority per outgoing route. Persist these fields in React Flow edge data, timeline route state, `executionGraphToReactFlow`, and `composeExecutionGraph`. Add UI affordances for route conditions on normal fanout and `onError` fanout, with validation messages surfaced before save. Add round-trip and store tests using edge-condition and priority fixtures. Future polish can replace the JSON editor with a richer condition builder that remains lossless for imported DSL.

Complexity/effort: L.

### 2. Delay step is supported by DSL/runtime but not by timeline creation/editing

Description:
The DSL and generated API model include `DelayStep` with `durationMs`, `breakpoint`, and `durable`. The direct runtime has delay execution support. However, `Delay` is missing from Rust step metadata registration, so the WASM-backed step picker does not expose it. The timeline NodeForm has no custom Delay field, icon, or duration editor.

Proposed change:
Register `Delay` in step metadata, add a timeline form for `durationMs`, expose an optional durable toggle if durability is part of the intended UI surface, add icon/label support, and add load/save tests for imported Delay workflows.

Complexity/effort: M.

### 3. Workflow/root metadata is not fully editable

Description:
The DSL workflow wrapper supports `memoryTier`, `trackEvents`, and root `durable`; `executionGraph` also supports graph-level `durable`. Timeline settings currently edit name, description, execution timeout, rate-limit budget, variables, input schema, and output schema. Save posts only `{ executionGraph }`, and the graph composer does not include graph-level durable.

Proposed change:
Decide which wrapper fields are version metadata versus graph DSL fields in the UI contract. Add settings controls for supported fields, preserve existing values on load/save, and ensure backend update endpoints can persist them without relying on repository carry-forward behavior. At minimum, preserve graph-level `durable` on round-trip even if it remains hidden.

Complexity/effort: M to L, depending on backend/API changes required for wrapper fields.

### 4. WaitForSignal `onWait` and `action` needed advanced editing

Status: Partially implemented on 2026-06-10. `action.key`, `action.correlation`, `action.context`, and `onWait` are editable from advanced WaitForSignal settings, and round-trip tests cover `action`, absent `pollIntervalMs`, and `onWait` preservation. Full nested timeline editing for the `onWait` graph remains open.

Description:
The DSL supports `WaitForSignal.onWait`, `timeoutMs`, `pollIntervalMs`, `responseSchema`, and `action`. Timeline UI originally edited only response schema, timeout, and poll interval. `onWait` was passively preserved through node data but had no editing surface, and `action` metadata needed explicit advanced controls. There was also a subtle default-materialization risk where missing `pollIntervalMs` could become an explicit default after form edit.

Proposed change:
Add WaitForSignal advanced settings for `action.key`, `action.correlation`, and `action.context`. For `onWait`, either support nested timeline editing for the wait subgraph or preserve/import it as a JSON advanced block until nested editing is available. Add tests for `onWait`, `action`, and absent `pollIntervalMs` round-trip behavior.

Complexity/effort: L.

### 5. Split advanced config is lossy

Description:
The DSL `SplitConfig` includes `value`, `parallelism`, `sequential`, `dontStopOnFailed`, `variables`, `maxRetries`, `retryDelay`, `timeout`, `allowNull`, `convertSingleValue`, and `batchSize`. Timeline exposes source value, item/output schemas, variables, sequential execution, and continue-on-failure. Save rebuilds only `value`, `parallelism`, `sequential`, `dontStopOnFailed`, and `variables`. Runtime currently warns when `parallelism != 1`, so this field needs careful UX treatment.

Proposed change:
Add an advanced Split section for retry count, retry delay, timeout, allow-null behavior, single-value conversion, and batch size. Keep `parallelism` either hidden with preservation or visible with an explicit warning that current runtime executes Split sequentially. Extend serializer/load tests to cover every SplitConfig field.

Complexity/effort: M.

### 6. Step-level breakpoint and durable controls are missing

Description:
Many DSL steps support `breakpoint`; Agent, Split, EmbedWorkflow, Delay, and AiAgent support `durable`. The timeline editor has no consistent control for these fields. Because the NodeForm schema does not include them, editing a node through the form can strip unknown fields even if untouched graph data might otherwise survive.

Proposed change:
Add a common advanced section for fields supported by the selected step type. Include `breakpoint` and `durable` where valid, and make unsupported fields impossible to submit. Update NodeForm schema to include these fields, and add per-step round-trip tests.

Complexity/effort: M.

### 7. AiAgent retry settings are not round-tripped

Description:
The DSL `AiAgentConfig` supports `maxRetries` and `retryDelay`. Timeline UI exposes provider, LLM connection, prompts, model, max iterations, temperature, max tokens, tools, structured output schema, and memory settings. The serializer omits AiAgent retry fields.

Proposed change:
Add retry count and retry delay controls to the AiAgent advanced settings. Parse them from DSL config on load and emit them on save. Add one conversion test for importing an AiAgent with retries and saving without loss.

Complexity/effort: S to M.

### 8. Log and Error context mappings are missing

Description:
The DSL `LogStep` has `context`, and `ErrorStep` has `context`. Timeline forms edit only log level/message and error code/message/category/severity. Existing context mappings authored in DSL can be dropped by the editor.

Proposed change:
Add a mapping editor for `context` on both Log and Error steps. Prefer the existing `MappingValueInput` or generic input-mapping machinery so context can be immediate, reference, template, or composite. Add load/save tests for dynamic error/log context.

Complexity/effort: S.

### 9. Schema field editing is much narrower than the DSL

Description:
DSL `SchemaField` supports type, description, required, default, example, items, enum, label, placeholder, order, format, min, max, pattern, properties, and `visibleWhen`. Timeline schema editing mostly supports name, type, required, description, and sometimes enum. The top-level schema parser/builder can carry several richer fields, but import staging and UI editing strip or hide many of them. Split schema editors are even narrower and omit integer/file and most metadata.

Proposed change:
Introduce an advanced schema field editor that can edit defaults, examples, enum, item schema, nested properties, display metadata, validation metadata, and `visibleWhen`. Preserve unknown schema extensions even before every field has a polished UI. Align top-level, AiAgent output schema, WaitForSignal response schema, and Split schemas on the same schema editing component.

Complexity/effort: L to XL.

### 10. Variable descriptions are visible but dropped on load/save

Description:
The variables editor includes a description column, and the DSL `Variable` includes `description`. However, current workflow load maps variables to `{ name, value, type }`, and save maps them back to `{ type, value }`, dropping descriptions.

Proposed change:
Include `description` in variable load and save paths. Add a small round-trip test covering a variable with description.

Complexity/effort: S.

### 11. Agent and EmbedWorkflow retry/timeout/compensation fields are unclear in UI

Description:
Agent supports `maxRetries`, `retryDelay`, `timeout`, `compensation`, `breakpoint`, and `durable`. EmbedWorkflow supports `maxRetries`, `retryDelay`, `timeout`, `breakpoint`, and `durable`. Some related fields exist in form schema defaults, but there is no clear rendered timeline control for them. Runtime validation currently warns that Agent/EmbedWorkflow timeout is not enforced and compensation is not enforced.

Proposed change:
Expose retry settings where runtime actually honors them, and preserve accepted-but-warning fields when importing. For timeout and compensation, either add read-only/advanced controls with warning copy or keep them hidden while preserving them and showing validation warnings. Avoid presenting unenforced fields as reliable runtime controls.

Complexity/effort: M.

### 12. Import/export loses workflow metadata

Description:
Export composes an execution graph without passing graph `name`, `description`, or `rateLimitBudgetMs`; it wraps name/description outside the graph. Import accepts wrapper or raw graph, but ignores wrapper name/description and rate-limit budget. This creates drift between exported JSON and what a user sees after import/save.

Proposed change:
Make import/export use the same metadata contract as normal save. Include name, description, rate-limit budget, execution timeout, variables, schemas, and any supported root/graph metadata. Add import/export round-trip tests for these fields.

Complexity/effort: S to M.

### 13. Entry point is recomputed instead of preserved

Description:
DSL has explicit `entryPoint`. The graph composer recomputes the entry point from nodes with no incoming edges and chooses the leftmost node when there are multiple candidates. Imported DSL with an explicit entry point can change after timeline save if layout or graph shape creates ambiguity.

Proposed change:
Preserve `entryPoint` from loaded graph when still valid. Add a timeline action to mark a step as the entry point when multiple roots exist. Keep the recomputation logic only as fallback for new graphs or invalid entry points.

Complexity/effort: M.

### 14. Static HTTP step metadata fallback is stale

Status: Implemented on 2026-06-10 for the stale `/api/runtime/metadata/workflow/step-types` endpoint. The main timeline step picker path already used WASM metadata with a registry-backed `/api/runtime/steps` fallback; the separate metadata endpoint now also derives from the DSL registry.

Description:
The frontend prefers WASM step metadata, which comes from Rust step registration. If WASM metadata fails, the step picker uses `/api/runtime/steps`, which is registry-backed. A separate HTTP metadata endpoint was still hardcoded and missing newer steps, creating inconsistent API/platform metadata for tools or future callers.

Proposed change:
Generate the HTTP metadata endpoint from the same Rust step metadata registry used by WASM, or remove the stale hardcoded list. Add a test asserting HTTP and WASM metadata contain the same step IDs.

Complexity/effort: S to M.

### 15. Some config values use restricted reference-only UI despite DSL MappingValue support

Description:
The DSL mapping model supports reference, immediate, composite, and template values. Generic input mapping supports those modes, but Split/Filter/GroupBy source selectors are specialized around array references and custom paths. That means some valid DSL `MappingValue` shapes for `config.value` are difficult or impossible to author in timeline UI.

Proposed change:
Use the shared `MappingValueInput` model for config source values while keeping the quick array-source picker as a convenience path. Ensure serializer preserves immediate, template, and composite values where the runtime accepts them.

Complexity/effort: M.

## Recommended Sequence

1. Finish deeper editors for fields that are now preserved through JSON or partial controls: nested `WaitForSignal.onWait`, full schema metadata, and MappingValue authoring in specialized source fields.
2. Normalize workflow metadata import/export/save behavior and add broader import/export round-trip tests.
3. Clarify Agent/EmbedWorkflow timeout and compensation UX so accepted-but-warning fields are not presented as reliably enforced runtime controls.
4. Add parity fixtures using existing runtime examples for every field the timeline should preserve.
