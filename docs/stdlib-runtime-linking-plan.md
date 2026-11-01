# Runtime-linked stdlib: implementation plan

Design and measurements: `docs/stdlib-runtime-linking.md`. Paths are relative
to `crates/`; `DW` = `runtara-workflows/src/direct_wasm`, `C` = `DW/compile`,
`CH` = `runtara-component-host/src`. Line numbers are as of `359e13d6e`.

Goal: compile the workflow stdlib once per process instead of once per
workflow, keep durability and behaviour identical, and cut the CPU of the
`direct_wasm_execute` CI step and of every deploy's compile.

| Phase | What | Ships on its own | Size |
|---|---|---|---|
| 0 | Test-only per-function compile cache | yes, CI only | S |
| 1 | Stdlib contexts (still fused) | yes, no behaviour change | M |
| 2 | Linked mode behind a switch (compiler + host) | yes, off by default | L |
| 3 | Production wiring, version pinning, rollout | flips the default | L |
| 4 | Scope owned by the context (stage B) | optional, separate | L |

Phase 0 is independent of the rest. Phases 1 → 3 are sequential.

## Phase 0: per-function compile cache in test engines

Measured −56% CPU on the suite locally. Changes tests only.

1. **Feature.** Add a `test-incremental-cache` feature to
   `runtara-component-host` that enables wasmtime's `incremental-cache`.
   - Enable it from the dev-dependency on `runtara-component-host` in
     `runtara-workflows/Cargo.toml:92`.
   - A crate can't enable its own feature from its dev-dependencies, so
     `component-integration-tests` implies `test-incremental-cache` for
     component-host's own suites.
   - Resolver 2 keeps it out of production builds.
2. **Engine.** Add `EngineConfig::incremental_cache: bool` (default false,
   `CH/engine.rs:14-33`). When set, `build_engine` calls
   `Config::enable_incremental_compilation` with a process-wide in-memory
   `CacheStore`: `Mutex<HashMap<Vec<u8>, Vec<u8>>>` behind a `OnceLock`.
3. **Literals.** Update the 14 `EngineConfig { .. }` literals that list every
   field to `..Default::default()`: `CH/precompile.rs:970,984`,
   `CH/removed_abi.rs:46`, `CH/workflow/raw_http_tests.rs:73`,
   `CH/workflow/cleanup_alarm_tests.rs:36,215,244`, and
   `runtara-component-host/tests/{cooperative_cancellation.rs:40,
   object_model_database.rs:143, cooperative_cancellation/{real_agent.rs:253,326,
   async_cancel_grace.rs:88, deadline.rs:74, soak.rs:155}}`.
4. **Enable in the shared harness engines.**
   - `runtara-workflows/tests/direct_wasm_execute.rs:6306` `embedded_executor()`
     (the cooperative, audit and wait suites derive from it).
   - `C/agent_deadline_tests.rs:427` `executor()`.
   - The `cache_dir: None` harness in `isolated_agent_execution`. Leave
     `cooperative_measurement` and `wasm_performance_baseline` cold: they are
     measurements, and a warm cache would skew them.
5. **Precompile fingerprint.** Check that `precompile_compatibility_hash`
   (`CH/precompile.rs:715`) ignores the incremental cache. If it doesn't,
   in-process precompile tests that pair engines need the same flag on both.
6. **Verify.**
   - `cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute`:
     all pass; record wall and CPU before and after.
   - The `test-composed-deadlines` CI job.
   - `clippy --workspace --features $GATE_FEATURES`.

Exit: CI `components-build` and `test-composed-deadlines` wall time recorded
before and after.

## Phase 1: stdlib contexts (fused, behaviour-neutral)

**As built:**
- **Emitter:** unchanged at its ~189 call sites. A post-pass
  (`C/stdlib_context.rs`, run at the end of `emit_direct_core_module`) adds
  the `ctx` global and one shim per stdlib import, and routes calls through
  the shims.
- **`init-manifest`:** takes the caller's previous context (`u32::MAX` for
  none), resets and reuses it, and returns it. Otherwise a published agent
  would open a new context on every invocation.
- **The two by-pointer functions:**
  - `agent-retry-error-info` is pure and takes no context;
  - the `agent-error` shim rewrites its argument area in place (`ctx @0`,
    `agent-id @4`, record unchanged at 8).
- **Stdlib:** keeps its thread-locals as the current context.
  `src/context.rs` swaps a context in for each call and back out, so
  `direct_json.rs` is untouched.

One stdlib instance will later serve the root logic and every published
workflow-agent logic in the run (`DW/compile_request.rs:430-475`; parallel
pools instantiate an agent K times, `DW/component.rs:542-547`). Each needs its
own manifest, arena and nonce. EmbedWorkflow children are inlined into the
parent core and share its context, so they need nothing.

0. **Key baseline first, in its own commit.** Before any change below, add a
   golden test over the key functions (`agent-cache-key`, `split-cache-key`,
   `while-cache-key`, `loop-deadline-key`, `state-key`,
   `embed-workflow-cache-key`, `ai-turn-*`, `retry-sleep-key`,
   `agent-attempt-result-key`). It pins today's output for fixed inputs.
   Checkpoint rows and scope collapse (`docs/checkpoint-scope-collapse.md`)
   depend on byte-identical keys, and every later phase must keep this test
   green unchanged.
1. **WIT** (`runtara-wit/wit/workflow-stdlib/runtara-workflow-stdlib.wit`).
   - `init-manifest: func(manifest: list<u8>) -> result<u32, string>`
     returns the context id.
   - Every other function takes `ctx: u32` first.
   - Bump the package version (`runtara-wit/src/lib.rs:35-42` shares one `v!()`
     across packages, so give the stdlib its own version constant).
   - Update `runtara-wit/src/tests.rs:27-61,109-208`.
2. **Stdlib crate** (`runtara-workflow-stdlib`).
   - Replace the three thread-locals (`src/lib.rs:89-91` `MANIFEST`;
     `src/direct_json.rs:72-85` `VALUE_STORE`, `WFREF_NONCE`) with one
     `thread_local! CONTEXTS: RefCell<Vec<Context>>`, where
     `Context { manifest, value_store, nonce }`.
   - `init_manifest` pushes a context instead of resetting.
   - Thread `ctx` through all 138 `Guest` methods (`src/lib.rs:105-1578`) and
     the `direct_json` helpers that read the arena or nonce.
   - The nonce must be per context. `stored_value` (`direct_json.rs:320-328`)
     panics on a same-nonce dangling handle.
   - Unknown `ctx` returns an error, never a panic.
3. **Emitter.**
   - Add global 1 `ctx: i32` next to the heap global (`C/core_module.rs:602-610`),
     set from `init-manifest`'s result at `C/core_module.rs:976-979`.
     `C/callback_lift.rs` copies existing globals through and derives its
     appended indices from the module's global count (`:97`), so it needs no
     change. Asyncify doesn't save globals, which is fine for a constant.
   - Push `global.get ctx` first at each stdlib call: 147 direct
     `Instruction::Call(indices.stdlib_*)` sites plus ~42 through wrappers
     (`C/abi.rs` `emit_call_wide_result`, `emit_invoke_err_return_from_retptr`
     `:139-185`, `emit_fail_if_retptr_error*` `:748-761`, `state.rs` `call()`,
     dispatcher arguments). Fold the push into those wrappers where possible.
     Heaviest files: `ai_agent_loop.rs` 33, `split.rs` 23, `wait.rs` 13,
     `while_loop.rs` 12, `split_parallel.rs` 12, `debug.rs` 11.
   - Helper and shim bodies read the global, so `C/cooperative_wait.rs`
     `HELPER_PARAMS` and `C/api_shims.rs` signatures don't change. Update the
     flat-parameter assertion at `C/api_shims.rs:897-907`.
   - Bump `direct_lowering_tag` (`DW/compile.rs:1144-1159`) so cached images
     recompile.
4. **Tests.**
   - Update the ~180 import-name assertions in `C/tests.rs` (names unchanged,
     signatures change).
   - Add a stdlib unit test: two contexts with different manifests interleave
     calls without interference; a handle from context A used in context B is
     plain data.
   - Run every component suite in CI `components-build`
     (`.github/workflows/ci.yml:557-703`) and `test-composed-deadlines`.
Exit: all suites green, and the step-0 key test passes unchanged.

## Phase 2: linked mode behind a switch

1. **Logic-side WIT.** A component type can't be both sync and async
   (wasmparser bails on `async_` mismatch), so the logic imports a second
   package, e.g. `runtara:workflow-stdlib-link/json`.
   - Derive it in `runtara-wit` from the stdlib WIT by text transform
     (`: func(` → `: async func(`, renamed package) rather than keeping a
     second copy.
   - A runtara-wit test checks the two stay function-for-function identical.
   - The emitted core ABI is unchanged: the import is sync-lowered, as for the
     49 `async func` in `runtara:api` today.
2. **Compiler switch.** A `DirectStdlibMode::{Fused, Linked}` compile option.
   - World generators: `DW/compile.rs:1803-1809` and `DW/component.rs:424`.
     The anchor helpers that key off the stdlib import line
     (`component.rs:213-251`) must accept either form.
   - Core imports: `is_stdlib_import` (`C/core_imports.rs:1141-1151`) matches
     both package names.
   - Composition: `emit_wac` (`component.rs:516,551`) omits the stdlib instance
     and spread. `http_guest_wac` (`:128-136`) inserts the API component without
     a stdlib anchor. `compose_direct_workflow_selected` (`DW/compile.rs:697-721`)
     skips the stdlib requirement but still records its digest in
     `artifact_metadata.shared_components` (`:926`).
   - Mixed modes compose and run. A fused published agent inside a linked
     parent keeps its own stdlib copy (no saving for that agent). A linked
     agent's import bubbles up through the parent's trailing `...` and merges
     with the parent's (same name), which contexts make safe. Published agents
     are staged once (`DW/compile_request.rs:430-475`) and can move to linked
     on their own schedule.
   - Add the mode to `direct_lowering_tag`.
3. **Host forwarders** (`CH/workflow.rs`).
   - **Linker.** Define the forwarders in `WorkflowExecutor::new`
     (`CH/workflow.rs:600-622`), not in `linker_with_trusted_pins`. Then every
     link site sees them: root (`prepare_component` `:823-844`), isolated
     children (`prepare_precompiled_package` `:877,906`), and tests that call
     `executor.linker.instantiate_pre` directly.
   - **Typed, never dynamic.** Use one `func_wrap_concurrent` per function,
     calling the stdlib export through `TypedFunc::call_concurrent`. Generate
     them with `wasmtime::component::bindgen!` over the stdlib world, or a
     macro over the signature-shape table; the spike's 20-shape table covered
     86 of 138. A `Val` forwarder is 5–30x slower on large payloads.
   - **Stdlib component.** A `OnceLock<Component>` set through
     `set_workflow_stdlib`, following `set_trusted_executor`
     (`CH/workflow.rs:518-525`), plus an `InstancePre` built from the base
     linker. Don't route it through `prepare_component`: its entry check would
     refuse the stdlib.
   - **Per-Store instance.** Add `stdlib: Option<StdlibInstance>` to
     `WorkflowState` (`:317-344`, built at `:1163-1195`). In `execute_entry`,
     instantiate it just before `pre.instantiate_async` (`:1280`), inside the
     watchdog-raced future, only when the component imports the link package.
     Fused artifacts don't import it and are untouched.
   - **Memory.** The stdlib's memory already counts against the Store limiter
     (`:253-288`) when fused, so limits don't change. The compile-time
     `memory_floor_bytes` (`DW/compile.rs:860,910`) must add the stdlib's floor
     from its sidecar.
4. **Test builds.** Generic lift/lower code is compiled in
   `runtara-component-host`; at opt-level 0 the typed path copies byte by byte
   and loses to fused. Two options:
   - **CI only (recommended):** pass
     `--config 'profile.dev.package.runtara-component-host.opt-level=3'` on the
     component test commands in `.github/workflows/ci.yml`. No repo change, and
     it affects only those suites (the spike measured with exactly this).
   - **Repo-wide:** `[profile.dev.package.runtara-component-host] opt-level = 3`
     next to `Cargo.toml:119-120`. This changes every dev and test build of the
     crate, including local `cargo run` of the server.
5. **Test harness.**
   - Run `direct_wasm_execute` in both modes: a `RUNTARA_TEST_STDLIB_MODE`
     variable read by the harness, and a second CI step or matrix entry.
   - Harnesses that build `WorkflowExecutor::new` directly get the stdlib
     `Component` from a shared helper keyed by the staged components dir.
   - Update the import-shape tests for linked mode:
     `direct_compose_leaves_only_wasi_for_the_host`
     (`tests/direct_wasm_execute.rs:1831-1865`), the import checks at `~:13982`
     and `~:14122`, and `component.rs` `:582,643,710,776,837`.
   - The ~180 core-import assertions in `C/tests.rs` match the module name
     `cm32p2|runtara:workflow-stdlib/json@1`, which becomes the link package's
     name in linked mode. Parametrize the expected name by mode.
6. **Measurement gate**, on CI hardware (x86, 4 vCPU), fused vs linked:
   - suite wall and CPU (local result: 273 s / 3,362 CPU-s →
     95 s / 1,102 CPU-s);
   - execution time of `split_large_scope_*`, `while_growing_accumulator_*` and
     a parallel-split test;
   - explain the higher summed execution under load seen locally (37 s vs 22 s)
     before Phase 3.

Exit: all suites green in both modes; gate numbers recorded in
`docs/stdlib-runtime-linking.md`.

## Phase 3: production wiring and rollout

1. **Pinning the stdlib version.** Each artifact must run against the stdlib
   it was built with. Artifacts are pinned by digest today, and a release
   change (`image_cache_hits`, `runtara-server/src/api/services/compilation.rs:180-193`)
   doesn't touch parked runs of older images.
   - Write the stdlib digest into the composed artifact (a custom section),
     and keep recording it in `sharedComponents`.
   - The precompile worker reports it in its response: bump the frame version,
     `CH/precompile.rs:58-63,160-216`.
   - The runner keeps `digest → compiled stdlib`.
2. **Getting the stdlib bytes.** The runner has no stdlib identity today
   (`runtara-environment/src/runner/mod.rs:40-70`, `traits.rs:430-460`).
   - Pass the release's `ArtifactStore` and stdlib digest through `RunnerParts`
     (`runtara-server/src/embedded_runtara.rs:136-157`).
   - Fetch other digests by content address (`ArtifactStore::local_path`).
   - GC (`runtara-artifact-store/src/gc.rs`) must retain any stdlib digest that
     a live image references.
3. **Compiling it.** Add a path-plus-deadline variant of
   `ChildComponentPrecompiler::exchange`
   (`runtara-environment/src/runner/embedded.rs:349-455`).
   - Precompile the current stdlib at runner build (`EmbeddedWasmRunner::new`,
     `:668-727`) and older digests on first use.
   - Load the result with `deserialize_trusted_precompiled_component`
     (`CH/precompile.rs:611-631`). Cranelift stays out of the parent, and the
     worker's wasmtime disk cache makes later boots cheap.
4. **Caching.** Keep the stdlib out of the prepared-component LRU
   (`CH/workflow.rs:385-397,1678-1691`), which evicts. The forwarders are in
   the base linker before any `InstancePre` is cached, so cached entries stay
   valid.
5. **Rollout.**
   - Flip `DirectStdlibMode` to `Linked`.
   - The lowering-tag bump recompiles images on next use.
   - Fused artifacts (parked runs, old images) keep running unchanged; the host
     supports both indefinitely at no cost.
   - Check whether the tag bump also recompiles staged published agents
     (`runtara_agent_<slug>.wasm`), or whether they stay fused until
     republished.
   - Remove the fused compile path once no supported release produces it.
6. **Release and e2e.**
   - The stdlib still ships in the bundle; staging scripts are unchanged
     (`scripts/stage-agent-components.py:28-33`, `scripts/build-bundle.sh:157-217`).
   - Run the e2e scripts that start a server: `e2e/test_child_run_*`,
     `test_durable_delay_parks_and_wakes.sh`, `test_durability_lifecycle.sh`,
     `test_lost_executor_fenced.sh`, `test_image_artifact_store.sh`.
   - Add one e2e that parks a run, upgrades the release, and resumes it.

Exit: compile latency per workflow and CI time recorded; a parked run survives
a release change.

## Phase 4 (optional): the context owns the scope

Stage B in the design note. The context holds the scope as parsed values and
only ids cross the boundary. Checkpoints keep storing plain JSON values: export
before save, import on a replay hit, never a handle or context id. Gate it on a
profile showing what share of a typical run goes to JSON parse and serialize.

## Risks

- **wasmtime API.** `func_wrap_concurrent` behind sync-lowered imports is
  newer API. The spike's published-agent failures were context errors, so
  calls from callback-lifted agents did execute through it. Pin the behaviour
  with the Phase 2 tests and re-run them on every wasmtime bump.
- **Concurrency.** Parallel pools call one stdlib instance concurrently. Sync
  stdlib exports serialize, and the stdlib never calls out, so it can't
  re-enter. Cover this with a parallel-pool test in linked mode.
- **Key drift.** The golden key test from Phase 1 guards replay and scope
  collapse.
- **Debug-build skew.** Without the Phase 2 opt-level override, linked tests
  are slower than fused and mislead.

## Not in this plan

Sharding `direct_wasm_execute` and larger CI runners. Both are independent and
combine with any phase.
