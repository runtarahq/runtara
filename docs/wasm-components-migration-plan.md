# WASM Components Migration Plan

Pluggable agents via the WebAssembly Component Model. Replaces the monolithic dispatcher + static-linked workflow pipeline with per-agent components composed at build time and loaded by an embedded wasmtime host.

---

## 1. Executive summary

Today every agent is a Rust module in `runtara-agents`, statically linked into two places: per-workflow binaries (via `quote!`-emitted Rust + direct `rustc` invocation) and a single shared "universal dispatcher" image (`__agent_dispatcher__:33`) used by the test path. Editing any agent requires bumping `DISPATCHER_VERSION` because the dispatcher's cache key is just the integer.

We migrate to:

- **One WIT contract** — `runtara:agent@0.1.0` — that every agent component implements.
- **Per-agent component crates** (`runtara-agent-{crypto,shopify,…}`) producing one `.wasm` each.
- **WAC-composed workflow images** built per workflow from the workflow-logic component + the agent components it imports.
- **Embedded wasmtime in `runtara-server`** (new `runtara-component-host` crate) for the test path and, eventually, the workflow runner.
- **Auto-discovered metadata** — `GET /api/runtime/agents` is populated from each component's `list-capabilities()` at server boot.

What this kills: `DISPATCHER_VERSION`, the giant match table in `runtara-workflow-stdlib::dispatch`, the universal dispatcher image, the "every workflow rebuilds when any agent changes" coupling, and the wasmtime-CLI binary dependency.

What it costs: a WIT contract, ~22 new agent crates, a new host crate (~1500 LOC), a codegen rewrite, and a multi-release rollout.

---

## 2. Current state (verified by discovery)

| Surface | File | Notes |
|---|---|---|
| Agent macros | `crates/runtara-agent-macro/src/lib.rs:285-541` | `#[capability]` emits named statics `__CAPABILITY_META_*`, `__CAPABILITY_EXECUTOR_*`, `__INPUT_META_*`. No `linkme`/`inventory` — pure named-static emission. |
| Static dispatch table | `crates/runtara-workflow-stdlib/src/dispatch.rs:80-1041` | Hand-written ~250-arm `match` table over `(module, capability_id)`. |
| Registry (native) | `crates/runtara-agents/src/static_registry.rs` | Same data via `&[CapabilityRegistration]`. Used by server's internal endpoint. |
| Connection plumbing | `crates/runtara-agents/src/connections.rs:14-27` | `RawConnection { connection_id, integration_id, parameters, rate_limit_config }`. Injected as `_connection` field on input. |
| Proxy client | `crates/runtara-http/src/lib.rs:175-238` | POSTs `{method, url, headers, body|body_raw, connection_id, timeout_ms}` envelope to `RUNTARA_HTTP_PROXY_URL`. Strips all `X-Runtara-*` headers; `X-Org-Id` carries tenancy. |
| Proxy server | `crates/runtara-server/src/api/handlers/internal_proxy.rs` | Lives inside `runtara-server` (not a separate process). Resolves connection by `(connection_id, tenant_id)` SQL; injects auth (Bearer, SigV4, Azure Shared-Key, OAuth refresh). |
| Workflow codegen | `crates/runtara-workflows/src/codegen/ast/program.rs:309` | `quote!`-based. Per-workflow `__workflow_dispatch` match contains only used capabilities. |
| Workflow build | `crates/runtara-workflows/src/compile.rs:628` | Direct `rustc --target=wasm32-wasip2` invocation; `--extern crate=path` against precompiled rlibs in `deps_dir`. No `Cargo.toml`. |
| Test dispatcher | `crates/runtara-server/src/api/services/dispatcher.rs` | `DISPATCHER_SOURCE` Rust source string, compiled at server boot, registered as `__agent_dispatcher__:33`. |
| Runner | `crates/runtara-environment/src/runner/wasm.rs:186-213` | Shells out to `wasmtime run` CLI (v43); WASI HTTP via `--wasi http --wasi inherit-network`. No wasmtime Rust API anywhere. |
| Image registry | `runtara-environment` | Stores both workflow images (`{workflow_id}:{version}`) and dispatcher image. Keyed by name. SHA-256 is integrity-only, not a lookup key. |
| Native-only agents | `runtara-agents/src/lib.rs:12-38` | `sftp`, `compression`, `xlsx` gated by `feature = "native"` — excluded from WASM builds. |

Roughly 24 agent modules, ~250 capabilities. All non-native agents already build for `wasm32-wasip2` today.

---

## 3. Target architecture

```
                                     ┌──────────────────────────────────────────┐
                                     │  runtara-server (single process)         │
┌──────────────┐                     │                                          │
│  Frontend    │  GET /agents        │  ┌────────────────────────────────────┐  │
│  Step Picker │ ───────────────────>│  │ ComponentDispatcherService         │  │
└──────────────┘                     │  │  - wasmtime::Engine (shared)       │  │
                                     │  │  - InstancePre per agent (cached)  │  │
┌──────────────┐  POST /…/test       │  │  - list-capabilities() at boot     │  │
│ Test Capab.  │ ───────────────────>│  └────────────────────────────────────┘  │
└──────────────┘                     │             │ invoke()                    │
                                     │             ▼                              │
┌──────────────┐                     │  ┌─────────────────────────────────────┐  │
│ AgentTesting │                     │  │ /api/internal/proxy (HTTP injector) │  │
│  Service     │                     │  └─────────────────────────────────────┘  │
└──────────────┘                     │             ▲                              │
                                     │             │ X-Runtara-Connection-Id      │
                                     └─────────────┼──────────────────────────────┘
                                                   │
       agent components on disk                    │ wasi:http/outgoing-handler
       target/agent-components/                    │
       ├── crypto.wasm                             │
       ├── shopify.wasm        ┌──────────────────┴──────────────────┐
       ├── stripe.wasm    ──── │ wasmtime guest (per call)            │
       ├── …                   │  runtara_agent_<name>.wasm           │
       └── manifest.json       │  exports runtara:agent/capabilities  │
                               └──────────────────────────────────────┘

  Workflow execution (per-workflow composed image):

  workflow.json ──► codegen ──► workflow-logic crate (Cargo + WIT bindgen)
                                       │
                          cargo component build
                                       │
                                       ▼
                              workflow-logic.wasm  ─┐
                                                     ├──► wac compose ──► workflow.wasm
                              agent-shopify.wasm   ─┤    (composed component)
                              agent-transform.wasm ─┘
```

Five components of the system, each with its own design (sections 4-8).

---

## 4. Component A — WIT contract (`runtara:agent@0.1.0`)

The contract every agent component implements. **Lock at end of Phase 0.**

### 4.1 Package layout

```
crates/runtara-agent-wit/
  Cargo.toml             # tiny crate; the WIT is the artifact
  wit/
    runtara-agent.wit
    deps/wasi-http-0.2.3/
    deps/wasi-cli-0.2.3/
    deps/wasi-clocks-0.2.3/
    deps/wasi-random-0.2.3/
    deps/wasi-io-0.2.3/
    deps.toml            # wit-deps lockfile
```

### 4.2 `wit/runtara-agent.wit`

```wit
package runtara:agent@0.1.0;

interface types {
    // Mirrors RawConnection (crates/runtara-agents/src/connections.rs:14-27).
    // parameters/rate-limit-config stay as JSON strings — integration shapes vary
    // wildly and freezing them in WIT would require a flag day per connector.
    record connection-info {
        connection-id: string,
        integration-id: string,
        connection-subtype: option<string>,
        parameters: string,                  // JSON object
        rate-limit-config: option<string>,   // JSON object
    }

    // Mirrors AgentError envelope and the JSON shape produced by the executor
    // wrapper (crates/runtara-agent-macro/src/lib.rs:406-413).
    // category/severity are strings (not WIT enums) so adding a variant is a
    // Rust-side minor bump, not a WIT major bump.
    record error-info {
        code: string,
        message: string,
        category: string,            // "transient" | "permanent"
        severity: string,            // "warning" | "error" | "critical"
        retryable: bool,
        retry-after-ms: option<u64>,
        attributes: option<string>,  // JSON object
    }

    record known-error {
        code: string,
        description: string,
        kind: string,                // "transient" | "permanent"
        attributes: list<string>,
    }

    record compensation-hint {
        capability-id: string,
        description: option<string>,
    }

    // Mirrors AgentModuleConfig (crates/runtara-dsl/src/agent_meta.rs:441-554).
    // One record per component image; not duplicated into every capability.
    record module-info {
        id: string,
        display-name: string,
        description: string,
        has-side-effects: bool,
        supports-connections: bool,
        integration-ids: list<string>,
        secure: bool,
    }

    record capability-info {
        id: string,
        function-name: string,
        display-name: option<string>,
        description: option<string>,
        has-side-effects: bool,
        is-idempotent: bool,
        rate-limited: bool,
        tags: list<string>,
        input-schema: string,        // JSON Schema document
        output-schema: string,       // JSON Schema document
        known-errors: list<known-error>,
        compensation-hint: option<compensation-hint>,
    }
}

interface capabilities {
    use types.{capability-info, module-info, connection-info, error-info};

    get-module-info:   func() -> module-info;
    list-capabilities: func() -> list<capability-info>;
    invoke: func(
        capability-id: string,
        input: string,                       // JSON-encoded value
        connection: option<connection-info>, // out-of-band, NOT inside `input`
    ) -> result<string, error-info>;          // JSON-encoded value on success
}

world agent {
    // Outbound HTTP — the only side-effect channel. The agent POSTs to
    // RUNTARA_HTTP_PROXY_URL just like today; the proxy injects credentials.
    import wasi:http/outgoing-handler@0.2.3;
    import wasi:http/types@0.2.3;
    import wasi:io/streams@0.2.3;
    import wasi:io/error@0.2.3;

    // Clocks (datetime), random (crypto/utils), stderr (tracing), env vars.
    import wasi:clocks/wall-clock@0.2.3;
    import wasi:clocks/monotonic-clock@0.2.3;
    import wasi:random/random@0.2.3;
    import wasi:cli/stderr@0.2.3;
    import wasi:cli/environment@0.2.3;   // reads RUNTARA_HTTP_PROXY_URL, …

    export capabilities;
}
```

### 4.3 No `runtara:host` interface in 0.1.0

Recommendation: **defer.** `tracing::error!` calls already route via `wasi:cli/stderr`; outbound HTTP via `wasi:http`; secrets never cross the boundary (proxy resolves them). YAGNI. If structured logging keyed by workflow span becomes necessary, add a single-function `runtara:logging/logger.log: func(level: string, fields: string)` later — additive minor bump.

### 4.4 Versioning policy

| Change | Bump | Example |
|---|---|---|
| New optional record field | `0.1.x` → `0.1.(x+1)` | adding `tags-extra: option<list<string>>` |
| New function in interface | `0.1.x` → `0.1.(x+1)` | adding `health-check()` |
| Renaming/removing fields; required field added; signature change | `0.x.y` → `0.(x+1).0` | renaming `is-idempotent` → `idempotent` |
| First externally stable | `0.y.z` → `1.0.0` | when external SDKs depend on it |

The package version is encoded in the WIT itself (`package runtara:agent@0.1.0`). wasmtime refuses to link a host importing `@0.1.0` against a guest exporting `@0.2.0` — compatibility is enforced at instantiation, not runtime. CI gate: every guest crate's `Cargo.toml` must pin the same `runtara-agent-wit` version as the host workspace.

### 4.5 Open questions deferred to 0.2.x

1. **Inline vs sidecar schemas.** Default to inline (self-describing). If component sizes get unwieldy, schemas move to a sidecar JSON; the WIT records permit empty strings as the sidecar sentinel — no breaking change later.
2. **Host-side `list-capabilities` caching.** Cache by image SHA-256. Static per image; one call per component per process restart.
3. **Streaming results.** Add `invoke-streaming` parallel to `invoke` using `stream<u8>` in 0.2.0 if needed (e.g., LLM streaming). Today's synchronous `invoke` covers every existing capability.

---

## 5. Component B — Per-agent component crates

### 5.1 Crate structure: per-agent crates + `runtara-agent-common`

```
crates/
  runtara-agent-wit/         # WIT package (§4)
  runtara-agent-common/      # shared utilities (RawConnection, AgentError, ProxyHttpClient)
  runtara-agent-macro/       # existing; gains #[agent_component] macro
  runtara-agent-crypto/      # one cdylib crate per agent
  runtara-agent-csv/
  runtara-agent-datetime/
  runtara-agent-text/
  runtara-agent-transform/
  runtara-agent-utils/
  runtara-agent-xml/
  runtara-agent-file/
  runtara-agent-http/
  runtara-agent-stripe/
  runtara-agent-shopify/
  runtara-agent-hubspot/
  runtara-agent-slack/
  runtara-agent-mailgun/
  runtara-agent-ai-tools/
  runtara-agent-bedrock/
  runtara-agent-openai/
  runtara-agent-s3-storage/
  runtara-agent-azure-blob-storage/
  runtara-agent-sharepoint/
  runtara-agent-commerce/
  runtara-agent-object-model/
  # native-only ports added in Phase 6:
  runtara-agent-compression/
  runtara-agent-xlsx/
xtask/                       # build & manifest emission
```

Per-agent crates over feature-flagged mega-crate because: build parallelism (~22 small crates compile in parallel; mega-crate serializes), per-component size (each `.wasm` carries only its own deps), independent SHA versioning, and cache locality on edits.

### 5.2 `runtara-agent-common`

Re-exports macros, holds `RawConnection`, `AgentError`, `ProxyHttpClient`, `FileData`, env-var caches. Each agent crate has one common dep, not two:

```toml
# crates/runtara-agent-common/Cargo.toml
[package]
name = "runtara-agent-common"

[dependencies]
runtara-agent-macro = { path = "../runtara-agent-macro" }
runtara-agent-wit = { path = "../runtara-agent-wit" }    # for WIT path
runtara-dsl = { path = "../runtara-dsl" }                # for metadata types
serde = { version = "1", features = ["derive"] }
serde_json = "1"
wit-bindgen = "0.36"

[package.metadata.component]
package = "runtara:agent"

[package.metadata.component.target]
path = "../runtara-agent-wit/wit"
world = "agent"
```

Content moves from current locations:
- `crates/runtara-agents/src/agents/integrations/integration_utils/{client,connection,env}.rs` → `runtara-agent-common::{http,connection,env}`
- `crates/runtara-agents/src/types.rs` (`FileData`, `AgentError`) → `runtara-agent-common::{types,error}`

### 5.3 Macro evolution

Keep `#[capability]` unchanged. Add **one** new macro `#[agent_component(module = "...", executors = [...])]` that emits the per-component `Guest` impl wrapping today's executor statics:

```rust
// What the macro expands to (sketch)
impl exports::runtara::agent::capabilities::Guest for Component {
    fn get_module_info() -> ModuleInfo { /* from AgentModuleConfig */ }
    fn list_capabilities() -> Vec<CapabilityInfo> { /* iterate __CAPABILITY_META_*  */ }
    fn invoke(id: String, input: String, conn: Option<ConnectionInfo>)
        -> Result<String, ErrorInfo>
    {
        let val: serde_json::Value = serde_json::from_str(&input)?;
        let val = inject_connection(val, conn);   // restore today's `_connection` convention
        let out = match (module, id.as_str()) {
            ("crypto", "hash") => (__CAPABILITY_EXECUTOR_HASH.execute)(val),
            ("crypto", "hmac") => (__CAPABILITY_EXECUTOR_HMAC.execute)(val),
            _ => return Err(ErrorInfo::not_found(&id)),
        }.map_err(parse_json_err)?;
        Ok(serde_json::to_string(&out)?)
    }
}
export!(Component);
```

This is mechanical and identical across agents — exactly what a derive macro should produce.

### 5.4 Worked example — crypto agent

`crates/runtara-agent-crypto/Cargo.toml`:

```toml
[package]
name = "runtara-agent-crypto"
version.workspace = true
edition.workspace = true

[lib]
crate-type = ["cdylib"]

[dependencies]
runtara-agent-common = { path = "../runtara-agent-common" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
base64 = "0.22"
hmac = "0.12"
md-5 = "0.10"
sha1 = "0.10"
sha2 = "0.10"

[package.metadata.component]
package = "runtara:agent-crypto"

[package.metadata.component.target]
path = "../runtara-agent-wit/wit"
world = "agent"
```

`crates/runtara-agent-crypto/src/lib.rs`:

```rust
use runtara_agent_common::{capability, CapabilityInput, CapabilityOutput};

mod caps {
    use super::*;
    // Identical to today's crates/runtara-agents/src/agents/crypto.rs
    // (HashInput, HmacInput, hash(), hmac(), … move here unchanged).
}

#[runtara_agent_common::agent_component(
    module = "crypto",
    executors = [caps::hash, caps::hmac],
)]
struct Component;
```

Native unit tests (`#[cfg(test)] mod tests` from today's `crypto.rs`) move with the capability functions.

Build:

```bash
cargo component build --release --target wasm32-wasip2 -p runtara-agent-crypto
# → target/wasm32-wasip2/release/runtara_agent_crypto.wasm  (~150 KB)
```

### 5.5 Build pipeline

An `xtask/` crate orchestrates:

```bash
cargo run -p xtask -- build-agent-components --all
# does, per crate:
#   1. cargo component build --release --target wasm32-wasip2 -p $crate
#   2. wasm-tools component wit $wasm  (validate world)
#   3. wasm-tools strip $wasm
#   4. wasm-opt -Os $wasm -o $opt
#   5. SHA-256
#   6. copy to target/agent-components/<name>-<version>-<short-sha>.wasm
#   7. one-shot instantiate to call list-capabilities()
#   8. append to target/agent-components/manifest.json
```

Manifest:

```json
{
  "schema_version": 1,
  "wit_package_version": "0.1.0",
  "components": [
    {
      "name": "crypto",
      "crate": "runtara-agent-crypto",
      "version": "5.0.0",
      "path": "crypto-5.0.0-a1b2c3d4.wasm",
      "sha256": "a1b2c3d4…",
      "size_bytes": 152340,
      "capabilities": [
        { "id": "hash", "module": "crypto", "requires_connection": false },
        { "id": "hmac", "module": "crypto", "requires_connection": false }
      ]
    }
  ]
}
```

The manifest is the contract between the build pipeline and `runtara-component-host` (§6) plus the workflow codegen (§7).

### 5.6 Native-only agents — thin wrapper components

Today's `native_agent_stub` (`crates/runtara-workflow-stdlib/src/dispatch.rs:20-74`) already handles native capabilities from WASM workflows: it builds a JSON envelope and POSTs to `http://127.0.0.1:7002/api/internal/agents/{module}/{cap}`, where the native side (host process) executes the real capability. **The component-model migration keeps this pattern.** Each native-only agent ships as a **thin wrapper component**.

- **`runtara-agent-sftp`, `runtara-agent-compression`, `runtara-agent-xlsx`** are full component crates, identical in shape to other agent crates. Their `invoke()` body is a single helper from `runtara-agent-common`:

  ```rust
  // crates/runtara-agent-common/src/native.rs
  pub fn invoke_native(
      module: &str,
      capability_id: &str,
      input: serde_json::Value,
      connection: Option<&ConnectionInfo>,
  ) -> Result<serde_json::Value, AgentError> {
      let url = format!("{}/api/internal/agents/{}/{}", agent_service_url(), module, capability_id);
      let body = serde_json::json!({
          "input": input,
          "connection": connection,
      });
      ProxyHttpClient::raw()
          .post(&url)
          .json(&body)
          .send_json()
          .map_err(AgentError::from)
  }
  ```

- The native-only crates' `invoke()` becomes one line per capability — call `invoke_native(module, cap, input, conn)`. Capability metadata (`list-capabilities()`, `get-module-info()`) stays declared in the component so the Step Picker sees them like any other agent. Connection-using native agents (sftp) pass the `ConnectionInfo` through; the native side resolves it the same way today's proxy does.

- The **native side** keeps the existing C-deps logic. The `http://127.0.0.1:7002/api/internal/agents/...` endpoint already exists; we just verify it's populated with handlers for `sftp`, `compression`, `xlsx`. The native execution runs inside the same process as `runtara-server` (or a sidecar — exact location is an implementation detail of `RUNTARA_AGENT_SERVICE_URL`).

This preserves three properties: (1) uniform component contract — every agent is a `runtara:agent/capabilities` component, (2) no rewrite of well-tested C-deps logic (libssh2 for SFTP, native zip libs for compression, calamine for xlsx), (3) no special-case in the workflow codegen or the host loader.

Trade-off: an extra HTTP roundtrip for native capabilities (already today's behavior under WASM workflows via `native_agent_stub`). Acceptable — SFTP/compression/xlsx are batchy operations where the per-call HTTP cost is dominated by the work itself.

### 5.7 Stateful agents (S3, Azure Blob)

Today's `s3_storage.rs:28` and `azure_blob_storage.rs:32` keep `OnceLock<RwLock<HashMap<connection_id, Arc<Client>>>>`. Under per-call component instantiation, each `invoke()` gets a fresh `Store` — the cache doesn't survive across calls *within* a workflow run.

Recommendation: **pool the `Store` host-side per workflow run, not per process.** One `Store` lives for the duration of one workflow instance; agent-side `OnceLock` caches survive across multiple `invoke()` calls inside that run. Drop the now-redundant `OnceLock<HashMap>` (per-process global) — it's pointless when `Store` is scoped to one workflow anyway. Cross-workflow client reuse is the proxy's job (HTTP keep-alive).

---

## 6. Component C — Embedded wasmtime host (`runtara-component-host`)

New crate. Replaces `DispatcherService` for the test path and, in Phase 6, replaces the `wasmtime run` CLI shell-out for workflows.

### 6.1 Layout

```
crates/runtara-component-host/
  Cargo.toml         # wasmtime, wasmtime-wasi, wasmtime-wasi-http
  src/
    lib.rs
    engine.rs        # build_engine(), EngineConfig
    host_state.rs    # HostState, WasiView, WasiHttpView with custom send_request
    registry.rs      # ComponentRegistry: agent_id → (Component, InstancePre, capabilities)
    bindings.rs      # wasmtime::component::bindgen!({ world: "agent", async: true })
    dispatcher.rs    # ComponentDispatcherService
    error.rs
```

### 6.2 Engine

One process-wide `wasmtime::Engine`:

```rust
pub fn build_engine(cfg: &EngineConfig) -> Result<Engine> {
    let mut c = Config::new();
    c.wasm_component_model(true);
    c.async_support(true);
    c.async_stack_size(2 * 1024 * 1024);
    c.consume_fuel(false);                       // opt-in only
    c.epoch_interruption(cfg.enable_epoch_interruption); // for deadlines
    c.cranelift_opt_level(OptLevel::Speed);
    c.parallel_compilation(true);
    c.wasm_backtrace(true);
    if let Some(dir) = &cfg.module_cache_dir { c.cache_config_load(dir)?; }
    Ok(Engine::new(&c)?)
}
```

Epoch interruption (100ms ticks) lets us enforce wall-clock deadlines per call without the per-instruction tax of fuel. Today's wasmtime-CLI runner has *no* in-call deadline; this is an upgrade.

### 6.3 Host state

```rust
pub struct HostState {
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    pub table: ResourceTable,
    pub ctx: Arc<CallContext>,   // tenant_id, proxy_url, instance_id, …
}

impl WasiHttpView for HostState {
    fn send_request(&mut self, mut req: hyper::Request<…>, cfg: …) -> … {
        // Defensive header injection: force X-Org-Id from the host, override any
        // value the guest set. Closes the "tampered SDK could spoof tenancy" hole.
        req.headers_mut().insert("X-Org-Id", self.ctx.tenant_id.parse()?);

        // If the destination isn't our proxy, strip credentials — credentials
        // must flow via the proxy, never directly from the agent.
        let host = req.uri().host().unwrap_or("");
        let proxy_host = self.ctx.proxy_host.as_str();
        if host != proxy_host {
            req.headers_mut().remove(AUTHORIZATION);
            req.headers_mut().remove(COOKIE);
        }
        default_send_request(req, cfg)
    }
}
```

`WasiCtxBuilder::envs(...)` exposes only what agents actually need (`RUNTARA_TENANT_ID`, `RUNTARA_HTTP_PROXY_URL`, `RUNTARA_OBJECT_MODEL_URL`, `RUNTARA_AGENT_SERVICE_URL`, and `RUNTARA_INSTANCE_ID` when present). No filesystem, no stdin.

### 6.4 Component loading & pre-instantiation

```rust
pub struct ComponentRegistry {
    engine: Engine,
    by_agent:      HashMap<String, Arc<LoadedComponent>>,
    by_capability: HashMap<(String, String), Arc<LoadedComponent>>,
}

impl ComponentRegistry {
    pub async fn load_from_manifest(engine: Engine, manifest_path: &Path) -> Result<Self> {
        let manifest = read_manifest(manifest_path).await?;
        let mut linker: Linker<HostState> = Linker::new(&engine);
        wasmtime_wasi::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::add_to_linker_async(&mut linker)?;

        for entry in &manifest.components {
            let component = Component::from_file(&engine, &entry.full_path())?;
            let pre = linker.instantiate_pre(&component)?;
            let caps = enumerate_capabilities(&engine, &pre).await?;
            // …populate by_agent and by_capability…
        }
        Ok(Self { engine, by_agent, by_capability })
    }
}
```

`InstancePre` is the typechecked-but-not-instantiated handle. Per-call we do `pre.instantiate_async(&mut store).await?` (single-digit ms with wasmtime 43). The slow part — `Component::from_file` (cranelift compile, hundreds of ms per agent) — happens once at server startup.

### 6.5 `ComponentDispatcherService` replaces `DispatcherService`

```rust
pub struct ComponentDispatcherService {
    engine: Engine,
    registry: Arc<ComponentRegistry>,
    deadline_per_call: Duration,   // default 60s
}

impl ComponentDispatcherService {
    pub async fn test_capability(
        &self,
        tenant_id: &str,
        agent_id: &str,
        capability_id: &str,
        input: serde_json::Value,
        connection: Option<ResolvedConnection>,
    ) -> Result<TestResult, AgentError> {
        let loaded = self.registry.get(agent_id).ok_or_else(…)?;
        let conn_info = connection.map(connection_info_from_resolved);
        let input_json = serde_json::to_string(&input).expect("serializable");

        let ctx = Arc::new(CallContext::for_test(tenant_id, /* proxy/agent-service URLs */));
        let mut store = Store::new(&self.engine, HostState::new(ctx));
        store.set_epoch_deadline((self.deadline_per_call.as_millis() / 100) as u64);

        let bindings = RuntaraAgent::instantiate_pre_async(&mut store, &loaded.pre).await?;
        let caps = bindings.runtara_agent_capabilities();
        match caps.call_invoke(&mut store, capability_id, &input_json, conn_info.as_ref()).await {
            Ok(Ok(out)) => Ok(TestResult::success(serde_json::from_str(&out)?)),
            Ok(Err(info)) => Ok(TestResult::error(AgentError::from(info))),
            Err(e) if e.is::<wasmtime::Trap>() => Err(AgentError::Trap(e.to_string())),
            Err(e) => Err(AgentError::Host(e.to_string())),
        }
    }
}
```

`AgentTestingService::test_agent` in `crates/runtara-server/src/api/services/agent_testing.rs` retargets from `runtime_client.execute_sync(image_id, …)` to `dispatcher.test_capability(…)`. **No image-registry hop.** `DispatcherService`, `DISPATCHER_SOURCE`, and `DISPATCHER_VERSION` all delete in Phase 4.

### 6.6 `/api/runtime/agents` metadata source

Today: baked into the server binary at `runtara-server/build.rs:448-464`. Under components: union of every loaded component's `list-capabilities()` output, populated at boot and cached in `OnceLock<Vec<AgentSpec>>`. The same `AgentSpec` / `CapabilitySpec` structs in `runtara-dsl::spec::agent_openapi` are populated from the new source — frontend DTOs unchanged.

### 6.7 Workflow runner: stay CLI, then migrate

**Phase 3 keeps `wasmtime run` CLI** for workflow execution. wasmtime 43 CLI handles composed components natively (no `--wasm component-model` flag needed in v43; component detection is automatic). The only changes to `crates/runtara-environment/src/runner/wasm.rs` are: respect `metadata.workflow.compileMode` for image selection logic, and record per-mode metrics.

**Phase 6+ migrates the workflow runner to embedded wasmtime** via the same `runtara-component-host` crate. Trade-offs:

- **Gain**: shared `Engine`+JIT cache across test + workflow paths; intercept `wasi:http` for workflows too; eliminate the wasmtime binary dep from the bundle.
- **Cost**: per-instance ~256 MiB linear-memory limit (via `ResourceLimiter`); host process memory budget shared with all concurrent workflows.

This is a clean follow-up after Phase 6 lands, not part of the core migration.

### 6.8 End-to-end test-capability walkthrough

1. `POST /api/runtime/agents/shopify/capabilities/list-products/test` with `{ input, connectionId }`.
2. `test_agent_handler` (existing) extracts `X-Org-Id`, calls `AgentTestingService::test_agent(tenant, "shopify", "list-products", input, Some(conn_id))`.
3. Rate-limit check (existing). `ConnectionsFacade.get_with_parameters(conn_id, tenant)` resolves connection (existing). 
4. `ComponentDispatcherService.test_capability(tenant, "shopify", "list-products", input, Some(conn)).await`.
5. `registry.get("shopify")` → pre-instantiated `LoadedComponent` (cached at boot).
6. New `Store<HostState>` with `CallContext { tenant_id, proxy_url, … }`. Epoch deadline set to 60s.
7. `bindings.call_invoke(...)` → guest code runs Shopify capability → `wasi:http/outgoing-handler` POST to `RUNTARA_HTTP_PROXY_URL/…`.
8. `HostState::send_request` injects `X-Org-Id` host-side, forwards via internal hyper client to `/api/internal/proxy`.
9. Proxy server (existing) resolves connection, injects auth (Bearer/SigV4/…), forwards to upstream.
10. Response streams back. Capability serializes its result to JSON. `invoke` returns `Ok(Ok(output_json))`.
11. `Store` drops; linear memory + resource table freed.
12. Handler returns `TestResult` JSON to client.

Failure paths map cleanly: WIT `error-info` → `AgentError::Guest`; `wasmtime::Trap::Interrupt` → timeout error; `Component::from_file` failure at boot → fail-fast server start.

---

## 7. Component D — Workflow codegen → WAC composition

### 7.1 Architecture

Per-workflow build produces a **composed component** = workflow-logic component + N agent components, statically linked by `wac compose`. Same OCI deployment story as today (one `{workflow_id}:{version}` image); the artifact format inside is a composed component instead of a monolithic binary.

- **workflow-logic** crate is generated per-workflow. Exports `wasi:cli/run@0.2.0` so wasmtime's `_start` invokes it. Imports one named instance of `runtara:agent/capabilities` per agent used (`shopify`, `crypto`, …), plus `wasi:cli/command`'s standard imports for the SDK's QUIC client.
- **runtara-sdk** and **runtara-workflow-stdlib** stay as library deps of workflow-logic. Not components — they're internal infrastructure.

After `wac compose`, the resulting composed component imports only `wasi:cli/command` (plus what agents declared, which compose merges).

### 7.2 New `CodegenArtifacts` from `emit_program`

Today's `emit_program(graph, &mut ctx)` returns a `String` of Rust source. New return:

```rust
pub struct CodegenArtifacts {
    pub lib_rs:          String,           // workflow-logic source
    pub cargo_toml:      String,           // Cargo.toml for cargo-component
    pub world_wit:       String,           // wit/world.wit
    pub wac_source:      String,           // workflow.wac
    pub agents_required: Vec<AgentRequirement>,
}

pub struct AgentRequirement {
    pub agent_id: String,    // "shopify" — both wit-import name and component package
    pub package:  String,    // "runtara:agent-shopify"
    pub version:  String,    // "0.3.1"
}
```

### 7.3 Per-workflow emitted artifacts

**`wit/world.wit`** (generated per workflow):

```wit
package runtara:workflow@0.0.1;

world workflow {
  import shopify:   runtara:agent/capabilities@0.1.0;
  import crypto:    runtara:agent/capabilities@0.1.0;
  import transform: runtara:agent/capabilities@0.1.0;
  include wasi:cli/command@0.2.0;
}
```

**`Cargo.toml`**:

```toml
[package]
name = "workflow"
version = "0.0.1"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
runtara-workflow-stdlib = { path = "<workspace>/crates/runtara-workflow-stdlib",
                            default-features = false, features = ["wasm"] }
runtara-sdk = { path = "<workspace>/crates/runtara-sdk" }
serde_json = "1"
wit-bindgen = "0.36"

[package.metadata.component]
package = "runtara:workflow-logic"

[package.metadata.component.target]
path = "wit"
world = "workflow"

[package.metadata.component.target.dependencies]
"runtara:agent" = { path = "<workspace>/crates/runtara-agent-wit/wit" }
```

**`src/lib.rs`** (skeleton):

```rust
wit_bindgen::generate!({ world: "workflow", path: "wit", generate_all });

extern crate runtara_workflow_stdlib;
use runtara_workflow_stdlib::prelude::*;

fn __invoke_agent(
    agent_id: &str,
    capability_id: &str,
    input: serde_json::Value,
    connection: Option<&shopify::ConnectionInfo>,   // structural — same WIT record
) -> Result<serde_json::Value, String> {
    let input_str = serde_json::to_string(&input).map_err(|e| e.to_string())?;
    let result_str = match agent_id {
        "shopify"   => shopify::invoke(capability_id, &input_str, connection.cloned()),
        "crypto"    => crypto::invoke(capability_id, &input_str, connection.cloned()),
        "transform" => transform::invoke(capability_id, &input_str, connection.cloned()),
        other => return Err(format!("unknown agent {}", other)),
    }.map_err(|e| format!("{}:{}: {}", e.category, e.code, e.message))?;
    serde_json::from_str(&result_str).map_err(|e| e.to_string())
}

struct Component;

fn __runtara_main() -> std::process::ExitCode {
    // EXISTING main() body — SDK init, input load, __root_span, execute_workflow(),
    // completion reporting, ExitCode mapping — unchanged from program.rs:1442-1590.
    …
}

impl Guest for Component {
    fn run() -> Result<(), ()> {
        if __runtara_main() == std::process::ExitCode::SUCCESS { Ok(()) } else { Err(()) }
    }
}
export!(Component);
```

**`workflow.wac`**:

```wac
package runtara:workflow-instance@0.0.1;

let shopify-comp   = new runtara:agent-shopify@0.3.1   { ... };
let crypto-comp    = new runtara:agent-crypto@0.1.2    { ... };
let transform-comp = new runtara:agent-transform@0.2.0 { ... };

let wf = new runtara:workflow-logic@0.0.1 {
  shopify:   shopify-comp.capabilities,
  crypto:    crypto-comp.capabilities,
  transform: transform-comp.capabilities,
};

export wf...;
```

### 7.4 Codegen changes

- `emit_imports` (`program.rs:383`): drop `use runtara_workflow_stdlib::agents::*`; rely on `wit_bindgen::generate!` for per-agent modules.
- `emit_workflow_dispatch` (`program.rs:285`): **delete**. Replaced by `__invoke_agent` (one trampoline emitted once per workflow).
- `steps/agent.rs:240-301`: the `__workflow_dispatch(agent_id, cap_id, inputs)` call site becomes `__invoke_agent(agent_id, cap_id, inputs, connection.as_ref())`. Per-step structure (span, debug events, retry wrapper, cache key) unchanged.
- `emit_connection_fetch` (`steps/agent.rs:563-601`): stop merging `_connection` into the JSON input. Build a `ConnectionInfo` WIT record and pass it as the third arg.
- `emit_main`: becomes `__runtara_main()` called from `Guest::run()`.

### 7.5 Build invocation

New `compile.rs` flow under `compileMode=components`:

```rust
// 1. Materialize the workflow-logic crate
fs::write(build_dir.join("Cargo.toml"), &artifacts.cargo_toml)?;
fs::write(build_dir.join("src/lib.rs"), &artifacts.lib_rs)?;
fs::write(build_dir.join("wit/world.wit"), &artifacts.world_wit)?;
fs::write(build_dir.join("workflow.wac"), &artifacts.wac_source)?;

// 2. Build the workflow-logic component
Command::new("cargo")
    .args(["component", "build", "--release", "--target", "wasm32-wasip2"])
    .current_dir(&build_dir)
    .env("CARGO_TARGET_DIR", build_dir.join("target"))
    .status()?;

// 3. Compose with the agent CAS
Command::new("wac")
    .args([
        "compose", &build_dir.join("workflow.wac").display().to_string(),
        "-d", &data_dir.join("agent-cas").display().to_string(),
        "--define", &format!("runtara:workflow-logic={}", workflow_logic_wasm.display()),
        "-o", &composed_out.display().to_string(),
    ])
    .status()?;

// 4. Optional size pass
if env::var("RUNTARA_WASM_OPT").as_deref() == Ok("1") {
    Command::new("wasm-opt").args(["-Os", &composed_out, "-o", &composed_out]).status()?;
}
```

`agent-cas` is the directory populated by `xtask build-agent-components --all` (§5.5), located under the data dir for persistence.

Build-time trade-off vs today's direct `rustc` (~3-5s cold, ~1-2s warm): cargo-component cold build climbs to 15-30s. Mitigations: shared `CARGO_TARGET_DIR` per tenant (keyed by agent set), pre-built stdlib rlibs, no LTO on workflow-logic (LTO happens during `wasm-opt` instead). Warm build target: ~3s.

### 7.6 Caching & idempotence

Extend the cache key:

```rust
pub struct CompilationFingerprint {
    pub source_checksum:   String,        // SHA-256(definition_json) — existing
    pub template_version:  &'static str,  // env!("CARGO_PKG_VERSION") of runtara-workflows
    pub stdlib_version:    &'static str,  // ditto runtara-workflow-stdlib
    pub agent_manifest:    String,        // SHA-256 of sorted "agent_id=version" pairs
}

impl CompilationFingerprint {
    pub fn compilation_checksum(&self) -> String { … }
}
```

Add `compilation_checksum TEXT` to `workflow_compilations`. The 3-gate cache check at `compilation.rs:150-246` becomes (1) row exists, (2) `compilation_checksum` matches, (3) image still in registry. Image name stays `{workflow_id}:{version}` (the runner doesn't care about format); metadata records `compileMode` and `compilationChecksum`.

### 7.7 Resilience & durability

`#[resilient(durable, max_retries, …)]` is a `runtara-workflow-stdlib` proc macro operating on a Rust function inside workflow-logic. The function body changes from `__workflow_dispatch(...)` to `__invoke_agent(...)`. **No WIT changes.** Decision tree (retry on `category=="transient"`, honor `retry-after-ms`) maps cleanly because Plan A's `error-info` mirrors today's JSON envelope.

For Phase 3, keep error stringification at the `__invoke_agent` boundary — preserves today's behavior. Follow-up phase converts the resilient macro to typed errors.

### 7.8 SDK compatibility

`runtara-sdk` already builds for `wasm32-wasip2`. Stays a library inside workflow-logic. No new WIT — the SDK is a client of `runtara-core` over QUIC, not a component boundary. Confirm `wasi:sockets` is available in the `wasi:cli/command` world (it is in WASI 0.2).

### 7.9 Size estimate

- Today: 12–18 MB (single-binary workflow with 2 agents, `opt-level=s`, `lto=fat`, stripped).
- Composed before opt: 7–10 MB (workflow-logic ~3-5 MB + agent-shopify ~2-3 MB + agent-transform ~1-2 MB).
- After `wasm-opt -Os`: 6–8 MB.

**~30-50% smaller** because agent code ships once in the CAS and is linked, not duplicated through static-linking of the monolithic `runtara-agents` crate.

---

## 8. Component E — Migration phases

Each phase ends deployable and e2e-verified. Rollback is one config flip until Phase 6.

### Phase 0 — Foundations (no behavior change)

**Lands.** New crates `runtara-agent-wit`, `runtara-component-host` (empty skeletons). Deps: `wasmtime = "43"`, `wasmtime-wasi`, `wasmtime-wasi-http`, `wit-bindgen`, `cargo-component`, `wac-cli`, `wit-bindgen-cli`. Bundle install in `scripts/build-bundle.sh`. Server config: `agent_components_enabled: bool` (default false), `agent_components_manifest_path: Option<PathBuf>`. Hidden endpoint `GET /api/runtime/_internal/components/status`. CI: `wit-package` and `component-host-skeleton` jobs.

**Acceptance.** `cargo build --workspace` passes. `cargo test --workspace` unchanged. Server boots. **`e2e-verify`** of existing crypto path → identical results to pre-Phase-0 baseline.

### Phase 1 — Pilot component (crypto)

**Lands.** Crate `runtara-agent-common` (with `#[agent_component]` macro). Crate `runtara-agent-crypto`. Build target: `xtask build-agent-components --crate runtara-agent-crypto`. `ComponentHost::load_manifest()` instantiates `crypto.wasm`. When `agent_components_enabled=true` AND `agent=="crypto"`, the test endpoint routes through `ComponentHost`; else legacy dispatcher. CI A/B job runs 12 crypto test cases through both engines.

**Acceptance.** CI A/B job: byte-identical JSON for crypto across both engines. Latency recorded in `metrics.agent_test.duration{engine=…,agent="crypto"}`. **`e2e-verify`** that `crypto/hash` runs through wasmtime-embedded path; legacy fallback still works for everything else.

### Phase 2 — All WASM-compatible agents

**Lands.** ~23 new `runtara-agent-*` crates — all 21 WASM-native agents plus thin wrappers for `sftp`, `compression`, `xlsx` (per § 5.6). Each ships its own `.wasm`. `agent_components_enabled` evolves into `agent_components_allowlist: Vec<String>` (default empty). `GET /api/runtime/agents` sources allowlisted agents from `ComponentHost::list-capabilities()`, merged with legacy metadata for the rest. OpenAPI snapshot test (`tests/openapi_snapshot.rs`) committed.

**Acceptance.** Per-agent A/B passes for every non-native agent. OpenAPI snapshot unchanged. **`e2e-verify`** of a 5-step workflow (`http → transform → csv → crypto → s3`) under both modes; outputs match. `DISPATCHER_VERSION` bumped one final time (note: this becomes obsolete in Phase 4).

### Phase 3 — Workflow codegen (opt-in)

**Lands.** New `crates/runtara-workflows/src/codegen/components/` subdir. Compilation API gains `compileMode: "rustc-legacy" | "components"` (default `rustc-legacy`). Source checksum now includes `compileMode + wit_package_version`. Image name unchanged. Runner reads `metadata.workflow.compileMode` and runs the right way (`wasmtime run` works for both with wasmtime 43).

**Acceptance.** Same workflow compiled in both modes produces matching final outputs and step outputs. Cache hits on identical re-compile; cache miss on mode switch. Compile time budget: components-mode within 3× legacy cold, 1.5× warm. **`e2e-verify`** compile→deploy→execute cycle with `compileMode=components` on a representative workflow; `inspect_step` matches reference run for at least two step types.

### Phase 4 — Test dispatcher cutover

**Lands.** `AgentTestingService` calls `ComponentDispatcherService` directly — no image registry hop. Delete `crates/runtara-server/src/api/services/dispatcher.rs`. Delete `DISPATCHER_VERSION`. Decision on the legacy `runtara-environment::handle_test_capability` endpoint (used by `runtara-management-sdk` and `runtara-ctl`): **return HTTP 410 with a JSON body for one release; reimplement `runtara-management-sdk::Client::test_capability` and `runtara-ctl` to hit the runtime endpoint; delete in Phase 4+1.** Update auto-memory: remove `feedback_bump_dispatcher_version.md`.

**Acceptance.** `runtara-environment` image registry has zero rows for `image_name='__agent_dispatcher__'`. `runtara-ctl test-capability` round-trips. Load test 100 req/s for 60s: p99 latency at or below pre-Phase-4 baseline. **`e2e-verify`** of test-capability endpoint matrix across all agents.

### Phase 5 — Default flip

**Lands.** Default `compileMode` flips to `components`. `rustc-legacy` accepted with a deprecation warning for one release. Already-registered legacy images keep loading (`metadata.compileMode IS NULL` → legacy execution path; only matters until Phase 6). Metrics: `workflow_compile_mode_total{mode}` and `workflow_execute_duration_ms{mode}`.

**Acceptance.** One full release on main with default `components`. Production metrics: `workflow_execute_duration_ms` p95 within 10% of legacy baseline. Zero rollbacks triggered by production incidents. **`e2e-verify`** of fresh tenant + fresh workflow → components-mode by default; recompile of existing workflow → new image components-mode while old image still loadable.

### Phase 6 — Cleanup

**Lands.** Delete `crates/runtara-workflows/src/codegen/ast/` (legacy emitters). Delete `crates/runtara-workflow-stdlib/src/dispatch.rs` (the giant match — replaced by the components dispatch path; the `native_agent_stub` *helper* survives, moved into `runtara-agent-common::native`). Reduce or delete `runtara-agents` crate (the C-deps logic for sftp/compression/xlsx moves to its consumer, the internal native-agent HTTP handler in `runtara-server`). Reject `compileMode=rustc-legacy` with 400. Bundle: drop wasmtime CLI from `scripts/build-bundle.sh` (workflow runner migrates to embedded wasmtime as a follow-up).

Native-only agents (`sftp`, `compression`, `xlsx`) ship as thin wrapper components per § 5.6 — already built in Phase 2 alongside the rest.

**Acceptance.** `cargo build --workspace` succeeds; tree smaller (target −15 kLOC). **`e2e-verify`** of `sftp.list`, `compression.zip`, and `xlsx.write` round-tripping through their wrapper components → native handler → result; offline bundle build with no wasmtime CLI; `compileMode=rustc-legacy` returns 400.

### Timeline

```
M1 ─────► M2 ─────► M3 ──┐
            │            ├─► M5 ──► M6
            └────► M4 ───┘
```

- M1 = Phase 0 + 1 (foundations + pilot crypto). Independent.
- M2 = Phase 2 (all agents). Depends on M1.
- M3 = Phase 3 (codegen opt-in). Depends on M2.
- M4 = Phase 4 (test dispatcher cutover). Depends on M2; parallel to M3.
- M5 = Phase 5 (default flip). Depends on M3 + M4 each stable for one release.
- M6 = Phase 6 (cleanup). Depends on M5 stable for one full release.

### Decision points (hard checkpoints)

1. **End of Phase 0 — WIT contract review.** Sign off on `runtara:agent@0.1.0` before ~22 crates start depending on it.
2. **End of Phase 1 — Bench gate.** Cold-start and warm-call latency for `crypto/hash`. Budget: warm within 1.5× legacy median; cold within 200 ms.
3. **End of Phase 3 — Codegen + WAC review.** Largest single diff. Mandatory code review by ≥2 engineers; explicit walkthrough of workflow-logic emission, WAC composition, image-naming preservation, checksum expansion.
4. **Before Phase 5 — Production-readiness review.** Sign off on the rollback runbook (`docs/runbooks/components-rollback.md`).
5. **Before Phase 6 — Soak period.** One full release post-Phase-5 with zero rollback incidents. Cleanup is irreversible.

---

## 9. Risk register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | WAC tooling maturity (bugs / breaking changes) | M | H | Pin `wac-cli` version in bundle; document `wasm-tools compose` fallback; CI runs WAC build on every PR. |
| 2 | Component cold-start latency | M | M | `InstancePre` warmed at boot. Phase 1 bench gates Phase 2. Pre-Phase-4 load test gates cutover. |
| 3 | `wasi:http` interception breaks under components | L | M | Integration tests with stock + intercepted variants; host-side `wasi:http` Linker override is switchable per env. |
| 4 | WIT schema drift host/guest | M | H | Lock `runtara:agent@0.1.0` end-of-Phase-0. CI gate `wasm-tools component wit` against every built agent component. Breaking changes require explicit major bump. |
| 5 | Workflow compile-time regression | H | M | Per-tenant Cargo target cache. Cold ≤3×, warm ≤1.5× legacy. Continuously measured in Phase 3. |
| 6 | Build pipeline bloat (~80 MB extra tools) | M | L | `scripts/build-bundle.sh` installs to `bundle/tools/`; bundle cap 250 MB; offline-install test in CI. |
| 7 | Frontend metadata drift on `/api/runtime/agents` | M | H | OpenAPI snapshot test in CI. Snapshot diffs require explicit commit. Frontend regenerated only on snapshot update. |
| 8 | Stateful agents lose cross-call client caches | M | M | Per-workflow `Store` reuse (S3/Azure caches survive within a run). Phase 2 bench. If still >50 ms p95, reintroduce host-level keyed cache. |
| 9 | Unknown callers of legacy `/api/v1/agents/test` | L | M | Phase 4 ships 410 with explicit redirect JSON for one release; logs reviewed before final delete. |
| 10 | Image registry holds both formats | L | L | `metadata.compileMode` is the single SoT; runner branches; both paths exercised in CI. |

---

## 10. Files touched (summary)

### New

- `crates/runtara-agent-wit/{Cargo.toml, wit/runtara-agent.wit, wit/deps/…}`
- `crates/runtara-agent-common/{Cargo.toml, src/lib.rs, src/{http,connection,env,types,error}.rs}`
- `crates/runtara-component-host/{Cargo.toml, src/{lib,engine,host_state,registry,bindings,dispatcher,error}.rs}`
- `crates/runtara-agent-{crypto,csv,datetime,text,transform,utils,xml,file,http,stripe,shopify,hubspot,slack,mailgun,ai-tools,bedrock,openai,s3-storage,azure-blob-storage,sharepoint,commerce,object-model}/`
- `crates/runtara-agent-{sftp,compression,xlsx}/` (Phase 2, thin wrappers per § 5.6)
- `xtask/{Cargo.toml, src/main.rs}`
- `crates/runtara-workflows/src/codegen/components/{mod,workflow_logic_crate,composition,build}.rs`
- `tests/openapi_snapshot.rs`, `tests/snapshots/openapi.json`
- `docs/runbooks/components-rollback.md`

### Modified

- `crates/runtara-agent-macro/src/lib.rs` — add `#[agent_component]` macro
- `crates/runtara-server/src/api/services/agent_testing.rs` — retarget dispatch
- `crates/runtara-server/src/api/services/compilation.rs` — checksum expansion, mode plumbing
- `crates/runtara-server/src/api/handlers/{agents,workflows}.rs`
- `crates/runtara-server/src/config.rs` — feature flags
- `crates/runtara-server/build.rs` — drop baked-OpenAPI step (data now comes from components)
- `crates/runtara-workflows/src/codegen/{ast/program.rs, ast/steps/agent.rs}` — replaced by components emitter in Phase 6
- `crates/runtara-workflows/src/compile.rs` — new components build path
- `crates/runtara-environment/src/runner/wasm.rs` — read `compileMode`; eventually migrate to embedded
- `crates/runtara-environment/src/handlers.rs` — legacy `handle_test_capability` → 410, then delete
- `crates/runtara-management-sdk/src/client.rs`, `crates/runtara-ctl/src/commands/test.rs` — use runtime endpoint
- `Cargo.toml` (workspace), `Makefile`, `.github/workflows/ci.yml`, `scripts/build-bundle.sh`
- `~/.claude/projects/.../memory/MEMORY.md` — rotate `feedback_bump_dispatcher_version.md`

### Deleted (Phase 4 / Phase 6)

- `crates/runtara-server/src/api/services/dispatcher.rs` (Phase 4)
- `crates/runtara-workflow-stdlib/src/dispatch.rs` (Phase 6)
- `crates/runtara-agents/src/static_registry.rs` (Phase 6)
- `crates/runtara-workflows/src/codegen/ast/` (Phase 6)
- `crates/runtara-agents/` crate itself (Phase 6, if no native consumers remain)

---

## 11. Cross-references

- **Discovery context** (verified against the codebase, May 2026): § 2 lists every file and line cited. Re-verify before each phase since the codebase evolves.
- **Plan A** (WIT contract): § 4. Final WIT lives at `crates/runtara-agent-wit/wit/runtara-agent.wit`.
- **Plan B** (per-agent components): § 5. xtask build pipeline emits manifest at `target/agent-components/manifest.json`.
- **Plan C** (host wasmtime layer): § 6. New crate `runtara-component-host` replaces `DispatcherService`.
- **Plan D** (workflow codegen → WAC): § 7. New `codegen/components/` subdir; build path via `cargo component` + `wac compose`.
- **Plan E** (migration phases): § 8. Six phases, five decision points.
