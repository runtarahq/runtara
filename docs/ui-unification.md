# UI Schema Unification Plan

## Status

**Complete and independently re-verified as of 2026-07-12.** The initiative was
reopened because the earlier completion claim was incorrect. The second pass
treated every checked item as untrusted, fixed NG-1 through NG-6 plus the
additional gaps found by adversarial review, exercised production REST/browser
paths, and converted infrastructure-dependent tests to explicit fail-closed
suites. The evidence and remaining compatibility boundaries are recorded below.

### Resolved gaps

- [x] **NG-1 — unsafe legacy update path:** public update has one mandatory,
  versioned, descriptor-aware patch contract. Legacy `connectionParameters`,
  client-owned status, provider tokens, unknown fields, and access violations
  are rejected.
- [x] **NG-2 — unintended default writes:** edit state tracks stored presence and
  explicit dirty/clear intent. Displayed defaults are never emitted by an
  unrelated save, and reauthorization is derived only from effective operations.
- [x] **NG-3 — conflict recovery:** 409 responses preserve the draft, fetch the
  current projection, identify changed fields, and offer explicit reload or
  reviewed reapply using the latest version.
- [x] **NG-4 — field ordering:** declaration order is preserved through macro
  metadata, normalized forms, serialization, and React; explicit order remains
  available as an override.
- [x] **NG-5 — advanced sections:** descriptor macros author section id, label,
  description, order, and advanced state with contradiction checks. MCP has a
  representative collapsed advanced section.
- [x] **NG-6A — option resolution:** report filters provide the production
  `OptionResolver`, backed by the report filter-options API and React Query.
- [x] **NG-6B — non-secret writes:** typed JSON `write` operations support
  write-only non-secret fields independently from secret storage and never
  project those values back to clients.
- [x] **NG-6C — false-positive tests:** required external suites are feature
  gated, fail closed, run in CI, and report executed/ignored counts explicitly.
- [x] Every earlier completion item, public/internal caller, retained boundary,
  and phase gate was adversarially audited; newly found defects were fixed.

### Verification discipline

Only assertions that executed are counted below. Ignored doctests are listed
separately, and required integration suites report zero skipped tests. Optional
infrastructure is not detected from inside a test anymore: the ordinary test
run omits an explicitly feature-gated target, while the corresponding CI/local
command enables it and fails if its database, Valkey, Docker, tool, or component
prerequisite is unavailable.

### Retained boundaries

The cleanup intentionally keeps only domain wire boundaries that the non-goals
require:

- Workflow input schemas may still arrive as a JSON string, JSON Schema
  `properties` envelope, or legacy array-shaped nested properties. TypeScript
  normalizes only that wire envelope; Rust/WASM owns all field semantics and
  condition normalization.
- Persisted report layout `showWhen` remains a report-owned wire/editor shape.
  Its adapter is lossless, while evaluation uses the shared canonical engine.
- Report queries, layouts, workflow graphs, mappings, references, and execution
  behavior remain owned by their existing DSLs.

These are not duplicate form DSLs or evaluators and are not candidates for
removal without an explicit versioned domain migration.

### Downstream consumer: connection page frame (2026-07-12)

The connection editor's **page frame** — the domain UI around `FormRenderer`
(status card, sticky save bar, danger zone, reconnect handoff, OAuth popup
lifecycle) — was rebuilt on top of the unified engine and shipped in seven
commits (`4e833e8a`..`b785b123`). See
[connection-page-ux-plan.md](connection-page-ux-plan.md) for that plan and its
implementation status. This is the first demanding real consumer of the engine
and it validated the ownership boundary: everything schema-shaped
(fields, controls, conditions, access, secret state, validation) stayed in the
shared engine, while everything connection-specific (authorization lifecycle,
grant health, provider reconnect, deletion) stayed in the page frame. Two
findings that surfaced while building it are now captured for reuse:

- **Controlled reset with `keepDirtyValues`.** The shared `FormRenderer`
  controls its inputs from `useWatch`. With react-hook-form
  `resetOptions: { keepDirtyValues: true }` (required so a background refetch
  never clobbers an in-progress edit), a bare `form.reset()` does **not**
  reliably re-emit to those controlled inputs — a discard or post-save
  re-sync must set each field explicitly. Any page adopting the renderer with
  the values-prop + keep-dirty pattern needs the same explicit re-set.
- **Grant state is connection-domain, not form-engine.** `ConnectionGrantState`
  (`hasAccessToken` / `hasRefreshToken` / `tokenExpiresAt` / `authorizedAt`) was
  added to `ConnectionDto`, not to `FormField`/`FormDefinition` — booleans and
  timestamps only, derived server-side, never token values. This is the
  intended shape for new connection lifecycle metadata: it rides the connection
  DTO alongside `editProjection`/`secretState`, keeping authorization concerns
  out of the shared form model.

### Known vocabulary gaps (open)

The engine is complete for the surface Runtara exposes today, but the connection
page frame wanted three pieces of field/section metadata the canonical model
does not yet express. None block current UI; each is an additive follow-up:

- **Per-field documentation links (`doc_url`).** Absent from `SchemaField` /
  `FormField` and the macro. Provider setup links ("where do I find my Intuit
  keys", the OAuth redirect URI to register) currently have nowhere to live in
  the descriptor. Adding a `doc_url` field attr → `ConnectionFieldMeta` → DTO
  (mirroring the recent section-metadata chain) would unblock them.
- **Authored enum option labels.** `FormOption.label` is produced only by
  `humanize_identifier(value)` (`form/mod.rs`), so an option value cannot carry
  a custom display label. Fine for `sandbox`/`production`; limiting for coded
  values.
- **Unit / suffix hints.** No way to annotate a numeric field with a unit
  (ms, req/s, MB) for display.

One correctness caveat worth tracking: `FormDefinition.allow_unknown_fields`
deserializes to `true` by default (`default_allow_unknown_fields`), so a
hand-authored definition that omits the key silently accepts unknown fields.
Both production producers (connection descriptors, workflow/report normalizers)
set it explicitly to `false`; a hand-written definition should too.

This document records the implemented architecture that unifies schema-driven
form rendering across connections, workflows, and reports without adding a
third-party form or schema framework. Its canonical foundation lives in Rust
and is shared with the browser through WebAssembly.

## Decision summary

Runtara has one shared **form schema and evaluation engine**, composed by the
existing domain DSLs:

- The workflow DSL continues to describe graphs, mappings, references, and
  execution behavior.
- The reports DSL continues to describe queries, datasets, blocks, layout,
  navigation, and interactions.
- Connection descriptors continue to describe connection-specific persistence,
  credentials, health, testing, and authorization behavior.
- The shared form engine describes fields, controls, sections, access,
  conditional state, and validation.

The form engine is implemented in Rust. Backend services call it natively,
while the frontend calls the same implementation through the existing
validation WASM pipeline. React owns rendering and interaction but does not
reimplement schema normalization, conditional evaluation, or validation.

No additional external dependencies are required.

## Goals

1. Establish one field and control vocabulary for connections, workflows, and
   reports.
2. Produce identical backend and frontend validation results from the same Rust
   implementation.
3. Replace connection field-name heuristics with explicit schema metadata.
4. Make connection editing safe for non-secret values, secrets, managed values,
   and authorization-sensitive changes.
5. Reuse existing workflow and report schema infrastructure without changing
   workflow execution semantics or report query/layout semantics.
6. Remove duplicate TypeScript schema parsing, conditional evaluation, and
   validation logic after compatibility gates pass.
7. Keep the shared schema small and composable instead of creating a universal
   UI or product DSL.

## Non-goals

- Rewriting the workflow execution DSL.
- Changing workflow graph, compiler, mapping, reference, template, or runtime
  semantics.
- Replacing the reports query, dataset, block, grid, or interaction DSL.
- Moving connection health, authorization, credential rotation, or persistence
  into the shared form engine.
- Defining a generic remote-query DSL for dynamic field options.
- Building a visual form-schema authoring product as part of this work.
- Adopting JSON Forms, RJSF, Ajv, or another third-party schema-form runtime.

## Existing foundation

The implementation consolidates code that already existed:

- `runtara_dsl::SchemaField` already represents types, requiredness, defaults,
  enums, labels, placeholders, ordering, formats, ranges, patterns, nested
  properties, and legacy `visibleWhen` metadata.
- `runtara-workflows::input_validation` already validates workflow input data in
  both native Rust and browser WASM.
- `runtara-validation-wasm` exposes the shared backend validation logic to the
  browser.
- `runtara-report-dsl` already evaluates `ConditionExpression` in native Rust
  and browser WASM.
- The frontend already fingerprints and builds validation WASM before its main
  build.

The first implementation step is moving the generic pieces to the correct
shared layer, not writing a second engine.

## Ownership boundary

| Shared form engine owns | Domain layers continue to own |
| --- | --- |
| Field types and constraints | Workflow graph and execution |
| Labels, descriptions, and static options | Workflow mappings and references |
| Preferred controls | Report queries, datasets, and aggregation |
| Sections and ordering | Report blocks, layout, and navigation |
| Conditional visibility, enablement, and requiredness | Connection health and testing |
| Form-definition and value validation | Connection authorization lifecycle |
| Read/write access metadata | Secret persistence and rotation operations |
| Structured field errors | Dynamic option retrieval |

## Canonical form model

The canonical types live in `runtara_dsl::form` and build on the existing
`SchemaField` type.

```rust
pub struct FormDefinition {
    pub schema_version: u32,
    pub fields: HashMap<String, FormField>,
    pub sections: Vec<FormSection>,
    pub allow_unknown_fields: bool,
}

pub struct FormField {
    pub schema: SchemaField,
    pub control: Option<FormControl>,
    pub section: Option<String>,
    pub conditions: FormConditions,
    pub access: FieldAccessMode,
    pub secret: bool,
}

pub struct FormSection {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub order: i32,
    pub advanced: bool,
    pub conditions: FormConditions,
}
```

`schema_version` starts at `1`. Normalization layers accept existing workflow,
connection, and report wire shapes and produce this canonical model.

### Field access

Access is an enum so invalid `read_only && write_only` combinations are not
representable.

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldAccessMode {
    /// Returned to clients and accepted in creates/updates.
    #[default]
    ReadWrite,

    /// Returned to clients, but not accepted in creates/updates.
    Read,

    /// Accepted in creates/updates, but never returned.
    Write,
}
```

Wire values are `read_write`, `read`, and `write`.

Access and secrecy are intentionally separate:

- `access` controls data flow and editability.
- `secret` controls secure storage, masking, redaction, logging, clipboard and
  reveal behavior, and audit handling.
- `control` controls visual presentation.

Supported combinations are:

| Access | Secret | Intended use |
| --- | --- | --- |
| `read_write` | `false` | Ordinary editable configuration |
| `read` | `false` | Provider- or server-managed values |
| `write` | `true` | Passwords, API keys, and client secrets |
| `write` | `false` | One-time input values, if needed later |

The form-definition validator rejects:

- `secret: true` with `read`;
- `secret: true` with `read_write`;
- a secret field configured with an ordinary unmasked control.

Connection patch validation separately rejects set, replace, or clear
operations that conflict with access mode and per-field lifecycle metadata.

Connection descriptor macros derive the safe combination from one
annotation. For example, `#[field(secret)]` generates `access: write`,
`secret: true`, and a password control. Authors do not need to configure
all three properties independently.

### Controls

```rust
pub struct FormControl {
    pub kind: ControlKind,
    pub options: Vec<FormOption>,
    pub option_resolver: Option<String>,
    pub option_dependencies: Vec<String>,
}

pub enum ControlKind {
    Text,
    Textarea,
    SecretTextarea,
    Password,
    Number,
    Toggle,
    Select,
    MultiSelect,
    Radio,
    Date,
    Datetime,
    DateRange,
    NumberRange,
    Tags,
    KeyValue,
    Lookup,
    File,
}

pub struct FormOption {
    pub value: serde_json::Value,
    pub label: String,
}
```

When `control` is absent, the renderer infers it in this order:

1. Secret status.
2. Static enum or labeled options.
3. Schema format.
4. Field type.

Explicit control metadata wins over inference. Inference is a compatibility and
convenience mechanism, not a replacement for explicit metadata on complex
fields.

## Conditional form state

The shared model describes resulting field states directly. It does not use
paired `SHOW`/`HIDE` or `ENABLE`/`DISABLE` effects because `ConditionExpression`
already supports `NOT`, `AND`, and `OR`.

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormConditions {
    /// When absent, the element is visible.
    pub visible: Option<ConditionExpression>,

    /// When absent, the element is enabled.
    pub enabled: Option<ConditionExpression>,

    /// Adds conditional requiredness to SchemaField::required.
    pub required: Option<ConditionExpression>,
}
```

`required` is valid for fields and rejected for sections.

Evaluation is deterministic:

```rust
visible = conditions.visible
    .map(evaluate)
    .unwrap_or(true);

enabled = conditions.enabled
    .map(evaluate)
    .unwrap_or(true);

required = schema.required
    || conditions.required
        .map(evaluate)
        .unwrap_or(false);
```

To hide or disable on a condition, the authored expression uses `NOT`. Multiple
requirements are composed inside a single `ConditionExpression` using `AND` or
`OR`. This avoids effect ordering, contradictory rules, and duplicate condition
shapes.

### Data behavior for conditional fields

- Conditions never mutate, clear, or remove values.
- A supplied hidden field remains type-validated.
- A missing hidden field is not rejected because of unconditional requiredness.
- A field with a false conditional `required` expression remains optional unless
  `SchemaField::required` is true.
- Disabled fields retain their values and remain type-validated when supplied.
- Secret clearing and authentication-method changes remain explicit domain
  operations; visibility never implies deletion.

### Condition evaluator location

The generic client-evaluable `ConditionExpression` implementation lives in
`runtara-dsl`. The reports crate uses the shared implementation through its
native compatibility facade.

The evaluator rejects operators that are meaningful only in server-side query
or vector-search contexts. Form-definition validation detects those operators
before a form is served.

## Shared validation

Generic form validation lives in `runtara_dsl::form`.

```rust
pub fn validate_form_definition(
    definition: &FormDefinition,
) -> Vec<FormIssue>;

pub fn analyze_form(
    definition: &FormDefinition,
    data: &serde_json::Value,
) -> FormAnalysis;
```

`analyze_form` combines effective field-state calculation and value validation
so the browser normally needs one WASM call after a change.

### Structured results

```rust
pub struct FormIssue {
    pub code: String,
    pub path: String,
    pub message: String,
    pub severity: FormIssueSeverity,
}

pub struct FormFieldState {
    pub visible: bool,
    pub enabled: bool,
    pub required: bool,
}

pub struct FormAnalysis {
    pub valid: bool,
    pub fields: HashMap<String, FormFieldState>,
    pub issues: Vec<FormIssue>,
}
```

The first version supports the schema surface Runtara already exposes:

- requiredness and conditional requiredness;
- string, number, integer, boolean, object, array, file, and null handling;
- enums;
- nested properties;
- array items;
- minimum and maximum constraints;
- string length and pattern constraints;
- explicitly supported formats;
- configurable unknown-field policy;
- access/secret invariants;
- control/type compatibility;
- section references;
- form-condition validity.

Workflow envelope validation such as `{"data": ..., "variables": ...}` remains
workflow-specific and delegates validation of `data` to the shared engine.

## Shared browser validation through WASM

The shared browser bridge is the domain-neutral `runtara-validation-wasm`
crate and `runtara_validation` generated bundle.

Exports:

```rust
#[wasm_bindgen(js_name = validateFormDefinitionJson)]
pub fn validate_form_definition_json(definition_json: &str) -> String;

#[wasm_bindgen(js_name = analyzeFormJson)]
pub fn analyze_form_json(definition_json: &str, data_json: &str) -> String;
```

Backend services call the same Rust functions directly. Native and WASM paths
serialize equivalent structured results for the same fixture.

The report DSL WASM bundle remains responsible for templates and formatting.
Condition evaluation is served by the shared validation bundle; the former
report-specific browser export has been removed.

### WASM initialization failure

Conditional state is essential to correct rendering, so the frontend must not
silently fall back to an independent TypeScript evaluator.

- Preload validation WASM at application startup.
- Gate schema-driven form rendering until initialization resolves.
- Show an explicit initialization error if loading fails.
- Keep backend validation authoritative for every write.
- Do not treat unavailable browser validation as successful validation.

## Shared frontend renderer

The controlled renderer lives under
`crates/runtara-server/frontend/src/shared/forms/`:

```text
crates/runtara-server/frontend/src/shared/forms/
  FormRenderer.tsx
  FormSection.tsx
  FieldControl.tsx
  control-registry.ts
  rust-form-validation.ts
  types.ts
  use-resolved-options.ts
```

Core control contract:

```ts
type FieldControlProps = {
  id: string;
  field: FormField;
  value: unknown;
  disabled: boolean;
  invalid?: boolean;
  options?: FormOption[];
  optionsLoading?: boolean;
  onChange(value: unknown): void;
};
```

The shared renderer is controlled and not coupled to React Hook Form. Pages may
adapt it to React Hook Form, but reports and inline editors can use it without a
form-library wrapper.

The shared control edits a value. Domain-specific frames determine how and when
that value is committed:

- `ConnectionFieldFrame` handles configured secrets, replacement, and explicit
  clearing, while the connection form's `FormFrameContract` translates commit
  events into parameter patches.
- Existing report filter and inline-editor components retain their chips,
  popovers, commit/cancel behavior, and writeback calls while delegating value
  controls to `FieldControl`.
- Existing workflow-owned mapping components retain immediate, reference,
  template, and composite modes outside `FormRenderer`.

### Dynamic options

Dynamic option retrieval remains domain-owned. The renderer receives a resolver
instead of interpreting query configuration:

```ts
interface FormOptionRequest {
  resolverKey: string;
  fieldName: string;
  field: FormField;
  currentData: Readonly<Record<string, unknown>>;
  signal: AbortSignal;
}

type OptionResolver = (request: FormOptionRequest) => Promise<FormOption[]>;
```

- `ReportFilterBar` is the production supplier. It resolves lookup and
  cascading-filter values through the report filter-options API and React
  Query, while `useResolvedOptions` owns only request cancellation, dependency
  tracking, loading/error state, and invalid-value removal.
- Connections and workflows do not currently need dynamic option suppliers;
  no speculative provider-query or agent-query plumbing is claimed.

The shared schema identifies a resolver key and dependency field names, but it
does not contain a general-purpose query language.

## Connection migration

Connections are the first migration target because they exercise conditional
fields, secrets, managed values, alternative authentication, and operational
actions.

### Safe edit model

The edit-specific response contains current readable values and secret state
without exposing write-only values:

```json
{
  "values": {
    "environment": "production",
    "base_url": "https://api.example.com",
    "realm_id": "123456789"
  },
  "secretState": {
    "client_secret": {
      "configured": true,
      "clearable": false
    }
  },
  "version": 7
}
```

Projection rules:

- `read_write` fields are returned in `values`.
- `read` fields are returned in `values`.
- `write` fields are never returned.
- Secret configuration state is returned separately and never masquerades as a
  field value.

### Patch update contract

```json
{
  "version": "7",
  "connectionParameterPatch": {
    "set": {
      "environment": "sandbox"
    },
    "write": {
      "client_secret": "new-secret"
    },
    "clear": []
  }
}
```

Server update sequence:

1. Load the current encrypted parameters.
2. Verify the optimistic concurrency version.
3. Reject writes to `read` fields.
4. Apply permitted `set`, typed `write`, and `clear` operations.
5. Evaluate effective form state.
6. Validate the complete merged configuration.
7. Persist atomically.
8. Apply connection-specific lifecycle behavior such as invalidating health or
   requiring reauthorization.

Blank strings never mean “keep the existing secret.” Secret preservation occurs
because untouched fields are absent from the patch.

The public create contract intentionally retains `connectionParameters`, but
create runs the same native form analysis and rejects unknown, `read`,
server-managed, and provider-token fields. The public update DTO uses
`deny_unknown_fields`, has no legacy wholesale parameter branch, and requires a
version even for title/default/rate-limit-only mutations. OAuth callback token
persistence uses an internal repository operation and never passes through the
public update DTO.

### Connection-specific behavior

Connection lifecycle metadata remains outside `FormField`:

```rust
pub struct ConnectionFieldBehavior {
    pub requires_reauthorization: bool,
    pub clearable: bool,
}
```

This prevents authorization and persistence concerns from leaking into report
and workflow forms.

### Descriptor generation

`ConnectionParams` generates `FormDefinition`.

Existing metadata maps automatically:

- `display_name` to label;
- `description` to description;
- `placeholder` to placeholder;
- `default` to default;
- `enum_values` to static options;
- `secret` to write access, secret classification, and password control;
- Rust optionality to unconditional requiredness.

Only attributes required for explicit UI behavior were added:

- section;
- control;
- read access for managed fields;
- advanced placement;
- visible, enabled, and required conditions.

The former connection `fields` compatibility DTO was removed after all
consumers migrated to `FormDefinition`.

### Pilot types

1. MCP: conditional bearer and API-key fields.
2. SFTP: password versus private-key authentication.
3. QuickBooks: readable managed Realm ID and authorization-sensitive changes.
4. PostgreSQL: one write-only secret connection string.
5. S3-compatible storage: endpoints, credentials, booleans, and defaults.

Every registered connection type was migrated, and field-name grouping
heuristics were removed from `DynamicConnectionForm`.

## Workflow adoption

This work does **not** change the workflow DSL wire format or execution
semantics.

1. Normalize existing workflow flat-map and object/properties schemas into
   `FormDefinition` at runtime.
2. Move generic value validation to `runtara-dsl::form` while preserving the
   existing workflow validation functions as compatibility re-exports.
3. Replace the duplicate workflow and trigger schema renderers with the shared
   controls.
4. Keep workflow mapping modes in existing workflow-owned mapping components.
5. Convert legacy `visibleWhen` to `FormConditions.visible` during
   normalization.
6. Keep workflow compiler, graph, mapping, runtime, and persisted JSON behavior
   unchanged.

The exit criterion is that existing workflows load, validate, execute, and
round-trip without structural changes.

## Reports adoption

Compatibility adapters avoid an immediate persisted report definition
migration.

### Filters

Adapt `ReportFilterDefinition` to transient form fields:

| Report filter | Shared control |
| --- | --- |
| Select | `Select` |
| Multi-select | `MultiSelect` |
| Radio | `Radio` |
| Checkbox | `Toggle` |
| Time range | `DateRange` |
| Number range | `NumberRange` |
| Text/Search | `Text` |

`appliesTo`, filter mappings, strict-reference behavior, and query semantics
remain report-specific.

### Inline editors

Map `ReportEditorConfig` to shared controls while retaining report-owned row
context, lookup requests, commit/cancel behavior, and writeback calls.

### Conditional rendering

Normalize legacy report `showWhen` into `FormConditions.visible` evaluated
against report filter state. Report row actions use `ConditionExpression`
through the shared evaluator.

Only bump `ReportDefinition` and write the canonical condition shape after the
adapter path is stable and the report editor preserves it losslessly.

## Compatibility and versioning

`FormDefinition.schema_version` starts at `1`.

Normalization accepts:

- workflow flat-map schemas;
- existing object/properties schemas;
- legacy workflow `visibleWhen`;
- legacy report `showWhen`.

Normalization always produces one canonical `FormDefinition`. Domain
definitions do not need to persist the full envelope immediately; they may
embed or reference canonical fields and construct the envelope at runtime.

Legacy readers and adapters remain until stored definitions and compatibility
fixtures pass round-trip tests.

## Testing strategy

### Authoritative Rust tests

Cover:

- schema normalization;
- form-definition validation;
- field access and secret invariants;
- control/type compatibility;
- condition validation and evaluation;
- visible, enabled, and required state;
- nested properties and arrays;
- hidden values;
- structured issue paths;
- legacy adapters.

### Native/WASM parity

Every canonical fixture runs through both:

1. The native Rust API.
2. The WASM export.

The resulting structured `FormAnalysis` must be equivalent.

Representative fixtures include:

- MCP authentication mode;
- SFTP password/private-key selection;
- QuickBooks managed fields;
- report filter-driven block visibility;
- report row-action conditions;
- workflow conditional input fields.

### Frontend tests

Frontend tests cover presentation and event behavior only:

- correct control selection;
- visible, enabled, and required state rendering;
- change and clear event emission;
- issue-to-field mapping;
- configured secret, replace, and clear behavior;
- report, connection, and workflow frame behavior.

Do not reproduce validation semantics in TypeScript tests or implementation.

### Compatibility tests

- Existing workflow JSON round-trips unchanged.
- Existing reports remain readable and editable.
- Existing connections remain operable during migration.
- All registered connection definitions normalize and render.
- A title-only connection update does not change parameters.
- Untouched secrets remain intact.
- Read-only/provider-managed fields cannot be patched by clients.

## Delivery phases

### Phase 0: Contract and fixtures

**Status: complete — verified.** The fixture corpus covers every control family,
all registered connection descriptors, representative workflow/report shapes,
and the five pilot types.

1. Add an architecture decision record or link this document from the relevant
   implementation plans.
2. Capture representative existing workflow, report, and connection schemas as
   compatibility fixtures.
3. Freeze the addition of new domain-specific field, control, and visibility
   shapes during implementation.

Exit gate: the fixture corpus covers every current field/control family and the
five pilot connection types.

### Phase 1: Shared Rust engine

**Status: complete — verified.** Native definition, normalization, validation,
condition, access, nested-value, and compatibility tests execute in
`runtara-dsl` and the domain adapters.

1. Add `FormDefinition`, `FormField`, sections, controls, access modes,
   conditions, field state, and structured issues to `runtara-dsl::form`.
2. Move generic condition evaluation into `runtara-dsl`.
3. Move generic value validation into `runtara-dsl::form::validation`.
4. Preserve workflow and report compatibility re-exports.
5. Add normalizers for legacy schema shapes.

Exit gate: native Rust fixture tests pass without frontend changes.

### Phase 2: General validation WASM

**Status: complete — verified.** The generalized validation bundle is preloaded,
fails visibly, and produces the same normalized analyses as native Rust for the
MCP, SFTP, QuickBooks, workflow, and report corpus.

1. Export form-definition validation and form analysis.
2. Add native/WASM parity tests.
3. Generalize the frontend wrapper and preload boundary.
4. Retain old workflow-validation exports during migration.
5. Rename the crate and generated bundle after the first non-workflow consumer
   is active.

Exit gate: all shared fixtures produce equivalent native and WASM results.

### Phase 3: Shared React controls

**Status: complete — verified.** The gallery renders all 17 controls, issue focus
and accessibility are tested, and `ReportFilterBar` supplies the production
dynamic-option resolver.

1. Build the controlled field registry from existing Runtara components.
2. Implement field and section rendering from WASM-produced state.
3. Implement structured issue rendering and first-invalid-field focus.
4. Add domain frame interfaces and dynamic option resolver hooks.

Exit gate: the pilot schema fixture gallery renders every shared control without
domain-specific field inference.

### Phase 4: Connection correctness and pilot

**Status: complete — verified.** Create and update are backend-enforced, update
has one versioned canonical patch, dirty intent preserves absent defaults and
secrets, and browser/API tests cover lifecycle plus conflicts.

1. Add safe connection edit hydration.
2. Add versioned patch semantics.
3. Add read/write/secret enforcement on the backend.
4. Generate `FormDefinition` from connection descriptors.
5. Migrate MCP, SFTP, QuickBooks, PostgreSQL, and S3-compatible storage.
6. Add connection-specific lifecycle behavior outside the form engine.

Exit gate: pilot connections create, edit, validate, test, and preserve secrets
correctly through native and browser validation.

### Phase 5: All connections

**Status: complete — verified.** Every registered descriptor normalizes,
declaration order is stable, authored sections validate, and old grouping/DTO
paths are absent.

1. Annotate and migrate every registered connection type.
2. Snapshot normalized form definitions.
3. Remove connection field-name grouping and validation heuristics.
4. Remove the old DTO after compatibility consumers migrate.

Exit gate: every registered connection type has explicit, validated form
metadata and passes create/edit tests.

### Phase 6: Workflow form adoption

**Status: complete — verified.** Execute, action, chat, and trigger consumers use
the shared renderer; stored-definition, compilation, parity, and 66-case direct
Wasm execution coverage preserve workflow wire/runtime behavior.

1. Route workflow input validation through the shared engine.
2. Replace duplicate workflow schema renderers.
3. Preserve mapping wrappers and persisted workflow JSON.
4. Run workflow round-trip and execution regression suites.

Exit gate: workflow behavior and wire format remain unchanged while the shared
renderer and validator are active.

### Phase 7: Reports form adoption

**Status: complete — verified.** Filters, inline editors, `showWhen`, row-action
conditions, production dynamic options, and native/WASM compatibility execute
without changing persisted report query/layout semantics.

1. Adapt filters to shared controls.
2. Adapt inline editors to shared controls.
3. Move report row conditions to the shared evaluator.
4. Normalize block/layout `showWhen` into shared visibility conditions.
5. Defer persisted report migration until the compatibility path is proven.

Exit gate: existing report definitions render and edit without loss, and report
visibility matches native/WASM evaluation.

### Phase 8: Cleanup

**Status: complete — verified.** Unsafe update compatibility and dead frame
plumbing are removed; only the documented workflow/report wire adapters and
internal OAuth persistence boundary remain.

1. Remove legacy TypeScript schema validators and conditional evaluators.
2. Remove duplicate schema-driven field renderers.
3. Remove compatibility DTOs and adapters only after stored-definition gates
   pass.
4. Remove the report-specific condition WASM export after all consumers use the
   shared validator bundle.
5. Document the canonical schema and macro authoring conventions.

Exit gate: one field/control vocabulary, one Rust validator, one shared
condition evaluator, and one frontend control registry remain.

## Rollout safeguards

- Backend validation remains authoritative throughout the migration.
- During rollout, new and old validators ran in shadow mode for representative
  requests before each surface switched to the shared engine.
- Structured issues must be compared by code and path, not only message text.
- Persisted workflow and report formats change only through explicit versioned
  migrations.
- Compatibility adapters are removed only after fixture and repository
  stored-definition audits succeed; irreducible domain wire adapters remain.
- Connection update semantics were corrected before completion of the new
  editor's production credential path.

## Completion audit

### Gap-by-gap resolution

| Gap | Production resolution | Executed evidence |
| --- | --- | --- |
| NG-1 | `UpdateConnectionRequest` requires `version`, denies unknown fields, and exposes only descriptor-aware `set`/`write`/`clear`; create is also access-checked; OAuth persistence is internal | Rust service/handler tests, OpenAPI/TS drift check, direct HTTP rejection and stale-write browser cases |
| NG-2 | Connection edit hydration tracks stored presence separately from displayed defaults and emits only explicit dirty/clear operations | Frontend payload tests plus PostgreSQL/API-created QuickBooks title-only regression preserving exact parameters, tokens, and status |
| NG-3 | Standardized 409 payload, draft retention, latest-projection fetch, changed-field review, reload, and explicit reapply | Shared error, hook, component, and two-editor-equivalent browser coverage for stale title and parameter writes |
| NG-4 | Macro metadata records declaration index; normalization uses it unless an explicit order is authored | Macro tests, normalized snapshots, React order test, browser checks for QuickBooks and SFTP |
| NG-5 | Macro section declarations support id/label/description/order/advanced with contradiction validation | Macro/Rust/React tests and keyboard browser coverage of MCP advanced fields with retained values |
| NG-6A | `ReportFilterBar` supplies `OptionResolver` from the production report filter-options API | Loading, failure, abort, stale-response, dependency, cache, and invalid-value tests |
| NG-6B | Typed JSON `write` operations are access-aware and independent of `secret`; write-only values never project | Rust, API/OpenAPI, generated TS, frontend, and native/WASM contract tests |
| NG-6C | External suites are explicit feature-gated targets that fail closed; CI enables every required feature | PostgreSQL, Docker pgvector, object store, Valkey/TLS, component, and direct-Wasm suites listed below; zero required skips |

### Additional gaps found and fixed

| Finding | Fix |
| --- | --- |
| Public create accepted client-owned status, read/server-managed/provider-token fields, and unknown fields | Removed status from the public create DTO/generated client and run native form/access analysis before persistence |
| Connection create/default/file-storage mutations could partially commit | Wrapped create and default transitions in one transaction with advisory serialization; duplicate-title rollback regression proves the prior default survives |
| Shared controls had incomplete accessible naming and file keyboard behavior | Added labels/group semantics and keyboard-accessible file control; the 17-control gallery is exercised |
| `clearField` frame plumbing had no production owner | Removed it; explicit connection secret clearing remains domain-owned |
| Native/WASM parity corpus omitted representative domain conditions | Added MCP, SFTP, QuickBooks, report visibility/row-action, and workflow-condition fixtures |
| Environment, server, core, object-store, Valkey, component, and direct-Wasm tests silently succeeded without prerequisites | Added package features, strict preflights, dedicated commands, CI jobs, and corrected stale fixtures exposed by actually running them |
| CI used the wrong Environment database variable and did not enable the new integration targets | CI now uses the domain-specific variables/features and runs plaintext/TLS Valkey, object-store, component, and direct-Wasm gates |
| Docker pgvector fixture raced the image's temporary init server and inherited ambient config | Added bounded connection retry, explicit local SSL mode, and isolated test config; the case now executes |

### Production entry points and consumers

| Shared capability | Backend enforcement / producer | Frontend production consumers |
| --- | --- | --- |
| `FormDefinition`, normalization, and `analyze_form` | `runtara-dsl`; connection create/update service; workflow validation adapters; report condition facade | Connection create/edit, workflow execute/action/chat, trigger inputs, report filters and inline editors |
| Validation WASM | `runtara-validation-wasm` exports the same Rust analysis used natively | Application preload plus `rust-form-validation`; rendering is gated on successful initialization |
| `FormRenderer` and control registry | Definitions are domain-produced; commit behavior stays outside the renderer | `DynamicConnectionForm`, `WorkflowExecuteDialog`, `ActionForm`, `ChatFormInput`, and `CronInputsField`; report surfaces reuse `FieldControl` inside report-owned frames |
| `OptionResolver` / `useResolvedOptions` | Request/query/auth semantics remain in the report filter-options endpoint and query layer | `ReportFilterBar` is the production supplier; no connection/workflow supplier is claimed |
| Shared condition evaluator | Native connection/workflow/report adapters and WASM call `runtara-dsl` | Conditional connection fields, legacy workflow `visibleWhen`, report `showWhen`, and row actions |
| Connection descriptor macro | Emits access, secrecy, declaration order, sections, controls, and lifecycle metadata for every registered descriptor | Connection create/edit projection adapter and shared renderer |

### Retained compatibility boundaries

- Public connection **create** retains `connectionParameters` because it creates a
  complete initial value, but native descriptor analysis rejects unknown,
  `read`, and server-managed inputs. Public **update** has no wholesale legacy
  branch.
- Workflow JSON-string, object/`properties`, and legacy nested-array envelopes
  normalize to the canonical form without changing stored workflow graphs,
  compiler input, mappings, references, or execution semantics.
- Persisted report `showWhen` and editor shapes remain lossless report-owned wire
  adapters; evaluation is shared. No unversioned report migration occurred.
- OAuth callbacks use an explicitly internal repository operation for provider
  tokens and status. It is not reachable through the public update DTO.
- There is no MCP connection-mutation tool in this repository. MCP graph paths
  read connections; external mutation uses the same generated REST/OpenAPI
  contract verified above, so no parallel MCP update bypass remains.

### Verification record

The following counts are from executed assertions, not discovered tests:

| Command / scenario | Executed | Ignored | Skipped | Result |
| --- | ---: | ---: | ---: | --- |
| `cargo test --workspace` | 3,442 | 40 doctests/tests | 0 early-return skips | pass |
| `cargo test -p runtara-connections` with required PostgreSQL URL | 146 (120 unit + 26 integration) | 0 | 0 | pass |
| `cargo test -p runtara-environment --features db-integration-tests -- --test-threads=1` | 257 | 4 doctests | 0 | pass |
| Core PostgreSQL operation suite plus backend parity | 19 | 0 | 0 | pass |
| `cargo test -p runtara-object-store --features db-integration-tests --test integration -- --test-threads=1` | 73 | 0 | 0 | pass |
| Server DB suites: audit, authorization, cleanup, report runtime/render, Docker Object Model | 17 | 0 | 0 | pass |
| Plaintext Valkey external cases | 15 | 0 | 0 | pass |
| `scripts/test-valkey-tls.sh` | 4 | 0 | 0 | pass |
| `cargo test -p runtara-component-host --features component-integration-tests` | 21 | 0 | 0 | pass |
| Direct-Wasm target plus three composition entry cases | 69 | 0 | 0 | pass |
| `cargo test -p runtara-agent-macro` | 55 | 4 doctests | 0 | pass |
| `cargo test -p runtara-dsl` | 202 (201 unit + 1 doctest) | 2 doctests | 0 | pass |
| `cargo test -p runtara-validation-wasm` | 20 | 0 | 0 | pass |
| Stored workflow form compatibility | 2 | 0 | 0 | pass |
| `cargo test -p runtara-report-dsl` | 83 | 0 | 0 | pass |
| Frontend `npm test -- --run` | 931 in 73 files | 0 | 0 | pass |
| Playwright local connection audit | 6 | 0 | 0 | pass |

The default workspace gate's 40 ignored items are explicit doctests/tests and
are not counted as integration coverage. Its ordinary run omits feature-gated
external targets by design; every external target above was then enabled
directly and had zero skipped/early-returned tests. `cargo fmt --all -- --check` and
`cargo clippy --workspace --all-targets -- -D warnings` passed. Frontend
`tsc -b`, production `npm run build`, and ESLint passed with 0 errors and the
same 33 pre-existing warnings (0 new warnings).

Offline OpenAPI/TypeScript regeneration was byte-identical before and after
generation (`6f4ba1bd3b0a7e6b891beffebcc2946434452f3edf9d548afcb88f709fa3368e`).
`scripts/build-agent-components.sh` rebuilt all 26 agent Wasm/meta pairs and the
two shared workflow components from current source.

### Real API and browser scenarios

The six local Playwright cases exercised the production server and generated
client paths:

1. SFTP conditional authentication, secret create/edit/preserve/explicit clear.
2. MCP conditional fields and first-invalid focus.
3. QuickBooks/SFTP declaration order plus keyboard-accessible advanced-section
   collapse/expand with value retention.
4. Direct HTTP rejection of managed create/status, legacy wholesale update,
   missing version, read-field patch, and stale title/parameter mutations; a
   safe versioned update succeeds.
5. Title-only save of an API-created QuickBooks row whose default-bearing keys
   are absent, proving no parameter/token/status mutation.
6. Conflict draft retention, latest-version review, and explicit reapply.

PostgreSQL lifecycle coverage separately proves that explicit changes to
reauthorization-sensitive fields atomically remove provider tokens and change
status, while unrelated saves preserve encrypted parameters byte-for-byte.

## Completion criteria

The initiative is complete when:

- connections, workflow forms, report filters, and report inline editors use
  the shared control vocabulary;
- backend and browser validation use the same Rust implementation;
- native and WASM form analysis passes the parity fixture corpus;
- connections no longer infer sections or authentication behavior from field
  names;
- connection editing uses safe readable-value hydration and explicit secret
  patch operations;
- no TypeScript implementation duplicates form validation or condition
  evaluation;
- workflow execution DSL and persisted workflow behavior remain unchanged;
- report query and layout semantics remain unchanged;
- no additional external form or validation dependency has been introduced.

**Current assessment: complete.** NG-1 through NG-6, all additional findings,
production-consumer checks, compatibility audits, external fail-closed suites,
direct API/browser scenarios, and final workspace/frontend gates are verified.
