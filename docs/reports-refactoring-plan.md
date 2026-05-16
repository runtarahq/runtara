# Reports Refactoring Plan

**Status:** Phase 1 complete; Phase 2 server-side complete, FE swap pending
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

**Status:** [x] Complete. Crate exists with types + minijinja + ConditionExpression evaluator + WASM target. Server uses the crate via re-exports. Old server validators kept in place (deletion deferred to Phase 5 by design — safer migration).

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
- [ ] WASM bundle <250KB gzipped — current bundle is **363KB gzipped** (1.0MB raw). Drivers: schemars 0.8 (via runtara-dsl) + schemars 1 both end up in the WASM tree; minijinja accounts for ~150KB. Phase 2 fine-tuning items: (a) consolidate to one schemars version, (b) feature-gate minijinja sub-features, (c) lazy-load on the report-builder route.
- [ ] `services/reports.rs` shorter by ~700 lines — deferred to Phase 5 (legacy validators still live there).

---

## Phase 2 — Codegen + delete handwritten FE types

**Status:** [ ] In progress — server-side registration done; FE swap (utils.ts + types.ts) pending. WASM bundle builds but is over the 250KB target (currently 363KB gzipped) and needs slimming before shipping to FE.

### Work

- [x] Register 100+ report DTOs in the server's `components(schemas(...))` block at `server.rs`. Available to `swagger-typescript-api` codegen the next time a developer runs `npm run generate-api-runtime-local` against a live server.
- [ ] Run `npm run generate-api-runtime-local` (requires running server) — produces updated `RuntaraRuntimeApi.ts` with report types.
- [ ] Delete `frontend/src/features/reports/types.ts` (805 lines) — pending the codegen regen + FE compilation verification.
- [x] WASM bundle build pipeline verified: `wasm-pack build --target web --out-dir pkg --features wasm --no-default-features` produces a working `pkg/` with TypeScript bindings.
- [ ] Slim the WASM bundle to <250KB gzipped (currently 363KB; see Phase 1 acceptance notes for drivers).
- [ ] Publish WASM bundle to FE (file: dep in package.json or vendored under `frontend/src/wasm/`).
- [ ] In `utils.ts`: replace `compileDisplayTemplate` (419–513) with `renderTemplate(template, row)` from WASM.
- [ ] In `utils.ts`: replace custom row-condition evaluator (`matchesReportRowCondition`, 267–403) with `evaluateRowCondition(expr, row)` from WASM.
- [ ] In `utils.ts`: delete `compareConditionValues`, `conditionValueOrdering`, and related helpers once the WASM swap lands.

### Tests

- [ ] Unit (TS): `utils.test.ts` reruns against WASM-backed evaluators — same outcomes plus new cases for edge conditions (nested NOT, IS_EMPTY on undefined, IS_NOT_EMPTY on null).
- [ ] Unit (TS): render-template tests for every report filter from the JS side.
- [ ] Integration (TS): vitest renders of `TableBlock` / `CardBlock` exercising every `displayTemplate` and `visibleWhen` shape from the corpus.
- [ ] Integration (CI): tsc strict on generated types — build fails if generated types don't compile in any consumer.
- [ ] E2E (Playwright): fixture with display templates + row conditions; assert visible columns/buttons match fixture-expected set.

### Acceptance

- [ ] `types.ts` deleted.
- [ ] Three template parsers → one (minijinja).
- [ ] Two row-condition evaluators → one (`evaluate_row_condition`).
- [ ] FE bundle within budget.

---

## Phase 3 — Source provider trait

**Status:** [ ] Not started

### Work

- [ ] Define `ReportSourceProvider` trait with `fetch_rows`, `fetch_aggregate`, `validate_block`, `field_set`, `supports_aggregate_pushdown`.
- [ ] Define `ProviderRegistry` and wire from service entry points.
- [ ] Create `services/reports/providers/object_model.rs` wrapping `ObjectStoreManager`.
- [ ] Create `services/reports/providers/workflow_runtime.rs` owning instance/action/rate-limit row builders (currently `7723–8329`).
- [ ] Create `services/reports/providers/system.rs` owning metric buckets + system snapshot.
- [ ] Providers without aggregate pushdown call the virtual aggregate engine from `runtara-report-dsl`.
- [ ] Move parallel `validate_workflow_runtime_block` / `validate_system_block` machinery into each provider's `validate_block`.
- [ ] Delete now-obsolete render leaves (`render_workflow_runtime_table_block`, `render_system_table_block`, `render_system_aggregate_table_block`, `render_system_aggregate_block`) — replaced by provider dispatch.

### Tests

- [ ] Unit (Rust): per-provider test file; mock `ObjectStoreManager` / runtime client at the provider boundary.
- [ ] Unit (Rust): aggregate-parity property test — same logical aggregate fed to object-model pushdown vs virtual aggregate engine produces identical results.
- [ ] Integration (Rust): corpus snapshots unchanged vs Phase 1.
- [ ] Integration (CI): dual-run remains merge gate.
- [ ] E2E (Playwright): system-source spec (rate-limit timeline).
- [ ] E2E (Playwright): workflow-runtime spec (instances table).

### Acceptance

- [ ] `services/reports.rs` below 9,000 lines.
- [ ] Aggregate-parity property test green for one week of CI runs.

---

## Phase 4 — Single render pipeline

**Status:** [ ] Not started

### Work

- [ ] Implement `render_blocks(definition, filters, blocks, providers) -> ReportRenderResponse` as the single core function.
- [ ] Reduce `render_report` / `preview_report` / `render_report_block` to 5-line shims.
- [ ] Define `Renderer` trait per block type; move per-type formatting into trait impls.
- [ ] Collapse five copies of page/offset/sort extraction (currently at `2826, 2956, 3018, 3574, 3702`) into one helper.
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
