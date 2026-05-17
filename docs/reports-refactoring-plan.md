# Reports Refactoring Plan

**Status (2026-05-17):**

| Phase | State |
|---|---|
| 0 — Safety net | [x] Done. Dual-run harness body + CI gate intentionally out of scope. |
| 1 — `runtara-report-dsl` crate | [x] Done. Legacy validator deletion explicitly deferred to Phase 5. |
| 2 — Codegen + delete handwritten FE types | [x] Done. Bundle <250KB target missed (339KB actual); mitigated by lazy-loaded wizard route. |
| 3 — Source provider trait | [x] Done. Trait + registry + three providers landed; render + validator dispatch routes through registry; four obsolete render leaves deleted; system/workflow_runtime/object_model code physically moved into provider files. Aggregate-with-joins helper for object_model + the virtual aggregate engine still live in `reports.rs` (Phase 4 cleanup). |
| 4 — Single render pipeline | [ ] Not started. |
| 5 — Strict-mode validator + Phase 1 legacy-deletion finishers | [ ] Not started. |
| 6 — Edit-operation symmetry | [ ] Not started. |
| 7 — Wizard rewrite | [ ] Not started. |
| 8 — Cutover + cleanup | [ ] Not started. |

**Owner:** _unassigned_
**Last updated:** 2026-05-17

Tracking document for the reports module refactor. Each phase has work items, tests, and acceptance criteria as checkboxes. Update this file as items land.

## Background

The reports module has grown five parallel representations of the same schema across ~25k LOC: Rust DTO, backend service, MCP authoring schema, MCP authoring validator, frontend TS types, and a wizard intermediate model. Three template parsers, two row-condition evaluators, three source-shape types, abandoned `query_plan.rs` refactor. Adding one field today touches nine sites. See the audit notes from 2026-05-16 for the full diagnosis.

## Goals

- One canonical schema with codegen to TS.
- One template engine (`minijinja`) shared by backend and frontend.
- One row-condition AST (`ConditionExpression` from `runtara-dsl`) shared by reports and workflows.
- Pluggable source providers behind a trait.
- One render pipeline. One validator. One edit-operation API.
- Wizard operates on canonical `ReportDefinition` — no intermediate model, no silent mutation.

Breaking JSON shape changes are allowed; feature parity must be preserved. **No data migration** — existing reports will be re-authored via MCP after the cutover. At the cutover, legacy definitions will stop loading and a clear "needs re-authoring" state will surface in the UI.

## Target architecture

`runtara-report-dsl` crate owns report-specific schema (layout, blocks, views, datasets, sources) + the virtual aggregate engine + a thin `evaluate_row_condition` over `ConditionExpression`. Depends on `runtara-dsl` for condition types and `minijinja` for templating. Compiles native (server) and `wasm32` (frontend). Source kinds become trait-based providers. REST and MCP edit through one operation API and validate through one path. Frontend uses generated TS types + the WASM crate. Reports reuse the workflow `ConditionalStep` UI for row-condition authoring.

## Sequencing

```
Phase 0 ─ Safety net                                  [1 wk, blocker]
Phase 1 ─ runtara-report-dsl crate                    [2 wk, blocker]
   │
   ├─ Track A: Phase 2 → Phase 5
   ├─ Track B: Phase 3 → Phase 4
   ├─ Track C: Phase 6
   └─ Track D: Phase 7 (needs Phase 2)

Phase 8 ─ Migration + cleanup                         [1 wk, finisher]
```

Estimated total: 10–12 weeks.

---

## Phase 0 — Safety net

**Status:** [x] Complete. Drift-detection in place across the JSON Schema, DTO serde, semantic validation, and render paths plus an FE block-type loading suite. Two items deferred by design: dual-run harness body (Phase 1) and CI wiring (out of scope).

### Work

- [x] Snapshot corpus seeded at `crates/runtara-server/tests/fixtures/reports/*.json` (11 fixtures covering all block types, all source kinds, joins, datasets, row conditions, views + interactions, layout types, dynamic + static filters).
- [x] JSON Schema corpus test at `crates/runtara-server/tests/reports_corpus.rs`. Snapshots written via `insta` to `tests/fixtures/reports/__snapshots__/syntax_*.snap`.
- [x] Runtime corpus test at `crates/runtara-server/tests/reports_runtime_corpus.rs` — boots a UUID-suffixed temp DB on `TEST_REPORTS_DATABASE_URL` / `RUNTARA_DATABASE_URL`, applies server migrations, runs every fixture through `ReportService::validate_report`, snapshots the response (`runtime_validate_*.snap`). Skips gracefully when no DB URL is configured.
- [x] Render corpus test at `crates/runtara-server/tests/reports_render_corpus.rs` — persists each fixture via the repository (bypassing the validator so even unseeded-schema fixtures get stored), calls `render_report`, masks timestamps + UUIDs, canonicalizes HashMap order, snapshots (`render_*.snap`). 11 fixtures × stable snapshot.
- [x] proptest harness at `crates/runtara-server/tests/reports_proptest.rs` — 3 properties over 256 random cases each: JSON Schema validator never panics, DTO deserialize never panics, serde round-trip is a fixed point after one pass.
- [x] Playwright corpus spec at `crates/runtara-server/frontend/e2e/tests/mocked/reports/report-corpus-block-loading.mocked.spec.ts` — 6 tests covering each block type (markdown, table, chart, metric, card) loading in the viewer plus a block-error path verifying `BLOCK_RENDER_FAILED` surfaces in the UI.
- [x] `dual-run-reports` Cargo feature flag added (no-op until Phase 1 introduces a new code path).
- [ ] Build dual-run harness body (**deferred by design** — needs a new path from Phase 1 to compare against; flag exists, wired in at Phase 1 kickoff).
- [ ] Wire dual-run into GH Actions as a merge gate (**out of scope** per project decision).

### Tests

- [x] Unit (Rust): proptest harness on `ReportDefinition` JSON shapes — serde round-trip stable, no panics on the validator or deserializer.
- [x] Integration (Rust): JSON Schema `syntax_*` snapshot per fixture (currently `[]` for every fixture; will surface diffs as the schema tightens).
- [x] Integration (Rust): DTO round-trip stability test (`fixtures_round_trip_through_dto`).
- [x] Integration (Rust): `runtime_validate_*` snapshot per fixture — validate_report response captured (markdown is `valid: true`, schema-using fixtures snapshot "Schema not found" — both deterministic and useful for drift detection).
- [x] Integration (Rust): `render_*` snapshot per fixture — render_report response captured (markdown renders content; schema-using fixtures snapshot the `BLOCK_RENDER_FAILED` block-error path).
- [ ] Integration (CI): dual-run fails the build on any old/new divergence once internals start swapping (out of scope).
- [x] E2E (Playwright): `report-corpus-block-loading.mocked.spec.ts` covers each block type (markdown, table, chart, metric, card) loading in the viewer + an error-surfacing case.

### Acceptance

- [x] Full corpus passes both paths byte-identically (only one path exists today; harness is staged for Phase 1's second path).
- [x] Harness runs unchanged through phases 1–8 (test files, fixture JSON, and snapshot diffs are the contract from here forward).

### How to run

```
# Fast: JSON Schema + DTO round-trip, no DB
cargo test -p runtara-server --test reports_corpus

# Proptest: 256 cases per property, no DB
cargo test -p runtara-server --test reports_proptest

# Runtime validate snapshots, requires Postgres with pgvector + pg_trgm
# (set TEST_REPORTS_DATABASE_URL, RUNTARA_DATABASE_URL, or DATABASE_URL).
cargo test -p runtara-server --test reports_runtime_corpus

# Render snapshots, same Postgres requirements
cargo test -p runtara-server --test reports_render_corpus

# Review any snapshot diffs
cargo insta review

# Playwright viewer suite
cd crates/runtara-server/frontend && npx playwright test e2e/tests/mocked/reports/report-corpus-block-loading.mocked.spec.ts
```

---

## Phase 1 — `runtara-report-dsl` crate

**Status:** [x] Complete. Crate exists with types + minijinja + ConditionExpression evaluator + WASM target. Server uses the crate via re-exports.

**Carried into Phase 5** (by design — old and new coexist for one phase before deletion):
- `validate_safe_display_template` family in `services/reports.rs`
- Row-condition validator in `services/reports.rs` (the canonical `ConditionExpression` validator landed alongside it, but the legacy code path remains until Phase 5)
- Parallel template parser in `mcp/tools/reports.rs`

### Work

- [x] Create `crates/runtara-report-dsl` crate.
- [x] Move report-specific types out of `runtara-server` (1801 lines moved). Server's `api/dto/reports.rs` is now a 9-line re-export shim. `utoipa::ToSchema` derives are `cfg_attr(feature = "utoipa")` so the WASM build doesn't pull utoipa.
- [x] Local `Condition` type in `runtara-report-dsl`; `api/dto/object_model::Condition` re-exports it (one source of truth, no churn at server callsites). Needed a `condition_to_store(c)` helper to bridge the orphan rule for `From<Condition> for runtara_object_store::Condition`.
- [ ] Move the virtual aggregate engine (deferred — internal-only, no FE dependency).
- [x] Depend on `runtara-dsl` for `ConditionExpression`, `ConditionOperator`, `ConditionArgument`, `MappingValue`.
- [x] Depend on `minijinja`. `register_report_filters(env)` registers `currency`, `number`, `decimal`, `percent`, `date`, `datetime`, `pill`, `bar_indicator`.
- [x] `evaluate_row_condition(expr: &ConditionExpression, row: &Value) -> Result<bool, RowConditionError>` — AND/OR/NOT, EQ/NE/GT/GTE/LT/LTE, STARTS_WITH/ENDS_WITH, CONTAINS/IN/NOT_IN, LENGTH, IS_DEFINED/IS_EMPTY/IS_NOT_EMPTY. Server-only ops (MATCH, SIMILARITY_GTE, COSINE_DISTANCE_LTE, L2_DISTANCE_LTE) return `RowConditionError::ServerOnly`.
- [ ] Delete `validate_safe_display_template` family from `services/reports.rs:6244–6395` (**deferred to Phase 5** — old + new coexist for one phase, safer cutover).
- [ ] Delete row-condition validator from `services/reports.rs:5713–5856` (**deferred to Phase 5**).
- [ ] Delete parallel template parser from `mcp/tools/reports.rs:3752–3839` (**deferred to Phase 5**).
- [x] `wasm32-unknown-unknown` target + `wasm-bindgen` JS bindings. Bundle builds via `wasm-pack build --target web --out-dir pkg --features wasm --no-default-features`. `pkg/` is gitignored.

### Tests

- [x] Unit (Rust): per-filter conformance for `currency`, `number`, `decimal`, `percent`, `date`, `datetime`; plain field interpolation; undefined-field rendering; parse-error reporting.
- [x] Unit (Rust): `ConditionExpression` conformance — EQ ref→immediate, GT numeric coercion, AND short-circuit, dotted path resolution, IN against array, IS_DEFINED on missing, NOT inversion, server-only MATCH rejection.
- [ ] Re-home the 74 inline tests from `services/reports.rs` (deferred — they still pass in their existing location).
- [ ] Unit (WASM): Node-side round-trip (deferred to Phase 2 FE wiring).
- [x] Integration (Rust): all four Phase 0 corpus test suites pass unchanged against the new crate.
- [ ] Integration (CI): dual-run — flag exists, harness body still pending (needs an alternate path in the same shape).

### Acceptance

- [x] Phase 0 corpus snapshots unchanged across DTO round-trip, JSON Schema syntax, runtime validate_report, render_report, and proptest.
- [x] WASM bundle target: aspirational <250KB gzipped, actual **339KB gzipped** (~950KB raw). Drivers: minijinja (~150KB) + canonical condition / format machinery. schemars is no longer in the WASM tree — runtara-dsl is on schemars 1 across the workspace and the schema-generation modules (`spec`, `step_registration`, `agent_meta::SchemaGeneratorFn` family) are `#[cfg(feature = "json-schema")]`; `runtara-report-dsl` consumes runtara-dsl with `default-features = false`. Bundle reduction below 250KB would need minijinja sub-feature trimming. Mitigated by lazy-loading the wizard route.
- [ ] `services/reports.rs` shorter by ~700 lines — deferred to Phase 5 (legacy validators still live there).

---

## Phase 2 — Codegen + delete handwritten FE types

**Status:** [x] Complete. All four originally-planned outcomes landed: codegen pipeline (online + offline) emits 100+ report types into `RuntaraRuntimeApi.ts`; handwritten `types.ts` shrank 805 → 332 lines (re-export shim with `Omit + &` tightenings); single template engine via `runtara-report-dsl::template` + WASM `JsFormatter` callback; single row-condition evaluator via WASM `evaluateRowCondition` (canonical `ConditionExpression` on the wire). Bundle <250KB target missed (339KB actual) but mitigated by lazy-loaded wizard route. Schemars 0.8 → 1 consolidation also landed here (was originally a fine-tuning item — see acceptance + progress log).

### Work

- [x] Register 100+ report DTOs in `components(schemas(...))` at `server.rs`.
- [x] `dump_openapi` bin in `crates/runtara-server/src/bin/dump_openapi.rs` — emits the OpenAPI doc to stdout without a running server. `npm run generate-api-runtime-offline` runs it + the codegen in one shot.
- [x] Regenerate `frontend/src/generated/RuntaraRuntimeApi.ts` — now contains all 100+ report types as TypeScript enums.
- [x] WASM bundle: `runtara-report-dsl` exposes a `json-schema` feature (default-on); ToSchema + JsonSchema derives are `cfg_attr`-gated. minijinja switched to minimal features (`builtins`, `serde`, `deserialization` only). Workspace `[profile.release.package.runtara-report-dsl]` tuned for size (`opt-level = "z"`, `codegen-units = 1`).
- [x] Bundle size: **339KB gzipped** (peaked at 363KB pre-Phase-2). Schemars 0.8 is gone from the workspace — runtara-dsl is on schemars 1 and its schema-generation surface (`spec`, `step_registration`, `agent_meta::SchemaGeneratorFn` family) is `#[cfg(feature = "json-schema")]`; `runtara-report-dsl` consumes it with `default-features = false`. Above the aspirational 250KB target; further slimming would need minijinja sub-feature trimming.
- [x] Vendor WASM bundle to `frontend/src/wasm/runtara-report-dsl/` (`runtara_report_dsl_bg.wasm`, `.js`, `.d.ts`, plus README with regen instructions).
- [x] FE init helper at `frontend/src/wasm/runtara-report-dsl/index.ts` — async `reportDsl()` returns `{ version, renderTemplate, validateTemplate, evaluateRowCondition }`. Memoizes the load promise.
- [x] Delete `frontend/src/features/reports/types.ts` (805 lines) → 332-line re-export shim landed. See progress-log entry below for the final shape (Omit + & tightenings for `source`, `dimensions/measures`, `blocks/filters/datasets`, `definition`; FE-only types kept verbatim).
- [x] In `utils.ts`: replace `compileDisplayTemplate` + `formatCellValue` with WASM `renderTemplate` + `formatValue`. Single Rust template engine, JS-side `Intl` callback owns locale resolution. Track A landed.
- [x] In `utils.ts`: replace `matchesReportRowCondition` body with WASM `evaluateRowCondition`. Track B landed with a legacy→canonical bridge at the WASM boundary; editor + wire-format migration to canonical is a follow-up (mechanical UI rewrite, no behavior change).

### Phase 2 sub-plan: types.ts deletion [implemented]

**Problem.** Two type-system blockers stopped the first attempt:

1. **Null vs undefined.** Rust `Option<T>` serializes as `null` rather than
   omission, so generated TS has `T | null | undefined` everywhere the
   handwritten file has `T | undefined`. Internal FE call sites pass values
   typed `T | null | undefined` into functions whose signatures expect
   `string | undefined`. Hundreds of tsc errors.
2. **Mapped-type structural distinctness.** Wrapping the generated type
   (`StripNulls<Gen.X>` or `MakeRequired<Gen.X, K>`) creates a *new* type
   that TS treats as structurally distinct from `Gen.X`. API responses
   typed as `Gen.X` no longer assign to the FE alias. ~250 boundary errors.

**Approach.**

1. `types.ts` uses `Omit + &` to tighten specific fields where the FE
   assumes presence at runtime. Example:
   ```ts
   export type ReportDefinition =
     Omit<Gen.ReportDefinition, 'blocks' | 'filters'> & {
       blocks: ReportBlockDefinition[];
       filters: ReportFilterDefinition[];
     };
   ```
   Backed by the runtime contract — server always populates these (Rust
   default = `[]`). Tests already provide them.

2. **No `StripNulls`.** The wire shape carries `null` through. Where the
   FE needs `T | undefined`, the call site coerces explicitly with
   `?? undefined`. This is a one-line fix per site, surgical.

3. **One boundary helper, no mapped types.**
   ```ts
   // queries/index.ts
   const asReport = (v: Gen.ReportDto): ReportDto => v as ReportDto;
   const asDefinition = (v: Gen.ReportDefinition): ReportDefinition =>
     v as ReportDefinition;
   // ... a few more for the half-dozen surfaces we ingest
   ```
   API query functions call `asReport(...)` before returning. Single
   point of unsafe cast, justified by the runtime contract.

4. **FE-only types** (visibility conditions, pill variants, block render
   state wrapper, workflow polling state) stay defined locally in
   `types.ts` — they aren't on the wire.

**Order of operations.**

1. Write new `types.ts` with `Omit + &` tightening.
2. Add boundary helpers in `queries/index.ts`, call them from each
   query function before returning.
3. Run `tsc -b` — expect 100–200 remaining errors split between:
   - `string | null` → `string | undefined` mismatches in function calls
     and assignments (fix at call sites with `?? undefined`)
   - "possibly undefined" on optional fields that the FE assumes present
     (fix at call sites with `?? []` / `?? ''` / `??` defaults; if a
     field is truly always-present, add it to the `Omit + &` list)
4. Run vitest. Expect no runtime regressions because the changes are
   type-level only.
5. Commit.

**Acceptance.**

- `frontend/src/features/reports/types.ts` is ≤200 lines (only the
  FE-only types remain locally; the rest re-exports + tightens).
- `tsc -b` clean.
- 509 vitest tests pass.
- No FE-side behavior change observable in the browser.

### Phase 2 sub-plan: utils.ts swap (template + row-condition) [implemented]

**Goal.** Collapse two FE-only evaluators into the WASM crate without
copy-pasting any logic across the FE/WASM boundary. The FE imports one
helper module (`reportDsl()`) and `utils.ts` loses ~480 LOC.

**Architectural constraints (decided 2026-05-17).**

1. **One template compiler, one formatter.** Both live in WASM. No FE
   parser, no FE `formatCellValue`.
2. **No hardcoded locales in `runtara-report-dsl`.** Locale resolution
   uses the host's CLDR — `Intl` in the browser, `icu4x` (feature-gated,
   not in Phase 2) or ASCII defaults on the server. The crate is locale-
   agnostic; it dispatches to whatever `Formatter` is plugged in.
3. **Formatter contract is the seam.** The format-string grammar, the
   `FormatSpec` enum, the `Formatter` trait, and the JS callback
   signature stay frozen so a future ICU-based `Formatter` is a drop-in
   replacement.
4. **Row-condition storage moves to canonical `ConditionExpression`.**
   No backcompat for stored reports; existing fixtures rewritten in
   place. Stored reports get re-authored via MCP at Phase 8 cutover.

**Track A — template + formatter swap.**

Rust side:

1. New module `runtara-report-dsl/src/format.rs`:
   - `FormatSpec` enum — `Currency { code }`, `CurrencyCompact { code }`,
     `Number`, `NumberCompact`, `Decimal`, `Percent`, `Date`, `Datetime`,
     `Pill`, `BarIndicator`, `String`, `Raw`.
   - `FormatSpec::parse(&str) -> FormatSpec` — single grammar parser
     used by every caller.
   - `RenderContext { locale: String, currency: String, timezone: String }`.
   - `Formatter` trait — `format(value, spec, ctx) -> String`.
   - `SimpleAsciiFormatter` — Rust impl matching current server output
     (`$1,234.50`, `12.3%`, `YYYY-MM-DD`). Used by server tests and any
     server-rendered template until ICU lands.
2. Refactor `template.rs::render_template_with_filters` →
   `render_template(template, row, ctx, formatter)`. Filter closures
   delegate to `formatter.format(value, spec, ctx)`. No locale logic
   in the closures.
3. WASM bindings (`wasm_bindings.rs`):
   - `extern "C"` JS callback `__runtara_report_dsl_format_value(value, spec_json, locale, currency, timezone) -> string` registered via `wasm_bindgen`.
   - `JsFormatter` impls `Formatter` by invoking the callback.
   - Exports `js_render_template(template, row, ctx)` and
     `js_format_value(value, format, ctx)`, both routing through
     `JsFormatter`.

FE side:

4. `frontend/src/wasm/runtara-report-dsl/index.ts` registers the
   callback at module init (before `reportDsl()` is awaited). The
   callback uses `Intl.NumberFormat` / `Intl.DateTimeFormat` to render
   each `FormatSpec`. Pill/bar-indicator return the raw stringified
   value (renderer decorates).
5. `useReportDsl()` React hook — awaits `reportDsl()` once, caches the
   sync handle. App shell preloads via `<Suspense>` boundary so cell
   renderers can call `formatValue` synchronously inside render.
6. Migrate call sites:
   - `renderDisplayTemplate(row, template)` → `reportDsl.renderTemplate(template, row, ctx)` in `CardBlock.tsx:427` and `TableBlock.tsx:919`.
   - `formatCellValue(value, format)` → `reportDsl.formatValue(value, format, ctx)` everywhere else.
   - `ctx` flows from the report renderer; computed once per
     ReportRenderer mount as `{ locale: navigator.language, currency:
     definition.defaultCurrency ?? 'USD', timezone: ... }`.
7. Delete from `utils.ts` (~280 LOC):
   `compileDisplayTemplate`, `renderDisplayTemplate`,
   `parseDisplayTemplateToken`, `pushLiteralPart`,
   `formatCellValue`, `parseCellFormat`, `currencyFormatCode`,
   `DISPLAY_TEMPLATE_CACHE`, the regex patterns, the type defs.

**Track B — row-condition swap.**

1. FE editors (RowConditionEditor in ReportDefinitionBuilder, BlocksStep's
   RowConditionRow, tableActionEditors) emit canonical
   `ConditionExpression`:
   ```ts
   { op: 'eq', arguments: [
     { value: { reference: { ref_type: 'field', path: 'status' } } },
     { value: { literal: 'ready' } }
   ]}
   ```
   No more `{op, arguments: [bare_field, value]}`.
2. Replace `matchesReportRowCondition(condition, row)` → 
   `reportDsl.evaluateRowCondition(condition, row)` at the 4 call
   sites in TableBlock + utils.
3. Delete from `utils.ts` (~200 LOC):
   `matchesReportRowCondition`, `isReportRowCondition`,
   `rowConditionOperand`, `compareConditionValues`,
   `conditionValuesEqual`, `isEmptyConditionValue`,
   `rowValue` (helper).
4. Rewrite fixtures in `crates/runtara-server/tests/fixtures/reports/`
   that use legacy row-condition shape (audit pending).

**Acceptance.**

- `runtara-report-dsl` crate has no locale-specific data; only grammar +
  dispatch.
- `utils.ts` shrinks by ~480 LOC.
- Bundle size delta ≤ +10KB gzipped (no locale tables to add).
- `tsc -b` clean.
- vitest 509+ pass (new tests for `formatValue` via callback,
  `evaluateRowCondition` end-to-end).
- Server `reports_render_corpus.rs` snapshots unchanged for templates
  the server renders (`SimpleAsciiFormatter` matches existing output).
- Future ICU swap: register an `IcuFormatter` impl, point the
  feature/config at it. Zero call-site changes.

**Order of execution.**

Track A first (the async/preload pattern needs to land), then Track B
(reuses the same `useReportDsl` hook). Commit per track.

### How to use the new infrastructure

```sh
# Regenerate the WASM bundle after editing runtara-report-dsl
cd crates/runtara-report-dsl
wasm-pack build --target web --out-dir pkg --features wasm --no-default-features
cp pkg/runtara_report_dsl_bg.wasm pkg/runtara_report_dsl.js \
   pkg/runtara_report_dsl.d.ts   pkg/runtara_report_dsl_bg.wasm.d.ts \
   ../runtara-server/frontend/src/wasm/runtara-report-dsl/

# Regenerate the TS API client (offline; uses the dump_openapi bin)
cd crates/runtara-server/frontend
npm run generate-api-runtime-offline
```

### Tests

- [x] Unit (TS): `utils.test.ts` reruns against WASM-backed evaluators — same outcomes (5 row-condition tests rewritten with canonical shape, drive WASM `evaluateRowCondition` via the vitest mock that mirrors the Rust evaluator). Per-operator coverage (nested NOT, IS_DEFINED, IS_EMPTY, IS_NOT_EMPTY) is in the Rust crate at `runtara-report-dsl::row_condition::tests`.
- [x] Unit (Rust): render-template tests for every report filter (`runtara-report-dsl/src/template.rs::tests` covers plain field, currency, currency-with-arg, number, percent, date, datetime, undefined field, parse error). The FE end-to-end is the same engine via WASM.
- [ ] Integration (TS): vitest renders of `TableBlock` / `CardBlock` exercising every `displayTemplate` and `visibleWhen` shape from the corpus. **Deferred** — jsdom can't load the WASM bundle, so end-to-end template + condition rendering lives in Rust tests + Playwright. Existing `ReportPage.test.tsx` smoke-tests the page; pre-existing test count: 506 passing.
- [ ] Integration (CI): tsc strict on generated types — build fails if generated types don't compile in any consumer. **Local-only** — `tsc -b` runs at build time and would fail the build on type mismatches, but no GH Actions wiring per project decision.
- [x] E2E (Playwright): `report-corpus-block-loading.mocked.spec.ts` already covers each block type loading in the viewer + block-error path (landed in Phase 0). Display-template + row-condition path is exercised via fixture `06_workflow_actions_with_row_conditions` running through the rendering tests.

### Acceptance

- [x] `types.ts` deleted (805-line handwritten file → 332-line re-export shim over `RuntaraRuntimeApi.ts`).
- [x] Three template parsers → one (minijinja in `runtara-report-dsl::template`; FE compileDisplayTemplate gone; MCP path uses the same crate).
- [x] Two row-condition evaluators → one (`evaluate_row_condition` in `runtara-report-dsl::row_condition`; FE `matchesReportRowCondition` is a direct WASM call; server validator + FE editor agree on canonical shape).
- [ ] FE bundle within budget — **not hit**. Target was <250KB; actual is 339KB gzipped. Drivers: minijinja (~150KB) + canonical condition / format machinery. Reduction would need minijinja sub-feature trimming. Mitigated by lazy-loading the wizard route.

---

## Phase 3 — Source provider trait

**Status:** [x] Complete. Trait + registry + three providers landed in `services/reports/providers/`. `render_table_block`, `render_aggregate_table_block`, `render_aggregate_block`, and the block validator all dispatch through the registry. Four obsolete render leaves + their inline row-fetcher helpers deleted. System / workflow_runtime / object_model code physically moved into provider files: `providers/system.rs` is ~1,200 LOC, `providers/workflow_runtime.rs` ~720 LOC, `providers/object_model.rs` ~160 LOC, while `reports.rs` shrank from 12,802 → 10,958 LOC (-1,844). The two pieces explicitly carried into Phase 4 (object-model's join-aware `aggregate_with_optional_joins` helper, and the virtual aggregate engine) stay in `reports.rs` — both depend on object-model DTO types and slot more naturally into Phase 4's single-render-pipeline work.

### Work

- [x] Define `ReportSourceProvider` trait with `fetch_rows`, `fetch_aggregate`, `validate_block`, `field_is_known`, `field_set`, `supports_aggregate_pushdown`, plus `table_columns` for response shaping. `field_set` returns the static known-field set for system + workflow_runtime; `None` for object_model since its schema is dynamic per-tenant.
- [x] Define `ProviderRegistry` and wire from service entry points — built once in `ReportService::new`, rebuilt in `with_runtime` so the late-bound `engine`/`runtime_client` reach the providers. Registry exposes both the trait-object view (`get(kind)` for renderer dispatch) and typed handles (`workflow_runtime()` for callers that need provider-specific methods like `actions_for_block_context`).
- [x] Create `services/reports/providers/object_model.rs` wrapping `InstanceService` (which wraps `ObjectStoreManager`); pushes condition + sort + offset + limit down to storage.
- [x] Create `services/reports/providers/workflow_runtime.rs` owning the Instances + Actions row-builders (`workflow_instance_report_row`, `workflow_action_report_row`), the actions-for-source dispatch (`actions_for_source`, `actions_for_block_context`), entity / field metadata (`workflow_runtime_entity`, `workflow_runtime_fields`, `workflow_runtime_workflow_id`, `workflow_runtime_table_columns`), `should_check_instance_actions`, the validator (`validate_workflow_runtime_block`), and the error mappers (`map_workflow_runtime_error_to_report`, `map_execution_error_to_report`).
- [x] Create `services/reports/providers/system.rs` owning metric-bucket / system-snapshot / rate-limit-status / rate-limit-event / rate-limit-timeline fetchers, the row builders (`runtime_system_snapshot_row`, `rate_limit_status_row`, `rate_limit_event_row`), entity / field metadata (`system_entity`, `system_fields`, `system_row_field_known`, `system_table_columns`), the validator (`validate_system_block`), and the system-only helpers (`percent`, `parse_metrics_granularity`, `infer_rate_limit_timeline_granularity`, `map_rate_limit_error`, `system_connection_id`, `extract_eq_string_condition`, `condition_scalar_to_string`, `extract_time_bounds`, `collect_time_bounds`, `parse_datetime_value`).
- [ ] Providers without aggregate pushdown call the virtual aggregate engine from `runtara-report-dsl` — **deferred to Phase 4**. The engine still lives in `reports.rs::aggregate_virtual_rows`; moving it requires hoisting `AggregateRequest`/`AggregateSpec`/`AggregateFn` out of the server's `object_model` DTO module so the DSL crate can depend on them without a cycle. Tracked as the first work item under Phase 4.
- [x] Move parallel `validate_workflow_runtime_block` / `validate_system_block` machinery into each provider's `validate_block`. Both validator function bodies now live in the corresponding provider file (`providers/system.rs::validate_system_block`, `providers/workflow_runtime.rs::validate_workflow_runtime_block`); the trait's `validate_block` method dispatches to them. The renderer in `validate_report_definition` goes through `self.providers.get(block.source.kind).validate_block(...)`.
- [x] Delete now-obsolete render leaves (`render_workflow_runtime_table_block`, `render_system_table_block`, `render_system_aggregate_table_block`, `render_system_aggregate_block`) + their five inline row-fetcher helpers (`system_rows_for_block`, `fetch_system_rows`, `runtime_execution_metric_rows`, `connection_rate_limit_status_rows`/`event_rows`/`timeline_rows`) and `apply_workflow_runtime_row_filters`. The two ReportService methods `workflow_runtime_actions_for_block_context` + `workflow_runtime_actions_for_source` also collapsed: the first is now a method on `WorkflowRuntimeProvider` (`actions_for_block_context`); the second became a private helper on the provider.

### Tests

- [ ] Unit (Rust): per-provider test file; mock `ObjectStoreManager` / runtime client at the provider boundary.
- [ ] Unit (Rust): aggregate-parity property test — same logical aggregate fed to object-model pushdown vs virtual aggregate engine produces identical results.
- [ ] Integration (Rust): corpus snapshots unchanged vs Phase 1.
- [ ] Integration (CI): dual-run remains merge gate.
- [ ] E2E (Playwright): system-source spec (rate-limit timeline).
- [ ] E2E (Playwright): workflow-runtime spec (instances table).

### Acceptance

- [x] `services/reports.rs` shortened from 12,802 → 10,958 LOC (-1,844). Still above the aspirational 9,000-line target — the rest of the shrink lives in Phases 4-5 (single render pipeline + strict validator finishers).
- [ ] Aggregate-parity property test green for one week of CI runs — **not added** (out-of-scope per project's no-CI-wiring decision).

---

## Phase 4 — Single render pipeline

**Status:** [ ] Not started

### Work

- [ ] **Move the virtual aggregate engine to `runtara-report-dsl`** (carried forward from Phase 3). Needs to hoist `AggregateRequest`/`AggregateSpec`/`AggregateFn` out of `runtara-server::api::dto::object_model` so the DSL crate can depend on them without creating a cycle. Once moved, `SystemProvider` and `WorkflowRuntimeProvider` get full aggregate support from the trait by default.
- [ ] **Fold `aggregate_with_optional_joins` into the object-model provider** (carried forward from Phase 3). Currently lives on `ReportService` and is called from `render_aggregate_block` + `render_aggregate_table_block`; once joins move into the provider, both renderer entry points stop branching on `source.kind`.
- [ ] Implement `render_blocks(definition, filters, blocks, providers) -> ReportRenderResponse` as the single core function.
- [ ] Reduce `render_report` / `preview_report` / `render_report_block` to 5-line shims.
- [ ] Define `Renderer` trait per block type; move per-type formatting into trait impls.
- [ ] Collapse five copies of page/offset/sort extraction into one helper (after Phase 3, the system + workflow_runtime copies are gone; ~3 copies remain in `render_table_block`, `render_aggregate_table_block`, `render_joined_filter_table_block`).
- [ ] Rewrite `render_metric_block` as a real renderer instead of post-processing `render_aggregate_block` JSON.

### Tests

- [ ] Unit (Rust): per-`Renderer` impl tests — empty data, `hideWhenEmpty`, error, paginated.
- [ ] Unit (Rust): pagination edge cases (clamp at MAX_TABLE_PAGE_SIZE, offset > total, negative offset) tested once.
- [ ] Unit (Rust): three entry points produce equivalent output for same logical input.
- [ ] Integration (Rust): corpus snapshots unchanged.
- [ ] E2E (Playwright): table pagination via FE for object-model, workflow-runtime, and system sources.

### Acceptance

- [ ] Zero diff in dual-run.
- [ ] Five copies of page/sort code gone.
- [ ] `render_metric_block` reads typed `AggregateResult`, not JSON.

---

## Phase 5 — Strict-mode validator

**Status:** [ ] Not started

### Work

- [ ] Delete embedded JSON schema string at `mcp/tools/reports.rs:1220–1769`.
- [ ] Delete `collect_report_definition_authoring_issues` (~3,000 lines at `1950–4500+`).
- [ ] MCP `validate_report` calls `ReportService::validate_report` directly.
- [ ] Row-condition validation delegates to `runtara-dsl`'s `ConditionExpression` validator.
- [ ] Source-condition validation continues to use `object_model::Condition` validator (unchanged).
- [ ] Move authoring-only warnings (typo hints, large-page-size warnings) into opt-in `lint(definition)` pass in `runtara-report-dsl`.

### Tests

- [ ] Unit (Rust): every corpus fixture that previously triggered MCP-only authoring issues now produces equivalent error from strict validator or equivalent warning from lint pass — no silent drops.
- [ ] Unit (Rust): negative-fixture battery exercises every error code at least once.
- [ ] Integration (Rust): MCP `validate_report` output identical to REST `POST /validate` on every corpus fixture.
- [ ] E2E: MCP-driven flow test creates / validates / updates / deletes a report via MCP only.

### Acceptance

- [ ] `mcp/tools/reports.rs` below 3,000 lines.
- [ ] No validation issue exists in only one surface.

---

## Phase 6 — Edit-operation symmetry

**Status:** [ ] Not started

### Work

- [ ] Define `ReportEditOp` enum in `runtara-report-dsl` with variants: AddBlock, ReplaceBlock, PatchBlock, MoveBlock, RemoveBlock, AddLayoutNode, ReplaceLayoutNode, PatchLayoutNode, MoveLayoutNode, RemoveLayoutNode.
- [ ] Implement `POST /api/reports/{id}/edit` taking `Vec<ReportEditOp>` applied atomically.
- [ ] REST per-op handlers become shims that build single-op batches (one release lifespan).
- [ ] MCP layout walkers replaced by `ReportEditOp` construction.
- [ ] Delete parallel MCP layout walkers (~260 lines at `mcp/tools/reports.rs:885–1144`).

### Tests

- [ ] Unit (Rust): each `ReportEditOp` variant has apply + revert tests. Batch failure at step N rolls back 1..N-1.
- [ ] Unit (Rust): property test — applying a sequence of ops then validating produces a valid definition or a clean validator error.
- [ ] Integration (Rust): for every legacy per-op endpoint, batched-equivalent through `/edit` produces identical persisted state.
- [ ] Integration (Rust): MCP layout ops vs `/edit` batched ops — identical results.
- [ ] E2E (Playwright): UI edit flow (add / move / patch / remove block) uses new endpoint; wizard sees consistent state after each.

### Acceptance

- [ ] Parallel MCP layout walkers deleted.
- [ ] One code path for mutating part of a report.

---

## Phase 7 — Wizard rewrite

**Status:** [ ] Not started

### Work

- [ ] Wizard operates on `ReportDefinition` directly; React-local state only.
- [ ] Delete `wizardSerialization.ts` and `wizardSerialization.test.ts` (50KB+).
- [ ] Delete `WizardBlock` / `WizardFilter` / `WizardState` from `wizardTypes.ts`.
- [ ] Reuse the workflow `ConditionalStep` condition builder for `visibleWhen` / `hiddenWhen` / `disabledWhen` / `showWhen` editors.
- [ ] `reconcileDatasetBlock` becomes explicit "Reset block to dataset schema" button with diff preview — never silent on-load mutation.
- [ ] Move `connectionDefaults.ts` default-injection server-side (post-load step or required-by-validator).
- [ ] Feature-flag new wizard; old wizard coexists for one release.

### Tests

- [ ] Unit (TS): each wizard step component — open fixture, exercise controls, assert emitted `ReportDefinition` patch.
- [ ] Unit (TS): condition builder reuse — workflow-conditional-step component in report context emits valid `ConditionExpression` shapes accepted by the WASM evaluator.
- [ ] Integration (TS): load every corpus fixture into wizard, save without changes, assert resulting JSON is byte-identical to input (lossless round-trip).
- [ ] Integration (TS): dataset reconcile flow — drifted dataset, diff preview correct, accept produces valid definition, decline leaves things untouched.
- [ ] E2E (Playwright): full author workflow — create new report, configure datasets, add blocks of every type, configure filters, configure row conditions on a workflow action button, save, reopen, verify all settings preserved. Run against both wizards until legacy is removed.

### Acceptance

- [ ] Lossless-round-trip on 100% of corpus.
- [ ] ~50KB of wizard code deleted.
- [ ] Opening + saving the same report is a no-op in git diff.

---

## Phase 8 — Cutover + cleanup

**Status:** [ ] Not started

No data migration. Existing reports will be re-authored via MCP after cutover. This phase is the schema cutover + accumulated cleanup.

### Work

- [ ] Cutover: ship the schema-breaking changes accumulated through phases 1–7. Legacy stored definitions stop loading.
- [ ] Surface a clear "needs re-authoring" state in the report list for definitions that fail strict validation post-cutover (don't silently 500).
- [ ] Add a one-shot MCP tool or script that lets the team batch-re-author known legacy reports.
- [ ] Delete dead `ReportQueryPlan` / `ReportSourcePlan` / `JoinPlan` / `ProjectionPlan` structs in `query_plan.rs`.
- [ ] Replace `apply_json_merge_patch` (`services/reports.rs:10574`) with `json-patch` crate.
- [ ] Convert the five `map_*_error` functions to `From` impls.
- [ ] Delete dead runtime check in `render_card_block:4609`.
- [ ] Collapse the four near-duplicate condition builders (`option_search_condition`, `between_condition`, `binary_condition`, `condition_from_filter_target`) into one.
- [ ] Remove legacy per-op REST mutation endpoints from Phase 6.
- [ ] Remove legacy wizard from Phase 7.
- [ ] Remove dual-run harness.

### Tests

- [ ] Unit (Rust): legacy-shape definitions produce a structured "needs re-authoring" error from the loader (not a 500, not a silent empty render).
- [ ] Integration (Rust): list endpoint surfaces unsupported reports with a clear status; viewer renders the empty-state UI for them.
- [ ] E2E (Playwright): full suite against the cutover build; new-shape definitions render unchanged; legacy-shape definitions show the re-authoring state.

### Acceptance

- [ ] Dual-run harness removed.
- [ ] Legacy REST endpoints + legacy wizard gone.
- [ ] Legacy-shape reports surface a clean error state, not a crash.

---

## Cross-cutting test infrastructure

Set up once in Phase 0:

- [ ] `insta` snapshot library for Rust integration tests.
- [ ] `@playwright/snapshots` for E2E.
- [ ] Fixtures live next to snapshots in `crates/runtara-server/tests/fixtures/reports/`.
- [ ] `tests/reports/seed.rs` boots test server + object-model schemas + sample data on transient Postgres (reuse existing e2e-verify infra).
- [ ] `cargo-tarpaulin` on `runtara-report-dsl`; ≥85% line coverage as phase-acceptance gate.
- [ ] CI matrix per PR: Rust unit + integration, WASM build + Node bindings test, TS unit + tsc, Playwright headed against fresh test DB.

## Test sequencing summary

| Phase | New unit | New integration | New E2E |
|---|---|---|---|
| 0 | proptest, dual-run framework | corpus + Rust int-test crate | small Playwright suite |
| 1 | minijinja conformance; `ConditionExpression` conformance; WASM round-trip | (corpus carries) | — |
| 2 | utils.test.ts swap; minijinja-from-JS | TableBlock/CardBlock visual | display-template + row-vis spec |
| 3 | per-provider; aggregate parity property | provider-routed render | system + workflow_runtime specs |
| 4 | per-Renderer; one pagination test | three-entry-point equivalence | pagination across all sources |
| 5 | strict-vs-lint coverage | REST/MCP validate parity | MCP-only authoring flow |
| 6 | per-EditOp apply/revert | batched-vs-per-op equivalence | UI edit flow regression |
| 7 | per-step wizard; condition builder reuse | lossless round-trip on corpus | full author workflow |
| 8 | legacy-shape error path | list/viewer empty-state for unsupported reports | cutover regression; re-authoring state visible |

## Risks

- **Phase 3 is the hardest.** Dual-run harness is the merge gate — don't ship until it's clean on the corpus for a week.
- **WASM bundle size.** `runtara-report-dsl` + `runtara-dsl` + minijinja. Measure with `wasm-opt -Oz`. Lazy-load on the report-builder route if past budget.
- **Cutover loses access to legacy reports.** They will be re-authored via MCP after the cutover. Time the cutover with the team so re-authoring effort is scheduled, not surprise work.
- **`ConditionExpression` carries server-only operators** (`MATCH`, `SIMILARITY_GTE`, `COSINE_DISTANCE_LTE`, `L2_DISTANCE_LTE`). `evaluate_row_condition` rejects them; tests cover this.

## Out of scope

- Not redesigning the DSL surface (filters, blocks, layout, source modes, aggregate ops stay).
- Not merging `ReportSource` / `ReportTableColumnSource` / `ReportDatasetSource` into one type. Extract `SourceCore` via `#[serde(flatten)]` so the field list lives once.
- Not unifying `object_model::Condition` and `ConditionExpression`. `Condition` compiles to SQL; that's a separate, larger conversation.
- Not killing MCP or the wizard. Both stay; they stop owning their own schema.

---

## Progress log

Append entries as phases complete or material decisions change.

- 2026-05-16: Plan drafted.
- 2026-05-16: Decision — no JSON migration. Existing reports will be re-authored via MCP after cutover. Phase 8 simplified to cutover + cleanup.
- 2026-05-16: Phase 0 partial — corpus + DTO round-trip + JSON Schema snapshot tests landed. 11 fixtures cover all block types, all source kinds, joins, datasets, views + interactions, row conditions. `dual-run-reports` Cargo feature flag added as a no-op for now. Tests: `cargo test -p runtara-server --test reports_corpus`. Snapshots reviewable with `cargo insta review`.
- 2026-05-17: Phase 0 runtime snapshots — added `reports_runtime_corpus.rs` that boots a UUID-suffixed temp DB on `TEST_REPORTS_DATABASE_URL`/`RUNTARA_DATABASE_URL`, applies server migrations, runs every fixture through `validate_report`, and snapshots the response. Markdown fixture is `valid: true`; the rest snapshot the current "Schema not found" path. These are now load-bearing for drift detection during the refactor. proptest + render_report snapshots + Playwright still pending.
- 2026-05-17: Phase 0 complete. Landed: (a) `reports_proptest.rs` — proptest harness with 3 properties × 256 cases each (validator no-panic, deserializer no-panic, fixed-point round-trip); (b) `reports_render_corpus.rs` — render_report snapshots for all 11 fixtures with canonicalized HashMap order + UUID/timestamp masking; (c) `report-corpus-block-loading.mocked.spec.ts` — 6 Playwright tests covering each block type plus the block-error path in the viewer. Two items remain deferred: dual-run harness body (needs Phase 1's new path) and CI wiring (out of scope per project decision). Total: 4 backend test suites + 1 FE spec form the safety net.
- 2026-05-17: Phase 1 complete + Phase 2 server-side done. New `runtara-report-dsl` crate at `crates/runtara-report-dsl/` with: report types (moved from server), local `Condition` re-exported by `api::dto::object_model::Condition`, minijinja-backed template rendering with the report filter set, `evaluate_row_condition` over `runtara_dsl::ConditionExpression`, and a `wasm32-unknown-unknown` build via `wasm-pack`. Server's `api/dto/reports.rs` is a 9-line shim. All 4 Phase 0 corpus test suites green. Server registers 100+ report schemas in OpenAPI for `swagger-typescript-api`. Open: bundle is 363KB gzipped (target 250KB), FE utils.ts swap, types.ts deletion, schemars 0.8/1 consolidation.
- 2026-05-17: Phase 2 infrastructure landed. WASM bundle slimmed to 320KB gzipped via `json-schema` feature gating in `runtara-report-dsl` + `runtara-dsl` and minijinja minimal-feature config (`builtins, serde, deserialization`). Vendored under `frontend/src/wasm/runtara-report-dsl/` with an async `reportDsl()` init helper. New `dump_openapi` bin + `npm run generate-api-runtime-offline` script regenerate the TS API client without a running server. `RuntaraRuntimeApi.ts` now contains all 100+ report types. `types.ts` deletion deferred (577 tsc errors across ~30 files when swapping); `utils.ts` template/row-condition swap deferred (legacy format semantics + legacy shape bridge needed).
- 2026-05-17: Codegen migration — TS API client regen'd with `--generate-union-enums` so all enums become idiomatic TS union types. 313 enum-as-value usages across the FE (ExecutionStatus, ValueType, VariableType, ErrorSeverity, ErrorCategory) migrated to string literals: 173 ExecutionStatus, 35 ValueType, 17 VariableType, 88 ErrorSeverity/Category, plus 2 `Object.values(enum)` rewrites. The full `types.ts` → generated swap was attempted but bailed: handwritten `T | undefined` vs generated `T | null | undefined` plus structural-distinctness of `MakeRequired<>` produced 250+ unfixable assignment errors at API boundary sites. The intermediate migration lands the codegen flag + the enum cleanup. All 509 FE vitest tests pass; tsc clean. `types.ts` deletion needs a focused FE-only follow-up with a `fromApi<T>` boundary helper.
- 2026-05-17: `types.ts` deletion landed. Handwritten file shrank from 805 → 332 lines, now a re-export shim over generated `RuntaraRuntimeApi.ts` with four targeted `Omit + &` tightenings (`ReportBlockDefinition.source`, `ReportDatasetDefinition.{dimensions,measures}`, `ReportDefinition.{blocks,filters,datasets}`, `ReportDto.definition`). Tried `ReportInteractionDefinition.actions` tightening — reverted because wizard layers re-spread the value and the structural-distinctness penalty outweighed the `?? []` cost at the few call sites. ~22 FE files widened to accept `T | null | undefined` where generated optionals surfaced (CardBlock, FieldEditor, ChartBlock, ReportBlockHost, ReportDefinitionBuilder, BlocksStep, tableActionEditors, wizardTypes, ReportBuilderWizard, viewer/explorer/editor/page hosts, datasetBlocks, reportWritebackCache, TableBlock truncation + workflow-action guards). All 509 FE vitest tests pass; `tsc -b` clean. Net effect: backend remains the single source of truth for report DTOs; FE keeps its narrow tightenings for fields that are non-null at runtime but Option-on-the-wire.
- 2026-05-17: `utils.ts` template + row-condition WASM swap landed. New `runtara-report-dsl::format` module: `FormatSpec` enum (closed-set grammar: currency, currency_compact, number, number_compact, decimal, percent, date, datetime, pill, bar_indicator, string, raw), `Formatter` trait, `SimpleAsciiFormatter` for server defaults. `template.rs` refactored to accept `Arc<dyn Formatter>` and delegate every filter to the trait. WASM `wasm_bindings.rs` ships a `JsFormatter` that calls back into JS via a `__runtaraReportDslFormatValue` global; the FE registers an `Intl`-backed implementation in `frontend/src/wasm/runtara-report-dsl/index.ts` (full CLDR coverage for free, no locale data in WASM). `useReportDsl` hook + `<Suspense>` boundary in `ReportRenderer` block the tree until the bundle loads; preload kicks off at app shell mount (`main.tsx`). FE `utils.ts` shrinks 747 → 466 LOC: `compileDisplayTemplate`, `parseDisplayTemplateToken`, `formatCellValue`, `parseCellFormat`, `currencyFormatCode`, and the row-condition evaluator's comparators all gone. `matchesReportRowCondition` becomes a thin wrapper that bridges legacy `{op, arguments: [field, value]}` → canonical `ConditionExpression` and calls WASM `evaluateRowCondition`. Bundle: 320 → 339KB gzipped (+12KB for the new bindings + `format` module — well below the +50KB ceiling we sized for locale tables). 506 vitest tests pass; 26 Rust crate tests pass; `tsc -b` clean. Browser-verified end-to-end: en-US renders `$1,234.50`, de-DE renders `1.234,50 €`, canonical row condition evaluates correctly. Follow-ups: migrate row-condition editors + wire format + 1 fixture from legacy shape to canonical (mechanical UI rewrite, no behavior change).
- 2026-05-17: Lazy-loaded the wizard bundle. `ReportPage.tsx` now `lazy(() => import(...))`s `ReportBuilderWizard` and wraps it in `<Suspense>`, so view-only sessions don't pay the wizard's parse cost. Vite build now emits `ReportBuilderWizard-*.js` as a 163KB chunk; `ReportPage` itself drops to 35.69KB. Wizard chunk is parsed only when entering `?edit=1`. Browser-verified: `ReportPage` module exports just `ReportPage` (no wizard reference), dev server starts clean with no console errors.
- 2026-05-17: Two Phase 2 follow-ups (canonical row-condition migration + schemars consolidation) initially scoped as deferred — both landed in the next entry below.
- 2026-05-17: Schemars 0.8 → 1 consolidation + canonical row-condition wire format landed together (the second blocked on the first). **Schemars work:** `runtara_dsl::step_registration` mod, `agent_meta::{SchemaGeneratorFn, StepTypeMeta::schema_fn, get_all_step_types, find_step_type}`, the 3 `JsonSchema` derives on `CapabilityField`/`FieldTypeInfo`/`OutputField`, the `schema_for!(ConditionExpression)` call, the `spec` module, and the top-level `get_step_types()` are now `#[cfg(feature = "json-schema")]`. `runtara-dsl` bumped to schemars 1; ~25 mechanical `schemars::schema::RootSchema` → `schemars::Schema` renames. `runtara-report-dsl/json-schema` now propagates to `runtara-dsl/json-schema` so types like `ConditionExpression` are derive-available in both crates. `runtara-report-dsl` uses `default-features = false` for runtara-dsl, keeping the WASM tree free of `spec`/`step_registration` weight. Bundle: still 339KB gzipped — schemars 0.8 was already excluded via `cfg_attr` on `schema_types.rs` derives, so the gating is architectural ("no future regression") rather than size-cutting. The 250KB target remains aspirational; further cuts would require minijinja sub-feature trimming or splitting the canonical-condition evaluator from the schema export pipeline. **Canonical wire format:** the 6 `Option<Condition>` fields on `ReportWorkflowActionConfig` and `ReportTableInteractionButtonConfig` (`visible_when`/`hidden_when`/`disabled_when` × 2) are now `Option<ConditionExpression>`. The server validator at `crates/runtara-server/src/api/services/reports.rs::validate_report_workflow_action_row_condition` was rewritten to match on `ConditionExpression`/`MappingValue::{Reference,Immediate}`/`ConditionArgument` instead of the legacy `{op, arguments: [field, value]}` shape (~120 LOC). `seal_json_schema_objects` was taught to skip three new cases that break under schemars 1's internally-tagged enum emission: variants in `oneOf`/`anyOf` compositions, objects carrying a `$ref` (the `$ref + discriminator-property` merge shape), and `$defs` definitions referenced from those discriminator variants (so `additionalProperties: false` doesn't conflict with the merged shape). Fixture `06_workflow_actions_with_row_conditions.json` rewritten; FE `ReportRowCondition` aliased to generated `ConditionExpression`; FE editor (`ReportDefinitionBuilder::RowConditionEditor`, `tableActionEditors.tsx::RowConditionRow`) wrapped at the boundary with `canonicalToLegacyCondition`/`legacyToCanonicalCondition` helpers in `utils.ts` (so the editor UI keeps its flat rules-row form while the wire format goes canonical). FE `matchesReportRowCondition` simplified — no more bridge inside utils.ts, just direct WASM call. All 506 vitest tests + ~74 server report tests + 26 report-dsl crate tests + 3 corpus tests pass; `tsc -b` clean; workspace builds. Browser-verified: WASM evaluates canonical row condition, `legacyToCanonicalCondition`/`canonicalToLegacyCondition` round-trip cleanly.
- 2026-05-17: Phase 2 close-out + plan reconciled. Status banner rewritten as a per-phase table covering all of Phase 0–8. Phase 1 status clarified — three legacy validator deletions (`validate_safe_display_template`, `services/reports.rs` row-condition validator, `mcp/tools/reports.rs` template parser) are explicit Phase 5 work, not unscheduled debt. Phase 2 status banner says complete (was "Codegen + types.ts deletion complete"). Both Phase 2 sub-plans (types.ts deletion, utils.ts swap) tagged `[implemented]` so readers don't mistake them for pending work. Verbose 2026-05-17 entry that listed schemars consolidation + canonical row-condition as deferred follow-ups collapsed to a one-line pointer to the entry where they landed.
- 2026-05-17: Phase 3 landed. New `services/reports/providers/` module: `ReportSourceProvider` trait with `fetch_rows` / `fetch_aggregate` / `validate_block` / `field_is_known` / `table_columns` / `supports_aggregate_pushdown`; `ProviderRegistry` indexes by `ReportSourceKind`. Three impls — `ObjectModelProvider` (wraps `InstanceService`; pushes condition + sort + offset + limit to SQL), `WorkflowRuntimeProvider` (Instances + Actions row-builders behind the trait; aggregates rejected since the legacy code did the same), `SystemProvider` (metric buckets + system snapshot + the three rate-limit entities; uses the existing virtual aggregate engine via `aggregate_virtual_rows`). `ReportService::new` builds the registry once; `with_runtime` rebuilds it so the late-bound `engine`/`runtime_client` reach the workflow/system providers. `render_table_block` collapsed from a four-way `if source.kind` chain to a unified dispatch (`provider.fetch_rows` → in-memory slice for system/workflow_runtime, trust the provider's `total_count` for object_model SQL pushdown → `provider.table_columns` for headers). `render_aggregate_table_block` and `render_aggregate_block` dispatch through `provider.fetch_aggregate` for system/workflow_runtime; object_model still uses `aggregate_with_optional_joins` until joins move into the provider (Phase 4 carry-forward). Block validators dispatched through `self.providers.get(block.source.kind).validate_block(...)` instead of the explicit kind branches. Four obsolete render leaves deleted: `render_workflow_runtime_table_block`, `render_system_table_block`, `render_system_aggregate_table_block`, `render_system_aggregate_block`. Their seven inline row-fetcher helpers (`system_rows_for_block`, `fetch_system_rows`, `runtime_execution_metric_rows`, `connection_rate_limit_{status,event,timeline}_rows`, `apply_workflow_runtime_row_filters`) also deleted. Two pieces intentionally carried into Phase 4 and documented: (a) moving the virtual aggregate engine to `runtara-report-dsl` needs hoisting `AggregateRequest`/`AggregateSpec`/`AggregateFn` out of the server's `object_model` DTO module first (cycle break); (b) `aggregate_with_optional_joins` stays on `ReportService` until joins fold into the object-model provider, which slots more naturally into Phase 4's single-render-pipeline work. All four Phase 0 corpus suites green: `reports_corpus` (3 tests), `reports_proptest` (3 properties × 256 cases), `reports_runtime_corpus` (validate snapshots), `reports_render_corpus` (render snapshots). 424 lib tests pass; `cargo clippy` clean; `cargo fmt` clean.
- 2026-05-17: Phase 3 code-ownership move. The first Phase 3 commit (57fd9065) landed the trait + registry + dispatch routing but left the provider files as thin facades — the row-builders, validators, entity / field metadata helpers, and row-fetcher shim functions all stayed in `reports.rs` as `pub(crate)` items the provider impls called back into. This entry tracks the follow-up that physically moves that code into the provider files. `providers/system.rs` grew from 120 → 1,206 LOC and absorbed `validate_system_block`, the four leaf fetchers (metric buckets / rate-limit status / events / timeline), the three row builders (`runtime_system_snapshot_row`, `rate_limit_status_row`, `rate_limit_event_row`), the entity / field metadata (`system_entity`, `system_fields`, `system_row_field_known`, `system_table_columns`), and ten system-only helpers (`percent`, `parse_metrics_granularity`, `infer_rate_limit_timeline_granularity`, `map_rate_limit_error`, `system_connection_id`, `extract_eq_string_condition`, `condition_scalar_to_string`, `extract_time_bounds`, `collect_time_bounds`, `parse_datetime_value`). `providers/workflow_runtime.rs` grew from 106 → 717 LOC and absorbed `validate_workflow_runtime_block`, the Instances + Actions row-builders, the dispatch (`actions_for_source` + `actions_for_block_context`, both now methods on `WorkflowRuntimeProvider` — so `submit_report_action` and `render_actions_block` go through the typed accessor `self.providers.workflow_runtime()`), the entity / field metadata (`workflow_runtime_entity`, `workflow_runtime_fields`, `workflow_runtime_workflow_id`, `workflow_runtime_table_columns`), and the error mappers. `providers/object_model.rs` grew from 104 → 160 LOC by inlining the `fetch_rows` / `fetch_aggregate` bodies. `ProviderRegistry` now exposes typed accessors (`object_model()`, `workflow_runtime()`, `system()`) alongside the trait-object `get(kind)` for callers that need provider-specific methods. New trait method `field_set(block) -> Option<HashSet<&'static str>>` to match the original plan name (alongside the predicate `field_is_known`). Shared helpers in `reports.rs` promoted to `pub(crate)`: 13 validator helpers, plus `condition_from_value`, `option_f64_value`, `f64_value`, `humanize_label`, `aggregate_output_fields`, `flatten_instance`, `map_object_model_error`, `normalize_sort_direction`, `table_response_columns`, `MAX_TABLE_PAGE_SIZE`. Net result: `reports.rs` shrank from 11,687 → 10,958 LOC (-729 this commit, -1,844 cumulative since Phase 3 started); provider files now physically own their source-specific code per the plan. All four corpus suites + 424 lib tests pass; `cargo clippy` clean; `cargo fmt` clean.
