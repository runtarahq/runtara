# Codebase usage review

Originally reviewed 2026-09-20; current-state reconciliation: **2026-09-22**. Scope includes the existing uncommitted trusted-capability changes. This is a usage and architecture review, not a full correctness or security audit. Finding numbers are stable across follow-ups.

## Current status

| Finding | Status | Remaining action |
| --- | --- | --- |
| 1 — Discarded channel session token | **Resolved** | None. The fix has channel E2E regression coverage; public HTTP token behavior is preserved. |
| 2 — Validator fingerprint inputs | **Resolved** | None. Both fingerprint lists and their invalidation behavior were verified. |
| 3 — Cancellation-map scaffolding | **Resolved** | None. Public Rust constructor signatures changed; in-repository callers are updated. |
| 4 — Lint-hidden frontend exports | **Resolved and pushed**, `a8afd0ce` | None for the 21 exports and two subsequently orphaned wrappers. |
| 5 — Test-only frontend helpers | **Resolved in this cleanup** | None. Removed seven exports plus two newly orphaned exports; live editing coverage remains. |
| 6 — Rust direct dependencies | **Resolved and pushed**, `6768eb1f` | None for the 11 removed declarations. Retain the native-transport and link-only dependencies described below. |
| Additional backend API candidates | **Resolved in this cleanup** | Removed all ten functions/methods following explicit approval. External Rust callers must migrate if they used these APIs. |

All six findings and the ten additional backend API candidates are resolved. The public session-token API needs a separate compatibility/product decision before any wider retirement. No whole Rust crate has been confirmed unused. Findings 1–4 and 6 were committed previously; finding 5 and the additional backend API removals are included in this cleanup on `feature/code-cleanup`. Unrelated trusted-capability work remains outside this cleanup.

## Completed verification by cleanup

The results below belong to their respective cleanup stages; they are not a claim that every suite was rerun for this documentation refresh.

Initial frontend cleanup verification with pinned Node 22.12.0: Knip passes without the removed exclusions; all 1,513 tests across 134 files pass; the full frontend build, including browser-WASM generation, passes; ESLint passes with 34 warnings and no errors. The last bridge edit also passed focused ESLint and Prettier checks. An initial test/Knip attempt overlapped WASM regeneration and failed on missing generated imports; both checks passed after generation completed. Existing backend and generated-client worktree changes were preserved.

Test-only frontend export cleanup (2026-09-22), with pinned Node 22.12.0: all **1,482 tests across 133 files passed**, including 163 focused report/analytics/validation tests. The full TypeScript/Vite build and ordinary Knip scan passed; ESLint reported no errors and the same 34 warnings. Browser validation WASM was already up-to-date. Tests emitted local backend connection errors under the sandbox but no assertions failed; no browser E2E or service-backed tests were run for this frontend-only cleanup. The smaller suite reflects retired tests for the deleted helpers.

Rust dependency follow-up (2026-09-21): removed the ten original dependency candidates plus the SDK's redundant direct `tracing-attributes` dependency; finding 6 now lists all 11 removals. The public SDK `tracing` feature remains, and `tracing::instrument` continues to use the macro supplied through `tracing`. Retained `runtara-connections`' dev dependency on `runtara-http`, which selects the native backend for standalone tests. Cargo regenerated only the corresponding eleven lockfile dependency edges; no third-party package versions changed. That dependency cleanup did not edit Rust implementation files; the subsequent cancellation cleanup did. Existing trusted-capability changes were preserved.

Rust cleanup verification with pinned Rust 1.97.0:

- Per-crate tests passed for `runtara-core` (with `test-support`), `runtara-agents`, `runtara-sdk`, `runtara-report-dsl`, `runtara-environment`, and `runtara-server`: **1,773 tests passed**, with 21 pre-existing ignored doctests. Server tests ran with one test thread.
- Five cross-target `cargo check` runs passed: SDK WASI with and without tracing, workflow-runtime WASI, host-agent WASI, and report DSL browser WASM. These are compilation checks, not component execution tests.
- Native SDK checks passed with `http,native` and `embedded,tracing`, each with default features disabled. Standalone `runtara-connections --all-targets` also passed with its required transport dependency retained.
- Clippy passed with `--all-targets -- -D warnings` for all six changed crates, enabling core test support, report OpenAPI, server embedded UI, server database/Valkey/TLS integration targets, and Environment database integration targets. `cargo fmt --all -- --check` and diff whitespace checks passed.
- The first sandboxed SDK test run could not bind its local HTTP listeners. The rerun outside that restriction passed. Remaining native checks used an isolated build directory to avoid other Cargo jobs' shared build lock; all Cargo checks used offline dependency resolution and server checks used `SQLX_OFFLINE=true`.
- Database/Valkey integration targets were compiled and linted, but not executed against services. Full-workspace tests, component runtime integration tests, and E2E tests were not rerun for this manifest-only cleanup.

The workspace contains 52 Rust packages, including 27 standalone agent components. I found no package that can be classified as wholly unused: packages without incoming Cargo dependencies are application, browser-WASM, workflow-component, or build-tool entry points. The ten backend API candidates listed below have also been removed. Wider retirement of the public session-token API remains a separate compatibility/product decision.

[Component diagram and package map](component-diagram.md)

## Findings, in priority order

### 1. P2 — Discarded session token can prevent channel execution

**Resolved (2026-09-21).** Removed the discarded signing call and now-unused import from [the channel session loop](../crates/runtara-server/src/channels/session.rs). The session ID, tenant/trigger checks, deterministic activity identity, execution admission, and message processing are unchanged. Public HTTP session signing and the token emitted by the SSE API remain intact.

**Reproduced before fixing:** the channel E2E ran a fresh server with `SESSION_TOKEN_SECRET` unset and an empty local dotenv file, preventing both cached initialization and developer configuration from masking the failure. An authenticated Teams activity received HTTP 200, but no workflow instance appeared. The background task logged `Failed to sign session token: SESSION_TOKEN_SECRET environment variable is not set` and exited before queuing. The signing result had no consumer. Since the router had already returned success, the webhook caller did not receive the background failure.

**Regression coverage:** [test_channel_reflush_provenance.sh](../e2e/test_channel_reflush_provenance.sh) now exercises this missing-secret setup and checks persisted instances and channel replies rather than HTTP acknowledgement alone. The exact same test failed before the production fix and passed after it:

- First delivery produced one instance and one reply. The harness deliberately runs a failing workflow, so the expected reply confirms that execution was admitted and its result reached the channel.
- Redelivery of the same activity after clearing its Valkey dedup reservation still left one instance and one reply; the foreign session did not re-flush the reply.
- A distinct activity produced a second instance and reply.
- Public HTTP session creation still returned its expected missing-secret error, confirming that the channel fix did not relax or change that signing contract.

The changed server binary built successfully, and all 1,197 server unit tests passed, including the five session-token signing/verification tests. Clippy passed with all targets and warnings denied, enabling embedded UI and database/Valkey/TLS integration targets; those feature-gated service suites were compiled/linted, not run in full. Rust formatting, shell syntax, and diff whitespace checks passed. The E2E used isolated ports, a mock Teams authority/connector, dedicated Valkey, and uniquely named databases. `KEEP_DB=1` retained the test databases and logs; each run stopped only its own server, mock, and Valkey container. Other channel providers were not exercised end-to-end; they share the corrected session loop.

The verifier and parsed claims remain an API-surface candidate with only local unit-test consumers. The HTTP API still emits signed tokens in [sessions.rs](../crates/runtara-server/src/api/handlers/sessions.rs); retiring that contract is a separate decision, not part of this fix.

### 2. P2 — Validator rebuild fingerprint includes unrelated components

**Resolved (2026-09-21).** Removed the six obsolete manifest/source inputs from both fingerprint lists. Focused checks using the actual Rust and JavaScript fingerprint implementations confirmed identical hashes on the repository and fixtures: edits to all ten retained manifest/source inputs invalidate the hash, edits to the six removed inputs do not, and relevant source additions/removals invalidate/restore it. The browser validation WASM build passed, and an immediate repeat reported it up-to-date. Rust formatting, JavaScript syntax, and script Prettier checks passed. Full server/frontend builds and test suites were not rerun for this build-input-only change.

The removed inputs were `Cargo.toml` and `src/` for each of `runtara-agents`, `runtara-ai`, and `runtara-http`. These crates are outside the validator's dependency graph. Both [the frontend script](../crates/runtara-server/frontend/scripts/build-validation-wasm.mjs) and [the server build script](../crates/runtara-server/build.rs) now retain the workspace manifest/lockfile and the validator, workflows, workflow-stdlib, and DSL inputs. The shared hash algorithm is unchanged.

### 3. P3 — Old cancellation-map infrastructure has no producer

**Resolved (2026-09-21).** Removed the map, state extractor, engine/worker arguments, `CancellationHandle` and its re-export, the empty-map shutdown phase, and its otherwise unused `RuntimeClient::signal_shutdown` wrapper. `ShutdownCoordinator` now takes only `ShutdownGrace`; its shutdown signal, intake-worker tracking, timeout/detach behavior, and execution grace remain. The server still drains intake before Environment, keeps the internal API/core alive during the Environment drain, and preserves the dev-mode bypass. These removals change public Rust constructor signatures and remove the wrapper; in-repository callers are updated. The underlying shutdown signal and normal cancel/pause APIs remain supported.

Git chronology (all commits are ancestors of the cleanup branch): `5c9b1ea02` (2026-04-03) introduced the map already marked unused, with an ignored trigger-worker parameter; `ab29372d` (2026-04-16) added both its shutdown consumer and the registry-backed Environment drain; `4b9706da4` (2026-04-17) retained the unused map during execution-engine consolidation. History searches found no handle construction beyond its type definition. This was unused scaffolding from introduction in this repository, rather than evidence of a previously working map-based cancellation path. Preserve the internal-API drain ordering from `1a720035` (2026-06-16) and intake-worker draining from `95520ead` (2026-09-14).

Removal verification with pinned Rust 1.97.0:

- The five shutdown tests and 19 execution-engine tests passed, followed by all 1,255 server and 151 Environment tests (10 pre-existing ignored doctests). Server tests ran with one test thread.
- Clippy passed for server and Environment with all targets and warnings denied, enabling server embedded UI, database/Valkey/TLS integration targets, and Environment database integration targets. Those feature-gated service suites were compiled/linted, not run in full. Rust formatting and diff whitespace checks passed.
- `scripts/build-agent-components.sh` passed using an isolated build directory. The existing `MODE=graceful e2e/test_recovery_environment_restart.sh` then passed against the changed server, isolated ports, a dedicated Valkey container, and uniquely named test databases. SIGTERM produced `suspended` / `shutdown_requested` with 10 checkpoints and no force-stop; after restart the workflow completed with 20 rows and 20 distinct item indices. `KEEP_DB=1` retained the test databases; the harness stopped its server and removed its Valkey container. Abrupt-restart and disabled-recovery modes were not rerun.
- Source searches found no remaining Rust references to the removed symbols. Baseline comparisons confirmed the retained shutdown methods and unrelated existing worktree changes were unchanged.

### 4. P3 — Knip exclusions hide 21 unused frontend exports

**Resolved and pushed in `a8afd0ce`.** Removed 19 value exports and two types, plus the subsequently orphaned `editReport` query wrapper and `validateFormDefinitionJson` bridge re-export. The `@lintignore` and shared-UI exclusions are absent from [knip.json](../crates/runtara-server/frontend/knip.json). The backend report-edit endpoint and live validation/UI functions remain.

The following inventory and line numbers record the original, now-removed exports.

All paths below are relative to `crates/runtara-server/frontend/src/`.

| File | Unused exports | Assessment |
| --- | --- | --- |
| `shared/queries/index.ts:91` | `getAuthorizationRedirect` | Obsolete throwing stub. Working OAuth code already uses `getOAuthAuthorizeUrl` in `features/connections/queries/index.ts:162`. |
| `features/workflows/components/WorkflowEditor/EditorTable.tsx:28` | `EditorRow` | Unused React component. |
| `features/reports/hooks/useReports.ts:215` | `useEditReport` | Explicitly reserved for a future UI. The backend edit endpoint is not unused. |
| `features/reports/utils.ts:221` | `getReportViewGroupViewIds` | Unused helper. |
| `shared/forms/rust-form-validation.ts:49` | `validateFormDefinitionWithRust` | Unused wrapper; preserve the other live validation bridge functions. |
| `shared/utils/file-utils.ts:173` | `downloadFileData` | Unused helper. |
| `shared/utils/platform-info.ts:113` | `getPlatformColor`, `isKnownPlatform`, `getAllPlatforms`, `groupPlatformsByType` | Four speculative helpers. |
| `shared/constants/error-condition-templates.ts:185` | `ERROR_CODE_PATTERNS`, `ERROR_CATEGORIES`, `ERROR_SEVERITIES` | Three unused reference tables. |
| `shared/components/ui/popover.tsx:31` | `PopoverAnchor` | Unused primitive alias. |
| `shared/components/ui/dropdown-menu.tsx:193` | `DropdownMenuPortal`, `DropdownMenuSub`, `DropdownMenuRadioGroup` | Unused primitive aliases; other dropdown components are live. |
| `shared/components/ui/sheet.tsx:140` | `SheetTrigger`, `SheetClose` | Unused primitive aliases; the sheet itself is live. |
| `shared/queries/query-keys.ts:321` | `QueryKeys` | Unused type. |
| `features/workflows/queries/index.ts:30` | `AgentDetailsQueryContext` | Unused type. |

Bundle-size savings were not measured; tree shaking may already have eliminated the removed values.

### 5. P3 — Some production helpers are referenced only by tests

**Resolved in this cleanup (2026-09-22).** Rechecked production consumers and removed the seven test-only exports below. Retired their direct tests while preserving report editor, layout-operation, and round-trip coverage. The identity-edit round-trip now checks every block in each fixture, including unplaced blocks; capability-schema assertions use the live `getStaticAgentWithRust` API.

All paths below are relative to `crates/runtara-server/frontend/src/` and record the removed exports.

| Source | Removed exports |
| --- | --- |
| `shared/hooks/useDialogState.ts` | `useDialogState`; deleted the unused hook, its tests, and its barrel re-export. |
| `features/reports/components/wizard-v2/layoutOps.ts` | `orderedBlocksFromDefinition`, `addBlock`, `removeBlock`, `updateGridItem` |
| `features/analytics/utils/pipeline.ts` | `stepsAreMeasured` |
| `features/workflows/utils/rust-workflow-validation.ts` | `getStaticCapabilitySchemaWithRust` |

Also removed two newly orphaned exports: `collectLayoutBlockIds` from `layoutOps.ts` and the `getCapabilitySchemaJson` bridge re-export from `shared/lib/rust-validation-wasm.ts`, plus the private `stripBlockReferencesFromGrid` and `updateGridItemInTree` helpers. Generated WASM bindings are unchanged. Mixed-use modules and live `GridContainer.tsx` add/remove operations remain.

The original, pre-cleanup production scan returned 39 value exports, three types, one test setup file, and two dependency findings before manual filtering. It also reports intentional `__*ForTests` exports, `src/test/setup.ts`, and the build-time `tailwindcss-animate` plugin; these are **not** unused-code findings. `@testing-library/dom` is test infrastructure and is a dependency-classification candidate, not an instruction to uninstall it.

### 6. P3 — Rust manifests retain likely unused direct dependencies

**Resolved and pushed in `6768eb1f`; manifests rechecked 2026-09-21.** Removed 11 direct dependency declarations across six crates and their corresponding lockfile edges, with no third-party version changes.

| Package | Removed declarations |
| --- | --- |
| [runtara-core](../crates/runtara-core/Cargo.toml) | `serde`, `thiserror` |
| [runtara-environment](../crates/runtara-environment/Cargo.toml) | `tracing-subscriber` |
| [runtara-agents](../crates/runtara-agents/Cargo.toml) | `strum`; dev dependencies `tempfile`, `tokio`, `wiremock`, `serial_test` |
| [runtara-server](../crates/runtara-server/Cargo.toml) | Dev dependency `testcontainers-modules` |
| [runtara-report-dsl](../crates/runtara-report-dsl/Cargo.toml) | Dev dependency `insta` |
| [runtara-sdk](../crates/runtara-sdk/Cargo.toml) | Direct optional `tracing-attributes`; the public `tracing` feature remains supported through `tracing` |

Retained `runtara-connections`' test dependency on `runtara-http` because it enables native transport, `runtara-agents`' optional `openssl` for its link/feature-only role, and `md-5`, whose Rust import name is `md5`. The removal builds, tests, cross-target checks, and Clippy results are recorded above. Removing a direct declaration does not imply the package disappears from the transitive graph or guarantee a build-time improvement.

## Additional backend API candidates

**Resolved in this cleanup (2026-09-22).** Removed all ten public functions/methods after explicit approval. A fresh repository search confirmed that none had production, unit-test, integration-test, or doctest callers. These are Rust API removals, not HTTP endpoint removals; unknown external Rust consumers may require migration. The inventory below records their original locations.

| Original source | Removed API |
| --- | --- |
| [runtime_client.rs:884](../crates/runtara-server/src/runtime_client.rs#L884) | `build_image_name` |
| [core_runtime/http_server.rs:877](../crates/runtara-server/src/core_runtime/http_server.rs#L877) | Convenience `run_http_server` wrapper; the underlying HTTP server remains active. |
| [execution_engine.rs:698](../crates/runtara-server/src/workers/execution_engine.rs#L698) | `release_durable_admission_for_instance`, `has_runtime`. Admission release itself is live through `ExecutionAdmissionLifecycleObserver` and the outbox. |
| [rate_limits.rs:762](../crates/runtara-connections/src/service/rate_limits.rs#L762) | `reset_window_counters`, `cleanup_old_events`. Neither method was wired to a caller; removing them does not disable an existing cleanup job. |
| [runtara-text-parser/src/lib.rs:61](../crates/runtara-text-parser/src/lib.rs#L61) | `try_single_field_parse`, `is_message_schema`; the crate's parsing and collection helpers are live. |
| [runtara-ai/src/provider.rs:70](../crates/runtara-ai/src/provider.rs#L70) | `create_openai_model`, `create_completion_model`; the `_with_connection` variants and AI crate remain live. |

The text-parser README now lists only retained helpers. AI constructor documentation was moved onto the live `_with_connection` variants; callers wanting direct OpenAI access can pass `None` as the connection ID. The shutdown-aware HTTP server and lifecycle/outbox admission-release paths remain.

Removal verification with pinned Rust 1.97.0:

- **1,482 Rust tests passed:** 1,255 server tests (including default integration targets), 29 AI tests, 50 text-parser tests, and 148 connections unit tests. Eight existing doctests were ignored. Initial sandbox runs failed when local mock servers could not bind ports; reruns with local networking allowed passed.
- Clippy passed for all four affected crates with `--all-targets -- -D warnings`, enabling server embedded UI and database/Valkey/TLS integration targets. The AI crate also passed `cargo check --target wasm32-wasip2 --no-default-features`.
- Rust formatting and diff whitespace checks passed. No Rust references to the ten removed symbols remain. The pending frontend cleanup separately passed 1,482 tests, lint, build, and Knip as recorded above.
- Connections container-backed integration suites, server database/Valkey/TLS service suites, component execution tests, and E2E were not run for these unused-API removals. The feature-gated server targets were compiled/linted. Checks used the current worktree; unrelated trusted-capability changes were preserved and excluded from the cleanup commit.

## Components deliberately retained

- `runtara-validation-wasm`: built by the frontend script and loaded by the browser, despite having no incoming Cargo dependency.
- `runtara-workflow-runtime`: built and composed as a WASM artifact, including the explicitly supported composed-runtime compatibility path.
- `runtara-agent-bundle-emit`: invoked by `scripts/build-agent-components.sh` to generate agent metadata.
- All 27 agent crates: discovered through the workspace build script and metadata/artifact registry. No static Rust call site is needed for dynamic capability dispatch.
- `runtara-agents`: still provides native SFTP execution, connection schemas, and the host S3 client. Its registry is partly legacy, but `execute_capability` and connection metadata are live.
- `runtara-agent-trusted`: used by current uncommitted host/compiler/provider work; it is not an orphan.
- WIT crates, generated TypeScript clients, generated browser bindings, public SDK APIs, feature-gated integration tests, and test mocks: lack of a default-build caller is insufficient evidence of disuse.
- `prototypes` and `spikes`: excluded from production cleanup conclusions.

## Current-state refresh and reproducibility

Historical 2026-09-21 documentation refresh (before finding 5 was removed):

- Rechecked the remaining candidates: finding 5's seven exports still have only test consumers; the ten backend API candidates still have no in-repository callers. Finding 1 was subsequently reproduced and fixed; its before/after verification is recorded above.
- Rechecked the six manifests: all 11 removed declarations remain absent. Offline Cargo metadata still lists 52 workspace packages, including 27 standalone agents.
- The ordinary Knip scan passed again with pinned Node 22.12.0 and the current configuration. Rechecked the commit/worktree state. The completed frontend, dependency, channel-signing, validator-input, and cancellation-map cleanups are included on `feature/code-cleanup`.
- The verification records above are from completed implementation turns. No Rust tests, full frontend tests/build, or E2E suite was repeated solely for this documentation refresh.

Run the ordinary frontend usage check from `crates/runtara-server/frontend` with pinned Node 22.12.0:

```sh
node node_modules/knip/bin/knip.js --reporter compact
```

For a production-only scan, copy the current `knip.json` to a temporary config, set `entry: ["src/main.tsx!"]` and `project: ["src/**/*.{ts,tsx}!"]`, then run Knip with `--config <temporary-config> --production`. Keep the explicit production root: simply adding `--production` to the ordinary configuration produced misleading dependency results in the original review. The old step of removing lint exclusions is no longer necessary; they were removed by finding 4's cleanup. Treat test infrastructure, build plugins, and intentional test hooks separately from production removal candidates.

The original September 20 review was static and did not run compilation, test suites, Clippy, or E2E. Subsequent cleanup verification is recorded above and supersedes that original limitation for the completed changes. The frontend and backend API removals are recorded with their validation above. Full-workspace and full service-backed integration coverage remain outside the checks performed here.
