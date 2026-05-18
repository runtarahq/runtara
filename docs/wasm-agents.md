# Phase 2 Agent Port Checklist

Every runtara agent ported to a WebAssembly Component. Order is easiest → hardest so each port reinforces the pattern before tackling the next category. Mark each row as it lands.

Totals: **23 agent crates, ~259 capabilities** (some shared schemas). Two legacy agents removed in Phase 2.1: `file` (10 caps — no direct FS in components; route file I/O through s3/sharepoint/etc.) and `commerce` (10 caps — generic facade was redundant; workflows use `shopify` directly).

Conventions:
- Crate name: `runtara-agent-<id>` (kebab-case of the module id).
- Build output: `target/wasm32-wasip1/release/runtara_agent_<id>.wasm`.
- Schema parity with `crates/runtara-agents/src/agents/<id>.rs` byte-for-byte — workflows still link the legacy module until Phase 3, so divergence breaks A/B parity.
- All gating logic (rate limits, retries, connection lookup) stays host-side; the guest only knows it received a `ConnectionInfo` blob and an input JSON.

## Phase 0/1 already landed

- [x] **runtara-agent-wit** — canonical WIT contract (`runtara:agent@0.1.0`).
- [x] **runtara-component-host** — embedded wasmtime + ComponentDispatcherService.
- [x] **runtara-agent-crypto** (2 caps: `hash`, `hmac`) — schema parity with legacy.
- [x] **runtara-agent-xml** (1 cap: `from-xml`)
- [x] **runtara-agent-csv** (3 caps: `from-csv`, `to-csv`, `get-header`)
- [x] **runtara-agent-utils** (13 caps)
- [x] **runtara-agent-datetime** (9 caps)
- [x] **runtara-agent-transform** (16 caps)
- [x] **runtara-agent-text** (28 caps)

## Phase 2.0 — Shared utilities (deferred)

- [ ] **runtara-agent-common** — extract _after_ 3-4 connection-using ports
  duplicate the same `ProxyHttpClient` / `NativeAgentClient` / error-envelope
  conversions. Pure agents (Phase 2.1) don't need it, so we don't waste design
  upfront. Slot lands between Phase 2.1 and Phase 2.2.

## Phase 2.1 — Pure agents (no outbound HTTP)

| Crate | Module | Caps | Notes |
| --- | --- | --- | --- |
| [ ] runtara-agent-xml | `xml` | 1 | Quickest port. |
| [ ] runtara-agent-csv | `csv` | 3 | Pure parsing/writing. |
| [ ] runtara-agent-utils | `utils` | 13 | `rand`, `delay_ms` — `wasi:clocks` is fine for delay. |
| [ ] runtara-agent-datetime | `datetime` | 9 | Pure `chrono`. |
| [ ] runtara-agent-transform | `transform` | 16 | JSON shape manipulation. |
| [x] runtara-agent-text | `text` | 28 | Largest pure agent. |
| ~~runtara-agent-file~~ | ~~`file`~~ | — | **Removed.** Components have no direct FS access; file content flows via base64-encoded `FileData` records inside connection-using agents. |

## Phase 2.2 — HTTP / connection-using agents

| Crate | Module | Caps | Notes |
| --- | --- | --- | --- |
| [ ] runtara-agent-http | `http` | 1 | First connection-using port — proves proxy plumbing under components. |
| [ ] runtara-agent-mailgun | `mailgun` | 1 | Trivial single-call API. |
| [ ] runtara-agent-slack | `slack` | 2 | Send message + upload file. |
| [ ] runtara-agent-ai-tools | `ai_tools` | 5 | Provider-routed via connection subtype. |
| ~~runtara-agent-commerce~~ | ~~`commerce`~~ | — | **Removed.** Generic commerce facade was a thin wrapper over Shopify; workflows use the shopify agent directly. |
| [ ] runtara-agent-openai | `openai` | 8 | Chat / embedding / vision. |
| [ ] runtara-agent-bedrock | `bedrock` | 7 | AWS Bedrock model invoke. SigV4 stays host-side. |
| [ ] runtara-agent-object-model | `object_model` | 12 | Internal HTTP to runtara-server. |
| [ ] runtara-agent-s3-storage | `s3_storage` | 10 | Drop `OnceLock<HashMap<conn_id, Arc<S3Client>>>` cache (per-call store now). |
| [ ] runtara-agent-azure-blob-storage | `azure_blob_storage` | 10 | Same cache drop as s3. |
| [ ] runtara-agent-sharepoint | `sharepoint` | 14 | MS Graph drive/item ops. |
| [ ] runtara-agent-stripe | `stripe` | 26 | Stripe Admin API. |
| [ ] runtara-agent-hubspot | `hubspot` | 31 | CRM CRUD + assoc + search. |
| [ ] runtara-agent-shopify | `shopify` | 50 | Largest integration — Shopify GraphQL Admin. |

## Phase 2.3 — Native-only wrappers (thin components)

These components do NOT compile the native logic into wasm. Their `invoke()` is a single call to `NativeAgentClient::invoke()` which POSTs to the host's existing `/api/internal/agents/{module}/{cap}` endpoint. The native side keeps the C-deps logic unchanged.

| Crate | Module | Caps | Notes |
| --- | --- | --- | --- |
| [ ] runtara-agent-sftp | `sftp` | 4 | libssh2 stays host-side. |
| [ ] runtara-agent-compression | `compression` | 4 | zip stays host-side (pure-Rust port deferred to a future phase). |
| [ ] runtara-agent-xlsx | `xlsx` | 2 | calamine stays host-side. |

## Per-agent acceptance checklist

For each ported agent:

1. Crate exists at `crates/runtara-agent-<id>/` with `cargo component build` succeeding.
2. `list_capabilities()` returns one entry per legacy `#[capability]` function, with matching `id`, `display_name`, `description`, `has_side_effects`, `is_idempotent`, `rate_limited` flags.
3. JSON Schemas for inputs/outputs round-trip identically with the legacy schemas (compare via `GET /api/runtime/agents/<id>`).
4. `dispatcher.test_capability` returns byte-identical output to the legacy path for at least one happy-path input per capability.
5. Connection-using agents: confirm `X-Runtara-Connection-Id` reaches the proxy and the response decodes correctly.
6. Stateful agents (s3, azure-blob): no `OnceLock` caches; per-call `Store` is acceptable cost.
7. `scripts/build-agent-components.sh` picks up the new crate automatically and reports the .wasm in the manifest.

## Out of scope for Phase 2

- Workflow codegen (still legacy until Phase 3).
- Dispatcher image deletion (Phase 4).
- Pure-Rust replacements for `compression`/`xlsx` C-deps (post-Phase 6).
- Frontend metadata source switch (Phase 5).
