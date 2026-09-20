# Codebase usage review

Reviewed 2026-09-20 against the current working tree, including the existing uncommitted trusted-capability changes. This is a static usage and architecture review, not a full correctness or security audit. The original review changed documentation only.

Follow-up: finding 4's 21 exports were rechecked and removed at the user's request, along with the newly orphaned `editReport` query wrapper and `validateFormDefinitionJson` bridge re-export. The `@lintignore` and shared-UI Knip exclusions were removed. Finding 4's inventory and line numbers below describe the pre-cleanup snapshot; the other findings remain review candidates.

Cleanup verification with pinned Node 22.12.0: Knip passes without those exclusions; all 1,513 tests across 134 files pass; the full frontend build, including browser-WASM generation, passes; ESLint passes with 34 warnings and no errors. The last bridge edit also passed focused ESLint and Prettier checks. An initial test/Knip attempt overlapped WASM regeneration and failed on missing generated imports; both checks passed after generation completed. Existing backend and generated-client worktree changes were preserved.

The workspace contains 52 Rust packages, including 27 standalone agent components. I found no package that can be classified as wholly unused: packages without incoming Cargo dependencies are application, browser-WASM, workflow-component, or build-tool entry points. The clearest cleanup opportunities are leftover session-token work, inactive cancellation plumbing, stale build inputs, and unused frontend exports.

[Component diagram and package map](component-diagram.md)

## Findings, in priority order

### 1. P2 — Discarded session token can prevent channel execution

**Confidence: high.** [channels/session.rs:445](../crates/runtara-server/src/channels/session.rs#L445) calls `session_token::sign`, stores the result as `_token`, and propagates an error before queuing the workflow. Nothing consumes that token. If `SESSION_TOKEN_SECRET` is unavailable and the secret has not already been initialized, an otherwise usable channel session fails for this unused operation.

The verifier and claims type in [session_token.rs:68](../crates/runtara-server/src/api/services/session_token.rs#L68) have only local unit-test callers. Session endpoints use tenant authentication, rather than this verifier. However, the HTTP session API **does** emit signed tokens: [sessions.rs:103](../crates/runtara-server/src/api/handlers/sessions.rs#L103) and `session_event_stream` retain a client-visible contract. Do not delete the whole module based on the unused verifier.

**Recommended change:** remove the discarded signing operation from the channel loop. Separately decide whether to implement or retire the future public-token API, preserving existing HTTP response compatibility. Validate channel intake without a signing secret in an isolated process because the secret is cached in a `OnceLock`.

### 2. P2 — Validator rebuild fingerprint includes unrelated components

**Confidence: high.** [build-validation-wasm.mjs:49](../crates/runtara-server/frontend/scripts/build-validation-wasm.mjs#L49) and [build.rs:243](../crates/runtara-server/build.rs#L243) fingerprint `runtara-agents`, `runtara-ai`, and `runtara-http`. The validator's current normal/build dependency graph contains only `runtara-dsl`, `runtara-workflows`, and `runtara-workflow-stdlib` as first-party dependencies.

Editing the unrelated tracked sources changes the fingerprint and unnecessarily rebuilds browser validation WASM; the server's `embed-ui` path can also rebuild the frontend afterward. These are stale dependency edges, not unused crates.

**Recommended change:** remove the six obsolete file/directory inputs from both lists together. Keep the shared fingerprint algorithm synchronized and verify that a relevant validator edit invalidates it while an unrelated HTTP/AI/host-agent source edit does not.

### 3. P3 — Old cancellation-map infrastructure has no producer

**Confidence: high for the shipped server.** [server.rs:1393](../crates/runtara-server/src/server.rs#L1393) creates an empty `running_executions` map. Its references only pass or retain it:

- [execution_engine.rs:343](../crates/runtara-server/src/workers/execution_engine.rs#L343) stores it under `allow(dead_code)`.
- [trigger_worker.rs:408](../crates/runtara-server/src/workers/trigger_worker.rs#L408) accepts it as `_running_executions` and never uses it.
- [shutdown.rs:275](../crates/runtara-server/src/shutdown.rs#L275) returns immediately when it is empty.
- [workers/mod.rs:19](../crates/runtara-server/src/workers/mod.rs#L19) defines `CancellationHandle`; no struct construction or map insertion was found in repository Rust sources.

Consequently, the map-based shutdown phase is inactive in normal server startup. This is **not evidence that workflow draining is broken**: [server.rs:2954](../crates/runtara-server/src/server.rs#L2954) separately calls the embedded runtime drain, which reaches Environment's active runners.

**Recommended change:** remove the redundant map, extraction implementation, constructor parameters, handle type, and obsolete drain phase together, or explicitly reconnect them if a separate caller-cancellation requirement exists. Retain and test the Environment drain. Public library constructors may have downstream consumers outside this repository.

### 4. P3 — Knip exclusions hide 21 unused frontend exports

**Confidence: high within this private frontend.** The checked-in [knip.json](../crates/runtara-server/frontend/knip.json) excludes `@lintignore` exports and all shared UI primitives. The normal scan reports zero issues; a temporary configuration removing only those two exclusions finds **19 value exports and two types**, with no unused files or dependencies. Independent symbol searches confirmed the listed symbols have no consumers in frontend source.

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

**Recommended change:** remove the obsolete OAuth stub first. Remove speculative application exports or attach a concrete reason to retain them. Treat the shared primitive aliases as optional library surface, not entire unused components. These are maintenance findings; bundle-size savings were not measured, and tree shaking may already eliminate the values.

### 5. P3 — Some production helpers are referenced only by tests

**Confidence: high for the examples below.** A production scan explicitly rooted at `src/main.tsx` exposes additional exports that the ordinary scan considers used because their unit tests import them:

| Source | Examples with test-only consumers |
| --- | --- |
| [useDialogState.ts:18](../crates/runtara-server/frontend/src/shared/hooks/useDialogState.ts#L18) | `useDialogState` |
| [layoutOps.ts:97](../crates/runtara-server/frontend/src/features/reports/components/wizard-v2/layoutOps.ts#L97) | `orderedBlocksFromDefinition`, `addBlock`, `removeBlock`, `updateGridItem` |
| [pipeline.ts:204](../crates/runtara-server/frontend/src/features/analytics/utils/pipeline.ts#L204) | `stepsAreMeasured` |
| [rust-workflow-validation.ts:250](../crates/runtara-server/frontend/src/features/workflows/utils/rust-workflow-validation.ts#L250) | `getStaticCapabilitySchemaWithRust` |

These are candidates to remove or connect to real UI behavior. Do not remove whole mixed-use modules: `layoutOps.ts`, for example, supplies other helpers to `GridContainer.tsx`. Tests that directly exercise retired helpers should be retired with them; keep integration coverage of the live editing paths.

The production scan returned 39 value exports, three types, one test setup file, and two dependency findings before manual filtering. It also reports intentional `__*ForTests` exports, `src/test/setup.ts`, and the build-time `tailwindcss-animate` plugin; these are **not** unused-code findings. `@testing-library/dom` is test infrastructure and is a dependency-classification candidate, not an instruction to uninstall it.

### 6. P3 — Rust manifests retain likely unused direct dependencies

**Confidence: medium; removal builds were not run.** A scan of all package source, tests, examples, benches, and build scripts found no references to the following dependency identifiers. Macros, feature unification, and link-only dependencies require compilation after each proposed removal.

| Package and manifest | Candidate dependencies |
| --- | --- |
| [runtara-core](../crates/runtara-core/Cargo.toml#L29) | `serde`, `thiserror` |
| [runtara-environment](../crates/runtara-environment/Cargo.toml#L50) | `tracing-subscriber` |
| [runtara-agents](../crates/runtara-agents/Cargo.toml#L53) | `strum`; dev dependencies `tempfile`, `tokio`, `wiremock`, `serial_test` |
| [runtara-server](../crates/runtara-server/Cargo.toml#L243) | Dev dependency `testcontainers-modules` |
| [runtara-report-dsl](../crates/runtara-report-dsl/Cargo.toml#L59) | Dev dependency `insta` |

Two other text-scan hits need feature analysis before classification: `runtara-sdk`'s `tracing-attributes` is named in its public `tracing` feature; `runtara-connections`' test dependency on `runtara-http` enables the native transport. Keep `runtara-agents`' optional `openssl`: its manifest explicitly documents a link/feature-only role. Also keep `md-5`, whose Rust import name is `md5`.

**Recommended change:** prune candidates one package at a time and run the owning package's relevant feature matrix. Do not infer that removing a direct dependency removes it from the transitive graph or guarantees a build-time improvement.

## Additional backend API candidates

The following public functions have no callers in repository Rust sources. They are lower-priority API-surface candidates, not proof that a published library can safely break compatibility:

| Source | Candidate |
| --- | --- |
| [runtime_client.rs:904](../crates/runtara-server/src/runtime_client.rs#L904) | `build_image_name` |
| [core_runtime/http_server.rs:877](../crates/runtara-server/src/core_runtime/http_server.rs#L877) | Convenience `run_http_server` wrapper; the underlying HTTP server remains active. |
| [execution_engine.rs:704](../crates/runtara-server/src/workers/execution_engine.rs#L704) | `release_durable_admission_for_instance`, `has_runtime`. Admission release itself is live through `ExecutionAdmissionLifecycleObserver` and the outbox. |
| [rate_limits.rs:762](../crates/runtara-connections/src/service/rate_limits.rs#L762) | `reset_window_counters`, `cleanup_old_events`. Neither method is wired to a caller; do not assume the advertised cleanup job exists because the method exists. |
| [runtara-text-parser/src/lib.rs:61](../crates/runtara-text-parser/src/lib.rs#L61) | `try_single_field_parse`, `is_message_schema`; the crate's parsing and collection helpers are live. |
| [runtara-ai/src/provider.rs:70](../crates/runtara-ai/src/provider.rs#L70) | `create_openai_model`, `create_completion_model`; the `_with_connection` variants and AI crate remain live. |

## Components deliberately retained

- `runtara-validation-wasm`: built by the frontend script and loaded by the browser, despite having no incoming Cargo dependency.
- `runtara-workflow-runtime`: built and composed as a WASM artifact, including the explicitly supported composed-runtime compatibility path.
- `runtara-agent-bundle-emit`: invoked by `scripts/build-agent-components.sh` to generate agent metadata.
- All 27 agent crates: discovered through the workspace build script and metadata/artifact registry. No static Rust call site is needed for dynamic capability dispatch.
- `runtara-agents`: still provides native SFTP execution, connection schemas, and the host S3 client. Its registry is partly legacy, but `execute_capability` and connection metadata are live.
- `runtara-agent-trusted`: used by current uncommitted host/compiler/provider work; it is not an orphan.
- WIT crates, generated TypeScript clients, generated browser bindings, public SDK APIs, feature-gated integration tests, and test mocks: lack of a default-build caller is insufficient evidence of disuse.
- `prototypes` and `spikes`: excluded from production cleanup conclusions.

## Original review verification and reproducibility

Completed:

1. Offline Cargo workspace metadata inventory (`cargo metadata --no-deps --format-version 1 --offline`): 52 packages.
2. Offline validator dependency tree (`RUSTC_WRAPPER= cargo tree -p runtara-validation-wasm --offline --edges normal,build`): confirms the three stale fingerprint dependencies are absent. The first attempt hit a sandbox error in `sccache`; disabling only that wrapper succeeded with the pinned Rust 1.97.0 toolchain.
3. Knip with installed dependencies and pinned Node 22.12.0: baseline zero issues; expanded and production scans as described above. The initial default-Node scan was repeated with the pinned version.
4. Searches of Rust module declarations, package references, frontend symbols, dynamic routes, component registration/build scripts, and the CI feature matrix. A filename/module heuristic found no obvious detached Rust source files; it is not a compiler-backed reachability proof.

To reproduce the expanded frontend check, copy `frontend/knip.json` to a temporary file, remove `tags`, remove only `src/shared/components/ui/**` from `ignore`, and run from the frontend directory:

```sh
node node_modules/knip/bin/knip.js --config /tmp/review-knip.json --reporter json
```

For the production check, use another temporary copy with `entry: ["src/main.tsx!"]` and `project: ["src/**/*.{ts,tsx}!"]`, then add `--production`. The explicit production root matters: simply adding `--production` to the repository configuration produced misleading dependency results, so those results were discarded. No repository lint configuration was edited.

Not run: Rust/frontend compilation, tests, Clippy, full frontend lint, component builds, database integration tests, or E2E suites. This review changes documentation only and does not claim build validation of the proposed removals. Database and external-integration checks require the isolated services and test configuration specified by CI; no credentials or environment files were inspected. `cargo-machete` was not installed, so Rust dependency findings remain source-based candidates.
