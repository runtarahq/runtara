# WIT refactor

Status: plan. Backward compatibility is explicitly de-prioritised: this is a
one-time clean break that resets every runtara WIT contract to `1.0.0`, drops
all legacy linked versions, and restarts the "released WIT is never edited"
rule from the new baseline.

Two goals:

1. **Structure.** Put every runtara WIT contract in one crate, grouped into
   packages by who may import them.
2. **One export shape.** Workflows and agents export the same
   `capabilities.invoke -> result<outcome, error-info>`. The `lifecycle`
   export disappears, three workflow export shapes become one, and published
   workflow-agents can be fully durable.

## Why

The host-provided capabilities are split into separate packages, and that is
the right unit: a WIT package is the unit of *versioning*, while the unit of
*linking and privilege* is the interface. What is wrong is everything around
the packages.

### Structure

- **Contracts live in five places.** `runtara-workflow-wit` (9 packages,
  including agent-facing ones), `runtara-agent-wit`, `runtara-agent-trusted/wit`,
  `runtara-agent-suspension/wit`, and `runtara:host-io` exists only as a string
  constant in `runtara-workflows/src/direct_wasm/compile.rs`.
- **Duplicated vocabulary.** `runtara:abi/types.error-info` and
  `runtara:agent/types.error-info` are the same record, kept apart "pending a
  coordinated migration". `runtara:abi.connection-info` has no user.
- **Packages split by accident, not by audience.** Eleven packages with one
  interface each, at five different versions (`0.1`–`0.4`). Allowlists
  enumerate interface names instead of following package boundaries.
- **Fragile resolution.** The compiler pushes WIT strings in hand-maintained
  dependency order with conditional pushes and "pushing twice is an error"
  special cases (`compile.rs` ~L1770–1880). CI validates `operation` and
  `control` by copying files into a scratch `deps/` dir. Agents reach host WIT
  through `../../runtara-workflow-wit/wit/<pkg>` paths.
- **Per-agent WIT generated four ways.** 27 identical `build.rs` files write
  27 committed `wit/agent.wit` files from 4 templates in
  `runtara-agent-wit/templates`, and the compiler has its own generators
  (`agent_wit_package*`) plus
  `runtara_agent_suspension::SUSPENDABLE_INTERFACE_WIT`.
- **Legacy linking.** The host still links `runtime@0.3.0`,
  `connection-resolver@0.1.0` and `lifecycle@0.1.0`.
- **The WIT crate is not a WIT crate.** `runtara-workflow-wit` carries ~3,000
  lines of Rust (`isolation_package`: checkpoint namespace, invocation path,
  manifest, scope) behind a feature flag.

### Export shapes

- **Three workflow export shapes** (`WorkflowAbi`, `direct_wasm/component.rs:55-104`):
  - `CliRunHttp` exports `wasi:cli/run`. It is test-only.
  - `InvokeHostImports` exports `lifecycle.invoke`.
  - `AgentCapabilities` exports `runtara:agent-<slug>/capabilities.invoke`,
    for publishing a workflow as an agent.
  - Native agents add a fourth: `suspendable.invoke`.
- **Two outcome and wake types that almost match.**
  - Lifecycle: wake = `at | on-signal | on-resume`, no state.
  - Agent suspension: wake = `at | instances`, plus a continuation `state`.
  - The discriminants conflict: `1` is `on-signal` in one and `instances` in
    the other.
- **Workflow-agents park through the error channel.** The capabilities shape
  has no suspended arm, so a published workflow-agent that parks returns
  reserved error codes, `__rt_suspended__` and `__rt_on_signal__`
  (`compile/abi.rs:314-329`). The parent sniffs every workflow-agent error for
  them (`emit_agent_suspend_sentinel_check`, `abi.rs:735-789`), and user
  errors that happen to use those codes are remapped (`direct_json.rs:5627`).
- **Wakes are lost or smuggled.**
  - A workflow calling a suspending agent collapses every agent wake into one
    `at` and silently drops the rest (`agent_suspend.rs:571-594`).
  - WaitForInstances has no wake at all. Its wait ids travel out of band in
    `InvokeRunResult.instance_waits`.
- **Two terminal channels.** Terminal status is persisted through
  `runtime.complete`/`runtime.fail`, and the return value is only "additive".
  The workflow-agent shape has to *suppress* those imports so a child does not
  finish its parent (`core_imports.rs:916-936`).
- **Published workflow-agents are only partly durable.** Publication refuses
  WaitForInstances, suspending capabilities, control-agent calls and AiAgent
  retry parking (`support.rs:253-417`). Staging refuses `suspends: true`
  (`workflow_agents.rs:168-173`). Parking is signalled by two ad-hoc
  certificates, `parks:1` and `non-suspending:1`.

## Target

### Packages

Packages are grouped by **who may import them**, so each package is one
privilege class and one versioning cadence. Everything starts at `1.0.0`.

| Package | Interfaces | Imported / exported by | Use cases |
|---|---|---|---|
| `runtara:agent@1.0.0` | `types` (error-info, signal-wait, wake, suspension, outcome), `capabilities` (reference shape for host bindgen), `continuation` (host func) | every component (types); native suspending agents (continuation) | Every capability and every workflow returns `completed` or `suspended` with wakes, or fails with `error-info` (retryable, category, retry-after). A long-running native capability parks the step runner-free and resumes with its saved continuation. |
| `runtara:agent-<id>@1.0.0` | `capabilities` | generated per agent **and per workflow**; the unique package name is what lets composition tell components apart | A workflow Agent step calls `http`, `csv`, `slack` and so on. Every compiled workflow exports this shape: the host starts a top-level run through it, and a parent calls a published workflow-agent through it, including one that waits, sleeps or waits on child runs. Parallel pools get phantom copies of one agent. |
| `runtara:host@1.0.0` | `http`, `sql`, `connections`, `timers` | ordinary agents and workflows | `http`: Shopify, HubSpot, Slack and other API agents send requests through the credential proxy without ever seeing secrets. `sql`: object-model queries and writes. `connections`: safe connection descriptors and resource lookups, such as the object-model layout or MCP tool config. `timers`: retry backoff, parallel-window waits and the whole-run abort alarm. |
| `runtara:workflow@1.0.0` | `runtime`, `tasks`, `operation`, `waits` | compiled workflow logic only, including published workflow-agents | `runtime`: checkpoints, durable sleep (Delay), WaitForSignal input, events, heartbeat, cancel and pause. `tasks`: EmbedWorkflow child runs. `operation`: replay-safe identity for suspending capabilities and control mutations. `waits`: WaitForInstances. |
| `runtara:workflow-stdlib@1.0.0` | `json` | composed stdlib component (not host-provided; separate because it churns most) | Pure JSON step logic compiled into every workflow: input mappings, conditions, Switch, Split, While, Filter, GroupBy, and retry and error shaping. |
| `runtara:trusted@1.0.0` | `executor`, `execution` | approved trusted agents only: their ordinary copy imports `executor`, and the host calls their `execution` export | S3 and Azure Blob agents that need real credentials run in a fresh host-managed instance with network and filesystem denied. Ordinary code reaches them only through `executor`. |
| `runtara:control@1.0.0` | `types`, `api`, `executor`, `execution` | the control agent only | A workflow step starts child runs, queries instances, lists and answers pending signals, and cancels, pauses or resumes its children. |

Deleted packages: `runtara:abi`, `runtara:agent-suspension`,
`runtara:outbound-http`, `runtara:database`, `runtara:connection-resolver`,
`runtara:host-io`, `runtara:workflow-lifecycle`, `runtara:workflow-runtime`,
`runtara:workflow-execution`, `runtara:workflow-operation`,
`runtara:workflow-wait`.

### One export shape

```wit
package runtara:agent@1.0.0;

interface types {
    record error-info { /* unchanged */ }

    /// A pending WaitForSignal: the deterministic signal id and the
    /// persisted absolute deadline.
    record signal-wait {
        signal-id: string,
        deadline-ms: option<u64>,
    }

    /// Re-invoke when ANY wake fires. The caller's step deadline always
    /// applies in addition.
    variant wake {
        at(u64),                // wall-clock ms since the Unix epoch
        on-signal(signal-wait), // workflows only
        on-resume,              // pause or drain; workflows only
        instances(string),      // a host-registered instance wait id
    }

    /// `state` is a native agent's continuation (at most 64 KiB). Workflows
    /// keep their state in checkpoints and always return it empty.
    record suspension {
        wakes: list<wake>,
        state: list<u8>,
    }

    variant outcome {
        completed(list<u8>),
        suspended(suspension),
    }
}

interface capabilities {
    use types.{error-info, outcome};
    invoke: async func(capability-id: string, input: list<u8>)
        -> result<outcome, error-info>;
}

interface continuation {
    continuation: func() -> option<list<u8>>;
}
```

**Rules:**
- A compiled workflow exports `runtara:agent-<id>/capabilities` with
  capability id `run`. `<id>` is the slug for a published workflow-agent and
  a fixed id for anything else (today's `CAPABILITIES_EXPORT_AGENT_ID`).
  Publishing no longer compiles a different ABI.
- The return value is the **only** terminal channel. `runtime.complete`,
  `runtime.fail` and `runtime.load-input` are deleted: input is the argument,
  and the result is the return. This lands separately, as Phase 6.
- A non-suspending agent may only return `completed`. `suspended` from an agent
  whose metadata does not declare `suspends` fails the step with the existing
  `AGENT_UNEXPECTED_SUSPEND` code.
- A native agent may return the wakes `at` and `instances`. `on-signal` and
  `on-resume` come only from workflow logic, and the host refuses them from a
  native agent with `AGENT_INVALID_SUSPENSION`. The Rust `Wake` enum in
  `runtara-agent-suspension` exposes only the two it may use.
- A caller forwards the wakes it receives. It adds `at(step deadline)` when
  the step has a timeout, instead of collapsing everything into one `at`.

**Layout.** The canonical-ABI layout the emitter writes by hand:

| Field | Offset |
|---|---|
| result tag | @0 |
| outcome tag | @8 |
| completed list ptr/len | @12/@16, the same as lifecycle today |
| suspended: wakes ptr/len | @12/@16 |
| suspended: state ptr/len | @20/@24 |
| wake | size 32, align 8 |

The wake discriminants keep lifecycle's order: `at`=0, `on-signal`=1,
`on-resume`=2, and the new `instances`=3. Every pause, drain and delay writer
keeps its discriminant; only the agent suspension code moves `instances` from
1 to 3 and its wake stride from 16 to 32.

### Rename map

Phase 3 changes **names and versions only**, so the emitter's hand-computed
offsets stay valid through it. Phase 5 then makes the shape change on top.

| Old | Phase 3 | Final (Phase 5) |
|---|---|---|
| `runtara:agent/types@0.4.0`, `runtara:abi/types@0.1.0` | `runtara:agent/types@1.0.0` | same, with the unified types |
| `runtara:agent-suspension/types@0.1.0` | `runtara:agent/suspension@1.0.0` | merged into `runtara:agent/types` |
| `runtara:agent-suspension/context@0.1.0` | `runtara:agent/continuation@1.0.0` | same |
| `runtara:agent-<id>/capabilities@0.4.0` | `runtara:agent-<id>/capabilities@1.0.0` | same, returning `outcome` |
| `runtara:agent-<id>/suspendable@0.4.0` | `runtara:agent-<id>/suspendable@1.0.0` | deleted, merged into `capabilities` |
| `runtara:workflow-lifecycle/lifecycle@0.2.0` (+`0.1.0`) | `runtara:workflow/lifecycle@1.0.0` | deleted, workflows export `capabilities` |
| `runtara:outbound-http/client@0.1.0` | `runtara:host/http@1.0.0` | same |
| `runtara:database/sql@0.1.0` | `runtara:host/sql@1.0.0` | same |
| `runtara:connection-resolver/resolver@0.2.0` (+`0.1.0`) | `runtara:host/connections@1.0.0` | same |
| `runtara:host-io/timers@0.1.0` | `runtara:host/timers@1.0.0` | same |
| `runtara:workflow-runtime/runtime@0.4.0` (+`0.3.0`) | `runtara:workflow/runtime@1.0.0` | same, without `load-input`, `complete` and `fail` |
| `runtara:workflow-execution/tasks@0.1.0` | `runtara:workflow/tasks@1.0.0` | same, `task-outcome` uses `runtara:agent/types.wake` |
| `runtara:workflow-operation/scope@0.1.0` | `runtara:workflow/operation@1.0.0` | same, `suspend` uses `runtara:agent/types.wake` |
| `runtara:workflow-wait/instances@0.1.0` | `runtara:workflow/waits@1.0.0` | same |
| `runtara:workflow-stdlib/json@0.1.0` | `runtara:workflow-stdlib/json@1.0.0` | same |
| `runtara:trusted/*@0.1.0`, `runtara:control/*@0.1.0` | same names `@1.0.0` | same |

`1.0.0` is frozen only by the first release that ships it. Phases 3–5 go out
in one release, so the interim Phase 3 shapes are never frozen.

`runtara:connection-info` is dropped. Internal composition names that are not
WIT contracts (`runtara:workflow-logic`, `runtara:builtin-artifacts/…`,
`runtara:trusted-artifacts/…`, `runtara:isolated-package`) are out of scope.

### Crates

```
crates/runtara-wit/                    NEW — the only home of runtara WIT
  wit/
    deps.toml, deps.lock, deps/        wasi 0.2.3 pins (moved from runtara-agent-wit)
    agent/runtara-agent.wit
    host/runtara-host.wit
    workflow/runtara-workflow.wit
    workflow-stdlib/runtara-workflow-stdlib.wit
    trusted/runtara-trusted.wit
    control/runtara-control.wit
  src/lib.rs                           name constants, WIT text constants,
                                       agent package generator, resolve helper
crates/runtara-invocation-contract/    NEW — isolation_package moved out verbatim
```

- **Deleted crates:** `runtara-workflow-wit`, `runtara-agent-wit`.
- **Kept as Rust-only helper crates:** `runtara-agent-trusted` (TrustedContext,
  `object_url`, `invoke`) and `runtara-agent-suspension` (Wake, Suspendable,
  SuspendContext, caps, error codes). Their `wit/` dirs and WIT constants go.
  `runtara-control-contract` and `runtara-database-contract` are serde
  contracts and stay; only their doc paths change.

`runtara-wit/src/lib.rs` provides:

1. **Name constants**, one module per package, derived from a single
   version literal (`macro_rules! v { () => { "1.0.0" } }` + `concat!`):
   `runtara_wit::host::HTTP`, `runtara_wit::workflow::RUNTIME`,
   `runtara_wit::agent::TYPES_PREFIX`, … These replace every
   `*_INTERFACE_NAME`, `LEGACY_*`, `PACKAGE`, `EXECUTOR_INTERFACE` and the ~60
   string literals scattered across crates.
2. **WIT text constants** (`include_str!`) and `PACKAGES: &[(&str, &str)]`
   in dependency order. This is the only place that knows the order.
3. **`fn resolve() -> wit_parser::Resolve`** (feature `resolve`, since
   wit-parser is heavy) that pushes every package once. The compiler starts
   from this and adds only the per-agent packages and the workflow world.
   Pushing unused packages is harmless: a world imports only what it names.
4. **`fn agent_package(id: &str, shape: AgentShape) -> String`**, where
   `AgentShape` is `Plain | Trusted | Control` plus `scoped` (the compiler's
   scoped-import variant). `Suspending` exists only until Phase 5 removes
   `suspendable`. This is the one generator. The compiler calls it at
   runtime; `runtara-agent-macro` calls it at expansion time and passes the
   text to `wit_bindgen::generate!` as `inline:`.
5. **`WIT_DIR`** = `concat!(env!("CARGO_MANIFEST_DIR"), "/wit")`, so the
   proc macro emits absolute `path:` entries instead of the `../../` convention.

### Allowlists after

In `runtara-workflows/src/direct_wasm/compile/artifact_metadata.rs`:

| Component | May import |
|---|---|
| ordinary agent | `wasi:*`, `runtara:agent/types@1`, `runtara:host/*@1` |
| + `trusted` grant | `runtara:trusted/executor@1` |
| + `suspends` grant | `runtara:agent/continuation@1` |
| + `control` grant | `runtara:control/{types,api,executor}@1` |
| staged workflow-agent | skips the list; may import `runtara:workflow/*`; may import `runtara:control/{api,executor}` only when its publish-time dependency manifest pins the approved control artifact; denied `runtara:agent/continuation@` (a workflow keeps its state in checkpoints) |

`AGENT_IMPORT_ALLOWLIST` becomes a package-prefix rule for `runtara:host/`
plus explicit entries, with no legacy entries.

**`runtara:trusted` stays its own package, and `executor` moves behind a
grant.** Trusted execution is for approved trusted agents only.
- **Today any ordinary agent may import `runtara:trusted/executor`.** It is on
  `AGENT_IMPORT_ALLOWLIST` (`artifact_metadata.rs:423`). The host executor
  checks the *target* capability, the tenant and the connection type
  (`component-host/src/trusted.rs` ~L375-425), but not who is calling. So
  ordinary and third-party agents can drive the trusted S3/Azure capabilities
  with any connection id they are handed.
- **After:** a new `AgentImportGrants::trusted` grants `executor`, the same way
  `control` is granted today. It requires both:
  - the sidecar declares `trusted`;
  - the component resolved from the primary components dir.

  Merging `trusted` into `runtara:host` would do the opposite and hand it to
  every agent.

### Versioning rule from 1.0.0

Replace the ABI rule in the `runtara-workflow-wit` crate doc with this rule in
`runtara-wit`:

- **Additive change** (new function or new interface in a package): bump the
  package minor version. The host links only the newest minor. Wasmtime's
  `Linker` matches semver-compatible names, so an artifact importing `@1.0.0`
  links against a `@1.1.0` definition. Phase 0 must verify this for
  wasmtime 46.
- **Any change to an existing function or type**: bump the major version. The
  host links the old major beside the new one until no parked run needs it.
- `frozen_abi_tests.rs` fixtures are regenerated for `1.0.0` and keep
  enforcing this.

The move from `0.x` to `1.0` is what makes additive changes cheap. Under
`0.x`, every minor bump is incompatible, which is why the host carries three
legacy versions today.

## Phases

### Phase 0: verify tooling (spike, no production change)

Done. All three checks hold; `frozen_abi_tests.rs` keeps the semver check as
a regression test.

1. A component importing `runtara:host/timers@1.0.0` links against a
   wasmtime 46 `Linker` that defines only `@1.1.0` (with an extra function).
2. `wit_bindgen::generate!` with `inline:` plus `path: [WIT_DIR/agent, …]`
   resolves a per-agent package that `use`s `runtara:agent/types`. The macro
   already mixes path arrays; `inline:` plus paths is the new part.
3. The proc macro can read `runtara_wit::WIT_DIR` and emit it as a literal.
   A proc-macro crate can depend on a normal crate.

If (1) fails, keep the `1.0.0` reset but document that minor bumps also need
linking the old name; nothing else changes.

### Phase 1: extract `isolation_package`

Done (8f53964a).

Move `runtara-workflow-wit/src/isolation_package*` to
`runtara-invocation-contract` unchanged. Repoint `runtara-component-host`,
`runtara-environment`, `runtara-server` and `runtara-workflows` (all
currently use feature `isolation-package`). This is a pure move, independent
of everything else, and can land first.

### Phase 1b: delete the composed runtime component

Done (c8e540ad).

`runtara-workflow-runtime` is a guest component that *exports*
`runtara:workflow-runtime/runtime` and talks to core over HTTP through
`runtara-sdk`. Production no longer uses it:

- The default `RuntimeBinding` is `HostImport` (`direct_wasm/component.rs:46-52`):
  the host implements `runtime` natively, in `component-host/src/runtime_host.rs`
  and `environment/src/runtime_host.rs`.
- `Composed` is reachable only through the `RUNTARA_DIRECT_RUNTIME_BINDING=composed`
  rollback lever (`compile.rs:1003-1028`). By the code's own account, that
  lever "is no longer an escape hatch": such an artifact crashes promptly
  under the production runner, as the environment test
  `a_composed_runtime_artifact_crashes_promptly_under_the_production_runner`
  pins. It is kept only "until no deployment is confirmed to set it".
- It is still built and shipped. `scripts/build-agent-components.sh:156-164`
  builds it (still declaring the stale `runtime@0.1.0`), and
  `scripts/stage-agent-components.py:29` stages it into the bundle.
- Its only users are tests and tooling:
  - the wasmtime-CLI A/B reference axis and binding-differential tests in
    `tests/direct_wasm_execute.rs`;
  - `tests/standalone_compile.rs` and `tests/cooperative_measurement`;
  - the `Composed` cases in `component.rs`, `compile/tests.rs` and
    `compile/operation_scoped_tests.rs`;
  - `scripts/measure-cooperative-workflows.py`.
- It is the **only** dependent of `runtara-sdk`, which in turn is the only
  dependent of `runtara-sdk-macros` (`#[resilient]`). Other mentions are
  comments (`environment/src/runtime_host.rs:29-74`, `agent-macro`,
  `runtara-agents/src/types.rs`).

**Delete:**
- the crates `runtara-workflow-runtime`, `runtara-sdk` and `runtara-sdk-macros`
  (~5.6k lines);
- `RuntimeBinding` itself, since only `HostImport` remains, and
  `RUNTARA_DIRECT_RUNTIME_BINDING`;
- the build and stage entries in both scripts;
- the Composed and CLI-axis tests, and the crash-pinning environment test;
- the wording in the MCP tool description (`mcp/tools/workflows.rs:545`).

Rewrite the comments that point at `runtara-workflow-runtime/src/lib.rs`
(`component-host/src/runtime_host.rs:44`, `:126`;
`runtara-workflow-stdlib/src/lib.rs:14`) to describe the host implementation.

After this, `runtara:workflow/runtime` has no guest implementation, so
`runtara-wit` declares no export world for it. The compiler imports it by
name, and the host links it by name.

This is independent of the WIT work. Landing it before Phase 3 removes one
guest bindgen site and a whole binding axis from the test battery.

### Phase 2: create `runtara-wit`

Done (11e33fd0).

- Write the six packages from the existing files, applying the Phase 3 column
  of the rename map. `workflow/tasks` `use`s `lifecycle.{wake}` inside the same
  package; `workflow/operation` `use`s `runtara:agent/suspension.{wake}`;
  `control` and `trusted` `use` `runtara:agent/types`.
- `host-io` becomes a real file (`runtara:host/timers`).
- Only client worlds per package (`http-client`, `sql-client`, …).
  Delete the export-side `database-host` and `connection-resolver` worlds if
  nothing binds them; grep first.
- Move `deps.toml` / `deps.lock` / `deps/` from `runtara-agent-wit`.
- Tests in `runtara-wit` replace the CI scratch-dir validation:
  every package parses via `resolve()`; every name constant names a real
  interface; `agent_package` output parses for every `AgentShape`; the
  existing structural tests (database has three async ops, stdlib function
  list, …) move over.

Nothing consumes the crate yet; the workspace still builds.

### Phase 3: atomic switch (one PR)

Done (d2b66430), together with Phase 4: moving the WASI deps broke the old
crates, so they were deleted in the same commit. `signal-wait` keeps its
`checkpoint-id` field name. The trusted-executor grant landed separately
(d5d201c1).

Guests and host must agree on names, and there is no compatibility shim, so
this is one change. Mechanical, grep-driven:

- **Guest bindgen sites:** `runtara-http/src/host_io.rs`,
  `agents/runtara-agent-object-model/src/sql_client.rs`,
  `agents/runtara-agent-mcp/src/lib.rs`, `runtara-workflow-stdlib/src/lib.rs`
  and `runtara-agent-trusted/src/lib.rs`. `runtara-workflow-runtime` is already
  gone (Phase 1b).
  Point them at `runtara-wit/wit/<pkg>` path arrays with the new world names.
- **Agent macro:** `runtara-agent-macro/src/component.rs` calls
  `runtara_wit::agent_package` and emits `inline:` plus absolute paths.
  Export names become `…@1.0.0`.
- **Compiler:** `runtara-workflows/src/direct_wasm/compile.rs` replaces the
  push sequence with `runtara_wit::resolve()` and deletes `HOST_IO_TIMERS_WIT`,
  `AGENT_TYPES_WIT` and the `agent_wit_package*` functions; phantom
  pool-member packages use the generator. `compile/core_imports.rs` and
  `core_module.rs` match imports via the constants. `component.rs` and
  `artifact_metadata.rs` update the allowlists as above.
- **Bump** `DIRECT_WORKFLOW_INVOKE_ABI_VERSION` to 3 so every cached artifact
  is stale and recompiles. Verify that the server cache check in
  `runtara-server/src/api/services/compilation.rs` keys on it.
- **Host:** `runtara-component-host` linker registration in `outbound_http.rs`,
  `database_host.rs`, `connection_resolver_host.rs`, `host_io.rs`,
  `runtime_host.rs`, `execution_host.rs`, `trusted.rs`, `control_host.rs`,
  `lifecycle.rs`, `registry.rs` (`is_capabilities_iface_name`), `bindings.rs`,
  `workflow.rs` and `workflow/invocation_abi.rs`. **Delete** the legacy
  registrations (runtime 0.3, sync resolver 0.1, lifecycle 0.1) and their
  code paths.
- **Environment/server:** `runtara-environment/src/runtime_host.rs`,
  `runner/common.rs`, `runtara-core/src/lib.rs`,
  `runtara-server/src/core_runtime/mod.rs`, `runtara-dsl/src/agent_meta.rs`.
- **Tests:** replace interface-name literals with constants where the test is
  not deliberately testing a bad name (`@9.9.9`, `typesx`, `agent-evil` stay
  as literals, re-versioned). Regenerate `frozen_abi/*.wat` for `1.0.0`.

### Phase 4: delete

Done as part of Phase 3.

- Delete crates `runtara-workflow-wit` and `runtara-agent-wit`, and the `wit/`
  dirs of `runtara-agent-trusted` and `runtara-agent-suspension`.
- Delete the 27 `crates/agents/*/build.rs` WIT generators, the 27 committed
  `crates/agents/*/wit/agent.wit` files, and
  `runtara-agent-wit/templates/*.wit.in`.
- `.github/workflows/ci.yml` `wit-package` job: `wit-deps lock` in
  `crates/runtara-wit`, then `cargo test -p runtara-wit --features resolve`.
  Remove the scratch-dir step.
- `scripts/build-agent-components.sh`: deps path moves to `crates/runtara-wit/wit`.
- Docs: `AGENTS.md` (Architecture: "the WIT crates"; Generated Files: WIT deps
  path), crate READMEs, and `docs/crates-structure.md`.

### Phase 5: one export shape

This is the shape change: the emitter's hand-written layouts change here, and
nowhere else. Sub-steps 5a–5d land as one PR (guest and host must agree).
Sub-step 5e can follow separately.

Progress: `CliRunHttp` deleted (532ef6c1); `WorkflowAbi` became
`WorkflowRole{Root, PublishedAgent}` (bed692a1); 5a–5c landed as one commit.
Differences from the text below:
- The ABI version went from 2 to 3; Phase 3 had not bumped it.
- A single forwarded wake still uses the @88 scratch writers. Two or more are
  returned verbatim, without the step-deadline `at` (to add with 5d).
- WaitForInstances still parks with `at`/`on-resume` plus the
  `instance_waits` side channel; the runner also accepts `instances` wakes.
  A composed workflow-agent shares its parent's Store, so the side channel
  covers it.
- Former reserved codes (`__rt_on_signal__`, `__rt_suspended__`) are ordinary
  error codes now, passed through unchanged.

5d differences:
- Workflow-agents do not set `suspends`. That flag makes a caller wrap the
  call in an operation scope, which a workflow-agent must not get. The
  `workflow-agent` tag alone marks one, and staging still refuses `suspends`.
- A staged workflow-agent has no import denylist: it inherits whatever its
  composed agents leave to the host, including `runtara:agent/continuation`.
  Control additionally needs the control pin, which must name the bundled
  control version, like trusted pins.
- The precompile audit counts a component that contains workflow logic as
  workflow logic. It does not hash a composition with no core code of its own
  as a control importer. The control agent nested in it is hashed as before.
- `StepContext::PublishedWorkflowAgent` and `ContextVerdict::PublishRefused`
  are deleted. They had no verdict left.

**5a. WIT (`runtara-wit`)**
- Replace `runtara:agent/types` and `suspension` with the unified `types`
  above. `capabilities.invoke` returns `outcome`.
- Delete `runtara:workflow/lifecycle` and the per-agent `suspendable`
  interface; `agent_package` loses `AgentShape::Suspending`.
- `runtime.load-input`, `runtime.complete` and `runtime.fail` stay until
  Phase 6.
- `workflow/tasks` `task-outcome` and `workflow/operation.suspend` `use`
  `runtara:agent/types.{wake}`.

**5b. Emitter (`runtara-workflows/src/direct_wasm`)**
- **Delete `WorkflowAbi`** and every match on it:
  - `abi.rs:383`, `:592`, `:629`, `:977`;
  - `core_module.rs:965`, `:1034`;
  - `agent_error.rs:591`;
  - `wait.rs:555`;
  - `agent_suspend::check_sites` (`:302-314`, `:385-396`);
  - `is_invoke_export`.

  `CliRunHttp` goes with it. Migrate its differential tests in
  `tests/direct_wasm_execute.rs` and `tests/isolated_agent_execution` to the
  one shape.
- **One entry.**
  - Export detection (`core_imports.rs:1137-1164`, `core_module.rs:680-688`)
    accepts only `runtara:agent-*/capabilities`.
  - One four-parameter prologue (`core_module.rs:953-958`).
  - One completed writer: outcome tag @8, list @12/@16. This replaces both
    `emit_invoke_ok_completed_return` and `emit_capabilities_ok_return`.
- **One suspended writer that takes N wakes.** Today every writer puts exactly
  one wake at the fixed scratch offset @88 (`abi.rs:378-969`). Forwarding a
  child's wakes plus a deadline needs up to `MAX_WAKES + 1` elements of 32
  bytes, so allocate the list instead of using low scratch.
- **WaitForInstances emits `instances(wait-id)`** (`wait_instances.rs:382-393`)
  instead of `at`/`on-resume` plus the `InvokeRunResult.instance_waits` side
  channel.
- **Native agent call sites:**
  - Every agent returns `outcome`, and completed is read at @12/@16.
  - The `@12/@16 → @8/@12` shuffle in `agent_suspend::emit_after_invoke` goes.
  - A `suspended` from a non-suspending agent raises `AGENT_UNEXPECTED_SUSPEND`.
  - For a suspending agent, `emit_suspend` keeps enter/suspend/exit but
    forwards all wakes plus the step-deadline `at`. Wake stride becomes 32 and
    `instances` becomes discriminant 3.
- **Workflow-agent call sites:**
  - No operation scope: the child's state lives in its namespaced checkpoints
    (`_cache_key_prefix`, `agent_io.rs:100-130`).
  - On `suspended`, forward the child's wakes plus the step-deadline `at`.
    The parent's step checkpoint stays incomplete, so on wake the parent
    replays, re-invokes the child, and the child replays into its wait. That
    is exactly how the sentinel path works today.
  - **Delete the sentinel machinery:** `AGENT_SUSPEND_SENTINEL_CODE`,
    `AGENT_SUSPEND_ON_SIGNAL_SENTINEL_CODE`, `emit_agent_control_return`,
    `emit_agent_suspend_sentinel_check`, and the `…:user` remap in
    `direct_json.rs:5627-5638`.
- **Terminal status is unchanged here.** A top-level run still reports through
  `runtime.complete`/`fail`, and a published workflow-agent still suppresses
  them (`report_terminal_status`). The three-way `WorkflowAbi` becomes one
  "published agent" flag that sets the export package id and that
  suppression. Phase 6 removes the flag's second job.
- **Bump** `DIRECT_WORKFLOW_INVOKE_ABI_VERSION` again (to 4), so Phase 3
  artifacts built by a development server recompile.

**5c. Host**
- **One entry type.**
  - `workflow.rs` invokes every workflow as
    `(String, Vec<u8>) -> Result<Outcome, ErrorInfo>` with capability `run`.
  - `InvocationEntry::{Lifecycle, Capability}` collapses to one.
  - `invocation_abi::validate` checks one shape.
  - The host mirrors `lifecycle.rs` (`WorkflowWake`, `WorkflowOutcome`,
    `WorkflowErrorInfo`) and `operation_scope_host.rs` (`SuspensionWake`,
    `SuspendableOutcome`) merge into one set.
  - `TaskOutcome::Suspended` carries the unified wakes.
- **Runner** (`runtara-environment/src/runner/embedded.rs`):
  - Terminal persistence moves to the runner in Phase 6, not here.
  - `park_invoke_suspend` reads wait ids from `instances` wakes.
  - Keep the "pure `on-resume` is already acked" early return (`:1396-1399`);
    instance waits can no longer be lost behind it because they are wakes now.
- **Gates:**
  - `image_registry.rs:70-85`, `compilation.rs:1446-1449`
    (`require_lifecycle_invoke_file`) and `embedded.rs:928-935` require the
    capabilities export plus the compiler's ABI custom section.
  - The precompile audit (`precompile.rs:662-666`, `:740-745`) must classify
    workflow logic by that ABI section instead of by export shape. Otherwise a
    workflow looks like an agent, and its `runtara:workflow/*` imports get
    rejected.
- **Direct capability calls:** the dispatcher, `test_capability` and
  `registry.rs` bindgen map a `suspended` outside a workflow to
  `AGENT_UNEXPECTED_SUSPEND`.
- **`operation_scope_host::check_suspension`:** native agents still may not
  return `instances` (no operation registers one yet), nor `on-signal` or
  `on-resume`.

**5d. Durable published workflow-agents (`runtara-server`, `runtara-dsl`)**
- `publish_workflow_agent` (`compilation.rs:1223-1330`) compiles the same shape
  as a top-level run; only the export package id is the slug.
- `analyze_workflow_agent_safety` (`support.rs:107-417`) drops these refusals:
  `wait-for-instances`, `suspending-capability` and `control-agent`. Keep
  `missing-child-closure`, `ambiguous-child-closure` and `child-closure-cycle`;
  those are correctness checks, not ABI limits. Lift the AiAgent
  retry-backoff refusal once a park test covers it.
- Replace the `parks:1`, `non-suspending:1` and `checkpoint-scope:1` tags
  (`agent_meta.rs:61-92`, `:1981`, `:2167`) with the standard `suspends` flag
  on the `run` capability. Every workflow-agent is recompiled, so every one
  honours `_cache_key_prefix` and the marker has nothing left to prove.
- `workflow_agents.rs` `stage()` and `merge_catalog` stop rejecting
  `suspends: true` for workflow-agents. They still reject `trusted` and the
  reserved `control` id.
- `check_workflow_agent_checkpoint_scope` (`artifact_metadata.rs:605-676`)
  keys on the `workflow-agent` tag alone. `STAGED_WORKFLOW_AGENT_DENIED_PREFIXES`
  shrinks to `runtara:agent/continuation@`.
- **Control inside a workflow-agent (decided).** A staged workflow-agent that
  composes the control agent exposes `runtara:control/{api,executor}` as root
  imports.
  - Accept them when the publish-time dependency manifest pins the approved
    control artifact. This is the same approved-pin check a top-level workflow
    gets.
  - Refuse them otherwise, and refuse them from any non-staged agent.
  - Record the control pin in the staged sidecar at publish so the parent's
    compile can check it.
- **Children of a workflow-agent (decided).** A workflow-agent runs inside its
  parent's instance, so the run is the instance:
  - runs it starts through control are children of the parent instance;
  - its WaitForInstances targets are those children;
  - control's `not-child` checks apply against the parent instance.

  Add a test in which a workflow-agent starts runs and waits on them, and the
  parent cancels one of them afterwards.
- DSL rule matrix: remove `PublishRefused{suspending-capability | control-agent
  | wait-for-instances}` for `StepContext::PublishedWorkflowAgent`
  (`step_context_rules.rs:318-331`), and revisit the E028/E029/E131/E132
  placements.
- **Operation identity.** Only one operation may be entered per Store
  (`operation_scope_host.rs:191-212`). This works because a parent never
  enters a scope around a workflow-agent call: the child's suspending and
  control sites enter their own. Their `op_hash` comes from a checkpoint key
  whose namespace slot carries the child prefix, so it is unique per call
  path. Verify that `parse_agent_operation_key` accepts namespaced keys
  (`:124-179`), and that WaitForInstances wait ids are namespaced the same way.

**5e. Cleanup**
- Remove comments that still describe the old refusals or the sentinel path:
  - `compilation.rs:317-319` and `:1309-1310`;
  - `component.rs:84-91`;
  - `support.rs:57-65`;
  - `agent_meta.rs:76-91`;
  - `artifact_metadata.rs:590-596`;
  - `core_imports.rs:923-931`;
  - the header and `:24-35` of `nested_suspend_tests.rs`;
  - `docs/control-agent.md:617` and `:731`.
- Publication errors get dedicated codes instead of a generic
  `CompilationError` mapped to HTTP 500 (`workflows.rs:772-775`).

### Phase 6: the return value is the only terminal channel

A separate commit after Phase 5, in the same release. After Phase 5 every
workflow already returns `result<outcome, error-info>`. Phase 6 makes that
value the only way a run finishes, so the host stops receiving the result
twice.

#### Why

- **Two channels.** A top-level run calls `runtime.complete(output)` or
  `runtime.fail(error)`, and the host persists from that call. It also returns
  the same result, which the embedded runner only logs
  (`runner/embedded.rs:1694-1713`).
  - Scoped runs stage the call in `DeferredTerminal` and then cross-check it
    against the return value (`workflow/deferred_terminal.rs:45-60`).
  - A published workflow-agent must suppress both calls
    (`report_terminal_status`, `compile/core_imports.rs:929-935`), or it would
    finish its parent.
- **Errors lost today.** About 90 guest paths (`return_if_retptr_error`,
  `abi.rs:130-147`) return `Err(error-info)` without calling `fail`: a failing
  checkpoint write, `custom-event`, `register-input` or `stdlib.error`.
  - Nothing persists those runs. The monitor later finds them still `running`
    and records `failed` with `termination_reason = crashed` and the message
    "Process terminated without SDK event" (`handlers.rs:1426-1450`,
    `observed_exit.rs:56-90`).
  - The real error is only logged.

#### Contract changes (`runtara-wit`)

- **Delete from `runtara:workflow/runtime`:** `load-input` (unused once Phase
  5 removes `CliRunHttp`), `complete` and `fail`.
- **Append one field to `runtara:agent/types.error-info`:**

  ```wit
  /// The producer's full structured error as JSON, when it has more to say
  /// than the fields above (a workflow's failing step, agent and child-run
  /// chain). Hosts persist it verbatim as the run's error. Agents leave it none.
  details: option<string>,
  ```

  **Why a field:** `error-info` cannot carry what `runtime.fail` persists today.
  The fail bytes are the stdlib's error envelope, and `invoke-error-fields`
  (`direct_json.rs:5596-5657`) drops:
  - `stepId` and `stepName` (Error step, `direct_json.rs:3322-3340`);
  - `stepId`, `agentId` and `capabilityId` (agent errors, `3143-3162`);
  - `stepId`, `stepType`, `childWorkflowId` and the whole nested `childError`
    chain (embed failures, `6263-6293`). The message alone would only say
    "Child workflow X failed".

  With `details` set to the exact bytes `fail` receives today, `instances.error`
  stays byte-for-byte identical for every error that reaches it now.

  **Why it is safe to append:** appending leaves every existing offset
  unchanged.
  - `error-info` grows from 72 to 80 bytes. The error arm at @8 ends at @88,
    flush against the @88/@92 staging slots, which are only read after the
    record is written.
  - `details` sits at @76 (tag), @80/@84 (string).
  - The agent retptr area also stays under the agent-args scratch at 128.
- 1.0.0 is not released yet, so this edits it in place (see "Rename map").

#### Persistence rule

The runner persists once, from the returned value:

| Exit | Persisted |
|---|---|
| `Completed(output)` | `Completed` with `output` |
| `Failed(error-info)` | `Failed` with `details` verbatim when present. Otherwise the plain `message` when `code` is empty (today's plain-text fail payloads), else JSON built from the fields (`{code, message, category, severity, retryable, retryAfterMs?, attributes?}`) |
| `Suspended(wakes)` | Unchanged: `park_invoke_suspend` |
| `Trapped` / `Timeout` / `Cancelled` / `CleanupAborted` | Unchanged |

The rules around that write:
- **The same guards as today's `PersistenceRuntimeHost::event`**
  (`runtime_host.rs:341-376`), moved into the runner:
  - drop the write when the run's cancel flag or token is set;
  - write through `complete_instance(...).if_running()`;
  - then `record_unacknowledged_cancel_exit`.

  Insert the completed or failed event row through the same
  `handle_instance_event` path, so event rows, observers and the store triggers
  (`wake_instance_waiters`, `close_terminal_instance_inputs`) fire exactly as
  now.
- **Before the completion guard drops.** Persist inside the runner task,
  before `TaskCompletionGuard` drops (`embedded.rs:203-212`). Otherwise the
  monitor's `settle_with_retry` sees `running` and records `crashed` first.
  - Retry transient store errors the way `settle_with_retry` does, because the
    guest can no longer retry by calling again.
- **Exactly once.** One write per run, guarded by `if_running`. A replayed
  guest can no longer publish twice, so `DeferredTerminal`'s conflict
  detection has nothing left to detect.

#### Emitter

- **Delete the three calls and what surrounds them:**
  - the `complete`/`fail` calls and their indices (`core_imports.rs:25-27`,
    `198-208`, `726-729`, `1192-1197`);
  - `emit_complete` (`core_module.rs:1060-1070`) and its two call sites (entry
    epilogue and the terminal onError handler, `agent_error.rs:581-611`);
  - the `runtime.fail` call inside `emit_runtime_fail_return`
    (`compile.rs:1794-1831`) and `emit_fail_if_retptr_error_inplace`;
  - `report_terminal_status`, and with it the last job of Phase 5's "published
    agent" flag. The flag then only sets the export package id.
- **Fill `details`.** `stdlib.invoke-error-fields` returns `details` = the
  input bytes (lossy UTF-8, like `message`). The err-arm fallback
  (`abi.rs:229-282`) also points `details` at the staged raw bytes.
- **Parents.** A parent that receives a workflow-agent child's `error-info`
  keeps its `details` as the `childError` of its own agent-error envelope.
  Today it has only the flattened fields.
- **Bump** `DIRECT_WORKFLOW_INVOKE_ABI_VERSION` so no cached artifact that
  still imports `complete` is reused (the cache tag includes it,
  `compile.rs:1055-1066`).

#### Host and runner

- **Plain path** (`runner/embedded.rs`):
  - replace the logging match with the persistence rule above;
  - `PersistenceRuntimeHost` loses `load_input`, `complete`, `fail` and their
    cancel suppression, which now lives in the runner.
- **Scoped root** (`workflow/scoped_execution.rs`):
  - delete `DeferredTerminal` and the staged-versus-returned validation
    (`:201-216`);
  - `terminal.publish(exit)` (`:260-293`) persists from the exit through
    `ScopedRootRuntime::publishable()`, then the persistence rule.
  - Every other step keeps its order: teardown → `close(cleanup_ok)` →
    `finalize()` → publish only if cleanup succeeded, not cancelled and within
    budget → release the lease.
- **Scoped children** (`runtime_host/scoped.rs:290-361`,
  `scoped/invocation.rs:238-258`): delete `ChildTerminal` staging and
  `check_terminal`. The child's terminal already comes from the exit
  (`execution_host.rs:129-161`, `invocation_io.rs:156-200`).
- **`RuntimeHost` trait** (`component-host/src/runtime_host.rs:139-148`):
  drop the three methods and their linker bindings (`:325-363`).
  - The cleanup-alarm check in `require_host` stays for the remaining runtime
    calls.

#### Guarantees, before and after

| Pinned today (`terminal_publication_tests.rs`) | After Phase 6 |
|---|---|
| Expired cleanup alarm rejects a new runtime publication (`:167`) | Other runtime calls keep the check. Publication has no guest call left to reject: the run is `CleanupAborted` and nothing persists |
| Coordinator closes after cleanup; no publication on cleanup failure (`:300`) | Unchanged: publish still gated on `cleanup_ok` |
| Native stop precedes terminal validation (`:345`) | Unchanged ordering; "validation" is now reading the exit |
| Pending coordinator IO bounded by timeout, cancel, abandonment (`:385`) | Unchanged |
| Callback waits for descendant cleanup, discarded on failure (`:432`) | The exit is published after cleanup and discarded on failure |
| Discarded on root trap, conflict, late cancel (`:456`) | Trap and late cancel unchanged. "Conflict" cannot happen with one channel, so that case goes |
| Publication failure, timeout, abandonment supervised (`:490`) | Unchanged |
| Fail payload preserved; identical callbacks publish once (`:533`) | `details` preserves the payload. Once per run is by construction |
| Publication does not restart the timeout after cleanup (`:566`) | Unchanged |
| Output mismatch cannot publish a stale success (`:586`) | Cannot happen with one channel; the test goes |

Fail-after-cleanup ordering (`agent_deadline_tests.rs:187-205`,
`parallel_agent_deadline_tests.rs:346-360`,
`checkpoint_failure_tests.rs:324,350`) is still owned by the guest: it returns
only after sibling cleanup. Those tests observe the return instead of the
`fail` call.

#### Behaviour changes

- **Errors that were lost now surface.** The ~90 paths that returned an error
  without `fail` now end `failed` with their real message and no
  `termination_reason`, instead of `crashed` with "Process terminated without
  SDK event".
- **Unchanged:**
  - the persisted error bytes of every run that fails through `fail` today;
  - statuses, events and triggers;
  - the admission and launch release that follows persistence.
- **Timing.** A top-level run's terminal write now happens right after the
  guest returns rather than just before. It still precedes the completion
  guard, the monitor and the launch release.

#### Tests

- **The `direct_wasm_execute` harness** derives `output_json` and `error_json`
  from the returned exit instead of `CapturingRuntimeHost` callbacks. Its
  error-shape assertions (`stepId`, …) read `details`.
  - Tests that count `complete_calls` (`:2343`, `:9869`, `:10096`, `:10331`)
    assert on the returned outcome and on exactly one persisted terminal.
- **New tests:**
  - A run whose checkpoint write fails ends `failed` with the host's message,
    not `crashed`.
  - The persisted error of an Error step, an agent failure and an embed
    failure is byte-identical to the envelope the stdlib builds.
  - A cancelled run's return does not overwrite `cancelled`.
  - A transient store error on the terminal write is retried before the
    monitor can record `crashed`.
- **Rework the terminal-publication suite** per the table above.
- **Update the runtime function list** in `runtara-wit`'s structural test.
- **Gate the commit on:**
  - the component-host, `direct_wasm_execute` and scoped-runner suites;
  - the environment DB suite;
  - the SIGKILL and replay e2e stages (`test_control_agent`,
    `test_recovery_environment_restart`), `test_durable_delay_parks_and_wakes`
    and `test_workflow_agent_parity`, on an isolated server.

#### Risks

- **Ordering in the plain path.** The write must stay ahead of the completion
  guard; the new retry test pins it.
- **Error-arm layout.** `details` is the only change to a hand-laid-out
  record. Every existing offset stays put; a `wit_parser::SizeAlign` test pins
  the new `details` offset.
- **Consumers of `instances.error`.** The API, UI, MCP and launch rows read the
  persisted string. Because `details` carries today's bytes, they see no
  change. Only the formerly `crashed` paths gain a real error, whose shape
  follows the persistence rule.

## Rollout

This is a hard break, by choice:

- Server, agent bundle (`scripts/build-agent-components.sh`) and stdlib ship
  together, as they already do in the GitHub bundle and Docker image.
- Stored workflow artifacts recompile because the ABI version bumped.
  Published workflow-agents in `$DATA_DIR/workflow-agents/<tenant>` are stored
  artifacts with old tags and the old export shape. **They are rebuilt
  manually** by re-publishing after the upgrade. There is no boot-time
  migration, only a release-notes entry.
  - Until they are rebuilt, a parent that composes one should fail at compile:
    the old component exports `capabilities@0.4.0`, and the parent imports
    `@1.0.0`.
  - Confirm that the error names the workflow-agent and says to re-publish it.
- **Runs parked on pre-refactor artifacts will not wake.** They fail at link
  time. Before upgrading a deployment, drain or cancel parked runs. Make the
  wake path fail such an instance with a clear
  `ARTIFACT_ABI_UNSUPPORTED`-style reason instead of retrying a link error.
- The control agent's bytes change, so its approved-artifact pin changes.
  Confirm boot registers the new approved row (old rows are only revoked).

## Verification

- `cargo test -p runtara-wit --features resolve`
- `cargo test -p runtara-workflows` (emitter `--lib` tests), then
  `scripts/build-agent-components.sh` and the `runtara-component-host` and
  `runtara-workflows` integration tests from CI with their features.
- `cargo clippy --workspace --all-targets --features $GATE_FEATURES -- -D warnings`
  (the CI gate; `-p` scoping hides breakage across this many crates).
- End-to-end on an isolated live server, one workflow per host package:
  HTTP agent (`host/http`), object-model (`host/sql` and `host/connections`),
  a retry with backoff (`host/timers`), `WaitForSignal`, `WaitForInstances`,
  an embedded child, a suspending agent and a control-agent call.
- End-to-end for durable workflow-agents. A parent calls a published
  workflow-agent that:
  - Delays;
  - waits for a signal, answered through the signal API;
  - starts runs through the control agent and waits for them with
    WaitForInstances;
  - calls a native suspending agent;
  - calls another published workflow-agent that parks (two levels deep).

  For each case, assert:
  - the parent parks runner-free with the right `termination_reason`;
  - it wakes on the right event and completes exactly once;
  - no step fires twice on replay.

  Also pause, resume and cancel the parent while the child is parked.
- `rg 'runtara:(abi|agent-suspension|outbound-http|database|connection-resolver|host-io|workflow-(lifecycle|runtime|execution|operation|wait))'`
  and `rg '__rt_suspended__|__rt_on_signal__|parks:1|non-suspending:1|WorkflowAbi|RuntimeBinding|runtara_sdk|runtara_workflow_runtime'`
  return nothing outside this document.

## Out of scope

- Agents still get `wasi:http` linked (`registry.rs`,
  `add_only_http_to_linker_async`) even though outbound traffic goes through
  `runtara:host/http`. Removing it is a separate hardening change.
- Replacing per-agent packages with plain-name imports of one shared
  `capabilities` interface. That might remove `runtara:agent-<id>` entirely,
  including the slug in a published workflow's export, but it changes the
  composition model. wac-graph can already wire an export to a differently
  named import, which is the likely route.

## Decisions

- **Control calls inside a published workflow-agent:** allowed when the
  publish-time manifest pins the approved control artifact (Phase 5d).
- **Runs started by a workflow-agent:** these belong to the parent instance
  (Phase 5d).
- **Existing published workflow-agents:** rebuilt manually by re-publishing
  (Rollout).
- **`runtara-workflow-runtime`:** test-only, so it is deleted together with
  `runtara-sdk` and `runtara-sdk-macros` (Phase 1b).
- **`runtara:trusted`:** stays separate from `runtara:host`, and `executor`
  moves behind a `trusted` grant ("Allowlists after").
