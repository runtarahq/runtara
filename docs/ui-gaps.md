# Timeline UI Parity Gaps

Audit date: 2026-06-10
Second audit (Claude, multi-agent): 2026-06-10 — see "Additional Findings" below
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

As of 2026-06-10, findings 1-15 below are closed for DSL editability (finding 4 has one residual hole, see finding 25). Some complex fields use advanced JSON editors rather than dedicated visual builders; those are now considered UX polish, not parity blockers.

A second, independent multi-agent audit (138 raw findings, re-verified against the tree after findings 1-15 were implemented) confirmed the implementation of findings 1-15 and identified additional gaps not covered by the first audit, tracked as findings 16-28 below.

As of the end of 2026-06-10, findings 16-28 are implemented (each in its own commit with tests; see the Resolution notes per finding). The only items still open are listed under Recommended Follow-Up.

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

Status: Implemented (the cannot-clear residual was closed via finding 25).

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

## Additional Findings (Second Audit, 2026-06-10)

These gaps were found by an independent multi-agent audit and re-verified against the tree after findings 1-15 were implemented. All file references are to the current tree. Statuses: Open / Partially fixed.

### 16. Condition-builder arguments are rewritten lossily on every save

Status: Implemented. Severity: high.

Resolution (2026-06-10): `convertConditionArguments` is now lossless and idempotent — reference arguments keep `type`/`default`, template and composite arguments pass through untouched, already-typed JSON immediates (boolean/number/array/null) are never stringified, and the editor's explicit immediate-type selector wins over inference (so `Boolean = true` saves a real boolean that matches at runtime). On the save path (`normalizeConditionExpression`, applied to Conditional/While/Filter), string IN/NOT_IN right-hand sides are parsed into real JSON arrays (JSON array syntax or comma-separated), turning previously dead conditions into working ones; the live editor path keeps strings so typing isn't disrupted, and the editor renders stored arrays/booleans/numbers correctly. Covered by new tests in `shared/utils/condition-type-conversion.test.ts`.

Description:
`normalizeConditionExpression` -> `convertConditionArguments` runs on every save for Conditional, While, and Filter conditions — including untouched steps (`CustomNodes/utils.tsx:532-543`, While `:1380-1382`, Filter `:1357-1364`, Conditional `:1066-1071`). The conversion (`shared/utils/condition-type-conversion.ts`):
- strips `type` and `default` from reference arguments (`:255-257`) although the runtime honors both (`direct_json.rs` `apply_reference`);
- rewrites template and composite `MappingValue` arguments to dead `immediate` literals (`:217-227`, `:259-267`) although the runtime evaluates all four kinds in condition args;
- ignores the form's immediate-type selector: under EQ/NE/CONTAINS, booleans and numbers are stringified (`:127-151`). The runtime's `values_equal` has no bool<->string coercion, so `Boolean = true` saved as `"true"` silently never matches;
- leaves IN/NOT_IN right-hand sides as strings (`:133-137`) while the runtime requires a real JSON array (`direct_json.rs:4158-4174`), so a typed `US, CA, MX` list yields a condition that is always false (IN) or always true (NOT_IN) with no warning. The condition editor offers no array/JSON immediate input (`condition-editor.tsx:676-764`).

Proposed change:
Stop renormalizing untouched conditions; preserve reference `type`/`default` and template/composite arguments through the conversion; honor the immediate-type selector when serializing; add an array/JSON immediate mode for IN/NOT_IN/CONTAINS (or comma-split like SwitchCasesField does); add a validator warning for string RHS under IN/NOT_IN.

Complexity/effort: M.

### 17. Condition-builder reference picker cannot author common legal references

Status: Implemented (picker). The `ConditionExpression::Value` variant (bare truthy condition) remains open as a medium-severity residual. Severity: high.

Resolution (2026-06-10): the condition variable picker now offers per-field `workflow.inputs.data.<field>` suggestions from the workflow input schema, a Variables group (built-in `variables._workflow_id`/`_instance_id`/`_tenant_id` plus user-declared constants as `workflow.inputs.variables.<name>`), and a free-text "use as custom reference path" row that commits whatever path is typed in the search box — so any legal reference form is authorable even when not suggested. Wired through Conditional, While, and Filter condition editors from NodeFormContext.

Description:
The condition editor's reference picker has no free-text path entry (`condition-editor.tsx:633-673` renders a pill that reopens the picker) and its suggestions omit per-field `data.<field>` references and the entire `variables.*` group (`condition-editor.tsx:287-390` — only whole-object `workflow.inputs.data`/`workflow.inputs.variables`, `item.*` guesses, `loop.*` inside While, and step outputs). Conditions like `data.country == "US"` or `variables.retryCount < 3` cannot be authored visually. `steps.__error.*` references are only partially reachable. `ConditionExpression::Value` (bare truthy check) can be neither created nor rendered — a While loop using it shows an empty builder.

Proposed change:
Add a free-text/expression entry mode to the reference pill, per-field `data.*` suggestions, and a Variables group (reuse `NodeForm/InputMappingValueField/VariableSuggestions.tsx` which already does both); render and allow creating the `value` condition form.

Complexity/effort: M.

### 18. Log/Error message value-mode selectors are no-ops masquerading as data binding

Status: Implemented. Severity: high.

Resolution (2026-06-10): `Log.message`, `Error.code`, and `Error.message` are now plain text inputs (no reference/template mode toggle), always saved as immediate values, with helper copy stating the text is emitted verbatim and pointing at the Context editor for dynamic values. Previously-saved literal path text is unaffected (the modes never resolved anything).

Description:
`Log.message`, `Error.code`, and `Error.message` are verbatim `String` fields in the DSL — no interpolation or reference resolution (`schema_types.rs:751-753`; runtime reads them with `as_str`). The forms render them with `MappingValueInput`, whose mode toggle offers reference and template modes; the serializer then stores the raw text. A user who "binds" an error message to `steps.x.outputs.reason` ships the literal path text to production logs. Dynamic data belongs in `context` (now editable per finding 8).

Proposed change:
Render message/code as plain text inputs (no mode toggle), with helper copy pointing at `context` — or add real interpolation to the DSL and runtime first.

Complexity/effort: S (UI-only fix).

### 19. While/Split forms misrepresent runtime semantics

Status: Implemented. Severity: high.

Resolution (2026-06-10): While timeout is now labeled "Timeout (milliseconds)" with example copy, matching the DSL/runtime unit (values continue to round-trip raw, so previously stored ms values now display correctly). The "Set to 0 for unlimited" hint is replaced with accurate exit semantics, the input floor is 1, and an inline warning explains that 0 means the body never runs when a stored 0 is loaded. The Split "Sequential Execution" description now states iterations always run sequentially in the current runtime and the flag is informational.

Description:
- While timeout: the form labels the field "Timeout (seconds)" and round-trips the raw number, but `WhileConfig.timeout` is milliseconds and is enforced as a ms deadline (`schema_types.rs:683-685`, `direct_wasm/compile/while_loop.rs`). Entering `60` produces a 60 ms loop budget that fails almost immediately; a stored `5000` displays as "5000 seconds".
- While maxIterations: help text says "Set to 0 for unlimited", but the compiled loop exits when `index >= maxIterations`, so 0 means the body never runs (`while_loop.rs`; the default-10 fallback applies only when the field is absent). No validation flags 0.
- Split "Sequential Execution" switch implies parallel execution exists when off; the DSL documents `sequential` as redundant ("sequential execution is the only mode the WASM runtime supports", `schema_types.rs:2034-2039`) and no validator warning covers it.

Proposed change:
Fix the While timeout label/conversion (label ms, or convert seconds<->ms consistently both directions); remove the "0 for unlimited" hint and warn on 0; replace the Split sequential switch with copy stating iterations run sequentially (keep `parallelism`'s W073 warning copy pattern).

Complexity/effort: S.

### 20. Split/While subgraph-level ExecutionGraph fields are dropped on save

Status: Implemented. Severity: high.

Resolution (2026-06-10): on load, the converter now captures all subgraph-level fields except `steps`/`executionPlan` (variables, input/output schema, name, description, notes, entryPoint, ...) into a UI-only `subgraphMeta` carrier on the container node; on save, the rebuilt subgraph is seeded from that carrier before child steps/edges are re-derived, and the carrier itself is stripped from the serialized step. Locked in by round-trip tests in `CustomNodes/utils.test.ts`.

Description:
`composeExecutionGraph` rebuilds Split/While subgraphs fresh from child nodes; subgraph-level `variables`, `inputSchema`, `outputSchema`, `name`, `description`, and `notes` authored via API/JSON are not carried through the rebuild and are silently destroyed by any UI save. (Split's `config`-level fields were fixed by finding 5; this is the nested `subgraph` ExecutionGraph itself.)

Proposed change:
Preserve unmodeled subgraph-level fields when rebuilding (spread the original subgraph object and overwrite only steps/executionPlan/entryPoint), mirroring the restData pass-through used for steps.

Complexity/effort: S to M.

### 21. onError routing coverage is narrower than the compiler

Status: Implemented. Severity: high.

Resolution (2026-06-10): `canStepHaveErrorHandler` is now a pure allowlist mirroring the compiler's `on_error_route_shape_supported` (Agent, EmbedWorkflow, Split, While, AiAgent, WaitForSignal) — the knownErrors gate is gone, and step types the compiler rejects (Delay, Log, Filter, GroupBy, ...) no longer offer the action either. AiAgent onError edges are creatable from the timeline and no longer hidden (timeline, canvas, and auto-layout all reclassify `onError` away from tool/memory attachments; AiAgentNode gained an `onError` handle; the loaded tools list and the step editor's tool list exclude the error route). MCP toolset edges are authorable via an "Add MCP toolset" popover creating `mcp.<toolset>` edges to an `mcp`-agent step, validating exactly the server's rules (non-empty suffix, unique per AiAgent — no charset restriction, matching `validation.rs:4776`). Covered by new `TimelineView.test.ts` (9 tests) and round-trip tests in `CustomNodes/utils.test.ts`.

Description:
- Agent steps: the timeline offers the error-handler add action only when the capability metadata has `knownErrors`; the DSL/compiler accept `onError` edges on every Agent step regardless (`utils/step-error-support.ts` gate vs `validation.rs`).
- AiAgent steps: onError is both un-addable (add action gated `!isAiAgentStep`) and invisible — `getHiddenNodeIds` hides targets of all AiAgent edges with non-`source` handles, so an existing AiAgent onError handler disappears from the timeline entirely, even though chat-turn/memory-provider failures route through it (`TimelineView.tsx`).
- MCP toolset edges (reserved label `mcp.<toolset>` emitting `<toolset>_search`/`_invoke` synthetic tools) cannot be created; only plain tool/memory edges are offered.
- The onError add action is offered on some step types the compiler does not support onError for (mismatch in the other direction).

Proposed change:
Drop the knownErrors gate; allow + render AiAgent onError edges (exclude only reserved tool/memory/next handles from hiding); add an MCP toolset edge affordance; align the offered step-type set with the compiler's supported set.

Complexity/effort: M.

### 22. Mapping editors: residual reference/immediate fidelity holes

Status: Implemented. Severity: high (first three), medium/low (rest).

Residuals resolution (2026-06-10, second pass): empty-string immediates now survive saves — rows auto-seeded by schema population carry an editor-only `autoSeeded` marker and only those are dropped when blank, so JSON-authored and user-typed `""` values persist (Log/Error keep blank-equals-cleared semantics for their direct fields). Composite editors emit and render real nested template values. Split `config.variables` route through the shared mapping serialization (template mode legal in the form schema, typed immediates coerce, no illegal type hints emitted). Dotted mapping keys validate per segment (they build nested objects at runtime). The error-condition templates emit canonical `steps.__error.*` (W053-clean). Finish blocks duplicate output names with per-row errors, and object/array immediates JSON-parse at the save boundary so they land as real JSON.

Resolution of the high items + default editor (2026-06-10):
- The mapping variable picker gained a free-text "use as custom reference path" row (any legal path is authorable: deep `data.*`, `steps.__error.*`, indices, schema-unknown outputs) and now renders the previously-computed-but-never-displayed Loop Context section.
- Steps inside a Split body get a Split Scope suggestion group (`variables._item`, `_index`, `_loop`, `_loop_indices` — list verified against `SPLIT_SCOPE_VARIABLES`, `validation.rs:1817`; `_loop` documented as While-inherited), driven by a new `isInsideSplit` context flag.
- `ReferenceValue.default` is now editable (a "Fallback value" input in reference mode) AND survives form saves: the node-form zod schema, the simple-editor entry rebuilds, and the EmbedWorkflow/Agent initialData conversions all preserve `defaultValue` — previously merely opening and saving a step's form silently deleted JSON-authored fallbacks.
- Finish schema-bound output rows derive their reference type hint from the schema field's declared type instead of hardcoded `'string'` (which silently stringified numbers/booleans at runtime via `apply_type_hint`); stale `'string'` hints on schema-bound rows are re-derived on load, and unknown types omit the hint rather than writing an illegal one.
Covered by new tests: `mapping-entries.test.ts`, `finish-type-hints.test.ts`, `VariableSuggestions.test.ts`, plus zod/nodeFormStore assertions.

Description:
- Reference pickers in mapping editors have no free-text path mode; arbitrary-but-legal paths (deep `data.*` chains, `steps.<id>.outputs.<deep>` not present in schemas) require picking from suggestions only.
- Split iteration scope (`variables._item`/`_index`, implicit `data.*` = current item) is not authorable from pickers inside Split bodies.
- `ReferenceValue.default` (fallback when a path resolves null/missing) has no editor anywhere in the generic mapping UI; on the EmbedWorkflow form-save path it is dropped from edited entries.
- Schema-bound Finish outputs lock the reference type hint to `string`.
- Empty-string immediates in Agent/EmbedWorkflow input mappings are dropped on save (cannot pass `""` to an input).
- Composite editors silently treat a nested template option as immediate in some paths; Split `config.variables` lacks template mode and typed immediates.
- Dotted mapping keys (`payload.address.city` building nested objects) are rejected by UI validation but legal in the DSL; templates emit the deprecated bare `__error.*` form (W053) instead of `steps.__error.*`.
- Finish output table allows duplicate output names (last wins silently) and saves object/array immediates as strings.

Proposed change:
Add free-text reference entry + a default-fallback input to `MappingValueInput`; surface `_item`/`_index` suggestions inside Split scope; fix the empty-string and dropped-default serializer branches; align dotted-key validation with the DSL.

Complexity/effort: M to L.

### 23. Switch residuals

Status: Implemented. Severity: medium.

Resolution (2026-06-10): the Switch "Value to Switch On" now uses the shared SourceMappingValueField (reference type hints, default fallbacks, and composite values all author and round-trip via the generic mapping serialization path). `config.default` is no longer fabricated: load pushes the entry only when authored, new nodes don't seed it, and the form shows "No default — execution fails when no case matches" with explicit Add/Remove Default controls. The dead moustache autocomplete was removed from value/output fields (the runtime resolves only reference/immediate objects there), and a Switch-specific zod superRefine blocks saving without a value (serde would reject `config` lacking `value`). Round-trip tests added in `CustomNodes/utils.test.ts`.

Description:
`config.value`'s reference type hint is locked to `string` with no UI to change it (`SwitchCasesField/index.tsx:192-193`) and its `default` fallback is dropped on save; a Switch authored with no `default` output gets `default: {}` injected on every save (changes no-match behavior from error to empty output); moustache `{{...}}` autocomplete suggests syntax that is dead in Switch outputs; the form does not enforce the DSL-required `config.value`; composite values dead-end.

Proposed change:
Reuse the generic mapping serialization path (finding 15) for `config.value`; only emit `default` when authored; remove moustache suggestions from Switch outputs; require value before save.

Complexity/effort: S to M.

### 24. EmbedWorkflow residuals

Status: Partially implemented — the high-severity version pinning item is fixed; the low items (child schema metadata in the mapping editor, optional step name) remain open. Severity: high (version), low (rest).

Resolution of the version item (2026-06-10): pinned versions round-trip from the DSL as integers (`ChildVersion::Specific`), but the node form's hidden `childVersion` field declared `z.string()`, so loading a pinned step made Save silently no-op ("Expected string, received number" on a field that renders no error). The schema now coerces to string, the selector normalizes numeric values, and a pin that is not in the fetched version list renders as an explicit "(not in version list)" option instead of a blank select.

Description:
Pinning `childVersion` to a specific integer (`ChildVersion::Specific`) is rejected by the form while the DSL/validator accept it (the selector offers only Latest/Current/listed versions, and free numeric entry fails form validation); child inputSchema field metadata (enum/default/etc.) only partially drives the embed mapping editor; the form requires a step name though the DSL allows unnamed EmbedWorkflow steps.

Proposed change:
Allow free numeric version entry (validated against existing versions with a warning, not a block); pass full child SchemaField metadata into the mapping editor; make name optional.

Complexity/effort: S.

### 25. AiAgent/WaitForSignal lifecycle residuals

Status: Implemented. Severity: medium.

Resolution (2026-06-10): the WaitForSignal serializer now explicitly deletes `responseSchema`/`timeoutMs`/`pollIntervalMs`/`action`/`onWait` when cleared in the form, killing the stale-value resurrection (this also closes the finding-4 residual). `timeoutMs` is restricted to immediate/reference modes — verified against the runtime: `wait_timeout_ms` evaluates the MappingValue generically but requires a Number, and templates always render strings, so template/composite unconditionally failed; legacy template strings can no longer emit serde-invalid JSON. pollIntervalMs enforces integers (u64). AiAgent memory gained a "Remove memory" control (clears the mapping entries, removes the hidden provider node + memory edge; the rebuilt config provably omits `config.memory`). Both add-memory flows (canvas and timeline) stop silently writing `strategy: summarize` — unset now means the DSL default SlidingWindow, which the form also displays. The tool-name charset matches the server exactly (`/^[\p{Alphabetic}\p{N}_]+$/u`). 7 new round-trip tests.

Description:
- AiAgent memory: once configured, memory cannot be disabled/removed from the form (no clear path); compaction strategy default in the UI diverges from the DSL default (SlidingWindow).
- WaitForSignal: `action`, `timeoutMs`, `pollIntervalMs`, and `responseSchema` cannot be cleared once set — the serializer only overwrites on non-empty values and resurrects stale values from node data (`CustomNodes/utils.tsx:1618-1679`), unlike Log/Error context which got an explicit delete path. `timeoutMs` as template/composite is over-rejected by the form; decimal pollIntervalMs values pass the form but fail serde (`Option<u64>`).
- AiAgent tool-edge label charset validation in the UI is stricter than the server's.

Proposed change:
Add explicit clear/delete paths in the WaitForSignal and AiAgent memory serializers (mirror the Log/Error `delete` pattern); align compaction default and label charset with the DSL.

Complexity/effort: S to M.

### 26. Timeline cannot author parallel fan-out, joins, or direct edge mutations

Status: Implemented (cross-branch step moves remain canvas-only). Severity: medium.

Resolution (2026-06-10): the timeline gained three topology affordances plus a visual condition editor: (1) "Add parallel branch" on steps with a single unconditional outgoing edge S→T creates the diamond S→N, N→T — verified E073-compliant by construction (`parallel_branches_reconverge` needs one step reachable from every branch start; T qualifies); (2) "Connect to step" on lane-end steps creates just an edge to a picked target (same scope, cycle-checked via `wouldCreateLoop`, hidden/connected/self targets excluded); (3) "Delete route" in the route-settings popover removes the edge via the same store path the canvas uses, with orphaned branches rendering as new roots (the dangling-step validator warning covers semantics). Edge conditions are now edited with the shared visual ConditionEditor, with an "Advanced (JSON)" fallback so exotic shapes stay authorable. TimelineView.test.ts grew to 21 tests covering offer-gating, target filtering, request shapes, and orphan rendering.

Description:
The compiler supports unconditional fan-out and joins (E073-compliant merging), but the timeline offers no affordance to add a second unconditional outgoing edge or converge two branches — add actions cover only sequential insertion, Conditional/Switch routes, onError, and AI tool/memory edges; `createInsertionRequest` only splices existing edges. Individual edges cannot be deleted/re-pointed in the timeline, and moving a configured step across branches or into/out of a Split/While body is not supported.

Proposed change:
Add "add parallel branch" and "merge into existing step" route actions plus per-edge delete/re-point controls (the route-settings popover from finding 1 is a natural host).

Complexity/effort: L.

### 27. Trigger/platform surfaces missing from the UI

Status: Implemented except two low-value discovery surfaces (a workflow-actions browser and an embed dependency/dependents viewer — both still API-only). Severity: high (cron inputs), medium/low (rest).

Second resolution pass (2026-06-10): HTTP/EMAIL triggers gained a Debug switch and a webhook-verification connection selector (`configuration.connection_id`; filtered to Mailgun — the only integration `webhook_verification.rs` implements — with non-Mailgun values preserved). The custom-cron validator now accepts 6-field expressions with `0` seconds, mirroring the server's `normalize_cron_expression`. TriggersGrid shows a "Last run" column and a copyable "Sync (30s, no history)" endpoint row for HTTP triggers. A Pause button (gated on exactly `running`, per `ExecutionEngine::pause` preconditions) joins Stop/Resume on the instance history page. APPLICATION triggers — verified inert server-side (no event constructor exists) — are no longer offered for new triggers, and existing ones show a warning banner while staying editable.

Resolution of the high items (2026-06-10): the trigger form now has a "Static inputs (JSON)" editor (validated live, must be a JSON object, blocks save while invalid) and a Debug mode switch for CRON triggers, round-tripping `configuration.inputs` and `configuration.debug` (stored as a real boolean, matching the scheduler's `as_bool` read). Edit-save no longer rebuilds trigger configuration from scratch for ANY trigger type: it merges form-managed fields over the loaded trigger's existing configuration, so API-authored keys (HTTP/EMAIL `debug` + webhook-verification `connection_id`, CRON `inputs`, etc.) survive UI edits instead of being wiped. Covered by unit tests in `features/triggers/utils/trigger-configuration.test.ts`.

Description:
- CRON trigger static `inputs` envelope: the scheduler reads `configuration.inputs` per fire (`workers/cron_scheduler.rs:230-236`), but the trigger UI cannot set it and edit-save rebuilds configuration as `{expression}` only, silently wiping API-set inputs. Cron-triggered workflows with required input schemas cannot be configured from the UI.
- HTTP/EMAIL trigger `debug` flag and webhook-signature `connection_id` are honored by the server but the UI saves `configuration: null`, wiping them.
- APPLICATION triggers are configurable in the UI but inert — no server code path constructs an Application trigger event (UI-only feature).
- Cron expression validation mismatch: UI requires exactly 5 fields; server accepts 6-field expressions with `0` seconds.
- No UI consumes: instance pause endpoint, workflow actions list/submit, embed dependency/dependents endpoints, the http-sync invocation URL, or the trigger `lastRun` timestamp (fetched and mapped but never rendered).

Proposed change:
Add an inputs editor (JSON, schema-aware) and debug/verification fields to the trigger form, and stop rebuilding configuration from scratch on edit; hide or implement APPLICATION triggers; accept 6-field cron with zero seconds; surface pause, lastRun, and the sync endpoint URL.

Complexity/effort: M overall.

### 28. Step ids and UI-written extension fields

Status: Implemented. Severity: low.

Step-id rename resolution (2026-06-10): the node form now shows the step id (monospace, with helper copy about reference paths) and allows renaming it with inline validation (`/^[a-zA-Z0-9_-]+$/`, unique across all steps incl. subgraphs, `__error` reserved per `RESERVED_IMPLICIT_STEP_IDS`). `workflowStore.renameStep` atomically renames the node, re-points edges and container `parentId`s, and boundary-safely rewrites every reference form across all node/edge data — `steps.<id>.` dot form, `steps['<id>']`/`steps["<id>"]` bracket forms (both quote styles, per `extract_step_id_from_reference`), and template strings — plus staged changes and editor selection state. 9 new store tests cover re-pointing, all rewrite forms, prefix-boundary safety, and the rejection matrix.

Resolution of the DSL fields (2026-06-10): `Note.metadata` (NoteMetadata width/height), `ExecutionGraph.executionTimeoutSeconds` (Option<u32>, documented as server-enforced at scheduling), and `SchemaField.nullable` (form-layer hint) are now modeled in `schema_types.rs` as optional skip-if-none fields, so typed deserialize→reserialize round-trips no longer drop what the editor writes. Known pre-existing quirk surfaced during this work: the OpenAPI spec registers both `runtara_dsl::Note` and the server DTO `Note` under one component name and the DTO wins, shadowing the DSL Note schema.

Description:
- Step ids are always machine UUIDs — no form shows or edits them, so readable reference paths like `steps.group-by-status.outputs.*` are JSON/API-only (`workflowStore.ts:379`).
- The UI persists fields the DSL does not define: `notes[].metadata.{width,height}` (Note resizing — survives only because the server stores raw JSON), `executionTimeoutSeconds` on the graph (enforced by the server reading raw JSON, invisible to DSL/validators), and `nullable` on schema fields (now directly settable via checkbox). These should be added to the DSL structs so typed round-trips don't drop them.

Proposed change:
Add an optional id (rename with reference-rewrite) affordance; add `metadata` to `Note`, `executionTimeoutSeconds` to `ExecutionGraph`, and `nullable` to `SchemaField` in `schema_types.rs`.

Complexity/effort: S (DSL fields), M (id rename).

## Recommended Follow-Up

Findings 1-28 are implemented. What remains open, in suggested order:

1. Browser/e2e verification pass: all fixes in findings 16-28 are verified by unit/round-trip tests and typecheck only — exercise the new affordances (condition picker custom paths, parallel branch/join/delete-route, AiAgent error routes + memory removal, Switch default controls, CRON inputs, step-id rename) against a running stack.
2. Discovery surfaces still API-only (finding 27): a workflow-actions browser (list/submit open actions) and an embed dependency/dependents viewer.
3. Timeline: moving an existing configured step across branches or into/out of a Split/While body remains canvas-only (finding 26 residue).
4. `ConditionExpression::Value` (bare truthy condition) can be neither created nor rendered by the condition builder (finding 17 residue, medium).
5. UX polish from the first audit: visual editors for `WaitForSignal.onWait` (nested timeline) and schema `items/properties/visibleWhen` (currently advanced-JSON dialogs); revisit Agent compensation UX if runtime enforcement lands.
6. Housekeeping surfaced during the work: the OpenAPI component name collision between `runtara_dsl::Note` and the server DTO `Note` (the DTO shadows the DSL schema in the spec).
