# WaitForInstances: change plan for runtarahq/runtara#270

Status: implemented, 2026-09-28, on the unmerged control-agent branch.

As built, where it differs from the plan below:

- The host interface also has `release(key)`, which drops the settled wait
  once the step's result is checkpointed.
- The deadline is persisted by the store at first registration and wins on
  every replay; it is not a guest checkpoint.
- An empty `instanceIds` settles at once with resolution `empty`, without
  registering a wait.
- Literal `instanceIds` or `timeoutMs` out of range are E133
  (`InvalidWaitForInstancesConfig`).
- The step is displayed as "Wait for Instances".
- Agent suspensions may wake only on `at`; an `instances` wake is refused with
  `AGENT_INVALID_SUSPENSION`.

## Decisions

- Waiting on other runs becomes a step type, `WaitForInstances`, next to
  `WaitForSignal`. It parks the run itself, so it is the run's own control
  flow, not an operation on another run.
- `control:wait` and the `poll-wait` API function are removed. There is one way
  to wait.
- Control keeps every operation that acts on another run by id: `start`, `get`,
  `query`, `list-pending-signals`, `send-signal`, `cancel`, `pause`, `resume`.
- Agent suspension stays as an extension point for future long-polling agents.
  No built-in agent uses it after this change. This covers
  `runtara:agent-suspension`, the continuations table (042), `scope.suspend`,
  the suspending-capability validation rules and the 16-wake cap.

Nothing affected is released: the control WIT, the operation WIT, the
suspension crate, the control contract and migrations 039-042 are all absent
from `origin/main`. `runtara:control@0.1.0` is edited in place.

## The step

```json
"waitApprovals": {
  "stepType": "WaitForInstances",
  "instanceIds": { "valueType": "reference", "value": "steps.startAll.outputs.instanceIds" },
  "mode": "all",
  "timeoutMs": { "valueType": "immediate", "value": 86400000 }
}
```

- `instanceIds`: 1 to 1000 distinct direct children of the run.
- `mode`: `all` or `any`.
- `timeoutMs`: optional. When it passes, the wait settles with what it
  observed; it never cancels children and is not an error. The first
  registration's deadline is persisted and wins on replay.
- Output: the settled wait, in today's `wait` output shape (resolution plus one
  outcome per target, with the same inline caps: 256 KiB per output, 16 KiB
  per error, 3 MiB total).
- `breakpoint`, like other steps.

The semantics are exactly today's control wait: D1 (children only; the run
itself is `invalid`, ancestors `denied`, others `not-child`), D4 (pausing a
waiting run pauses it immediately), D7 (unknown targets are `not-found`), and
the durable waits layer (migrations 039-041, the wake trigger and the
reconciler).

## How it runs

**Host interface.** A new package, imported by the root workflow, because no
existing runtime function fits (`register-input` / `poll-input` are managed
signals):

```wit
package runtara:workflow-wait@0.1.0;

interface instances {
    /// Register (or, on replay, find) the wait for this step and evaluate it.
    register: func(key: string, request: list<u8>) -> result<list<u8>, string>;
    /// Read the wait without blocking.
    poll: func(key: string) -> result<list<u8>, string>;
}
```

- **Identity.** The compiled code passes a v2 key of kind `wait-instances`
  (`["wait-instances", workflow, namespace frames, loop path, [step_id]]`). The
  host hashes it into the `wait_id`, as `op_hash` is today. The host takes the
  tenant and the run from its own store.
- **Parking.** The lifecycle ABI is unchanged; `runtara:workflow-lifecycle` is
  released on `main`. As control wait does today, the workflow suspends with
  `at(deadline)` (or no timed wake), and the host attaches the pending wait out
  of band through `InvokeRunResult.instance_waits`. The runner parks with
  `ParkReason::Instances`. What changes is the source: `instance_waits` is
  filled from the waits this interface registered during the invoke, not from
  `OperationScopeState`.
- **Lowering** follows `emit_wait_for_signal_plan` (`compile/wait.rs:262-677`):
  breakpoint, key, `register`, and on pending a suspend. After wake, the replay
  reaches the step again, `register` finds the settled wait, and the step
  builds its output and runs its mappings. Once the step's checkpoint is saved,
  the resolved wait row is deleted, as `scope.release` does today.

**Server side.** Move the wait logic out of `NativeControl`
(`api/services/control.rs`) into an `InstanceWaits` service:
`authorize_wait_targets` (D1), `settle_admission_outcomes`, `current_wait`,
`wait_error` and `wait_poll` with its caps. It implements a new
`InstanceWaitHost` trait in the component host, installed at boot like
`ControlHost` and linked into every workflow store. This is compiled workflow
code calling the host, not agent bytes, so there is no approved-digest executor
and no binding check.

## Validation

`OperationScopedKind` (`runtara-dsl/src/step_context_rules.rs`) gains
`WaitForInstances`, with the rules today's suspending kind has:

- durable workflow required (E028);
- not in `onError`, `onWait`, AiAgent tools or AiAgent memory (E131, E132);
- serialized out of parallel windows (W075);
- a warning under an enclosing retry (W076);
- refused when the workflow is published as an agent.

E029 and W078 do not apply: they bound an agent step's own timeout against the
suspension margin, and `timeoutMs` here is the wait's deadline.

The compiler backstop (`agent_suspend.rs::check_sites`) refuses the same
placements.

## Control after the change

Control no longer suspends, so it drops what existed only for `wait`:

- WIT: `wait`, `poll-wait`, `wait-mode`, `wait-request`, `target-outcome`,
  `wait-resolution`, `wait-progress`, `wait-settled`, `wait-poll` and the
  `wait-closed` error code. The wait records move to
  `runtara:workflow-wait` if the host interface uses typed records rather
  than JSON. `suspension-reason.waiting-instances` stays.
- `executor.invoke` and `execution.invoke` return a plain result instead of
  an `outcome`, and `execution` loses its continuation argument. `execution`
  itself stays: it is how D2 runs approved bytes.
- The composed control copy no longer exports `suspendable`
  (`crates/runtara-agent-wit/templates/control-agent.wit.in`).
- The macro's `control_executor` drops its suspension glue
  (`runtara-agent-macro/src/component.rs:471-560`).
- `ControlExecutor` drops continuation passing, the suspended-state size check
  and `record_waits`.
- `artifact_metadata.rs` drops control's `runtara:agent-suspension/types` grant.
- Control steps become the `Control` kind only, so E028/E029/W078/E131 in
  `onError` no longer apply to them. W075 and W076 still apply.
- `runtara-control-contract`: remove `CONTROL_CONTINUATION_V1` and
  `ErrorCode::WaitClosed`; move `MAX_WAIT_TARGETS` and the wait inline caps
  next to the new service.
- Remove the dead `RuntimeClient::close_instance_wait`
  (`runtara-server/src/runtime_client.rs:887`).

Nothing in core, environment or the migrations is removed. `InstanceWaits`,
`park_instance_on_targets`, `ParkReason::Instances`, the reconciler and D4 are
all used by `WaitForInstances`. Comments that name control `wait` are reworded.

## Adding the step type

A new `Step` variant must be added everywhere the enum is matched (the
`GroupBy` arm marks each place):

- **DSL:** `schema_types.rs` (variant and struct with `deny_unknown_fields`),
  `step_registration.rs`, `step_output_shape.rs`, `DSL_VERSION` and the DSL
  changelog.
- **Validation:** the `validation.rs` walks listed for `WaitForSignal`
  (references, mappings, execution order, names, step type name, templates,
  configuration), `graph_identity.rs`, `workflow_features.rs`.
- **Direct compiler:**
  - `support.rs`, including the step type table, direct support, publish
    safety and runtime timeout;
  - `manifest.rs`;
  - `plan.rs` (`DirectRunPlan` variant, the step lists and every plan walk
    including `plan_contains_suspension`);
  - `dispatcher.rs`, `split_parallel.rs`, `branch_parallel.rs`;
  - a new `compile/wait_instances.rs`.
- **Stdlib:** the key kind, the debug start and output arms in `direct_json.rs`,
  and the output builder.
- **Server:** `middleware/entitlement.rs`, `handlers/workflows.rs`,
  `dto/workflows.rs`, OpenAPI and the generated client.
- **Frontend:**
  - the step type union, node type, icon, step picker and form;
  - error-route support and container handling in `CustomNodes/utils.tsx`;
  - the parked-step text that says "control wait".
- **MCP:** authoring schema and step reference text.

## Tests

**Removed with `control:wait`:**

- `runtara-workflows/tests/control_wait/mod.rs`;
- the wait cases in `agent_suspend_tests.rs`, `component-host/tests/control_agent.rs`
  and `server/tests/control_service.rs`;
- the wait parts of `control_component.rs`.

**Retargeted to `WaitForInstances`:**

- Parks per iteration in Split, While, branch arms and embedded workflows, and
  an embed retry registering again (from `control_wait/mod.rs`).
- D1 checks and the stable result caps (from `control_service.rs`).
- Parking without a slot and resuming after a restart (from
  `control_component.rs`).
- The runner test (`environment/tests/control_wait_runner_test.rs`): parks
  runner-free, a restarted runner resumes it, and a parked parent resumes on its
  bound image after recompile and cleanup.
- D4: pausing a waiting run pauses it immediately.

**Re-homed:** the control upgrade and revocation tests for parked runs
(`control_wait_runner_test.rs:640`, e2e stage 6, `e2e/test_control_upgrade.sh`)
park on `WaitForInstances` and make a control call after the wake, which is
where the pin history matters.

**Kept, and filled where coverage is lost:**

- Generic agent suspension keeps its fixture-agent tests
  (`agent_suspend_tests.rs` probe agents, the `waiter:pause` rule tests, the
  macro UI tests, the frozen ABI fixtures, continuation conformance).
- Once control stops suspending, no test runs a suspending agent in the real
  embedded runner. Add one: a fixture agent suspends on `at`, the run parks,
  and a restarted runner resumes it with its continuation.
- Fixtures that only mention wait are adjusted: `control_test.wat`,
  `operation_scoped_tests.rs`, the synthetic catalogs in
  `validation_operation_scoped_tests.rs` and `runtara-validation-wasm`, the WIT
  pinning tests (`runtara-workflow-wit/src/lib.rs:606-790`, error count 19 to
  18) and the frozen ABI fixtures (`control-api.wat`, `control-executor.wat`).

**E2E:**

- `e2e/test_control_agent.sh` stages 5 and 6 move to `WaitForInstances`:
  parallel approvals, D4 pause while parked, `any` with `leave_running`, the
  deadline, restart while parked, placement errors, and the upgrade.
- `e2e/test_control_upgrade.sh` parks on `WaitForInstances`.
- `e2e/test_control_signals_only.sh`: header comment only.

## Docs

- `docs/control-agent.md`: remove the `wait` capability and describe
  `WaitForInstances`, keeping the durable wait contract.
- `docs/control-agent-implementation-plan.md`: note the change.
- `CHANGELOG.md`: rewrite the control wait entries as the step type; keep the
  agent suspension entry as a general mechanism.
- MCP, DSL and frontend strings that say "control wait".

## Order

1. `InstanceWaits` service and `InstanceWaitHost`, moved from `NativeControl`
   with its tests. Control wait still works on top of it.
2. `runtara:workflow-wait@0.1.0`, the `WaitForInstances` step end to end, and
   its tests, alongside control wait.
3. Remove `control:wait`, simplify the control executor, WIT and macro, and
   update fixtures.
4. Retarget the e2e scripts and docs.

Each step builds and passes on its own.

## Verification

- `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets
  --features "$GATE_FEATURES" -- -D warnings`.
- `scripts/build-agent-components.sh`, then the component-host, workflows
  (`direct_wasm_execute`), environment, store-postgres and server test suites
  from CI, once each.
- Frontend: `npm test`, `npm run lint`, `npm run build`; regenerate the API
  client.
- Live: `e2e/test_control_agent.sh` and `e2e/test_control_upgrade.sh` on an
  isolated server.
