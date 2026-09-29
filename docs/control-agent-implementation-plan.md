# Control agent: implementation plan

This plan delivers the `control` agent (nine capabilities, `start` through `wait`) and the typed agent suspension behind `wait`, so a workflow can start, signal and coordinate child runs and park runner-free (headline: parallel approvals). The brief is [control-agent.md](control-agent.md); owner decisions D1-D8 in [control-agent-decisions.md](control-agent-decisions.md) override it.

**Changed after this plan (2026-09-28):** control `wait` and `poll-wait` were
removed before merge. Waiting on children is now the `WaitForInstances` step
([wait-for-instances-plan.md](wait-for-instances-plan.md)): it keeps the
durable wait records, D1/D4/D7 and the caps of slice 9, but is compiled
workflow code calling `runtara:workflow-wait@0.1.0` rather than a suspending
agent. Control has eight capabilities and none suspends. Typed agent
suspension (slice 10) stays as an extension point that no built-in agent uses,
and an agent may only wake on `at`. Mentions of `wait` below are historical.

## Scope
- **Typed suspension:** a `suspends: true` capability returns `completed` or `suspended { wakes, state }` via an additive `suspendable` interface; the host keeps the continuation (max 64 KiB) per operation; steps must be durable with a timeout.
- **Control service:** `runtara:control` host service and `runtara-agent-control` crate; a compiler-emitted `runtara:workflow-operation` scope makes mutations replay-safe under `(caller, op_hash)`.
- **Parent link and capacity:** `parentInstanceId` field/filter (D8); labels unique per parent, 1024 bytes; depth cap 16 (D6); `not-found`/`not-runnable` fail fast, not-compiled accepted (D7); all tiers; parked runs free their slot; control children cap at `max(1, floor(0.8 × limit))`; retryable `capacity` at the limit (D5; a limit ≤ 1 fails permanently with `CONTROL_CAPACITY_UNSATISFIABLE` because the parent holds the only slot).
- **Durable instance waits:** host-owned wait records, AFTER-trigger nudge, race-safe park, authoritative reconciler, fenced unlaunched-child outcomes.
- **Ownership:** one-level retention pin; required `parentClosePolicy` (`cancel`, preselected, or `leave_running`; cascade on any parent end, 5 s grace, D3); cancel before launch; parked runs pause immediately in control and API (D4, release note).
- **Hardening and upgrades:** import allowlist, `__rt_on_signal__` spoof fix, drop dead raw-signal wrapper and guest `RUNTARA_HTTP_URL`, `start_gate_failed` label; approved-digest history; parked runs survive cleanup, recompiles, upgrades.

## Not in v1
- Brief exclusions (report redesign, parallel embedded graphs, future-match subscriptions, compensation); caller user identity (authorization uses instance relations instead); force-terminate; pause propagation; lifecycle ABI changes.
- Control or suspending steps in AiAgent tools, `WaitForSignal.onWait`, published workflow-agents, legacy scoped isolation or Composed builds; suspending steps in onError. Parallel windows serialize them (W075).
- `Idempotency-Key` fingerprint conflicts, stricter public `resume`, six filed follow-ups. (Pruning pinned children, slice 12, and trusted option B, slice 11, are now implemented.)

## Where the code differs from the brief
1. Operation identity lives in `WorkflowState`; `CallContext` is off the production path.
2. The host cannot tell which composed component called, so control runs approved host-loaded bytes in fresh stores (`denied` stubs elsewhere); D2 adds load and per-call checks. *Changed after this plan (2026-09-29):* control is an ordinary composed agent calling `runtara:control/api` in the run's own store, the compile-time allowlist is the only gate, and the executor, pin, control history and load audit are gone ([control-simplification.md](control-simplification.md)).
3. No new lifecycle wake case (wasmtime 46 needs exact variants): the workflow parks `at(deadline)`, the host attaches waits.
4. The signal id is the target's WaitForSignal step id, narrowed by `action.key` (not `InputRequestSpec.signal_id`).
5. Admitted children lack a core row and expired `launching` requests can still launch; slices 7-8 add a fence and `queued`/`not-started`.
6. `RUNTARA_INSTANCE_ID` stays (the stdlib reads it); only `RUNTARA_HTTP_URL` goes.
7. A pinned child is cleanup-eligible once its parent is terminal and both are past retention; slice 8 rewords the brief.
8. Stale brief wait text ("a step rather than an agent capability"); slice 13 fixes it.

## Slices
| Slice | Title | Depends on | Ships |
|---|---|---|---|
| S0 | Risk spikes and gates (G0) | — | Spikes, race suites, S0.3 lifecycle test |
| D | Trusted-pin patch | — | Call-time pin check; per-compile pins recorded |
| 1 | Hardening | 19da3324 on `main` | Allowlist, spoof fix, env and enum cleanup |
| 2 | Run labels | — | 1024-byte labels; unused trigram index dropped |
| 3 | Contracts | 1; S0.2 | Metadata, WIT, contract crates, macro; no behaviour change |
| 4 | Validation and classification | 3 | E028/E029/E131/E132, W074-W078; serialized op sites |
| 5 | Executor and reads | 1, 3, 4, D | `get`, `query`, `list-pending-signals`; e2e 1 |
| 6 | Identity and mutations | 5 | `send-signal`, `cancel`, `pause`, `resume`; immediate pause; e2e 2 |
| 7 | Parent link and `start` | 2, 6 | `start` (capacity, depth, labels); `parentInstanceId`; e2e 3 |
| 8 | Ownership | 7 | Pin, launch fence, outcomes, close cascade, cleanup guard; e2e 4 |
| 9 | Durable instance waits | 3, 5, 6, 8; S0.4 | Wait store, trigger, reconciler, host wait |
| 10 | Typed suspension and `wait` | 6, 9; S0.2 | Parallel approvals; e2e 1-5 acceptance |
| 11 | Upgrade safety | 5, 8, 10, D | Cleanup and frozen-ABI tests, trusted option B (owner-approved, implemented); e2e 6, upgrade |
| 12 | Prune pinned children | 8, 9 | Implemented; prunes what retention would already have deleted |
| 13 | Surfaces and docs | 4-11 | UI, MCP, docs, changelog, `/steps` fixes; e2e 5.1 |

Slices land alone, V-fmt and V-gate green; 1-13 follow G0; if K1 is still open at G0, G0 exits for 1-9 only, slice 3 freezes the suspendable-only (K1) shapes, and 10 waits for the K1 outcome; D is independent, before 1. **Kill criteria:** K1 (S0.2 not green in 6 days): agent imports only `suspendable`; while unresolved, 1-9 ship without `wait`. K2 (S0.4 deadlock, lost wake, double fence winner, >15% overhead): drop the trigger's waiter stamp (keep `wake_pending`), then reconciler-only. K5 (digest history rejected): installed control digests only, no trusted option B.

## Cross-cutting
- **Generated:** `RuntaraRuntimeApi.ts` via `generate-api-runtime-offline` in slices 2-4, 6, 7, 13 (no CI drift check); control's committed `wit/agent.wit` from `build.rs`; `wit/deps`, agent catalog and `.sqlx` unchanged.
- **CI:** `wit-package` covers the new WIT; `components-build` gains `control_component` (5), Valkey (7), `control_wait_runner_test` (10); new features join `GATE_FEATURES`; TS drift, live e2e and upgrade are manual.
- **Security:** tenant, caller, operation only from `WorkflowState`/`CallContext`, never WIT args or input; `api` real only in `ControlExecutor` stores; allowlist, byte scan, per-call check (D2), reserved `control` slug. *Changed after this plan (2026-09-29):* `api` is real only for a run's own prepared entry, and the allowlist alone gates control; the byte scan and per-call check are gone. Authorization: reads tenant-wide; `wait`/`cancel`/`pause`/`resume` direct children; `send-signal` children, ancestors or `action.key` opt-ins (D1); no self-mutation; audit without payloads.
- **Migrations:** provisional numbers (core `032`-`042`); forward only, never edit, rename or `sed`; transactional; enum `ADD VALUE` alone; new CHECKs `NOT VALID` first; every new table has a removal path.

## Top risks
| Risk | Mitigation | Slice |
|---|---|---|
| Suspension breaks images or binds the wrong import | No lifecycle change (S0.3); S0.2 tracer; import-resolution tests; K1 | S0, 10 |
| Deadlock or lost wake across trigger, park, reconciler | `SKIP LOCKED` trigger; ground-truth reads; fixed lock order; S0.4 fuzz; K2 | S0, 9 |
| Launch race reports a running child `not-started` | Shared advisory-lock fence; the core row wins | 8 |
| Parallel windows mix op identities, or replay re-applies a mutation | Serialized operation sites; idempotency lookup first; intent-first receipts; crash tests | 4, 6, 7 |
| Upgrade strands parked parents, or a composed component reaches control | Revocable digest history; versioned continuation; cleanup guard; frozen-ABI, upgrade tests; allowlist | 1, 5, 8, 11 |
| Fan-out exhausts tenant or runner capacity | 80% control share; retryable `capacity`; Still open | 7, 13 |

## Verification
- Every slice: V-fmt (`cargo fmt --all -- --check`), V-gate (workspace clippy, `GATE_FEATURES` from `ci.yml`), `cargo test --workspace --lib`; `cargo test -p runtara-workflows` after emitter edits.
- By boundary: core, store-postgres, env and server DB suites, `migration_versions_test`; `build-agent-components.sh`, then component, direct-wasm, scoped-runner suites; frontend `tsc -b`, test, lint, build.
- E2E: `STAGES=<n> e2e/test_control_agent.sh`, plus `e2e/test_control_upgrade.sh` in slice 11, on an isolated server (own DBs, Valkey, `TENANT_ID`, ports), never `:7001`; crash windows: SIGKILL in a durable Delay.
- **Labels:** V-units `cargo test --workspace --lib` plus `--tests` for `runtara-agent-macro`, `runtara-agents`, `runtara-agent-mcp`; V-wf `cargo test -p runtara-workflows`; V-core `runtara-core --features test-support`; V-pg / V-env store-postgres / environment `db-integration-tests`; V-server server `db-integration-tests,valkey-integration-tests`; V-comp server `db-integration-tests,component-integration-tests`; V-mig `migration_versions_test`; V-stdlib `RUNTARA_ONLY_WORKFLOW_COMPONENTS=1 scripts/build-agent-components.sh`; V-build `scripts/build-agent-components.sh`; V-host component-host `component-integration-tests`; V-emit `agent_deadline_tests` (`direct-wasm-integration-tests`); V-exec `direct_wasm_execute`; V-scoped environment `scoped-workflow-integration-tests`; V-wit `wasm-tools component wit` on each new WIT dir; V-fe frontend regen, `tsc -b`, test, lint, build; V-valwasm `npm run build:wasm-validation`.

## Still open
- **Composed binding:** does any deployment set `RUNTARA_DIRECT_RUNTIME_BINDING=composed`? Until ops answers, slice 1 does not fail closed; slices 4 and 13 add a release note.
- **Launch-queue timeout:** children and woken parents can hit the 300 s timeout when the limit exceeds runner capacity; owner caps limits or exempts control children.
- ~~**K5 security review** of the digest history~~: trusted option B implemented (slice 11) and security-reviewed 2026-09-28: approved with no findings (guest cannot set launch kind, pins or bytes; Start never reads history; `admits` precedes every credential lookup; revocation fails closed; migration keeps the append-only trigger).
- ~~**Slice 12 trigger**~~: replaced by the owner; pruning runs every pass on exactly the children retention would already have deleted (slice 12).
- **Per slice:** cancel-reason source (8), trusted digest backfill (5), `/steps` pairing rewrite if duplication confirmed (13).

### Slice 0: Risk spikes and gates (G0)
**Goal.** Prove typed agent suspension via the direct compiler, the control executor, and race-free PG16 wait records; spikes seed the real suites. **Depends on.** Nothing; slices 1-13 start after G0 exit (1-9 only while K1 is open).

**Changes.**
- S0.1 (non-blocking, isolated live server): answering `legal` while parked on `WaitForSignal finance` should give `INPUT_NOT_FOUND`; record in `docs/control-agent.md`.
- S0.2 tracer bullet (6-day box, K1), all kept:
  - WIT: only host-called `runtara:control/execution.invoke` takes `continuation`; `suspendable.invoke` and `runtara:control/executor.invoke` do not.
  - Skeleton `crates/agents/runtara-agent-control` (`control_executor = true`, fake `wait`); minimal `ControlExecutor` in a fresh store; `denied` stubs for `runtara:control/api` and `/executor` in `registry::build_linker`.
  - `runtara:workflow-operation` scope keyed by `op_hash` (sha256 of canonical v2 Agent key).
  - `runtara-workflows/src/direct_wasm/`: imports keyed by `(agent_id, AgentInterface)`; suspending agents import `capabilities` and `suspendable`, wac wires both.
- S0.3 (merged): `prepared_launcher_tests.rs` proves the lifecycle `wake` ABI cannot change.
- S0.4: draft slice-8/9 migrations plus `register_or_evaluate`, `poll_wait`, `park_instance_on_targets`, `publish_external_outcome`; rows keyed `(waiter, op_hash)`; one `clock_timestamp()` rule; `replay-conflict`, `too-large` (`MAX_WAIT_TARGETS = 1000`), `not-started`, `not-found`.
- S0.5: no `-- no-transaction` in v1; `037_instance_parent_index.sql` (core) and `20260927000101_execution_request_parent_indexes.sql` (server) use plain `CREATE INDEX`; time `032`/`033`; K5 review of approved digests.

**Tests.**
- `direct_wasm/compile/agent_suspend_tests.rs` (`direct-wasm-integration-tests`): composition, per-site import, `SizeAlign` offsets, continuation relaunch, root gets `denied`.
- `runtara-store-postgres/tests/instance_waits_race.rs` (`db-integration-tests`): no lost wake, no `40P01`, one fence winner, S0.4 rules.
- Optional guard `runtara-server/tests/migration_versions_test.rs` rejects `CONCURRENTLY` misuse.

**Done when.** S0.2-S0.4 green or a K1/K2 fallback taken; K5 answered; V-fmt, V-gate, V-units, V-wf, V-build, V-host, V-emit, V-pg (PG16), V-wit, V-mig (if guard lands) green.

**Risks.**
- K1: proofs red after 6 days; the suspendable-only import fallback is pre-approved; the `__rt_suspended__` sentinel or a root-linked `api` needs the owner. Slices 1-9 ship without `wait` meanwhile.
- K2: deadlock, lost wake, double fence winner or >15% overhead; drop the trigger's waiter stamp, keep `wake_pending`, then reconciler-only scans.

### Patch D: trusted pins checked at the call (standalone)
**Goal.** A trusted-agent upgrade (S3, Azure) fails at the call, not at load, and readiness recompiles. Independent of G0; lands on `main` before slice 1.
- `linker_with_trusted_pins` (`runtara-component-host/src/workflow.rs`) drops the load-time match; per-call `TRUSTED_VERSION_REQUIRED` stays. Server forward migration `crates/runtara-server/migrations/20260926000100_compiled_trusted_pins.sql` (provisional) records pins; every `compiler_provenance_matches` caller uses "all pins installed".
- A parent composed from a workflow-agent published before the upgrade still carries the old pin, so its recompile is refused (`StaleTrustedDependency`, naming the workflow-agent) and recorded as a terminal failure with that pin. Republishing the workflow-agent releases every such failure in the tenant for one retry on the next launch; no forced recompile is needed. Operators must still republish each workflow-agent that uses S3 or Azure after an upgrade of either.
- Done when `direct_wasm_execute.rs` (stale pin loads, call fails without resolving credentials) and a server readiness test pass, V-fmt/gate/units/build/host/exec/server/mig green, and a live `s3-storage` upgrade fails only at the call. Risk: a retryable 409 `NotCompiled` recompile wave per release.

### Slice 1: Hardening and hygiene
**Goal.** Add an agent import allowlist, block the `__rt_on_signal__` spoof, add `start_gate_failed`, and remove the raw-signal wrapper and guest `RUNTARA_HTTP_URL`. Valid workflows are unchanged, except failed start gates now end as `start_gate_failed`.
**Depends on.** No slice; land after `feature/remove-internal-api` (19da3324) merges (both touch `server.rs`).
**Changes.**
- `runtara-workflows` (`compile/artifact_metadata.rs`): reject any unlisted agent import with a `DirectCompileError`. Allow `wasi:*`, the `build_linker` interfaces and `runtara:agent/types@`; explicit list that slices 3/5 extend. Never `runtara:workflow-operation/`.
- Staged workflow-agents skip the list but are denied `runtara:control/` and `runtara:workflow-operation/`.
- `runtara-workflow-stdlib` (`direct_json.rs`): remap guest `__rt_on_signal__` to `__rt_on_signal__:user`; append `on-signal-remap=v1` to `direct_lowering_tag`.
- `runtara-server`: delete both `send_custom_signal` wrappers; add missing labels incl. `StartGateFailed` to `TerminationReason` (`runtime_types.rs`).
- `runtara-component-host`, environment `runner/`, `embedded_runtara.rs`: drop `core_http_url`, `DispatcherEnv`, `core_client_addr`. No composed fail-closed until ops confirms.
- Fix stale docs (workflow-agent suspend, Composed binding, Agent-step `timeout`).
**Migrations.** `crates/runtara-environment/migrations/20260926000200_start_gate_failed_termination.sql`: add enum value `start_gate_failed` only (000000 and 000100 are taken by runtara-server migrations, and server and environment versions must not overlap).
**Tests.**
- Unit: stdlib remap; `direct_lowering_tag`/`required_stdlib_markers` assertions; server `runtime_types` label round-trip.
- `runtara-workflows` lib tests (WAT components): forbidden imports rejected, allowed pass, incl. staged agents.
- `bundled_agents_satisfy_import_allowlist` (`direct-wasm-integration-tests`).
- `runner/common.rs`, `runner/embedded.rs`: no guest env holds `RUNTARA_HTTP_URL`.
- `launch_queue_test.rs` (`db-integration-tests`): failed start gate ends `start_gate_failed`.
**Done when.** V-fmt, V-gate, V-units, V-wf, V-stdlib, V-build, V-host, V-exec, V-env, V-mig and server `outbound_http` (`db-integration-tests,component-integration-tests`) pass; `wasm_emitter_audit.rs` output unchanged; e2e `test_durable_delay_parks_and_wakes.sh` completes on an isolated server.
**Risks.**
- A bundled agent may import something unlisted: check `wasm-tools component wit` first.
- The lowering-tag bump recompiles every workflow once; needs a release note.

### Slice 2: Run labels up to 1024 bytes
**Goal.** Raise the run-label limit from 250 to 1024 printable ASCII bytes on every surface.
**Depends on.** Nothing.
- `runtara-dsl`: `MAX_RUN_LABEL_LENGTH = 1024` in `src/run_label.rs`; README says 1-1024.
- `runtara-server`: label docs in `api/dto/workflows.rs` (OpenAPI), `mcp/tools/{executions,workflows}.rs`, `mcp/server.rs`. Keep the 250-char `search` limit.
- Frontend: `maxLength={1024}` in `InvocationHistoryFilters.tsx`; regenerate `RuntaraRuntimeApi.ts`.
- `CHANGELOG.md`: 1024-byte labels; first-boot index drop, btree rebuild under lock.
**Migrations** (`runtara-store-postgres/migrations/postgresql/`, plain transactional, numbers provisional; never edit 025/029, never `sed`):
- `032_run_label_1024.sql`: `statement_timeout = 0`, no `lock_timeout`; drop `valid_run_label` and unused `idx_instances_run_label_search`; widen to `VARCHAR(1024)`; re-add CHECK `NOT VALID`.
- `033_validate_run_label_1024.sql`: validate CHECK; `ANALYZE instances (run_label)`. Btree `idx_instances_tenant_label_created` stays.
**Tests.** 1024 accepted, 1025 rejected: `runtara-dsl` units, core `conformance.rs` (memory+PG), env `db/integration_tests.rs` and server `execution_outbox_test.rs` (`db-integration-tests`), `workflow_runtime.rs` provider, `e2e/test_start_run_labels.py`.
- New PG probe, `runtara-store-postgres/tests/conformance.rs` (`db-integration-tests`): seeded 031-to-033 upgrade drops the index, keeps btree valid, validates CHECK, restores stats.
**Done when.** 1024 bytes round-trip admission, launch and label filter; 1025 rejected by `normalize_run_label` and DB CHECK.
- V-fmt, V-gate, V-units, V-core, V-pg, V-env, V-server (with `migration_versions_test`), V-fe green; e2e script passes on an isolated server. No e2e stage.
**Risks.**
- Constant raised before the DB widens: launches fail after `start` succeeded. Ship both in one binary.
- First boot locks `instances` for the btree rebuild: <1 s at <10^5 rows, unverified in prod.

### Slice 3: Contracts (metadata, WIT, contract crates, macro)

**Goal.** Freeze every new ABI and metadata contract with no behaviour change; later-slice functions return `unsupported`, so 0.1.0 never bumps.

**Depends on.** Slice 1 (import allowlist). Merge after S0.2 confirms the shapes.

**Changes.**
- **Metadata** (`runtara-dsl/src/agent_meta.rs`, macro `lib.rs`): `suspends: bool` on `CapabilityArgs`, `CapabilityMeta`, `CapabilityInfo` (omitted when false).
  - Add `CONTROL_AGENT_ID = "control"`, `AgentCatalog::capability_suspends`, per-capability `is_operation_scoped`.
  - Add `rateLimited`/`trusted`/`suspends` to `spec/agent_openapi.rs`; regenerate `RuntaraRuntimeApi.ts`.
- **Server:** overlay (`workflow_agents.rs`) treats `suspends` like `trusted`; `slug_reserved` reserves `control`; boot check logs and excludes slugs folding onto `control`.
- **`runtara:agent-suspension@0.1.0`** (new `crates/runtara-agent-suspension/`, serde-only natively): `types` (`wake`, `suspension`, `outcome`), `context.continuation()`.
  - Rust `SuspendContext`, `Suspendable<T>`, `Wake`; `SUSPENSION_UNSUPPORTED`, `MAX_CONTINUATION_BYTES` (64 KiB), `MAX_WAKES`/`MAX_WAKE_ID_BYTES`.
  - Template `suspendable-agent.wit.in` adds `suspendable.invoke`; `runtara:agent@0.4.0` unchanged; `trusted`+`suspends` rejected; allowlist admits `context` only for `suspends` agents.
- **`runtara:workflow-operation@0.1.0`** (`runtara-workflow-wit/wit/operation/`): sync `scope` = `enter`, `suspend`, `exit`, `release`; compiler-only.
- **`runtara:control@0.1.0`** (`runtara-workflow-wit/wit/control/`): typed records; no tenant/parent/operation params.
  - `types`: `parent-close-policy {cancel, leave-running}`, `instance-status` (with `queued`, `not-started`), 19 `error-code`s (incl. `not-child`, `requires-instance`, `requires-operation`).
  - `api` (10 async): `start`, `get`, `query`, `list-pending-signals`, `send-signal`, `cancel`, `pause`, `resume`, `wait`, `poll-wait`.
  - `executor` (composed-copy import; slice 5 checks approval at load and per call), `execution` (export, takes continuation); worlds `control-client`, `control-agent-host`.
  - Doc comments pin: `send-signal` to children, ancestors or `action.key` opt-in; `wait`/`cancel`/`pause`/`resume` children only; depth 16 `invalid`; `start` `not-found`/`not-runnable`, uncompiled accepted; `capacity` retryable; parked pause immediate.
- **Constants** (`runtara-workflow-wit/src/lib.rs`) in allowlist: `runtara:control/*` only for canonical `control`; `workflow-operation` for no agent; staged workflow-agents denied both.
- **`crates/runtara-control-contract/`**: size/paging caps, `MAX_LINEAGE_DEPTH` = 16, `control_share` = `max(1, floor(0.8 × limit))`, retry jitter, cancel bounds (5 s parent-close grace), tag `runtime:requires-run`, `CONTROL_CONTINUATION_V1`.
  - Codes `CONTROL_<CODE>`; `capacity` → `CONTROL_CAPACITY_RATE_LIMITED` (retryable) or `CONTROL_CAPACITY_UNSATISFIABLE`.
  - `start` input: `parentClosePolicy` required, `[cancel, leave_running]`, no default; editor preselects `cancel`.
- **Macro** (`runtara-agent-macro/src/component.rs`): `suspends = true` needs `fn(I, &SuspendContext) -> Result<Suspendable<O>, E>`; `suspending = [...]` exports `suspendable.invoke`, asserts metadata; `control_executor = true` forwards to `executor`, generates `execution.invoke`.
- **Workspace/CI:** both crates in root `Cargo.toml`; CI `wit-package` parses the 3 new WIT dirs.

**Tests.**
- `runtara-workflow-wit` lib: packages parse; functions and enums pinned; `SizeAlign` parity of wake/outcome types.
- `runtara-dsl`: `suspends` default, omission, round-trip; `is_operation_scoped`.
- `runtara-workflows` allowlist: suspension import needs `suspends`; `runtara:control/*` only for `control`.
- `runtara-agent-macro` (`cargo test --tests`, new `trybuild`): metadata mismatch is a compile error; signatures; `control_executor` expansion.
- `runtara-server` lib: overlay rejects `suspends`; `control` slug reserved; boot collision check.
- Contract crates: `control_share` values, unique codes, parent-close reason.

**Done when.** `suspends` round-trips meta.json, catalog API and TS; macro builds suspending and control-executor fixtures; V-fmt, V-units, V-wf, V-wit, V-build, V-fe, V-gate green; no diff in existing `agent.wit`, `tests/catalog/agent_catalog.json`, `wit/deps`. No e2e stage.

**Risks.**
- Older compilers/servers ignore `suspends`; safety = export shape + plain `invoke` refusing.
- `suspendable.invoke` and `capabilities.invoke` share a signature, so miswiring passes validation; S0.2 proves each site.

### Slice 4: Validation and compiler classification (inert)

**Goal.** Enforce where operation-scoped (suspending or control) steps may appear (save, compile, browser); serialize them in parallel lowering. No existing workflow changes shape. **Depends on.** Slice 3.

**Changes.**
- `runtara-dsl`: `step_context_rules` table (v1 matrix) feeds validator codes and authoring schema. Allowed: top level, branch arms, sequential loops, embeds, retries. Serialized (W075): parallel Split, branch groups. Suspending only: non-durable E028, timeout none/0 E029, onError E131. Both kinds: `onWait`, AiAgent tool/memory (E131/E132).
- `validation.rs`: phase `validate_suspending_steps` after `validate_agents`, recursing loops, `onWait`, embeds; tracks durability (manifest rule), retries, AI edges, fan-out, onError region.
  - E028 `SuspendingCapabilityNotDurable`, E029 `SuspendingCapabilityMissingTimeout`, E131 `SuspendingCapabilityUnsupportedContext`, E132 `ControlCapabilityUnsupportedContext`.
  - W074 `ConstantRunLabelInLoop`, W075 `SerializedOperationScopedStep`, W076 `OperationScopedStepUnderEnclosingRetry`, W077 `DynamicControlStartTarget`, W078 `WaitTimeoutBelowDeadlineMargin`.
  - `validate_workflow_closure`: child durability, embed as AI tool, W076 at embed sites.
- `direct_wasm/`: `DirectAgentManifest.suspends`/`.operation_scoped`; `static_data.rs` accessors; `plan.rs` Agent plans gain `suspends`.
- `split_parallel.rs`, `branch_parallel.rs`: exclude these sites as for workflow-agents (mandatory); update the existing W073 Split-parallelism advisory text.
- `DirectCompileError` backstops: `CliRunHttp`/`AgentCapabilities` ABI, `Composed`, scoped isolation, untimed/non-durable suspend, AI tool, no catalog, sidecar `suspends` mismatch.
- `analyze_workflow_agent_safety` takes the catalog, walks the closure; publish refuses `suspending-capability`, `control-agent` (not parking sites); compile refuses suspending workflow-agents.
- Server/browser: `ValidationErrorDto` arms; `runtara-validation-wasm` runs single-graph rules; browser catalog carries `suspends`.
- Docs: matrix in authoring schema; AgentStep `timeout`/`durable` docs (regen `RuntaraRuntimeApi.ts`); release note: `composed` binding cannot compile these steps (use unconfirmed).

**Tests.**
- `validation.rs` lib tests: every rule, onError join, warning edges.
- `tests/validation_integration_test.rs`: non-durable child, embed as AI tool, W076 at call site; `support.rs`: publish refusals.
- Hermetic emitter tests: backstops, sidecar mismatch, serialized Split body/branch group.
- `tests/wasm_emitter_audit.rs` (`compiler`): manifests without such sites byte-identical.
- Server lib: DTO mapping; authoring schema renders every rule.

**Done when.** Every rule has a stable code (server, browser, compile); V-fmt, V-units, V-wf, V-gate, V-valwasm, `generate-api-runtime-offline`, `npx tsc -b` pass; registry sweep clean (`ValidationError`/`Warning`, `DirectRunPlan`/`DirectAiToolPlan::Agent`, `DirectAgentManifest`). No e2e stage.

**Risks.** Durability sources disagree (`support.rs`, DSL doc, manifest): follow the manifest, file a follow-up. A launch pass missing the exclusion runs them concurrently; the plan test guards it.

### Slice 5: Control executor and reads

**Goal.** Workflow steps can call `control:get`, `query` and `list-pending-signals` on a live server. `runtara:control/api` runs only in fresh host stores over approved bytes, checked at load and on every call (decision 2).

**Depends on.** Slices 1, 3 and 4 (4 first); patch D (readiness predicate).

**Changes.**
- **runtara-component-host:**
  - `control_host.rs` (new): `ControlHost` trait over `ControlAuthority {tenant, caller, operation}`; `add_control_to_linker` (never traps, retryable `unavailable`) and `add_denied_control_to_linker`.
  - `control_executor.rs` (new, like `trusted.rs`): `from_bundle` hashes installed wasm+meta; `invoke`: fresh 64 MiB restricted-WASI store, deadline `min(step, 90 s)`; caps 4 MiB outcome, 64 KiB state; no payload logs.
  - `audit_control_importers` (`precompile.rs`): sha256 of each nested `runtara:control/` importer; fails closed on imported components and agents importing `runtara:workflow-operation/*`.
  - `workflow.rs`: denied `api` stubs for roots; requires one `runtara:builtin-artifacts/control-h<wasm>-h<meta>@0.1.0` pin; pin and importers must be in approved history; `executor.invoke`: 1 MiB input cap, re-checks current approvals; adds `WorkflowState.control_executor` and `.registered_waits`, and `executor.invoke` appends the returned wait ids to `registered_waits`.
  - Dispatcher: denied stubs (full bundle loads); `test_capability` routes `control` to `ControlExecutor` (`suspended` -> `SUSPENSION_UNSUPPORTED`).
- **runtara-workflows:** only canonical `control` (primary dir, exports `runtara:control/execution`) may import `runtara:control/{types,api,executor}`; append the pin via `compile/trusted.rs`.
- **runtara-environment:** load `ApprovedBuiltins` before wake/recovery; shared `is_explicitly_paused` (Rust + SQL); narrow capped instance read.
- **runtara-server:**
  - Boot approves control digests from both components dirs; manual revocation applies next boot; unapproved pin: image not ready.
  - `api/services/control.rs` (new): late-bound `NativeControl` (waits <= 30 s, else `unavailable`); tenant check; pageSize 1..100 else `invalid`; 4 MiB responses.
  - `get` inlines output <= 1 MiB, error <= 64 KiB, else `*-omitted` (+ `output-bytes`); unknown id `not-found`; `query` sorts by created/finished only; oversized signal pages `too-large`.
  - Identity calls and caller-relative filters: no caller `requires-instance`, else `unsupported`.
  - `entitlements.rs`: `control` on every tier, not allowlist-gated (decision 5); MCP notes, FE icon.
- **New crate `crates/agents/runtara-agent-control`:** `agent_component!(control_executor = true)`; idempotent `get`, `query`, `list_pending_signals`; `CONTROL_*` knownErrors; registered in root `Cargo.toml` and `runtara-agent-bundle-emit`.

**Migrations.** `crates/runtara-environment/migrations/20260927000000_approved_builtin_artifacts.sql`: approved builtin digests, revoked but never deleted.

**Tests.**
- `runtara-component-host/src/workflow/control_tests.rs` + WAT (lib): denied roots, spoof-proof authority, approval, revocation, size caps.
- `runtara-component-host/tests/control_agent.rs` (`component-integration-tests`): real executor, fake host, forwarder parity.
- runtara-workflows: pin, foreign importers. runtara-environment (`db-integration-tests`): digests, `is_explicitly_paused`.
- `runtara-server/tests/control_service.rs` (`db-integration-tests`): reads, paging, caps, error codes; lib tests: entitlements, readiness.
- `runtara-server/tests/control_component.rs` (`db-integration-tests`, `component-integration-tests`): composed `control:get`, digest match; new CI step.

**Done when.** Roots get `denied`; full bundle boots; revoked digest fails with `denied`; V-fmt, V-gate, V-build, V-units, V-wf, V-host, V-exec, V-env, V-server, V-comp (local + CI), V-wit, V-fe green; `STAGES=1 e2e/test_control_agent.sh` green on an isolated server.

**Risks.** wac is assumed to keep nested bytes verbatim (`control_component.rs` asserts it). Compile and dispatcher dirs may hold different control bytes; both are approved at boot.

### Slice 6: Operation identity, command receipts, send-signal and lifecycle commands

**Goal.** Identity comes from compiler-emitted scope calls, not agent input. `send-signal`, `cancel`, `pause`, `resume` ship with decision-1 authz and replay-safe receipts; waiting runs pause immediately in control and public API (decision 4).

**Depends on.** Slice 5.

**Changes.**
- **Component host** (`operation_scope_host.rs`): `parse_agent_operation_key` (kind `agent`, no retry suffixes, max 16 KiB) returns `OperationIdentity {key, op_hash, attempt, agent_id, step_id}`; frozen for `runtara:workflow-operation@0.1.0`.
  - `WorkflowState.operation` feeds `ControlAuthority`; second `enter` fails closed; `suspend` stubbed until slice 10.
- **Emitter** (`runtara-workflows`): at `is_operation_scoped` sites emit the key (also non-durable), `enter` before the deadline check, `exit` on every path; `enter` error = `AGENT_OPERATION_SCOPE`. Import (`core_imports.rs`) only with scoped sites.
- **Core**: `receipt_by_operation`; `Persistence::control_receipts()` `begin`/`complete`/`discard`, success-only; `pause_parked`, `pause_suspended_instances`.
- **Environment**: pause parked runs (request + recovery pass); race fixes in `launch_queue.rs`, `handle_resume_instance` (`require_paused`); receipt handlers.
- **Server auth** (`api/services/control.rs`): `requires-instance`, then `requires-operation`; pure `decide`: send-signal to child, ancestor or `action.key` opt-in, else `denied`; cancel/pause/resume/wait children only (ancestor `denied`, else `not-child`); self `invalid`. Resolver: `other` until slice 7. Authorize before `begin`; `audit_events` without payload; reserve `control:` op-id prefix.
- **send-signal** (`workflow_runtime.rs`): match open request by step id (+`actionKey`/`requestId`), else `not-waiting`/`ambiguous`; `replay-conflict` on changed args; submit via `InputAcceptanceContext("control", ...)`; returns `{requestId, replayed}`.
- **cancel/pause/resume**: authorize, `begin`, apply, `complete`/`discard`; Pending re-application per command. `stop_instance_with` (grace 0..3600 s, default 5 s); `pause_for`/`resume_for` in `workers/execution_engine.rs`.
- **Public API** (`api/handlers/workflows.rs`): `data.outcome`; resume of failed/cancelled -> 400 `NotResumable`; regen TS client; release note on immediate pause.
- **Agent/MCP**: `runtara-agent-control` adds `send_signal`, `cancel`, `pause`, `resume` (`side_effects`, `runtime:requires-run`); docs state the rules.

**Migration.** `runtara-store-postgres/migrations/postgresql/034_instance_control_receipts.sql`: receipts keyed `(caller_instance_id, operation_id)`, where `operation_id` holds the `op_hash`; cascade with caller.

**Tests.**
- V-units: key parser, double `enter`, `decide` matrix, prefix rejection. V-wf: emission, unscoped byte-identical.
- `direct_wasm_execute.rs` (`direct-wasm-integration-tests`): keys distinct per iteration and embed, stable on retry. `agent_deadline_tests` (V-emit): fault replay reuses key.
- Core conformance, memory + `store-postgres/tests/conformance.rs` (`db-integration-tests`): receipts, `pause_parked`.
- `tests/handlers_test.rs` + launch-queue tests (`db-integration-tests`): pause/resume races, stop grace/reason.
- `tests/control_service.rs` (`db-integration-tests`): outcomes, replay/conflict, authz, audit, public API.
- `tests/control_component.rs` (`db-integration-tests,component-integration-tests`): send-signal e2e. `tests/control_agent.rs` (`component-integration-tests`): validation.

**Done when.** V-fmt, V-gate, V-units, V-wf, V-core, V-pg, V-env, V-server, V-comp, V-host, V-build, V-emit, V-exec, V-fe green; `STAGES=2 e2e/test_control_agent.sh` on an isolated server (SIGKILL replay answers once); `Persistence`/`InputRequests` impl and `core_imports.rs` sweep done.

**Risks.**
- Receipts (runtime DB) vs slice-8 admission (server DB): intent-first receipts, defined Pending re-application.
- One scope per Store: parallel exclusion (slice 4) and fail-closed second `enter`.

### Slice 7: Parent link, `start`, idempotent admission, parent-aware query

**Goal.** `control:start` durably, idempotently admits a child with per-parent unique labels under capacity rules; the parent link in both DBs drives authorization, `get` and `query(parent)`.

**Depends on.** Slices 2 and 6.

**Changes.**
- **Admission** (`workers/execution_engine.rs` `start_child`), in order:
  - Normalize (no DB; id not slug, `_` vars dropped, no `\u0000`/empty label) → `invalid`.
  - Idempotency `(caller, op_hash)` first: same fingerprint → stored child/version, `replayed: true`; else `replay-conflict`; unknown prefix → `unavailable`.
  - Label (`label-conflict`) → depth 16 (`invalid`) → workflow (`not-found`) → inputs → compile (terminal failure → `not-runnable`, no row). Not-compiled-yet is admitted (decision 7); relay requeues it until the deadline (`launch_deadline_not_compiled`).
  - Fingerprint: `v1:` + sha256 of sorted canonical JSON of the normalized envelope.
- **Capacity** (decision 5): children count against `maxConcurrentExecutions`, parked runs free slots; control share `max(1, floor(0.8 × cap))`, advisory-locked in `enqueue`; full → retryable `capacity`, 3-8 s hint (agent `CONTROL_CAPACITY_RATE_LIMITED`); other denials → `denied`; Valkey/PG failure → `unavailable`; cap ≤ 1 → permanent `CONTROL_CAPACITY_UNSATISFIABLE` + boot warning.
- **Errors:** `ExecutionOutboxError::{ParentRunLabelConflict, StartReplayConflict, ControlShareFull}`, `ExecutionError::WorkflowNotRunnable`.
- **Plumbing:** `TriggerEvent.{parent_instance_id, parent_close_policy}`, `TriggerSource::Control`; parent, policy and `admitted_at` reach env `claim_initial` (same tenant) and core `InstanceRecord`.
- **Authorization:** `child`/`ancestor` relation feeds slice 6 `decide`; env `input_candidate_ids_for_parent` (pending signals); unstarted child → `not-pausable`/`not-paused`.
- **NativeControl:** `start` → `{instanceId, workflowId, version, runLabel, replayed}`; `get` adds `parentInstanceId`, in-admission child = `queued`; `query(parent)` = server in-flight read + one env `UNION ALL` paged by `admitted_at`.
- **Public API/MCP** (decision 8): `WorkflowInstanceDto.parentInstanceId`, list filter `parentInstanceId`, MCP `ListExecutionsParams.parent_instance_id`; authoring text: decision 1, `start` inputs.
- **Agent** (`crates/agents/runtara-agent-control`): `start` (side_effects, `runtime:requires-run`); required `parentClosePolicy` enum `[cancel, leave_running]`, no default (keeps E022).
- **Frontend:** `InputMappingField/index.tsx` pre-fills required enums with `enum[0]` (`cancel`, decision 3); regen `RuntaraRuntimeApi.ts`.
- **CI/docs:** `components-build` Valkey service; release note (first-boot locks, no downgrade); `docs/install.md`, `docs/pipeline-monitoring.md`.

**Migrations.**
- Core `035_instance_parent_link.sql` (parent, policy, `admitted_at`, `NOT VALID` CHECKs, no FK); `036_validate_instance_parent_link.sql` validates them.
- Core `037_instance_parent_index.sql`: `idx_instances_parent_admitted`, transactional (SHARE lock).
- Server `20260927000100_execution_request_parent.sql`: parent/label/fingerprint/policy/operation/outcome + slice 8 cancel columns, `NOT VALID` CHECKs.
- Server `20260927000101_execution_request_parent_indexes.sql`: validates; unique `execution_requests_parent_run_label_key` + three partial indexes.

**Tests.**
- `runtara-server/tests/execution_outbox_test.rs` (`db-integration-tests`): replay/conflicts, label races, control share caps, not-compiled requeue.
- `runtara-environment` (`db-integration-tests`) + conformance: parent fields persist, cross-tenant rejected, merged children dedup/page/count.
- `runtara-server/tests/control_service.rs`: start/get/query paging, capacity, depth, not-runnable vs not-compiled, lifecycle, relations.
- `runtara-server/tests/control_component.rs` (`db-integration-tests,component-integration-tests`, Valkey): real child launch via `NativeControl`.
- Unit: fingerprint, capacity mapping, depth, `parse_filters`, MCP query, enum pre-fill; `migration_versions_test`.

**Done when.** On an isolated live server, `STAGES=3 e2e/test_control_agent.sh` and `e2e/test_start_run_labels.py` pass (labels, replay after SIGKILL, parent queries, lifecycle); V-fmt, V-gate, V-units, V-wf, V-core, V-pg, V-env, V-server, V-comp (Valkey), V-mig, V-build, V-fe green.

**Risks.**
- Each child adds a 2 s-polling analytics watcher; skipping it must still emit completion events.
- Tiers above runner capacity can hit the 300 s launch-queue timeout (owner decision open).

### Slice 8: Ownership: retention pin, fenced outcomes, parent-close cascade, cancel before launch

**Goal.** A finished child stays readable until its parent is terminal. A never-launched child gets one fenced `not-started`/`cancelled` outcome; `cancel` children stop on any parent ending (5 s grace); cancel works in every admission state.

**Depends on.** Slice 7 (parent/policy and cancel-intent columns, `outcome_published_at`, core `admitted_at`, parent `query`); slice 6 `decide`, receipts.

**Changes.**
- Fence: `Persistence::publish_external_outcome -> Published|AlreadyPublished|Launched` under a per-instance advisory lock; env `claim_initial` (`launch_queue.rs`) takes it for parented launches (`LaunchFenced`); a core row wins.
- Publisher: new `runtara-server/src/workers/control_children.rs`, idempotent passes: (a) publish outcomes with reason, (b) apply cancel intents once `accepted`, (c) cascade to `cancel` children still in admission.
- `NativeControl`: `get` publishes inline, else "no longer retained"; `query(parent)` adds an `instance_external_outcomes` branch, kept out of the public list; `cancel` = `decide`, receipt, admission then Environment.
- `ExecutionOutbox::cancel_request`: `queued|delivered` cancels, freeing reservation and control share; `launching` stores an intent; `accepted` stops in Environment.
- Cascade (`runtara-environment/src/wake_scheduler.rs`): bounded task off the wake path, not while draining; selects `cancel` children of terminal/missing parents without row locks, skipping pending cancels; stop as `handle_stop_instance`, principal `platform:parent-close`; suspended parents do not trigger.
- Retention pin: SELECT-only in `op_get_terminal_instances_older_than` (`runtara-store-postgres`), memory; pin by parent status, one level, GREATEST age, per-pass cursor; outcomes same by `published_at`; logs `pinned_terminal_children`. `image_cleanup_worker.rs` skips images in `instance_launches`.
- Docs: reword retention line in `docs/control-agent.md`; sweep `Persistence for`, `ExecutionOutboxError::`.

**Migrations.**
- Core `038_instance_external_outcomes.sql`: outcomes table (`not_started|cancelled`, reason, `admitted_at`, `published_at`) plus parent index.
- Optional partial index for active `cancel` children: in core 038.

**Tests.**
- Fence race (env `db-integration-tests`, memory): publish-then-launch refused, launch-then-publish `Launched`; promote the S0.4 launch-fence race loop (`instance_waits_race.rs`).
- core `persistence/conformance.rs` (`test-support`, `db-integration-tests`): pin, release, missing parent, grandchild, cursor, outcome cleanup.
- Env `tests/db_cleanup_worker_test.rs` (1e5 pinned rows read once), image starvation, `tests/wake_scheduler_test.rs` (cascade once after crash).
- Server (`db-integration-tests,valkey-integration-tests`): cancel per admission state, launching races, idempotent passes, public list excludes outcomes.

**Done when.** V-fmt, V-gate, V-core, V-pg, V-env, V-server, V-comp green; `STAGES=4 e2e/test_control_agent.sh` shows cascade after parent fail, completion and SIGKILL restart, `leave_running` survives, held child never runs; retention shown by DB tests. No child reads `not-started` while running.

**Risks.**
- Advisory-lock contention on parented launches (measured by the S0.4 launch-fence race loop).
- Pinned rows grow under long-lived parents; slice 12 prunes each one's bulky data once it is past retention by its own finish, and keeps the row.

### Slice 9: Durable instance waits

**Goal.** A parent can park until its children finish: wait records, race-safe park, trigger + reconciler, `NativeControl` `wait`/`poll-wait` (used from slice 10). `any` stays stable on replay.

**Depends on.** Slices 8 (parent columns, launch fence, external outcomes), 6 (`pause_parked`), 5 (`NativeControl`, `registered_waits`), 3 (`wait-result` fields); S0.4 green.

**Changes.**
- **Core vocabulary:** `WakeReason::InstancesTerminal`, `SuspensionReason::WaitingInstances`, `ParkReason::Instances`; server `TerminationReason::WaitingInstances`; `pause_parked` covers it (decision 4).
- **Wait store:** trait `InstanceWaits` (`runtara-core/src/persistence/waits.rs`, accessor defaults `None`); PG `runtara-store-postgres/src/waits.rs`; memory backend repeats trigger nudges in its writers.
  - Key `(waiter, wait_id = op_hash)`; fingerprint = sha256(sorted unique targets + mode), no deadline.
  - One rule on `clock_timestamp()`: empty, satisfied, deadline (`>=`, ms-truncated), else pending; core row beats external outcome.
  - `register_or_evaluate` (fixed lock order; first deadline wins; fingerprint change = `Conflict`), `poll_wait` (explicit `Closed`), `close_wait`, `delete_resolved_wait`, `reconcile_wait_wakes` (pass A `wake_pending`; pass B every 12th poll, rotating).
- **Race-safe park:** `park_instance_on_targets(.., ParkTargets { signal_ids, wait_ids })` in one txn, stamping satisfied waits; paused waiters never stamped.
- **Environment:** `wake_scheduler.rs` calls `reconcile_wait_wakes`; handlers + clients for register/poll/close wait, `publish_external_outcome`, narrow `instance_statuses`.
- **Server `NativeControl`:** `requires-instance`, then `requires-operation`; >`MAX_WAIT_TARGETS` (1000) = `too-large`; `not-found`; `not-child`/`denied` (ancestor), none register; `Conflict` = `replay-conflict`; foreign `poll-wait` id = `denied`.
  - First publishes admission-terminal children under the slice-8 fence (fail = `unavailable`); never-launched show `not-started`/`cancelled`; deleted accepted child = `not-found`.
  - Caps in `runtara-control-contract`: 256 KiB output, 16 KiB error per target, 3 MiB total; fills `output-bytes`, `output-omitted`/`error-omitted`, persisted `deadline-ms`.
  - Records `wait_id` in `WorkflowState.registered_waits`; `Suspended` already frees the slot (decision 5).

**Migrations** (`runtara-store-postgres/migrations/postgresql/`, provisional numbers):
- `039_waiting_instances_termination.sql`: enum value `waiting_instances`, alone in its txn.
- `040_instance_waits.sql`: `instances_wake_reason_valid` NOT VALID replaces unnamed 024 CHECK; `instance_waits`, `instance_wait_targets`, `instance_input_parks.wait_ids`; `wake_instance_waiters()` + 3 SKIP LOCKED triggers. Never edit 030.
- `041_validate_wake_reason.sql`: validate the constraint.

**Tests.**
- `runtara-core/src/persistence/conformance/waits.rs`, memory (`test-support`) + PG (`db-integration-tests`): resolution, replay, deadline, lifecycle, park, reconciler rotation.
- PG only (`db-integration-tests`): raw-SQL terminal writes wake in-commit, no blocking on locked waiters, 024 CHECK replaced, promoted S0.4 `tests/instance_waits_race.rs`.
- Environment (`db-integration-tests`): `runner/embedded.rs` park; `wake_scheduler_test.rs` crash-before-stamp wakes once; `launch_queue.rs` writers wake waiters.
- Server (`db-integration-tests,valkey-integration-tests`): `control_service.rs` check order, error codes, 1000 cap, `not-started`, budget stable across polls.

**Done when.** Persisted deadline, stable `any`, explicit missing-target errors and no lost wake proven at core, env, server; lifecycle WIT unchanged; V-fmt, V-gate, V-core, V-pg, V-env, V-server, V-mig green. No component build, no e2e.

**Risks.**
- Deadlock or lost wake: SKIP LOCKED, fixed lock order, pass B, fuzz tests; fallback K2 drops the trigger's waiter stamp.
- Fan-in cost per child finish: bounded by the 1000 cap and pruned target index; S0.4 measures it.

### Slice 10: Typed agent suspension and control `wait`

**Goal.** A capability can return a typed suspension; workflow logic stores its continuation, attaches waits and parks without a runner, then re-invokes the agent with it on wake. `control:wait` delivers parallel approvals.

*Since replaced: parallel approvals use the `WaitForInstances` step, control no longer suspends, and agent suspensions may only wake on `at`.*

**Depends on.** Slices 6 and 9, and the S0.2 tracer code (no continuation argument on `suspendable.invoke`).

**Changes.**
- **Persistence** (core, store-postgres, memory): `agent_continuations()`: attempt-matched `get`; `put` (`MAX_CONTINUATION_BYTES`, checkpoint fences); `delete`. Not checkpoints.
- **`RuntimeHost`** (`runtara-component-host/src/runtime_host.rs`): `operation_continuation_load/_store`, `operation_wait_close`, `operation_release`, default unsupported; `PersistenceRuntimeHost` implements, `DeferredTerminal` forwards.
- **Scope** (`operation_scope_host.rs`): `enter(key, attempt, load)`; `suspend(state, wakes)` caps 64 KiB/16 wakes/64-byte id, stores state, attaches registered waits; `exit(failed)` closes the wait and deletes state; idempotent `release`.
- **Delivery/park:** control via `executor.invoke` into `execution.invoke`, others via `runtara:agent-suspension/context@0.1.0` `continuation()`; `InvokeRunResult.instance_waits` feeds `park_invoke_suspend` (`runner/embedded.rs`).
- **Compiler** (new `runtara-workflows/src/direct_wasm/compile/agent_suspend.rs`): imports keyed by `(agent_id, AgentInterface)`; only `suspends` sites call `suspendable.invoke`, excluded from parallel windows; other workflows byte-identical.
  - Suspended: park at clamped `min(agent_at, DEADLINE)`, no `::attempt::` key; within 1 s of DEADLINE: `AGENT_TIMEOUT`. `release` after the result checkpoint.
  - New errors: `AGENT_INVALID_SUSPENSION`, `AGENT_CONTINUATION_REJECTED`, `AGENT_UNEXPECTED_SUSPEND`.
- **`wait`** (`crates/agents/runtara-agent-control`): `suspends = true`, `runtime:requires-run`; input `{instanceIds, mode: all|any, deadline?}`, max 1000 targets.
  - Registers once via `api.wait`, then only `api.poll-wait`; `too-large`/`not-found`/`not-child`/`denied` (ancestor) register nothing.
  - Output `{mode, resolution: satisfied|deadline|empty, finished[], remaining[]}`; oversize values omitted and flagged.

**Migrations.** `crates/runtara-store-postgres/migrations/postgresql/042_agent_continuations.sql` (number TBD): `instance_agent_continuations`, 64 KiB cap.

**Tests.**
- `persistence/conformance/continuations.rs` (memory + `db-integration-tests`): attempt get, size cap, cascade.
- `cargo test -p runtara-workflows`: SizeAlign offsets, per-site import resolution, site shape, byte-identity.
- `compile/agent_suspend_tests.rs` (`direct-wasm-integration-tests`): park/relaunch, fault replay, deadline ties, error codes, cancel race.
- `tests/direct_wasm_execute.rs` (same feature): `wait` in Split/branches/While/Embed; Embed retry re-registers.
- `runtara-component-host/tests/control_agent.rs` (`component-integration-tests`): delivery, `requires-instance`, `SUSPENSION_UNSUPPORTED`.
- New `runtara-environment/tests/control_wait_runner_test.rs` (`scoped-workflow-integration-tests`, in `components-build`): park, relaunch.
- `runtara-server/tests/control_component.rs` (`db-integration-tests,component-integration-tests`): real `NativeControl`, slot freed, restart, revoked digest gives `denied` at the call, unapproved digest leaves the image not ready (D2).

**Done when.** V-fmt, V-gate, V-stdlib, V-build, V-wit, V-units, V-wf, V-emit, V-exec, V-host, V-comp, V-core, V-pg, V-env, V-server, V-fe, V-scoped green; new tests run in CI; `STAGES=1-5 e2e/test_control_agent.sh` passes: approvals, pause holds (D4), capacity share (D5), restarts, `any` + `leave_running` (D3), deadlines, races, E028/E029/E131.

**Risks.**
- `suspendable` and `capabilities` are type-identical, so a wrong import or offset passes validation; SizeAlign and import tests guard it.
- If S0.2 took the K1 suspendable-only fallback, suspending agents import only `suspendable`, so all their capabilities follow operation-scoped rules.

### Slice 11: Artifact and ABI retention, upgrade safety
**Goal.** Parked runs survive image cleanup, recompiles, control (if K5 approves), trusted-agent and binary upgrades, and host ABI changes.
**Depends on.** Slice 5 (approved-digest history, per-call check), slice 8 (retention, `NOT EXISTS instance_launches` guard), slice 10 (parking), patch D.
- **Image cleanup** (`runtara-environment/src/image_cleanup_worker.rs`): ensure slice 8's guard exists, or protected images fill the batch.
- **Trusted pins, option B** (implemented, owner-approved; `runtara-component-host/src/trusted.rs`): accept a non-revoked `approved_builtin_artifacts` pin only on `Wake`/`Resume` (paused waits: decision 4), never `Start`; run current bytes.
  - *As built.* Boot records the installed trusted pins in `approved_builtin_artifacts` (environment migration `20260927000300_approved_trusted_artifacts` widens its pin check to `runtara:trusted-artifacts/…`) and hands the approved, non-revoked ones to `TrustedExecutor::set_approved_history` (`ApprovedBuiltins::install_trusted`). `TrustedExecutor::admits` runs before any credential lookup: installed pin on any launch; otherwise an approved earlier pin of the same agent only when `TrustedLaunch` is `Wake`/`Resume`; everything else `TRUSTED_VERSION_REQUIRED`. The launch kind is host authority: `LaunchOptions.launch_kind` (from the launch queue row) sets `PersistenceRuntimeHost::with_trusted_launch`, children read it through their scoped runtime host (`RuntimeHost::trusted_launch`, default `Start`, wrappers delegate); the guest never supplies it. Readiness stays installed-only (`ApprovedBuiltins::pins` excludes trusted pins), so starts recompile. Revoking a trusted pin (effective next boot) cuts off parked runs on it; revoking the installed pin denies every call to that agent (`TrustedExecutor::set_revoked_pins`), like a revoked control digest.
  - *Security reasoning.* The pin selects no bytes: only installed, operator-approved bytes run, in the same fresh restricted store. Credentials are still resolved by the run's host-supplied tenant, the connection and its allowed types. A start can never use the history, so old pins cannot spread to new runs, and a never-approved or revoked pin is refused on every launch. The only relaxation is the version-compatibility gate for runs that already parked under an approved version, and revocation is the operator's switch for it.
- **ABI rule** (`runtara-workflow-wit/src/lib.rs` + README): every shipped host version stays linked (lifecycle 0.1/0.2, runtime 0.3/0.4, connection-resolver 0.1, control, workflow-operation and agent-suspension 0.1.0, builtin/trusted pins); approved rows only revoked; released WIT never edited, only versioned.
**Tests.**
- New `runtara-environment/tests/image_cleanup_db_test.rs` (`db-integration-tests`): parked and pinned-child images survive; no batch starvation.
- `control_wait_runner_test.rs` (`scoped-workflow-integration-tests`): resume on bound image after recompile; control upgrade loads via history (if K5 rejects: installed digests only, and the test asserts the old-pin parent is not ready); revocation fails the call, not the load.
- New `runtara-component-host/src/workflow/frozen_abi_tests.rs`: WAT fixtures link frozen `control/api`, `control/executor`, `workflow-operation/scope`, `agent-suspension/context` @0.1.0.
- Trusted B tests: `trusted.rs` unit `earlier_approved_pins_run_installed_bytes_only_on_wake_or_resume` and `tests/trusted.rs` `approved_earlier_pin_presigns_only_when_a_parked_run_continues` (real S3 signer): old pin works on Wake/Resume; Start, revoked, unapproved fail with zero credential lookups. `approved_builtins` db test: trusted pins share the history but never count as control/readiness pins. `e2e/test_trusted_pin_upgrade.sh`: the run parked on the old S3 version wakes and presigns after the upgrade.
- E2E stage 6 (`STAGES=6 e2e/test_control_agent.sh`): bundle swap while parked; `RUNTARA_IMAGE_CLEANUP_MAX_AGE_DAYS=1` keeps the package; trusted parents per B.
- New `e2e/test_control_upgrade.sh` (bin+bundle N, N+1; not in `run_all.sh`): parked parents survive binary N to N+1.
**Done when.** Package survives cleanup until terminal; V-fmt, V-gate, V-build, V-env, V-host, V-scoped, V-exec (if B), stage 6, upgrade script green.
**Risks.** Same-source builds share digests: force scratch version bumps, assert digests differ. Guard scans unindexed `instance_launches(image_id)`; index only if measured.

### Slice 12: Pruning pinned children (implemented)
- **Goal.** Strip bulky data of old pinned terminal children; keep the `instances` row.
- **Trigger (owner-adapted).** No threshold: every retention pass prunes exactly the pinned children retention would already have deleted had they not been pinned (terminal, finished before `RUNTARA_DB_CLEANUP_MAX_AGE_DAYS` by their own `finished_at`, parent exists and is not terminal). A child of a recently finished parent is left to retention, which deletes it soon.
- **As built.** `Persistence::prune_pinned_terminal(older_than, after, limit) -> PrunePage { pruned, next }` (default no-op; memory and Postgres implement it). Postgres (`ops_common/ops/retention.rs`): keyset page read without locks, then one transaction per page that locks the still-terminal children `FOR NO KEY UPDATE` and runs the full variant: deletes `checkpoints`, `pending_signals`, `pending_checkpoint_signals`, closed `instance_input_requests`, `instance_input_parks`, `invocation_attempts`, `invocation_root_leases`; NULLs `input` and `stderr` (not `OF status`, so no terminal trigger fires). Kept: the row and outcome, events, accepted input receipts (send-signal replay), control receipts, waits, continuations. `pruned` counts only children that still had something to prune, so a rerun reports 0. `db_cleanup_worker.rs` runs it after `cleanup_old_instances` with the same cutoff and batch size and logs `pruned_children`.
- **Depends on.** Slice 8 (pin, cursor, log), slice 9 (wait check).
- **runtara-core** `persistence/mod.rs`: `prune_pinned_terminal(older_than, after, limit)`.
- **runtara-store-postgres** `retention.rs`: slice 8 pin inverted, keyset cursor, one txn per batch.
- Light first: drop `checkpoints`, NULL `stderr`. Full: also signals, closed input requests, parks, leases, attempts, NULL `input` (keep `accepted`).
- **runtara-environment** `db_cleanup_worker.rs`: prune after `cleanup_old_instances`, no new env var; log `pruned_children`.
- **Tests.** `conformance.rs` (both backends): only old pinned rows pruned, get/wait unchanged, rerun no-op; `db_cleanup_worker_test.rs` (`db-integration-tests`): order, cursor.
- **Done when.** V-fmt, V-gate, V-core, V-pg, V-env green (conformance `run_prune_pinned_sequence` on both backends; `db_cleanup_worker_test.rs` `pinned_children_are_pruned_after_deletion_with_a_cursor`).
- **Risks.** Full variant losing `send-signal` receipts (accepted requests are kept; conformance asserts replay); pruned data not inspectable.

### Slice 13: Surfaces and docs closeout

**Goal.** Show control and suspending steps accurately in editor, history, MCP, docs; ship release notes. API: one DTO field, status relabel, steps filter.
**Depends on.** Slices 4-11 (12 if shipped). Pairing rewrite may land earlier; no migrations or WIT.

**Changes.**
- **Author reference**, one text in `docs/control-agent.md` "As built" and `workflow_authoring_schema()` (`mcp/tools/workflows.rs`): authorization (decision 1), `start` (decisions 6-7, depth 16), 0.8 capacity cap + `CONTROL_CAPACITY_UNSATISFIABLE`, statuses, `wait`, limits, replay table, E028/E029/E131/E132, W073-W078, lifecycle (decisions 3-4).
- **Docs:** `docs/control-agent.md` (executor model, retention, debug; replace the stale "a step rather than an agent capability" wait text); `docs/deployment/entitlements.md` (all tiers, capacity); READMEs: `runtara-agent-control`, `runtara-control-contract`, `runtara-agent-suspension` (64 KiB `MAX_CONTINUATION_BYTES`, `AGENT_CONTINUATION_REJECTED`); fix WIT version, agent counts.
- **Release notes:** `CHANGELOG.md` [Unreleased] (control agent, `parentInstanceId`, downgrade caveats, `composed` binding cannot compile control or suspending steps; verify earlier notes, e.g. waiting runs pause at once) + `get_dsl_changelog()`.
- **MCP:** `test_capability`: run-only caps give `CONTROL_REQUIRES_INSTANCE`; pause/resume text (`data.outcome`); `add_agent_step` gains `timeout`, `durable`, `suspends` hint.
- **Server:** `api/handlers/step_summaries.rs` reports Running as `suspended` when instance suspended; accepts `status=suspended`; `WorkflowInstanceDto.suspensionReason` (`paused|waiting_signal|waiting_instances|sleeping|shutdown`).
- **Step pairing:** one start/end rule per (step, scope) in `runtara-store-postgres/src/dialect/postgres.rs` and `runtara-core/src/persistence/memory.rs`; confirm duplication live first.
- **Frontend** (`frontend/src/features`): `suspends` badge, timeout+durability hint; `runtime:requires-run` in Test; `parentInstanceId` column+filter (decision 8), Started by / Child runs; Resume only when paused; `suspended` timeline badge; real breakpoint vs "Waiting".

**Tests.**
- `step_summaries.rs` and `suspensionReason` mapping units.
- Pairing conformance (S,S,E; S,E,S,E; E,S,E): memory `--features test-support`, `runtara-store-postgres/tests/conformance.rs` `--features db-integration-tests`.
- MCP `add_agent_step`; authoring-schema drift vs contract constants; Vitest for each new UI branch.

**Done when.** V-fmt, V-gate, V-units, V-core, V-pg, V-server, V-fe green; generated diff = DTO field only; `STAGES=5 e2e/test_control_agent.sh` passes with stage 5.1 (parallel approvals) extended: parked wait step `suspended`, one row after; docs/CHANGELOG complete.

**Risks.**
- Pairing rewrite changes all step history; if ~4.5-6 days is tight, ship relabel + frontend, defer pairing.
- Duplicated reference prose may drift; tests check only codes/limits.
