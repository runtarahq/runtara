# UI Schema Unification Plan

## Status

**Reopened and in progress as of 2026-07-12.** The earlier completion claim was
incorrect. It proved the primary browser path but did not adversarially verify
public compatibility paths, migration behavior, production consumers for every
shared abstraction, or whether required integration tests fail closed. No
previously checked item or phase exit gate is considered complete until the new
audit records direct production and executed-test evidence.

### Known gaps being resolved

- [ ] **NG-1 — unsafe legacy update path:** public `PUT` still accepts
  `connectionParameters` without the canonical version, field-access, and
  explicit write/clear enforcement applied to `connectionParameterPatch`.
- [ ] **NG-2 — unintended default writes:** edit hydration displays defaults for
  absent stored keys and patch generation mistakes them for user changes,
  potentially triggering reauthorization during an unrelated save.
- [ ] **NG-3 — conflict recovery:** the useful 409 message is lost, the draft
  receives no conflict-specific UI, and the page retains a stale version.
- [ ] **NG-4 — field ordering:** connection metadata discards declaration order,
  so equal-order fields render alphabetically.
- [ ] **NG-5 — advanced sections:** the connection macro cannot author section
  metadata and generated sections always set `advanced: false`.
- [ ] **NG-6A — unused option resolution:** `OptionResolver` has no production
  supplier and therefore is not yet a production-integrated capability.
- [ ] **NG-6B — unsupported non-secret writes:** `write` with `secret: false` is
  documented but routed through secret replacement and rejected by the backend.
- [ ] **NG-6C — false-positive integration tests:** required PostgreSQL tests
  return success when Docker or database startup fails.
- [ ] Adversarially audit every earlier completion item, public and internal
  caller, retained compatibility boundary, phase exit gate, and verification
  command; fix additional gaps rather than documenting them as exceptions.

### Historical verification warning

The earlier verification commands remain useful regression history, but they
are not completion evidence for the reopened initiative. In particular, a
Docker-backed test run could silently return success without executing its
assertions, interface-only code was counted as a production consumer, and the
real browser run did not cover legacy/API updates, absent-key defaults,
concurrent edits, declaration ordering, or advanced sections. A replacement
verification record will explicitly distinguish executed, ignored, and skipped
tests and will include direct API plus browser evidence.

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

- Reports resolve Object Model lookup values and cascading filters.
- Connections can resolve provider resources without changing the shared
  engine.
- Workflows resolve agent or type metadata where applicable.

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
  "version": 7,
  "set": {
    "environment": "sandbox"
  },
  "replaceSecrets": {
    "client_secret": "new-secret"
  },
  "clear": []
}
```

Server update sequence:

1. Load the current encrypted parameters.
2. Verify the optimistic concurrency version.
3. Reject writes to `read` fields.
4. Apply permitted `set`, `replaceSecrets`, and `clear` operations.
5. Evaluate effective form state.
6. Validate the complete merged configuration.
7. Persist atomically.
8. Apply connection-specific lifecycle behavior such as invalidating health or
   requiring reauthorization.

Blank strings never mean “keep the existing secret.” Secret preservation occurs
because untouched fields are absent from the patch.

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

**Status: reopened; exit gate unverified.** The existing contract, snapshots,
fixtures, and stored-definition audits require adversarial re-verification.

1. Add an architecture decision record or link this document from the relevant
   implementation plans.
2. Capture representative existing workflow, report, and connection schemas as
   compatibility fixtures.
3. Freeze the addition of new domain-specific field, control, and visibility
   shapes during implementation.

Exit gate: the fixture corpus covers every current field/control family and the
five pilot connection types.

### Phase 1: Shared Rust engine

**Status: reopened; exit gate unverified.**

1. Add `FormDefinition`, `FormField`, sections, controls, access modes,
   conditions, field state, and structured issues to `runtara-dsl::form`.
2. Move generic condition evaluation into `runtara-dsl`.
3. Move generic value validation into `runtara-dsl::form::validation`.
4. Preserve workflow and report compatibility re-exports.
5. Add normalizers for legacy schema shapes.

Exit gate: native Rust fixture tests pass without frontend changes.

### Phase 2: General validation WASM

**Status: reopened; exit gate unverified.** Form exports and the physical rename
exist, but parity and every production consumer must be re-proven.

1. Export form-definition validation and form analysis.
2. Add native/WASM parity tests.
3. Generalize the frontend wrapper and preload boundary.
4. Retain old workflow-validation exports during migration.
5. Rename the crate and generated bundle after the first non-workflow consumer
   is active.

Exit gate: all shared fixtures produce equivalent native and WASM results.

### Phase 3: Shared React controls

**Status: reopened; exit gate not met.** The registry, WASM-produced state,
issues, and submit focus exist, but `OptionResolver` has no production supplier.

1. Build the controlled field registry from existing Runtara components.
2. Implement field and section rendering from WASM-produced state.
3. Implement structured issue rendering and first-invalid-field focus.
4. Add domain frame interfaces and dynamic option resolver hooks.

Exit gate: the pilot schema fixture gallery renders every shared control without
domain-specific field inference.

### Phase 4: Connection correctness and pilot

**Status: reopened; exit gate not met.** The primary UI patch path exists, but
legacy updates bypass its guarantees and hydration can emit unintended defaults.

1. Add safe connection edit hydration.
2. Add versioned patch semantics.
3. Add read/write/secret enforcement on the backend.
4. Generate `FormDefinition` from connection descriptors.
5. Migrate MCP, SFTP, QuickBooks, PostgreSQL, and S3-compatible storage.
6. Add connection-specific lifecycle behavior outside the form engine.

Exit gate: pilot connections create, edit, validate, test, and preserve secrets
correctly through native and browser validation.

### Phase 5: All connections

**Status: reopened; exit gate not met.** Descriptors produce canonical forms,
but declaration order and authorable advanced sections are missing; snapshots
must be refreshed and re-audited.

1. Annotate and migrate every registered connection type.
2. Snapshot normalized form definitions.
3. Remove connection field-name grouping and validation heuristics.
4. Remove the old DTO after compatibility consumers migrate.

Exit gate: every registered connection type has explicit, validated form
metadata and passes create/edit tests.

### Phase 6: Workflow form adoption

**Status: reopened; exit gate unverified.** Existing workflow consumers and
wire-format invariants require production-path and regression re-verification.

1. Route workflow input validation through the shared engine.
2. Replace duplicate workflow schema renderers.
3. Preserve mapping wrappers and persisted workflow JSON.
4. Run workflow round-trip and execution regression suites.

Exit gate: workflow behavior and wire format remain unchanged while the shared
renderer and validator are active.

### Phase 7: Reports form adoption

**Status: reopened; exit gate unverified.** Persisted report migration remains
intentionally deferred, but adapters, shared controls, option retrieval, and
condition evaluation require production-consumer and losslessness evidence.

1. Adapt filters to shared controls.
2. Adapt inline editors to shared controls.
3. Move report row conditions to the shared evaluator.
4. Normalize block/layout `showWhen` into shared visibility conditions.
5. Defer persisted report migration until the compatibility path is proven.

Exit gate: existing report definitions render and edit without loss, and report
visibility matches native/WASM evaluation.

### Phase 8: Cleanup

**Status: reopened; exit gate not met.** Known unsafe compatibility behavior and
unused shared plumbing remain; all earlier cleanup claims require a fresh audit.

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
- New and old validators may run in shadow mode for representative requests
  before a surface switches to the shared engine.
- Structured issues must be compared by code and path, not only message text.
- Persisted workflow and report formats change only through explicit versioned
  migrations.
- Compatibility adapters are removed only after fixture and repository
  stored-definition audits succeed; irreducible domain wire adapters remain.
- Connection update semantics must be corrected before the new editor is used
  for production credential changes.

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

**Current assessment:** incomplete. NG-1 through NG-6 and the adversarial audit
must be resolved and fully verified before any completion claim is restored.
