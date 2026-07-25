# Step Editor: Why Authoring Happens in MCP, and What to Fix

**Scope:** live testing against a running server (:7001, tenant `org_p0IkAFnrVqVOvQw9`, workflows *Price List Import* and *Demo AI*) plus 7 parallel code audits of `WorkflowEditor/NodeForm`, `NodeConfigDialog`, `TimelineView`, the canvas shell and the shared condition/mapping components, adversarially verified. Refuted and overstated findings have been dropped or re-rated. I independently re-verified the load-bearing claims behind every Wave 1 item.

**RC0 below was found only by live testing** — it is invisible to static reading because it depends on the data a real workflow carries. The static sweep did not find it, and it is the single highest value-per-effort fix in this document.

---

## Diagnosis: six root causes

### RC0 — The good form exists and is silently bypassed for a large class of steps · **live-proven**

The backend already serves everything a rich form needs. `GET /api/runtime/agents/object-model` returns, per input: `displayName` ("Conflict Columns"), a prose `description`, a real `type` (`boolean`, `array` with `items.type`, `object`), `required`, and an `example`. The frontend fetches all 27 agents' full detail on page load. The step editor then throws it all away for any step whose stored `agentId` is snake_case.

A/B on one real workflow (*Price List Import*), same form, two adjacent steps:

| | `csv / from-csv` | `object_model / bulk-create-instances` |
|---|---|---|
| Type column | `string`, `boolean` | `auto` for every field |
| Field label | "CSV Data", "Trim Whitespace" | `conflict_columns`, `nullify_empty_strings` |
| Description | ⓘ tooltip per field | none |
| Required | `*` marker | none |
| Boolean input | checkbox | text box containing `true` |
| Array input | — | **text box containing `[ "sku"]`** |
| Unmapped optional fields | "Add optional parameter (6 available)" | absent — 3 declared fields unreachable |
| Header | "CSV → from-csv" | "object_model → bulk-create-instances" |

**Mechanism.** The stored graph carries `agentId: "object_model"`; the catalog serves `object-model`. The backend folds these everywhere — `canonical_agent_id` (`crates/runtara-dsl/src/agent_meta.rs:1838`), whose doc comment explicitly says *"legacy `object_model` resolves to…"* — so the runtime, the compiler and MCP all accept snake ids. The frontend never received that fold. Nine bare `===` lookups:

```
NodeForm/InputMappingField/index.tsx:190, :504
NodeForm/shared.ts:158, :188
NodeForm/StepOutputPanel.tsx:107
NodeForm/TestAgentButton/TestAgentInline.tsx:79
NodeForm/StepPickerModal.tsx:289
NodeForm/NameField/index.tsx:236
CustomNodes/BasicNode.tsx:113 + NameField/index.tsx:177-180 (`.toLowerCase()` — does not help snake vs kebab)
```

The lookup misses → `hasEnhancedMetadata` is false (`index.tsx:506-509`) → `filteredInputs = []` (`:516`) → `SimpleInputMappingEditor` receives zero field definitions → every row is treated as a custom field: `typeHint: 'auto'`, raw wire name, no description, no required marker, no enum, no optional-field discovery.

**It compounds downstream.** `shared.ts:188` is the *output suggestion* builder and has the identical bug, so no downstream step can discover `steps.insert.outputs.created_count` in the reference picker. That reference in the live workflow had to be hand-authored. `StepOutputPanel.tsx:107` means the Output panel is empty for the same steps.

**This closes a self-reinforcing loop:** author via MCP → MCP writes snake ids → the UI cannot render those steps → stay on MCP.

**Aggravating factor — schema drives create, not edit.** `InputMappingField/index.tsx:238` gates schema seeding on `!isEdit`. Even for a correctly-matched agent, the schema is consulted when a step is *created*; editing an existing step is structurally the weaker path. Any step authored outside the UI is therefore permanently second-class.

**Fix:** port `canonical_agent_id` to TypeScript (lowercase, `_`→`-`) as `shared/utils/agent-id.ts` and use it at all nine sites; normalise on write in `add_agent_step`/`patch_step`; add a one-shot migration folding existing graphs. Effort **S**. See 1.0.

### RC1 — The form is a lossy round-trip, and every loss is silent

This is the reason a person who has used MCP once never comes back. The editor does not merely fail to express things; it *degrades what is already there*, with no warning and no undo.

- **Opening an MCP-authored step corrupts it.** An immediate array (`{"valueType":"immediate","value":[1,0,0]}` — the shape the API produces) is invisible: `SimpleInputMappingEditor.tsx:249-268` does `JSON.parse(String([1,0,0]))`, which throws, so the row reads "Click to configure…" and the editor reads "No items in array". The first click on **Add Item** replaces the real value (`CompositeArrayEditor.tsx:145-157`). If any key is `null`, `CompositeValueItem.tsx:191-197` dereferences it during render and — with no error boundary below the router (`router/index.tsx:166`) — **the whole editor route unmounts**.
- **Every mode toggle destroys the value.** `MappingValueInput.tsx:209-225` calls `onChange('')` on all four transitions, and it writes straight through to the store. One stray click on a 36px unlabeled icon deletes a *saved* reference path.
- **Changing a condition operator deletes its operands.** `condition-editor.tsx:1017-1048` rebuilds `newArgs` purely from the new operator's arity and never reads the existing `args`. EQ→NE wipes both sides; AND→OR discards every nested sub-condition.
- **Re-picking the capability you already have wipes every mapping.** `NameField/index.tsx:219-220` calls `setValue('inputMapping', [])` with no equality guard, and `CapabilityPickerModal` never marks the currently-selected capability (it accepts `currentCapabilityId` at `:37` and never reads it), so a user clicking the subtitle *to read what capability this step uses* can lose ten minutes of work.
- **Closing the dialog discards everything.** `NodeConfigDialog/index.tsx:151` hands `onOpenChange` straight to Radix with no `onEscapeKeyDown`/`onPointerDownOutside`, and `hideCloseButton` is never passed anywhere in `src/`. Esc, backdrop-click, X, and clicking another timeline row are four one-click total-loss paths. The staging buffer is dead code: `stagedDataRef` is written at `:70/:80/:101/:125` and **read nowhere**, and `onStagedChange` is gated on `if (isCreate)` (`:103`).
- **Touching a reference fallback downgrades its type.** `MappingValueInput.tsx:277-295` passes `e.target.value` through as a string; `CustomNodes/utils.tsx:843-846` assigns it with no coercion. An MCP-authored `default: 0` becomes the string `"0"`.
- **Auto-typed composite values cannot express a decimal or a leading zero.** `CompositeValueItem.tsx:277-292` (auto is the default, `:263-265`) coerces on every keystroke in a controlled input: typing `1.5` yields `15`, `007` yields `7`.

Undo does not exist to rescue any of this: `workflowStore.ts:2037/2048/2105/2110` define `undo`/`redo`/`canUndo`/`canRedo` and **grep returns zero callers** in `src/`.

### RC2 — The editor is a validity gate, not an authoring tool. MCP was given a door the UI never got.

The single most damning artifact in this audit is a backend comment. `crates/runtara-server/src/api/services/workflows.rs:834-838`, inside the `PUT /versions/{version}/graph` handler:

> *"Only validate DSL structure (deserialization). Skip workflow validation (reachability, connection checks) — the graph is built incrementally via atomic mutations, so intermediate states will have unreachable steps. Full validation happens at compile time."*

A validation-skipping write path was built **specifically so an incremental authoring client could exist**. `mcp/tools/graph_mutations.rs:196-290` routes every MCP mutation through it. The frontend does not have the route at all — I grepped the generated client; `PUT .../graph` is absent.

The UI does the opposite, and does it at *step* granularity: `node-edit-rust-validation.ts:316-338` composes the **entire graph** and returns `canApply: status !== 'invalid'`, which `WorkflowEditor/index.tsx:1701-1707` turns into a blocked save. E022 `MissingRequiredInput` is a hard error (`validation.rs:3243`) — so **one half-configured step anywhere in the graph blocks Save on every other step's dialog**. Combined with RC1's discard-on-close, that means: you cannot author top-down, you cannot fix step 12 while step 30 is unfinished, and the work you did in the meantime is gone.

And when validation *is* the blocker, the user often cannot tell:

- **Invalid JSON makes Save a literal no-op.** `NodeFormItem.tsx:888-926` refines `inputMapping` items to reject unparseable JSON at path `inputMapping.<i>.value`. `NextForm/index.tsx:35` is `form.handleSubmit(onSubmit)` with **no `onInvalid`** — I grepped: the string `onInvalid` appears nowhere in the frontend. `handleSave` calls `requestSubmit()` and discards the outcome. `SimpleInputMappingEditor` — the component that renders the rows — contains zero references to `formState`/`errors`/`fieldState`. Click Save: nothing happens, nothing anywhere says why.
- **When it does report, it reports one error, in a 4-second toast, naming a UUID** (`index.tsx:1700-1706`, `errors[0]`; `sonner.tsx` sets no duration).
- **The panel holding the full list is rendered under the modal's blur.** `validationStore.ts:138-142` dutifully expands the Problems panel on error; `ValidationPanel/index.tsx:68-77` is an in-flow div with no z-index, and `dialog.tsx:15-26` puts the overlay and content at `z-50`.

### RC3 — Real dead ends that leave hand-written DSL JSON as the only path

Not friction — impossibility. Each of these has a task a competent author will attempt in their first week:

| Task | What the UI does |
|---|---|
| Emit a per-branch payload from a Switch | The dialog **instructs you to type the wire format**: `SwitchCasesField/index.tsx:913` — *"for a dynamic value use a `{"valueType": "reference", "value": "path.to.value"}` object."* The Output cell is a bare `<Input>` running `JSON.parse` per keystroke with a silent string fallback (`:723-741`). No `MappingValueInput`, no `ReferencePill`, no picker — verified by import list. |
| Set an `any`-typed input to an array or scalar | `isUntypedField` folds `any` into the object-only editor (`SimpleInputMappingEditor.tsx:81-85, :228`), which offers only `reference \\| build` (`ObjectMappingEditor.tsx:29-32`), seeds `{}`, and passes `showModeSwitcher={false}` (`:183`) so the root can never become an array. **73 `any` inputs across 110 of 305 capabilities** — including required ones (`openai:create-embedding.input`, every HubSpot `filter_groups`/`sorts`/`properties`). |
| Give a custom parameter an object value | Composite mode renders a green *"Composite object — configure below"* banner (`MappingValueInput.tsx:319-331`) **with nothing below it** — `CustomFieldRow.tsx:216-232` has no expansion sibling. Picking "Array" displays "JSON Object" (duplicate `value: 'json'`, `:27-28`). |
| Edit a 40-line AI system prompt | A 36px single-line box. `MappingValueInput.tsx:413-430` returns `<Input type="text">` for `textarea \\| json \\| object \\| array`; **the file imports no `Textarea` at all** (verified). |
| Define an object/array workflow variable | Impossible by typing. `VariablesEditor.tsx:55-73` returns `{}`/`[]` on parse failure and `:191-199` re-displays it in a controlled input — type `{`, the box snaps back to `{}`, forever. |
| Use STARTS_WITH / ENDS_WITH in a condition | Declared at `condition-editor.tsx:67-68`, handled by the renderer at `:733-736`, and **absent from all four hardcoded `OPERATORS.filter` allowlists** (`:1093, :1103, :1113, :1125`). An MCP-authored `STARTS_WITH` renders as a blank "Op" placeholder — and the first touch of that dropdown wipes both operands (RC1). |

And there is **no escape hatch on the primary surface**. The good one already exists — `MappingObjectField.tsx:248-270`, a structured editor with a scoped "Edit as JSON" toggle that round-trips losslessly and degrades gracefully — wired only to Log/Error/WaitForSignal/compensation. `SimpleInputMappingEditor` has nothing. There is no step-level JSON view either; the only whole-shape escape is exporting the entire workflow and re-importing it.

### RC4 — The editor sees declarations, never data

MCP gives you `trace_reference`, `inspect_step`, `why_execution_failed`, `test_capability`, `preflight_compile`. The editor's equivalent loop is save → deploy → execute → read summaries.

- **Nothing in the authoring context comes from a run.** `NodeFormProvider.tsx:112-132, :296-331` is built from the agent catalog, step types, workflow list and the composed graph. `getStepSummaries` has exactly four import sites, none in `NodeForm`. *(Honest caveat: the History bottom panel is on the same page and does print real per-step inputs/outputs — `HistoryPanelContent.tsx:422-475` — and Agent steps have a working Testing tab. The gap is that this data is never brought **into the field**, not that it is unreachable.)*
- **The template "preview" is fabricated.** `template-preview-utils.ts:30-73` infers rendered values from substrings of the variable *name* (`total` → `99.99`); `:129-132` deletes `{% if %}`/`{% endif %}` without evaluating; `:124` renders an unknown path as `[some.path]`, indistinguishable from success. It sits under a green header claiming *"Jinja2-style template with syntax highlighting"* on a raw `<textarea>` (`TemplateEditorModal.tsx:373-378, :506-513`). An unbalanced `{% if %}` previews clean and fails at runtime.
- **Reference validation is off exactly where mistakes are most expensive.** `validateReferencePath` has two call sites in the entire codebase (`MappingValueInput.tsx:195`, `CompositeValueItem.tsx:216`). Conditions get none — `condition-editor.tsx` renders every path `border-success` with no error variant (`:551-589`). Agent/AiAgent/EmbedWorkflow outputs get none either: their shape is `Dynamic` (`step_output_shape.rs:190-209`) so `validateStepReferencePath` returns `null` (`reference-type.ts:466`) — **even though 240 of 305 capabilities declare their output fields** and `resolveReferenceType` already matches against them. A correct path gets a type badge; a typo gets silence.
- **Inside an onError branch, the picker actively steers you wrong.** `findPreviousSteps` (`shared.ts:474-508`) never reads the edge `label`, so the step that *failed* is offered as a normal upstream step with its full output list — every field of which is null at runtime. Meanwhile `steps.__error.*` appears in no suggestion group at all.
- **13 of 14 step types have no test affordance.** `NodeFormItem.tsx:132-140` returns `null` for `FormTabs` unless `capabilityId` is set.
- **Pasting a path from an MCP transcript is a hard dead end.** `filterSuggestions` (`VariableSuggestions.tsx:352-370`) matches `label` and `description` but never `suggestion.value`; when the pasted path exactly equals a real suggestion, `VariablePickerModal.tsx:203-206` *also* suppresses the free-text row. Result: "No variables found", with no selectable row.

### RC5 — One-at-a-time everywhere; MCP is batch and structural

- **74 of 305 capabilities open to an empty form.** `SimpleInputMappingEditor.tsx:496-526` splits on `f.required` alone; everything else lives behind a `max-h-64` dropdown that closes on every pick (`:1024-1064`). `shopify:query-products` has 30 optional inputs and zero required — five filters is five modal round-trips through a scrolling menu. `http:http-request` declares `method`, `body`, `body_type` optional, so "POST a JSON body" is three round-trips before you type a character. One `set_mapping` object literal in MCP.
- **No duplicate, no copy, no multi-select.** `selectedNodes` exists in the store (`workflowStore.ts:253, :390`) and nothing reads or sets it. The only keyboard handler in the editor is single-node delete.
- **A step cannot be moved into or out of a Split/While.** `parentId` is derived once at creation and never rewritten; there is no `onNodeDragStop`; `moveTimelineItem` (`workflowStore.ts:1316-1340`) early-returns unless both nodes are in the same list. The workaround (delete + recreate) passes through a graph state RC2 refuses to save.
- **"Where is this used?" has no answer.** No `find_references` counterpart anywhere in `src/` (verified). No workflow-wide search. Answering it means N dialog round-trips through a modal that occludes the canvas.
- **Publish-as-agent and `set_workflow_slug` are MCP-only** — `grep -rn 'publish' src/features/workflows` returns nothing. Any composed workflow forces the user into MCP, and once there they stay.
- **No version diff**, while every UI Save mints a new version (`pages/Workflow/index.tsx:1282-1560` → `POST /update`) versus MCP patching one version in place.

---

## What is already good — do not regress it

The editor is not badly built. It is *unevenly* built: nearly every pattern these fixes need **already exists in this codebase**, correctly implemented, on one or two surfaces, and was never backfilled to the rest. That is why Wave 1 is mostly wiring.

- **All 14 DSL step types reach a purpose-built editor.** No step falls through to a generic key/value dump. `AiAgentStepField` (live model discovery from the connection, structured-output schema editor, tools merged from edges, memory + compaction) is genuinely strong; `ErrorStepField` and `WaitForSignalStepField` are thoughtful.
- **`ConditionEditor` is a real recursive builder** — nesting, typed immediates, per-argument mode selection, scope-aware picker, readable expression preview. Its problems are two specific defects, not the architecture.
- **`MappingObjectField.tsx` is the editor the product needs**: structured rows, inline key rename with duplicate detection, per-row type hint, a lossless "Edit as JSON" toggle, and graceful degradation when a value isn't representable (`:151-160`). Same for `CompensationField` (`NodeFormItem.tsx:308-556`), which renders the caught `SyntaxError.message` in a `FieldError` at `:541-551` — the single best JSON hatch in the repo.
- **Step rename beats MCP.** `workflowStore.ts:720` re-points edges, child `parentId`s and rewrites every `steps.<oldId>` reference across the graph. MCP has no rename tool at all.
- **`reference-type.ts` is disciplined** — it never guesses a type from path substrings and returns `undefined` for genuinely dynamic shapes. `utils/container-scope.ts` correctly models the subtle Split-rebinds / While-passes-through rule. `utils/step-output-shapes.ts` pulls control-step shapes from the canonical Rust table rather than hand-copying.
- **Client validation is real backend parity**, not a reimplementation (`runtara-validation-wasm/src/lib.rs:441` calls the same `validate_workflow`). The problem is how the result is used, not what it checks.
- **Field count is NOT the problem.** Median capability has 3 inputs; only 3 of 305 exceed 12. The scroll burden is the always-open advanced block, the non-collapsible composite editors and the empty-form-plus-dropdown pattern.

**Claims we are deliberately not acting on** (audited, then downgraded or refuted on verification): the editor does *not* have "zero access to run data" — the History panel is on the same page and Agent steps have a working Testing tab; the variable picker *is* keyboard-reachable (every row is a real `<button>`, and the free-text row renders first); modal-vs-sheet is a design preference argued as a defect; array-element index paths affect 4 of 305 capabilities; the `[item].` prefix in `StepOutputPanel` is a deliberate not-a-path marker, not mis-teaching.

---

## The plan

Effort: **S** ≤1 day · **M** 1–3 days · **L** ~1 week · **XL** multi-week.

---

## Wave 1 — Stop the bleeding (~2 sprints)

**Thesis:** the owner does not avoid the UI because it lacks features. He avoids it because it *loses work, refuses to save, and gives no reason*. Wave 1 is the minimum that makes the form trustworthy enough to open. Nothing here changes the authoring model; almost all of it is wiring components that already exist.

### 1.0 — Fold `agentId` canonically in the frontend · **S** · *do this first*
New `shared/utils/agentId.ts` exporting `canonicalAgentId(id) = id.toLowerCase().replace(/_/g, '-')` — mirroring `agent_meta.rs:1838`. Apply at the nine lookup sites listed under RC0, comparing `canonicalAgentId(a.id) === canonicalAgentId(agentId)`. Separately, normalise `agentId` on write in the MCP `add_agent_step`/`patch_step` handlers, and add a migration folding stored graphs.
**Why:** this single mismatch turns the good schema-driven form into the raw `auto`/JSON table for every step using a multi-word agent (`object_model`, `ai_tools`, `s3_storage`, `azure_blob_storage`), and simultaneously empties the output picker for everything downstream of them. It is the concrete mechanism behind "it has direct JSON inputs".
**Done:** opening the `object_model / bulk-create-instances` step in *Price List Import* shows "Conflict Columns" / "Nullify Empty Strings" with types, descriptions, a checkbox for the boolean, and "Add optional parameter (3 available)"; the Finish step's picker offers `created_count` and `skipped_count`.

### 1.0b — Consult the capability schema when editing, not only when creating · **S**
`InputMappingField/index.tsx:236-240` — drop `!isEdit` from `shouldAutoPopulate`, or (safer) keep seeding create-only but always pass the schema through for *rendering*, which `:516` already does once RC0 is fixed. Verify no duplicate rows appear for steps whose mappings already exist.
**Why:** editing is the dominant lifecycle action and is currently the degraded path by construction.
**Done:** an existing step shows the same labels, types and optional-field affordance as a freshly created one.

### 1.1 — Make Save either work or say why · **S**
`shared/components/NextForm/index.tsx:35` — pass an `onInvalid` to `form.handleSubmit(onSubmit, onInvalid)` that toasts the first failing field path and calls `setFocus`. Then thread the RHF `inputMapping` error array from `InputMappingField/index.tsx` into `SimpleInputMappingEditor`'s `FieldRow` and render a `FieldError` under the offending value cell. Change the refine message at `NodeFormItem.tsx:888-926` from `'Invalid JSON format'` to the caught `SyntaxError.message` (it carries a character position), copying `CompensationField`'s pattern at `:541-551`.
**Why:** today, malformed JSON makes the Save button do *nothing at all*. `onInvalid` appears zero times in the frontend. This one change also repairs silent no-ops on `name`, `executionTimeout`, `maxRetries`.
**Done:** typing `{"a":1,}` into a `headers` field and pressing Save produces a red message under that row naming the position, and the row scrolls into view.

### 1.2 — Stop blocking a step's save on unrelated graph errors · **M**
`WorkflowEditor/node-edit-rust-validation.ts:316-338` — split the result into `stepErrors` (target resolves to this node; `applyTargetFallback` at `:337` already stamps `stepId`) and `graphErrors`. `WorkflowEditor/index.tsx:1701` gates only on `stepErrors`; `graphErrors` go to the Problems panel non-blockingly.
**Why:** one half-configured Agent step anywhere (E022) currently blocks Save on *every other step's* dialog. This is the everyday case, not an edge case.
**Done:** with step A missing a required input, editing and saving step B succeeds; A's error still shows in Problems.

### 1.3 — Dirty guard on dialog close · **M**
`NodeConfigDialog/index.tsx:151` — thread `formState.isDirty` out of `NodeForm` (it already owns `entireForm` at `NodeForm/index.tsx:42-46`) and intercept `onOpenChange(false)` with an AlertDialog. Mirror it in `TimelineNodeConfigPanel`, whose `handleEdit` on another row (`TimelineView.tsx:2353`) remounts and drops state the same way.
**Why:** four one-click paths currently discard an entire configuration session with no prompt. The Zustand store is no safety net — `nodeFormStore.ts:231-238` wipes and refills from `node.data` on every mount.
**Done:** Esc/backdrop/X/row-switch with unsaved edits prompts "Discard changes to this step?".

### 1.4 — Stop destroying data on click (bundle) · **S each**
Four independent two-to-ten-line changes with the same shape:
- `MappingValueInput.tsx:210-225` — add `coerceValueForMode(value, from, to)`: preserve the string across immediate↔template↔reference, seed `{}`/`[]` only when entering composite from a non-object. Apply the same at `ObjectMappingEditor.tsx:89-97`, `ArrayMappingEditor.tsx:103-111`, `CompositeValueItem.tsx:230-253`, `MappingObjectField.tsx:365-371`. Also make `handleRemoveReference` (`:234-237`) keep `valueType: 'reference'`, matching `CompositeValueItem.tsx:501`.
- `NameField/index.tsx:209-221` — early-return when `newAgentId === agentId && newCapabilityId === capabilityId`; on a genuine change, gate the mapping reset behind an AlertDialog. Render the check mark in `CapabilityPickerModal` using the `currentCapabilityId` prop it already accepts and ignores (`:37`).
- `condition-editor.tsx:1017-1048` — carry `args` across arity-compatible operator changes; preserve nested Conditions unconditionally for AND/OR/NOT.
- `MappingValueInput.tsx:282-286` — run the fallback value through `coerceValueToType` using the row's existing `typeHint`.

**Done:** no single click anywhere in the form clears a value the user did not select for clearing.

### 1.5 — Display MCP-authored values faithfully; stop crashing on null · **M**
`SimpleInputMappingEditor.tsx:249-268, :907-960` — stop deriving composite mode from `valueType !== 'reference'`; render an immediate array/object **as itself** (pretty-printed JSON, editable, round-tripping the immediate shape — do *not* silently lift it to composite, which would rewrite the author's DSL on next save). Null-guard `value?.valueType` throughout `CompositeValueItem.tsx:191-197`.
**Why:** this is the exact mechanism by which "I opened my workflow in the UI and it broke". Today a `null` key takes down the whole editor route.
**Done:** opening a step with `{"valueType":"immediate","value":[1,0,0,0]}` shows `[1,0,0,0]`; saving without touching it produces a byte-identical graph.

### 1.6 — "Edit as JSON" on the primary mapping editor · **M**
Add `MappingObjectField`'s toggle (`:248-270`) to `SimpleInputMappingEditor`, bound to the whole `inputMapping`, reusing `formatMappingObjectJson`/`parseMappingObjectJson` (`mapping-entries.ts:186-207`) — which already round-trip losslessly and already force JSON view when a value isn't structurally representable.
**Why:** this is the universal escape valve. Whenever the structured editor is wrong or lossy, the recovery is 100 lines away in the same directory and is not wired up. It also makes paste-from-MCP work.
**Done:** any step's mapping can be viewed and edited as JSON and round-trips identically.

### 1.7 — Long-form fields get long-form editors · **S**
`MappingValueInput.tsx:413-430` — split the branch: autosizing `<Textarea>` (min 3 rows) for `textarea`, JSON-aware multi-line for `json`/`object`/`array`. Drop the `h-9` clamp at `:478`. Show the maximize button for every long-form type, not only name-matched ones (`:150-167`).
**Why:** AI system prompts and SQL currently render in a 36px single-line box — the file does not import `Textarea` at all.
**Done:** an AI System Prompt shows ≥3 lines inline and expands.

### 1.8 — Unblock `any`-typed roots and custom-field composites · **M**
Drop `showModeSwitcher={false}` at `ObjectMappingEditor.tsx:183` / `ArrayMappingEditor.tsx:203` so an array root is reachable. Wrap `CustomFieldRow` (`SimpleInputMappingEditor.tsx:1005-1017`) in the same expansion `<TableRow>` schema fields get at `:932-985`. Fix `CustomFieldRow.tsx:27-28` / `AddCustomFieldDialog.tsx:31-32` — `{value:'json', label:'JSON Object'}` and `{value:'array', label:'Array'}` (two characters; today both are `'json'` so "Array" relabels itself).
**Why:** `hubspot:search-contacts.properties = ["email","firstname"]` is currently unexpressible. 110 of 305 capabilities have an object/array/any input.
**Done:** an `any` field can be set to an array literal; a custom `headers` object can be built without typing JSON.

### 1.9 — Show all errors, in the dialog, named · **M**
New `NodeFormValidationSummary` in `NodeConfigDialog`'s footer, subscribed to `useValidationStore` filtered by `stepId === nodeId` — the messages are already all pushed to the store at `WorkflowEditor/index.tsx:1694-1697`; only the toast shows one. Substitute step names for UUIDs (`applyTargetFallback` already resolves `stepName`). Debounce a background revalidation on form change.
**Why:** the full list exists and is rendered behind a `z-50` blurred overlay. Effective error budget while editing today: one toast line.
**Done:** saving a step with three problems shows three named, persistent messages inside the dialog.

**What Wave 1 is not:** none of this makes the UI *better* than MCP. It makes it non-hostile. Expect the owner to try it again, not to switch.

---

## Wave 2 — Change the authoring model (~1 quarter)

This is where the UI becomes preferable for the common case.

### 2.1 — Incremental save · **L**
Regenerate the API client to include `PUT /api/runtime/workflows/{id}/versions/{version}/graph` (absent today — verified), add `patchVersionGraph` to `features/workflows/queries`, and in `pages/Workflow/index.tsx:1501-1512` stop hard-returning on `status === 'invalid'`: save via PUT, mark the version draft, route errors to Problems. Keep the hard block on Compile/Deploy only. Patch the current draft version in place so a session of edits is one version row, not ten.
**Why:** this is RC2. The backend built this door for MCP and never opened it for the UI. **Done:** you can save a workflow with an unreachable step and come back to it; ten edits produce one version.

### 2.2 — Structured Switch outputs · **L**
Replace the Output `<Input>` at `SwitchCasesField/index.tsx:723-741` and the Default Output at `:820-838` with `MappingValueInput` + `ObjectMappingEditor`, copying `FinishStepField.tsx:413-487`. Delete the raw-JSON edit dialog and the instruction string at `:913`. Give `range` match patterns two numeric inputs instead of `{"gte":100,"lt":500}` (`:490-521`).
**Done:** the sentence telling users to type `{"valueType":"reference",...}` no longer exists in the product.

### 2.3 — Bring run data into the field · **L**
Add a `lastRun` slice to `NodeFormProvider` from `getStepSummaries` (the query shape is proven in `HistoryPanelContent`). Render a muted "last run → …" line under each `ReferencePill` (`MappingValueInput.tsx:249-296`) and as a third line on each Step Outputs picker row. Add a `StepLastRunPanel` sibling to `StepOutputPanel` showing resolved inputs/outputs per mapping key (port `resolve_input_mappings` from `executions.rs`). Un-gate `DebugStepInspector` from `isSuspended` (`pages/Workflow/index.tsx:2220`).
**Done:** you can see what a reference actually resolved to without leaving the field.

### 2.4 — Reference validation everywhere · **M**
(a) In `validateStepReferencePath` (`reference-type.ts:411-467`), when the shape is `dynamic` and the step has a statically-derived field list, check the first segment against it — **240 of 305 capabilities declare output fields**, so this fires for the large majority of Agent steps. Guard it to skip `any`/fieldless-object parents so opaque payloads still pass. (b) Add a `validateReference?: (path) => string \\| null` prop to `condition-editor.tsx` and pass it from all five call sites (`FilterStepField`, `WhileStepField`, `InputMappingField`, `TimelineView`, `ConditionalNode`); give `ReferencePill` (`:551-589`) a destructive variant. (c) Add `suggestion.value` to `filterSuggestions` (`VariableSuggestions.tsx:362-369`) so a pasted MCP path resolves.
**Done:** `steps['fetch'].outputs.statuscode` is red at author time; a pasted path selects the real suggestion.

### 2.5 — onError scope correctness · **M**
Expose `isInsideErrorScope` on `NodeFormContext` (the `label === 'onError'` is already stamped on every composed plan edge, `CustomNodes/utils.tsx:314-317`). Add an "Error Context" suggestion group with the six envelope fields (`message`, `stepId`, `code`, `category`, `severity`, `attributes`). Exclude or annotate steps reachable only via an onError edge in `findPreviousSteps` (`shared.ts:474-508`).
**Done:** `steps.__error.message` is one click; the failed step's outputs are marked "unavailable in this branch".

### 2.6 — Progressive disclosure and defaults · **M**
Collapse `StepAdvancedFields` (`NodeFormItem.tsx:188`) into the `Collapsible` pattern `StepOutputPanel.tsx:323-346` already uses; nest `CompensationField` one level deeper (the code itself warns it is W070 warning-only at `:388-391`); move Breakpoint to the canvas context menu. Auto-promote optional fields that declare an `enum` or a `default` into `visibleFields` (`SimpleInputMappingEditor.tsx:521-526`) — that alone surfaces `method`, `body_type`, `response_type` for HTTP — and make the add-dropdown multi-select. Give `inputMapping` a real "Inputs" heading (`NodeFormItem.tsx:787` currently sets `label: ''`, rendering a 16px empty spacer).
**Done:** a POST with a JSON body needs zero dropdown round-trips; an Agent form reads as Inputs / Advanced / Output.

### 2.7 — Fix the remaining silent corruptions · **S–M**
Auto-mode retyping (`CompositeValueItem.tsx:277-292`) → parse on blur, keep the typed string. `VariablesEditor.tsx:191-199` → per-row draft string, or reuse `IterationVariablesField.tsx:358-378`'s composite editor. `MappingObjectField` JSON textarea → local draft + rendered `SyntaxError` instead of a silent `delete filteredRestData.context` (`CustomNodes/utils.tsx:1020-1054`). `coerceValueToType` (`:529-542`) → handle `json`, or remove the no-op "JSON" type hint from `CompositeValueItem.tsx:152`. Force JSON mode with a banner when `parseConditionValue` returns `undefined` (`condition-editor.tsx:775-796`) instead of rendering a blank builder — port the toggle from `TimelineView.tsx:2027-2050`.

### 2.8 — Composite editor completeness · **M**
Inline key rename (reuse `MappingObjectField.tsx:409-433`), array item reorder/duplicate, and delete the divergent `CompositeObjectEditorInline`/`CompositeArrayEditorInline` copies (`CompositeValueItem.tsx:712-878`) in favour of recursion. Fix the inline composite editor that cannot be closed (`SimpleInputMappingEditor.tsx:935` — the second disjunct isn't gated on `isEditingThisObject`, so the X is a no-op).

### 2.9 — Step-level "Edit step JSON" · **S**
A collapsible raw-`data` textarea in `StepAdvancedFields`, validated on blur by the WASM parser, applied via `updateNode` (which merges, `workflowStore.ts:315-320`, so unknown fields survive). This is the generic answer to every future "the form doesn't model field X" — `connectionRef` is the live example.

---

## Wave 3 — Reach parity on the things people open MCP *for*

- **`find_references` in the UI** · **M** — a `ReferenceUsagePanel` beside `StepOutputPanel` doing the same traversal as `collect_reference_locations`, plus "used by 3 steps" on output rows and a Cmd-F workflow search. Prerequisite for safely renaming or deleting anything.
- **Duplicate step + multi-select** · **M** — `duplicateNode` in `workflowStore`, Cmd-D, wire `onSelectionChange` to the `selectedNodes` state that already exists and is never used.
- **Reparent into/out of containers** · **L** — `reparentNode` + `onNodeDragStop` hit-test + a "Move to…" action. Depends on 2.1 (the intermediate state is currently unsavable).
- **Publish-as-agent + slug** · **M** — a section in `ValidationPanel/SettingsContent.tsx`. Today this alone forces every composition author into MCP.
- **`connectionRef`** · **M** — make the connection row a `MappingValueInput` with literal/reference modes; add `connectionRef` to the zod schema (`NodeFormItem.tsx:838`) so it stops being silently dropped, and show "connection from `data.crm`" instead of a stale literal.
- **Version diff** · **M** — structural diff over two `executionGraph`s in `VersionsPanelContent`, click-through to the changed node.
- **Real template rendering** · **L** — replace `renderTemplatePreview` with an actual minijinja render. Note the obvious route does not work: `runtara-validation-wasm` depends on `runtara-dsl` + `runtara-workflows`, **not** `runtara-workflow-stdlib` where `template.rs` lives. Either depend on minijinja directly or hoist `template.rs` into `runtara-dsl` first. Interim: flag unbalanced `{% %}`, mark unresolvable paths red, and relabel the green "Preview" header to "Approximation — not evaluated".
- **Discovery: search + categories** · **M** — token-AND search over agent + capability + `tags` + `integrationIds` (today "post to slack" returns nothing: whole-phrase substring, `StepPickerModal.tsx:214-239`), and delete the `getAgentCategory` stub that returns `'Other'` for all 27 agents (`CapabilityPickerModal.tsx:56-59`), deriving from `integrationIds` against the existing connection taxonomy.
- **Connection-resource pickers beyond AI models** · **M**; **"+ New connection" in the connection picker** · **M** (today a first-run Slack step dead-ends inside the dialog).
- **Test affordances beyond Agent** · **L–XL** — condition preview via the already-shipped `evaluateCanonicalCondition` (`rust-validation-wasm.ts:128-138`, currently used only by the reports feature); seed `TestAgentInline` from the step's real mapping instead of a hand-retyped copy (`TestAgentInline.tsx:234-239` passes no `initialData` and sets `hideReferenceToggle`).

---

## Wave 4 — Cosmetic. Bundle opportunistically; do not schedule.

Section headings and the duplicated dialog title; `AutocompleteInput` in template mode (it exists and is wired into exactly one file-upload field); arrow-key/Enter in the variable picker; consolidating the three divergent `ReferencePill`s; surfacing the 53 declared `example` values that are parsed and discarded; capability *display name* in the step header instead of the raw id; the phantom "modified" badge (`SimpleInputMappingEditor.tsx:667-677` — mostly evaporates once 1.3 lands); replacing the Split parallelism copy that says the feature does nothing (`SplitStepField.tsx:761-763, :836-842`) — wasip3 landed, and W073 says concurrency *does* apply to single-Agent bodies; deleting the `console.log`s that fire on every render (`CompositeObjectEditor.tsx:65,74,80,88,99,175,213`); the `SplitStepField.tsx:352-738` duplication of `IterationVariablesField` that means every iteration-variable fix must be made twice.

---

## Live session notes (2026-07-25, v8.6.3 @ 5a38dde7)

Observed directly in the browser against :7001; recorded here because some are interaction defects that static reading would not surface.

> **Retracted on re-test (2026-07-26).** Two items originally listed here — "the first Edit click after a page load is swallowed" and "the AI Agent step config cannot be opened at all" — were **wrong**. Both were artifacts of the probe reading the DOM before React had rendered, compounded by Edit behaving as a toggle. On careful re-test with a settle delay, a single click opens every step editor reliably, including AI Agent (which renders LLM Connection, System Prompt, User Prompt, Model, tools and memory as designed). Do not spend time chasing either. The AI Agent observations that *did* hold are kept below.

- **The Timeline and Canvas surfaces have drifted.** Canvas renders the AI Agent node well (model badge "GPT-4.1m", TOOLS/MEMORY slots); Timeline renders the same step's subtitle as a raw connection UUID (`04d24747-5f6d-…`).
- **The AI Agent editor contains zero `<textarea>` elements.** Measured live: the System Prompt ("You are a helpful assistant…") and User Prompt are both 32px single-line `<input>`s. This is Wave 1 item 1.7, confirmed rather than inferred.
- **The custom-parameter type selector shows "JSON ObjectArray"** as its selected value — live confirmation of the duplicate `value: 'json'` in `CustomFieldRow.tsx:27-28` / `AddCustomFieldDialog.tsx:31-32` (Radix renders the text of every item matching the selected value). Wave 1 item 1.8.
- **A false-positive type warning fires on a correct mapping.** With the schema lost (RC0), the amber "Reference is array; this field expects **auto**" appears on a working `instances` mapping. Warnings that fire on correct work train users to ignore all warnings — gate `referenceTypeMismatch` on a known target type.
- **Two controls advertise their own non-functionality inside the form:** Timeout ("runtime validation currently reports it as warning-only") and Compensation ("reports compensation as warning-only (W070) — it is not enforced at runtime"). Ship them working, hide them, or move them behind Advanced (see 2.6) — do not render an internal caveat as field help.
- **Truncation makes labels unreadable.** The Finish step's Type column renders the select as a single character (`s ⌄`); reference pills truncate mid-word (`Upsert price rows → created_c…`).
- **The Finish step's output schema is type-lossy** — `errors` (array), `imported` (integer) and `skipped` (integer) all display as `STRING`.
- **The Testing tab does not test the step you configured.** It renders an independent, empty parameter set that neither inherits the Main tab's values nor accepts references, so "Run Test" cannot exercise the step as authored (`TestAgentInline.tsx:234-239` passes no `initialData` and sets `hideReferenceToggle`). Covered in Wave 3; consider promoting.
- The step/capability picker is decent — real search box, agent descriptions, drill-down — but see Wave 3 on token-AND search ("post to slack" currently returns nothing).

---

## Bottom line

The step editor's problem is not missing design — it is **unfinished backfill**. A structured value editor, a labelled mode selector, a lossless JSON escape hatch with legible parse errors, a JSON-vs-visual toggle, conservative reference validation, progressive disclosure: all six exist, correct, in this repo, on one or two surfaces each. They were added surface by surface as features shipped and never propagated to the four or five surfaces left behind — which happen to include the most-used one.

Wave 1 is eleven items, nine of them S/M, and it is mostly wiring. Item 1.0 alone — a five-line string normalisation applied at nine call sites — restores labels, types, descriptions, required markers, boolean checkboxes, optional-field discovery and downstream output suggestions for every step built on a multi-word agent. It will not make the UI better than MCP. It will make it stop losing work, stop refusing to save, and stop staying silent about why — which is the actual precondition for anyone choosing it.