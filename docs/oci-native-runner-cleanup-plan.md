# OCI / native runner cleanup plan

> **Status: completed 2026-08-25** (commits `a420a6e4`..`f7e4a343`, 54 files,
> +782 / −1656). Phases 0–4 all landed, including the schema drops that this
> plan originally proposed deferring — the drops were verified non-lossy
> against the live dev database first (`containers` held 0 rows,
> `images.bundle_path` 0 non-NULL, `images.runner_type` only `'wasm'`,
> `container_registry.pid` / `process_killed` entirely default), and both the
> upgrade and fresh-install migration paths were exercised end to end.
>
> Phase 5 (the container → instance rename) was **not** done and remains a
> separate effort. `ActiveEngine` / `TestEngine` were deliberately left alone:
> they are residue of the rustc-dispatcher migration (`ce21a173`), not the
> runner removal, and they are live OpenAPI surface — verified at runtime that
> `?engine=legacy` still parses and routes.
>
> Work found during execution that was not in the original plan: the whole PID
> subsystem (`kill_and_confirm_pid`, `is_process_alive`, `get_unkilled_containers`,
> `kill_surviving_processes`, the `nix` dependency), `RunnerHandle.child`, the
> dead `cpu_user_usec` / `cpu_system_usec` metrics, `ExecutionError::BundlePreparationFailed`,
> `spawn_container_monitor`'s unused parameters, CI's wasmtime CLI download,
> `start.sh`'s `.data/bundles` directory, and `SECURITY.md`'s advice to restrict
> container capabilities.

Commit `4ba33fcb` ("Phase 3 step 11: delete OCI and Native runners") removed the
runners themselves. `EmbeddedWasmRunner` (in-process wasmtime) and `MockRunner`
are the only implementations of the `runner::Runner` trait left, and
`build_runner` (renamed from `runner_from_env` during this cleanup) resolves
every input to the embedded one.

What survived the removal is the scaffolding: single-variant enums, coercion
helpers, DB columns nothing writes, an endpoint that only returns an error, and
docs that still tell readers OCI is the production default. This plan removes
that, in five independently landable phases ordered lowest-risk first.

Phases 0–2 are pure subtraction with no wire or schema impact. Phase 3 touches
an HTTP field. Phase 4 touches the schema. Phase 5 is a rename and should be its
own effort.

---

## Phase 0 — Docs (no code change)

The highest-value phase: several of these actively mislead a new contributor
into installing `crun` and believing containers are the default execution model.

| File | Line | Fix |
|---|---|---|
| `crates/runtara-environment/README.md` | 7, 24, 26 | Drop "pluggable (OCI, native, or WASM)" and "OCI (runc) is the production default". Replace with: the embedded in-process wasmtime runner is the only backend; `MockRunner` exists for tests. Line 26 "requires OCI tooling on the host" is false — delete. |
| `crates/runtara-environment/src/lib.rs` | 80 | Delete the whole "Runner Types" table (`OCI (default)` / `Native` / `Wasm (planned)`). Replace with one sentence naming `EmbeddedWasmRunner`. |
| `crates/runtara-environment/src/lib.rs` | 31 | ASCII diagram box "(OCI containers)" → "(in-process wasmtime)". |
| `crates/runtara-environment/src/lib.rs` | 181 | Module doc "Container/process execution backends (OCI, Native, Wasm)" → "In-process WASM execution backend". |
| `README.md` | 92 | `runners (Wasm/OCI/Native/Mock)` → `embedded WASM runner`. |
| `README.md` | 133 | Delete the `crun` prerequisite line entirely — there is no way to enable the OCI runner. |
| `CONTRIBUTING.md` | 66 | Delete "For the OCI runner path, you'll also need Linux with `crun` installed." |
| `crates/runtara-server/src/api/handlers/workflows_sync.rs` | 6 | "Uses native binary execution via crun launcher" → describe the embedded runner. |
| `crates/runtara-environment/src/heartbeat_monitor.rs` | 301 | "(uses crun kill + crun delete)" → it sets an `AtomicBool` cancel flag. |
| `crates/runtara-environment/src/cleanup_worker.rs` | 7 | Drop the `config.json - Per-instance OCI configuration` bullet; only `stderr.log` is written now. |
| `crates/runtara-environment/src/runner/traits.rs` | 104–106, 96, 160, 166 | Comments referencing pasta/crun/"container_id for OCI, PID for native". |
| `crates/runtara-server/src/embedded_runtara.rs` | 35, 99, 278 | `pasta --config-net` networking rationale — moot for an in-process runner. |
| `crates/runtara-server/src/server.rs` | 2185 | "workflow containers use pasta networking". |
| `crates/runtara-server/src/workers/compilation_worker.rs` | 52 | Same pasta rationale. |
| `crates/runtara-component-host/README.md` | 10 | Historical framing ("replaces the legacy ... OCI image" ) — accurate as history; **keep**, it explains why the crate exists. |

**Verify:** `grep -rniI 'crun\|runc\b\|pasta\|OCI runner\|native runner' --include='*.rs' --include='*.md' crates/ README.md CONTRIBUTING.md` returns only the intentional history note.

---

## Phase 1 — Dead code, no external surface

Nothing here is reachable; removal cannot change behavior.

1. **`RunnerError::BundleNotFound` / `BundleCreation`**
   (`crates/runtara-environment/src/runner/traits.rs:25,29`) — constructed
   nowhere in the tree. Delete both variants. The enum is `#[non_exhaustive]`
   and crate-internal, so this is safe.

2. **The PID plumbing.** `RunnerHandle.spawned_pid` is hardcoded `None` by both
   surviving runners (`embedded.rs:724`, `mock.rs:167`), so
   `container_registry.pid` is always NULL — and `EmbeddedWasmRunner::stop`
   (`embedded.rs:754`) ignores the field entirely, looking up by `instance_id`.
   Remove the field and the three sites that reconstruct handles from
   `container.pid`: `handlers.rs:887`, `heartbeat_monitor.rs:307`,
   `runtime.rs:638`. Keep `ContainerInfo.pid` for now (Phase 4 drops the column).

3. **`runner_from_env` / `RUNTARA_RUNNER`** (`runner/mod.rs:22-38`) — the match
   exists only to log a warning. Collapse to a direct `EmbeddedWasmRunner::new`.
   Decide explicitly whether to keep the warning for operators with the var
   still set in a config file; **recommendation: keep the warn, drop the match**
   (one `if std::env::var(..).is_ok()` guard).

4. **The `chmod 0o755` on registered images** (`handlers.rs:329`,
   `http_server.rs:811`) — a `.wasm` file is never exec'd. Native-runner
   residue. Delete both blocks and the `#[cfg(unix)]` wrappers.

5. **The `let _ = bundle_path; let bundle_path_str: Option<String> = None;`
   dance** (`handlers.rs:337-339`, `http_server.rs:821-824`) and the dead
   `images_dir.join("bundle")` at `handlers.rs:303`. Remove the binding and the
   `if let Some(bp) = &bundle_path_str` branch at `handlers.rs:353`.

6. **`ActiveEngine`** (`crates/runtara-server/src/api/services/agent_testing.rs:103`)
   — single-variant enum with `pick_engine` guard and a one-arm match at 209/219.
   Collapse, or keep if a second engine is genuinely planned. **Recommendation:
   collapse**; reintroducing an enum is cheap.

**Verify:** `cargo clippy --workspace --features $GATE_FEATURES` plus
`cargo test -p runtara-environment -p runtara-server`. Note the tests in
`crates/runtara-environment/tests/` construct `RunnerHandle` literals in six
places (`handlers_test.rs:1311,1427,1518,1648`, `embedded_runner_test.rs:171`,
`container_registry_test.rs`) — they need the field dropped too.

---

## Phase 2 — The tombstone endpoint and its client chain

`handle_test_capability` (`crates/runtara-environment/src/handlers.rs:1515`) is
a handler whose entire body returns an error explaining that OCI was removed. It
is still routed at `http_server.rs:2173` (`POST /api/v1/agents/test`).

Tracing the callers:

- `ManagementClient::test_capability` (`crates/runtara-management-sdk/src/client.rs:1512`)
  POSTs to that route. Its **only** caller is `runtara-ctl.rs:798` — a CLI
  subcommand that therefore always fails.
- `agent_testing.rs:271`'s `.test_capability(` is a **different** method — the
  `ComponentDispatcherService`'s. That path is live and unaffected.

Remove, in order: the `runtara-ctl` subcommand → `ManagementClient::test_capability`
→ the route → the handler → `TestCapabilityRequest`/`TestCapabilityResponse` in
`runtara-environment` → the four tests at `handlers_test.rs:1068-1145`.

**Decision needed:** whether `runtara-ctl` should instead point at the working
server endpoint (`POST /api/runtime/agents/{name}/capabilities/{cap}/test`).
That is a feature, not cleanup — **recommendation: delete the subcommand now**,
add the redirect separately if anyone misses it.

**Verify:** `cargo test -p runtara-environment -p runtara-management-sdk`; confirm
the component-dispatcher test path still passes via
`cargo test -p runtara-component-host`.

---

## Phase 3 — Collapse `RunnerType` (touches the wire)

`RunnerType` is defined **twice** as a single-variant enum:
`crates/runtara-environment/src/image_registry.rs:23` and
`crates/runtara-management-sdk/src/types.rs:532`. Both carry one-arm
`Display`/`FromStr`/`From<RunnerType> for i32` impls. It is threaded through
`RegisterImageOptions.runner_type`, `.with_runner_type()` builders, and
`ImageBuilder::runner_type`.

`runner_type_from_string` (`client.rs:331`) already discards its argument and
returns `Wasm`; `http_server.rs:815` coerces whatever a caller sends.

Remove the enum, the builder methods, and the struct fields. Keep accepting
(and ignoring) a `runnerType` key in the registration request body so older
clients don't 400 — a `#[serde(default)] _runner_type: Option<String>` on the
request struct, dropped on the floor.

**Do not remove** the `"oci" | "native" => Wasm` coercion behavior
(`image_registry.rs:46`) until Phase 4 drops the column — existing rows still
carry `'oci'` (it is the column's `DEFAULT`).

Note that `runtara-management-sdk` is not published to crates.io — releases are
GitHub bundles and Docker images — so this is an internal refactor, not a
breaking public API change.

**Verify:** register an image through the HTTP API with and without a
`runnerType` field; both must succeed. `cargo test -p runtara-management-sdk`.

---

## Phase 4 — Schema

Four columns and one whole table:

1. **`containers` table** (`crates/runtara-core/migrations/postgresql/001_initial_schema.sql:142-154`)
   — created, indexed twice, and **never read or written by any code**. The only
   reference in the tree is `DELETE FROM containers` in test cleanup
   (`crates/runtara-core/tests/common/mod.rs:316`). There is no SQLite
   counterpart. Its `bundle_path TEXT NOT NULL` is pure OCI residue. **Drop the
   table.** Cleanest win in this phase.
2. `images.bundle_path` — always written as `NULL` since Phase 3 step 11.
3. `images.runner_type TEXT NOT NULL DEFAULT 'oci'` — note the default is still
   `'oci'`. Drop after Phase 3 lands.
4. `container_registry.bundle_path` — always NULL.
5. `container_registry.pid` — always NULL after Phase 1.

All in `crates/runtara-environment/migrations/20250102000000_environment_schema.sql`
(19, 20, 52) plus the core initial schema. Add **one new forward migration**;
never edit the existing files — `sqlx::migrate!` checksums them and the comment
at the top of the environment schema explicitly warns about collisions.

**Rollback hazard:** a rolled-back binary would `SELECT` dropped columns and
fail at startup. Deployment is one process per tenant VM, so this is a per-tenant
concern rather than a fleet-wide one, but it is real. **Recommendation:** land
the `containers` table drop (item 1, zero risk — nothing reads it) and defer
items 2–5 by one release, so a rollback target always predates the drop.

**Verify:** `reset-local-env` then a full `e2e-verify` run — migrations are
auto-applied at server startup, so a broken migration surfaces as a boot
failure. Also run against a database that already has data (an existing local
`.data` env) to confirm the `DROP COLUMN` path, not just fresh-create.

---

## Phase 5 — Rename container → instance (separate effort)

Still load-bearing but misnamed, since nothing is a container: the
`container_registry` module and table, `ContainerInfo`, `ContainerStatus`,
`ContainerMetrics`, `container_cancellations`, `container_status`,
`container_id` (→ `handle_id`), `spawn_container_monitor`, and the
`ContainerRegistry` type.

This is a large mechanical diff that touches DB object names and therefore needs
its own migration and its own review. **Recommendation: do not bundle it with
Phases 0–4.** It has no correctness benefit — only clarity — and mixing it in
would obscure the real removals.

---

## Also in scope: `scripts/measure_memory.py`

Not caught by a Rust-only sweep, and it will silently misbehave:

- `--runner-type` and `--runtime-runner` both offer `choices=["auto", "oci", "native", "wasm"]` (lines 1343, 1349).
- `resolve_runner_type` and `runtime_runner_for_args` (lines 1094-1110) **return `"oci"`** on the non-wasm branch.
- `RUNTARA_RUNNER` is passed into the runtime env at line 1017 — now only produces a warning.

Reduce both flags to `wasm` (or drop them), delete the `"oci"` fallbacks, and
remove the `RUNTARA_RUNNER` passthrough.

---

## Explicitly out of scope — do not touch

- **`.github/workflows/release.yml:408`** — builds the GHCR Docker image. That
  is a genuine OCI *image*, unrelated to the removed OCI *runner*.
- **`CHANGELOG.md`** — historical record.
- **`crates/runtara-component-host/README.md:10`** — describes the pre-migration
  model as history and explains why the crate exists.
- **`RunnerType::Wasm = 2`** wire code — preserves the historical proto
  numbering; if any part of `RunnerType` survives Phase 3, keep the number.
- **The `Runner` trait itself** — `MockRunner` depends on it and the tests need
  the seam. Two implementations is not over-abstraction.

---

## Suggested landing order

Phase 0 alone first (docs-only, zero risk, fixes the actively-wrong parts).
Then 1 + 2 together (pure subtraction, one review). Then 3. Then 4 item 1.
Defer 4 items 2–5 one release, and schedule 5 independently.
