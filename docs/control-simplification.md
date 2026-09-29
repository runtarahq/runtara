# Control agent simplification

Status: plan, 2026-09-29, against main at a7a0e401 (after #278, queryable
workflow state). Nothing here is implemented.

Today the composed copy of the control agent only forwards. It calls
`runtara:control/executor`, and the host's `ControlExecutor` runs the
host-installed control bytes in a fresh restricted store per call. That store
is the only place where `runtara:control/api` is real; every workflow store
links `denied` stubs for it. Around the executor sits decision D2: a content
pin for the control bytes, an approved history of pins with revocation, a
load-time audit that hashes every composed component importing control, and a
per-call re-check of all of it.

This plan makes control an ordinary composed agent that calls
`runtara:control/api` directly. It deletes the executor and every
control-specific piece of D2: the pin, the control rows of the approved
history, revocation for control, the load-time audit, and the per-call binding
check. The compile-time import allowlist becomes the only gate, as it is for
every other host interface.

## Decision change: D2

D2 (2026-09-26) reads: "Host verification that only the approved control agent
can call control. In v1: check the composed bytes at load and on each call."

This plan replaces it with the following:

> **D2 (revised 2026-09-29).** Only the canonical control agent from the
> primary components dir may import `runtara:control/*`. The compile-time
> import allowlist enforces this. The host verifies nothing further: no pin,
> no approved history, no load audit, no per-call binding.
> `runtara:control/api` is real only in a workflow store that carries a run
> identity. All authorization is in `NativeControl`.

[control-agent-decisions.md](control-agent-decisions.md) is updated in slice 4.

## Why

### The threat is a component the author didn't choose

Control authority is tenant-scoped, and any workflow author in the tenant can
already add a control step. Reaching control therefore gains an author nothing.
The real threat is a component the author did not choose to give control, such
as a third-party or marketplace agent, quietly importing `runtara:control/*`.
The compile-time allowlist stops that:

- `check_agent_component_imports`
  ([artifact_metadata.rs:550](../crates/runtara-workflows/src/direct_wasm/compile/artifact_metadata.rs))
  checks every root import of every composed agent.
- `AGENT_IMPORT_ALLOWLIST` (:422) contains only timers, HTTP, connections and
  SQL.
- Control imports are granted only to the canonical control agent resolved
  from the primary components dir (`AgentImportGrants::for_agent`, :458).
- Only root imports matter. A nested component receives only what its parent
  passes, and the compiler wires by import name.

### Every artifact comes from the server's compiler

No API registers caller-supplied workflow bytes. Workflow artifacts come only
from the server compiler. Staged workflow-agents also come only from the
server: it compiles them under the same allowlist when a workflow is published,
and only the server writes the staging dir. The load-time checks therefore
defend only against a tampered artifact store or a compiler bug. An attacker
who can write artifacts can run arbitrary code in the tenant's process anyway;
under the tenancy model, the tenant is the whole deployment.

### The load audit doesn't stop what it targets

`audit_control_importers` exempts any component carrying the
`runtara.direct_workflow.abi` custom section as workflow logic
([precompile.rs:767](../crates/runtara-component-host/src/precompile.rs), the
`!frame.workflow_logic` condition). A crafted component can add that section.
The audit therefore defends against tampering only in theory.

### The host is authoritative

Tenant, caller and operation come from the store, never from WIT arguments or
input. `NativeControl` enforces D1 and D3–D8 and re-validates every input that
matters. For example, `start` goes through `execution_engine::normalize_start`
([control.rs:1570](../crates/runtara-server/src/api/services/control.rs)), and
ids, reasons, page sizes and grace periods are checked in the same service.
The guest's own validation gives authors early, friendly errors. A component
that reached `api` without the guest would bypass nothing that matters.

### Nothing else needs the fresh store

- The component model gives every composed component its own linear memory.
- Control handles no secrets, unlike trusted capabilities.
- No failure path traps. Every expected failure is a handled `control-error`:
  - the guest has no panic reachable from input, since decoding returns an
    error and every validation returns `invalid`;
  - the `api` binding always returns a typed result;
  - in `NativeControl`, the only panics are invariant `expect`s that fire on a
    code bug.

  The executor turned a failed task join into `CONTROL_EXECUTION_FAILED`, but
  that was a side effect of spawning a task (wasmtime forbids nested concurrent
  event loops), not a designed containment. After this change, a host bug
  fails the run, as it does for `runtara:host/sql` and `runtara:host/http`.

The same reasoning applies to the rest of the load audit. It also refuses a
composed agent that imports `runtara:workflow/operation`, `waits` or `state`.
Those interfaces must stay out of agents:

- **operation:** receipts are keyed by `(caller, op_hash)`, so an agent could
  forge or reuse an operation identity;
- **waits:** an agent could park the run on instances the author never chose;
- **state:** an agent could overwrite the run's published state.

The compile-time allowlist already refuses all three ("`runtara:workflow/` is
never an agent import"), and #278 refuses `state` to staged workflow-agents
too. The load-time copy has the same exemption weakness, so the whole audit
goes.

## What stays enforced

- **Compile-time allowlist.** It is unchanged, except that the control grant
  becomes `[TYPES, API]` and no longer requires the `execution` export.
  Staged workflow-agents keep skipping the list, minus `runtara:workflow/state`.
- **The store decides where `api` is real.** `api` is real only in a workflow
  store invoked as the run's own prepared entry, with a host-supplied tenant
  and instance. Isolated capabilities, isolated child workflows and unprepared
  loads link the same function but get `denied`. This follows the pattern of
  `RunInstanceWaits::for_run` and `RunStateAccess::for_run`
  ([workflow.rs:1052-1062](../crates/runtara-component-host/src/workflow.rs)):
  a flag in `InvocationControl`, set exactly where `control_binding` is set
  today, defaulting to off.
- **`NativeControl`:** D1 and D3–D8, receipts per `(caller, op_hash)`, and the
  mutation audit in `audit_events`.
- **Trusted capabilities are untouched:** `TrustedExecutor`, trusted pins, the
  approved history for trusted pins (`install_trusted`), and trusted readiness.
  Trusted keeps them because it handles credentials.

## Findings

### 1. Suspension and wait registration

Nothing needs migrating: control no longer suspends or registers waits.

- `wait` and `poll-wait` were removed on 2026-09-28 (see
  [wait-for-instances-plan.md](wait-for-instances-plan.md)).
- `execution.invoke` has no continuation.
- Waits come from the `WaitForInstances` host interface: `run.instance_waits`
  feeds `park_invoke_suspend`
  ([embedded.rs:1380](../crates/runtara-environment/src/runner/embedded.rs)).

If a control capability ever needs to suspend, it can use the standard
`runtara:agent/continuation` path, gated by `grants.suspends`.

### 2. Per-call limits

Every capability makes exactly one api call: nine call sites for the nine
capabilities in `runtara-agent-control/src/lib.rs`. A per-api-call bound is
therefore equivalent to today's per-capability bound, and the operation identity
read at the api call is the one entered for the capability. Slice 2 adds a test
that fails if a capability ever makes a second api call.

| Limit | Today | After |
|---|---|---|
| 64 MiB memory | Fresh-store limiter | Dropped. The control instance lives in the run's store under `WorkflowLimiter`, like every agent. |
| 90 s | `min(active deadline, now + 90 s)` around the whole capability | Kept as a host bound on each api call: `timeout_at` around the `ControlHost` future, returning the new `timeout` error code (`CONTROL_TIMEOUT`, not retryable, as today). The step deadline cancels the call through standard subtask cancellation. |
| 16 concurrent | Semaphore in `ControlExecutor` | Dropped. Concurrency is bounded by run permits, W075 serialization of operation sites and the DB pool, as for `runtara:host/sql`. |
| 1 MiB input | Whole agent input | Enforced in `NativeControl` on `start-request.input` and `send-signal-request.payload` as `too-large`. |
| 4 MiB output | Checked after the capability returns | Already bounded host-side. `get` inlines at most 1 MiB of output and 64 KiB of error. `get-state` returns a state document capped at 64 KiB at write time. `query` pages at most 100 summaries. `list-pending-signals` checks `MAX_RESPONSE_BYTES` (control.rs:1948). |

### 3. test_capability, error codes, audit logging

**test_capability.** Today it routes control to `ControlExecutor::invoke` with
tenant-only authority
([dispatcher.rs:319](../crates/runtara-component-host/src/dispatcher.rs), :433).
After the change, the dispatcher links the installed control bytes with a
linker whose `api` is real and reads `HostState.control_api`.

- `add_control_api_to_linker<T: ControlApiView>` already works for `HostState`
  ([control_host.rs:179](../crates/runtara-component-host/src/control_host.rs),
  :217).
- It sets tenant-only authority and runs the generic guarded path, with
  `RUNTARA_TEST_CAPABILITY_TIMEOUT_SECS` and `…_MEMORY_MAX_BYTES`.
- There is no approval check.
- Every other agent keeps `build_linker`, where `api` is denied.

**Error codes.** Retryability comes from `ErrorCode::retryable`
([runtara-control-contract/src/lib.rs:218](../crates/runtara-control-contract/src/lib.rs)).

| Code | Change |
|---|---|
| Service codes (`CONTROL_DENIED` … `CONTROL_NOT_PAUSED`), including the capacity split | Unchanged. The guest maps them (`runtara-agent-control/src/lib.rs:978`). |
| `CONTROL_DENIED` from binding or approval checks | Gone with those checks. `denied` now comes only from `NativeControl` or from an entry without a run identity. |
| `CONTROL_UNAVAILABLE` when no service is configured | **Behaviour change:** now the guest's transient, retryable mapping. The executor made it permanent. |
| `CONTROL_TIMEOUT` | Unchanged for authors. Needs a new `timeout` case in the WIT `error-code` and `ErrorCode::Timeout` in the contract, with `retryable()` returning `false`. |
| `CONTROL_TOO_LARGE` | Unchanged for authors (permanent). The input cap moves to the `NativeControl` fields. |
| `CONTROL_EXECUTION_FAILED` | Removed. Only a failed executor task produced it (a host bug or an instantiation failure). |

**Audit.** `NativeControl` writes every mutation attempt to `audit_events`
without payloads (control.rs:550), unchanged. The executor's tracing line
(`InvocationAudit`) moves into the `api` binding, logged per api function with
the same fields and message.

### 4. Upgrades

Control upgrades behave like any other agent upgrade:

- A parked run keeps and runs the control bytes composed into its artifact.
  `runtara:control/api` is frozen WIT once released, so old bytes still link.
- A run's replayed prefix and its remaining steps come from the same code. The
  executor model mixed control versions inside one run.
- New starts pick up a new control version when something forces a recompile.
  If a release changes control and should take effect at once, bump
  `direct_lowering_tag()` in that release.
- There is no kill switch for old control bytes. The remedy for a flawed old
  version is to cancel the affected runs. A buggy guest can at worst send a
  wrong but authorized request for its own run, because the host enforces
  every rule.

Removing the pin also removes the control half of `StaleTrustedDependency`.
Staged workflow-agents that compose control no longer need republishing after
every control upgrade.

### 5. Compatibility

Control is unreleased: #270 (05ba03a3) is not in v8.9.12. `runtara:control@1.0.0`
is frozen only by the first release that ships it (see
[wit-refactor.md](wit-refactor.md)). #278 already relied on this when it added
`get-state` and the `state` field of `query-request` in place.

The WIT is edited in place:

- delete `executor`, `execution`, and the `control-client` and
  `control-agent-host` worlds;
- add `timeout` to `error-code`.

This holds only if slices 1–3 land before the next release tag.

**Existing compiled artifacts.** They import `runtara:control/api@1.0.0` and
`runtara:control/executor@1.0.0` (`AgentShape.control` imports both,
runtara-wit lib.rs:229-231), plus the `runtara:builtin-artifacts/control-…` pin.

- After slice 2 their `executor` calls are denied. After slice 3 they fail at
  link, because the `executor` and pin imports are no longer provided.
- New starts recompile, because slice 2 adds a flag to `direct_lowering_tag()`
  ([compile.rs:1025](../crates/runtara-workflows/src/direct_wasm/compile.rs)).
  Production deployments recompile everything in that release anyway, because
  of the WIT refactor's ABI bump.
- Runs parked across the upgrade exist only on deployments that run main
  between #270 and this change. They follow the WIT refactor's rule: drain or
  cancel before upgrading. A parked forwarding run wakes and fails, not
  silently.
- Staged workflow-agents built with the forwarding control must be
  republished, as the WIT refactor already requires.

**Precompiled packages.** The native index's `control_importers` field goes
away
([compiled_package.rs:45](../crates/runtara-component-host/src/precompile/compiled_package.rs)).
The index is `deny_unknown_fields`, so reading must keep accepting and
ignoring the field for one release. Otherwise a cached package with the field
must count as a cache miss, not a hard error.

**`approved_builtin_artifacts`.** No schema change and no migration.

- The control rows (`runtara:builtin-artifacts/…`) stay as inert history
  (rows are append-only), and nothing reads them.
- The table's pin `CHECK` still allows them. Tightening it to trusted pins
  only can be a later forward migration, once operators have no reason to keep
  the rows.
- Committed migrations `20260927000000` and `20260927000300` are not touched.

### 6. Forward compatibility with runners as separate processes

- **The RPC seam is the `ControlHost` trait**
  ([control_host.rs:84](../crates/runtara-component-host/src/control_host.rs)):
  nine typed methods plus `ControlAuthority`. A `RemoteControl: ControlHost` in
  the runner replaces `NativeControl` there.
- **`ControlAuthority` stays data-only** (tenant, caller, op_hash) so it
  serializes.
- **Nothing else crosses the process boundary.** With D2's host checks gone,
  there is no approved set to ship to the runner and no pin to send with each
  RPC.
- **Caller identity** is not solved by this plan and not blocked by it. The
  server must not trust a caller the runner claims; it should derive the
  caller from the run's invocation lease.

### 7. Alternative: control as a pure host capability

The Rust host would do the validation and shaping, with no guest bytes, and
the compiler would lower control steps to a host `invoke(capability, json)`
import. This plan already removes most of what argued for that option: the
pin, the history and the audit. What remains in favour is that host fixes
reach parked runs at once.

Against it:

- It adds a second agent-execution path to the compiler, the operation-scope
  classification and the validation.
- Control would need a special "host agent" catalog entry for metadata,
  step-editor schemas, MCP discovery and `test_capability`.

Not recommended.

## Slices

Each slice lands on its own with `cargo fmt --all -- --check` and the workspace
clippy gate green. Slices 1 to 3 must land before the next release tag.

### Slice 1: real api in workflow stores for run entries (additive)

Goal: a composed component that imports `runtara:control/api`, in a store
invoked as the run's own entry, reaches `NativeControl` directly. The
forwarding path keeps working, and no guest changes behaviour.

- **Host on the executor.** `WorkflowExecutor` gains
  `control_host: OnceLock<Arc<dyn ControlHost>>` and
  `set_control_host`, alongside `instance_waits` and `run_state`. The server
  passes `NativeControl` to it at boot, next to today's
  `executor.set_host(native_control)`.
- **Store-level rule.** Add a `control: bool` to `InvocationControl`
  (workflow.rs:146). It is set where `control_binding` is set today, for the
  prepared run entry, and is `false` by default, so isolated capabilities,
  isolated child workflows and unprepared loads are denied.
  - `ControlApiView` for `WorkflowState` returns a call when the flag is set, a
    host is configured, and the spec carries a non-empty tenant and instance.
    Otherwise the call is denied.
  - Authority is that tenant and instance, plus `operation.current()` read at
    call time, exactly as the executor forwarder reads it today.
- **Real api.** `WorkflowExecutor::new` replaces
  `add_denied_control_api_to_linker` (workflow.rs:531) with
  `add_control_api_to_linker`. Each call is bounded by
  `min(database_deadline(), now + 90 s)`, and the `InvocationAudit` line is
  logged in the binding.
- **WIT, in place.** Add `timeout` to `error-code`. Add `ErrorCode::Timeout`
  (`CONTROL_TIMEOUT`, not retryable) to `runtara-control-contract` and to the
  agent's exhaustive mapping (`runtara-agent-control/src/lib.rs:978`).
- **Input cap.** In `NativeControl`, cap the `start` input and the
  `send-signal` payload at `MAX_INPUT_BYTES` (`too-large`).
- **Tests.** Extend `workflow/control_tests.rs` with WAT fixtures:
  - a run entry calling `api` gets host authority with the entered op;
  - an isolated capability, an isolated child workflow, an unprepared load and
    a store without tenant or instance are all denied;
  - the 90 s bound yields `timeout`;
  - a step deadline cancels a parked api call;
  - host-authority parity: for each input the guest rejects as `invalid`,
    calling `NativeControl` directly with the equivalent request is also
    rejected (`control_service` tests);
  - update `frozen_abi_tests.rs` and `frozen_abi/control-api.wat` for the new
    enum case.

### Slice 2: control becomes an ordinary composed agent

Goal: workflows run their own composed control bytes, and no artifact carries a
control pin.

- **Macro** (`crates/runtara-agent-macro/src/component.rs`). Add an
  ordinary-mode `control = true` that imports `types` and `api`, with no
  `executor` import and no `execution` export (runtara-wit lib.rs:229-238).
  The control agent switches to it (`runtara-agent-control/src/lib.rs:1392`).
  Capability bodies do not change.
- **Compiler.**
  - `CONTROL_AGENT_IMPORTS` becomes `[TYPES, API]`.
  - Drop the execution-export requirement for the control grant
    (artifact_metadata.rs:572).
  - Drop the control branch of `pin_trusted_dependencies`
    ([trusted.rs:10](../crates/runtara-workflows/src/direct_wasm/compile/trusted.rs)).
  - Drop the staged workflow-agent control-pin requirement
    (artifact_metadata.rs:583-596).
  - Staged workflow-agents that import control still need the control agent
    composed inside them. That is already true of how they are built.
- **Force recompiles.** Add `control-api=direct-v1` to `direct_lowering_tag()`.
- **Readiness.** Stop adding `runtara.approved_builtins().pins()` to
  `set_installed_trusted_pins`
  ([server.rs:1270](../crates/runtara-server/src/server.rs)). Readiness is
  trusted pins only.
- **test_capability.** Route control through the dispatcher's guarded path
  with a real-api linker and tenant-only authority (dispatcher.rs:207, :319,
  :433). Stop constructing `ControlExecutor`.
- **Executor stub.** Bind `runtara:control/executor` in workflow stores to a
  denied stub that says to recompile, so a parked forwarding artifact loads
  and fails at its call.
- **One api call per capability.** Add the test described in finding 2.
- **Tests.**
  - `tests/control_agent.rs`: rewrite `plain_agent_instances_only_forward`
    (:434) as "`api` is denied outside the control linker". Keep the read,
    error and mutation cases through `test_capability`.
  - `crates/runtara-server/tests/control_component.rs`: the composed copy runs
    in the workflow store, and the artifact has no control pin.
  - `wait_for_instances_runner_test.rs`: rewrite the upgrade case (:692) so a
    run parked on old control bytes resumes on them. Delete the revocation
    half.

### Slice 3: delete the executor and D2's host checks

- **Executor.** Delete:
  - `control_executor.rs`, `ControlCall` and `ControlBinding`;
  - `WorkflowState.control_executor` (workflow.rs:240, :1043) and
    `WorkflowExecutor::set_control_executor` (:483);
  - `add_control_executor_to_linker` and
    `add_denied_control_executor_to_linker` (control_host.rs:277,
    registry.rs:42);
  - `HostServices.control` (`crates/runtara-environment/src/runner/mod.rs:44-47`,
    :83), replaced by the `ControlHost`.

  The bundle file-name constants move to `dispatcher.rs` for
  `test_capability`.
- **Load binding.** Delete `control_binding` (workflow.rs:653) and the
  `control_binding` field of `InvocationControl` and `PreparedWorkflow`.
  `prepare_audited` drops its `control_importers` parameter.
- **Load audit.** Delete `audit_control_importers`, `ControlAudit` and
  `is_agent_export` (precompile.rs:650-800), which includes the root
  component/module import refusal and the workflow-interface refusal.
  - In `compiled_package.rs`, delete the audit calls and the isolated-member
    control refusal (:62-80). Stop writing `control_importers`, but keep
    reading it as ignored (see finding 5).
  - Before deleting, confirm the audit's parse is not the precompile worker's
    only structural check of the artifact. `validate_precompile_response` and
    Wasmtime's own parse should cover it.
- **Approved history, control half.** Delete:
  - `ApprovedBuiltins::install` and the control pin/revoked sets
    ([approved_builtins.rs:180](../crates/runtara-environment/src/approved_builtins.rs));
  - `ControlBoot`, the control approval at boot (server.rs:1217-1240,
    `embedded_runtara.rs:89-101`) and `approved_builtins().pins()`;
  - `bundled_builtin_pin` and the `runtara:builtin-artifacts/` helpers in
    `runtara-dsl/src/agent_meta.rs` once nothing else uses them.

  `install_trusted` and the trusted sets stay. The module doc becomes
  trusted-only.
- **WIT.** Delete `executor`, `execution`, `control-client` and
  `control-agent-host` (`runtara-control.wit:390-410`). Add a host-only world
  (`import api;`) for the type bindgen in control_host.rs:20-34. Delete
  `runtara_wit::control::{EXECUTOR, EXECUTION}` (lib.rs:132-134).
- **Frozen ABI.** Delete `frozen_abi/control-executor.wat` and its entries in
  `frozen_abi_tests.rs` (:12, :37, :63, :84), plus the "control executor's
  fresh stores" case (:93).
- **Macro.** Delete the `control_executor` mode (`component.rs:8-12`, :57-101,
  :144-146, :420 onwards) and its UI tests (`tests/ui/pass/control_executor.rs`,
  `tests/ui/fail/control_executor_suspends.rs`).
- **Contract.** Drop `CONTROL_EXECUTION_FAILED` and the "execution" wording of
  `EXECUTION_TIME_LIMIT_MS`. Keep the 90 s constant as the per-call bound.
- **Tests.** Delete `an_older_pin_runs_via_the_history_and_a_revoked_one_fails_the_call`
  (control_tests.rs:423) and the audit tests. Keep the store-level denial
  tests from slice 1.

### Slice 4: docs, changelog, e2e

- [control-agent-decisions.md](control-agent-decisions.md): replace D2 with the
  revised wording above, dated 2026-09-29.
- `docs/control-agent.md`: rewrite "Executor model" (:228), the D2 enforcement
  section (:239-267) and "Exposure" (:567).
- Add a dated "Changed after this plan" note to
  [control-agent-implementation-plan.md](control-agent-implementation-plan.md)
  for item 2 (:29) and the Security bullet (:61).
- Rewrite the CHANGELOG `[Unreleased]` control entry (CHANGELOG.md:45-65).
  Drop the pin, approval and revocation text, and call out:
  - the retryable `CONTROL_UNAVAILABLE`;
  - the removed `CONTROL_EXECUTION_FAILED`;
  - that parked runs keep their control version across upgrades.
- Update `crates/agents/runtara-agent-control/README.md` ("How it runs"),
  `crates/runtara-control-contract/README.md`, and the module docs of
  `control_host.rs` and `approved_builtins.rs`.
- Update the CI step comment for `control_component` (`.github/workflows/ci.yml`
  around :590 and :676).
- `e2e/test_control_upgrade.sh`: a parked parent's `control:get` after the
  upgrade runs its own composed (old) bytes. Remove the revocation stage.

## What stays identical for authors

- The nine capabilities, their input and output schemas, tags
  (`runtime:requires-run`), `operationScopedSteps`, catalog metadata,
  step-editor schemas and MCP discovery.
- D1 and D3 to D8, all enforced in `NativeControl`, `InstanceWaits` and
  admission.
- Replay safety: the compiler-emitted operation scope, receipts per
  `(caller, op_hash)`, and W075 serialization.
- Every `CONTROL_*` service code, `CONTROL_TIMEOUT` at 90 s (permanent) and
  `CONTROL_TOO_LARGE`. The exceptions are the removed
  `CONTROL_EXECUTION_FAILED` and the now-retryable `CONTROL_UNAVAILABLE`.
- A run parked across an upgrade keeps working, on its own control bytes.

What changes for operators: there is no control revocation and no
control-specific readiness. A control upgrade reaches existing workflows when
they recompile.

## Risks

| Risk | Mitigation |
|---|---|
| A release is cut before slices 1–3 land, freezing `executor`/`execution` and `error-code`. | Land them together before the next tag. Otherwise keep a legacy `ControlExecutor` for `executor@1.0.0` until no parked run needs it, and put `timeout` in a new major. |
| A tampered artifact store or a compiler bug gives a non-control component control. | Accepted. Artifacts come only from the server compiler, the removed audit could be fooled anyway, and an attacker who can write artifacts already runs arbitrary code in the tenant. |
| No kill switch for flawed old control bytes in parked runs. | Cancel the affected runs. The host enforces every rule, so a buggy guest cannot exceed its own run's authority. |
| A host bug in `NativeControl` (a failed invariant `expect`) now fails the run instead of returning `CONTROL_EXECUTION_FAILED` on the step. | Same as every other host interface. Optional follow-up: turn the invariant `expect`s in `control.rs` into `unavailable` errors. |
| The store-level rule misses an entry path, giving an isolated entry real `api`. | The flag defaults to off and is set in one place. Slice 1 tests every isolated and unprepared path. |
| The operation identity is now read at the api call instead of the capability call. | Equivalent while each capability makes exactly one api call, which a test enforces. W075 serializes operation sites in parallel regions. |
| Unbounded concurrent control host calls after the semaphore is removed. | Bounded by run permits, W075 and the DB pool, like `runtara:host/sql`. Watch DB pool metrics in the e2e. |
| Trusted regresses through the shared approved-history code. | Only the control half is removed. `install_trusted`, `TrustedExecutor` and trusted pins stay, and `tests/trusted.rs` and the trusted-pin readiness tests run in every slice. |
| A cached precompiled package with `control_importers` fails to read. | Keep reading the field as ignored for one release. |

## Verification

Per slice:

- `cargo fmt --all -- --check`.
- Workspace clippy with `GATE_FEATURES` from `.github/workflows/ci.yml:41`.
- `cargo test --workspace --lib`.

By boundary:

- **Components.** Run `scripts/build-agent-components.sh`, then:
  - `cargo test -p runtara-component-host --features component-integration-tests --tests`
    (includes `tests/control_agent.rs` and `tests/trusted.rs`);
  - `cargo test -p runtara-component-host --features component-integration-tests --lib`
    (`workflow/control_tests.rs`, `frozen_abi_tests.rs`, precompile tests).
- **Emitter and allowlist.** `cargo test -p runtara-workflows`, plus
  `cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib agent_deadline_tests`
  and `--test direct_wasm_execute`.
- **Macro.** `cargo test -p runtara-agent-macro --tests` (UI tests).
- **Contract.** `cargo test -p runtara-control-contract`.
- **WIT.** `cargo test -p runtara-wit --features resolve`.
- **Server, per CI.**
  - `cargo test -p runtara-server --features db-integration-tests,component-integration-tests --test control_component --test wait_for_instances`;
  - `cargo test -p runtara-server --features db-integration-tests,valkey-integration-tests -- --test-threads=1`
    (`control_service`, readiness).
- **Environment.**
  - `cargo test -p runtara-environment --features db-integration-tests -- --test-threads=1`
    (`approved_builtins`, trusted half);
  - `cargo test -p runtara-environment --features scoped-workflow-integration-tests --test wait_for_instances_runner_test --test agent_suspension_runner_test --test scoped_runner_test -- --test-threads=1`.
- **E2E**, on an isolated live server (own databases, Valkey, `TENANT_ID` and
  ports, never :7001):
  - `STAGES=1,2,3,4,5,6 e2e/test_control_agent.sh`;
  - `e2e/test_control_upgrade.sh`: a parked parent's `control:get` runs its own
    old bytes after the upgrade;
  - one manual check that a workflow compiled before slice 2 recompiles on its
    next start.

None of these were run while writing this plan.
