# Phase 2 Agent Port Checklist

Every runtara agent ported to a WebAssembly Component. **Phase 2 complete.**

Final state: **23 agent crates, ~265 capabilities** built as standalone `.wasm` components. Two legacy agents retired (`file`, `commerce`).

## Phase 0/1 — Foundations

- [x] **runtara-agent-wit** — canonical WIT contract (`runtara:agent@0.1.0`).
- [x] **runtara-component-host** — embedded wasmtime + ComponentDispatcherService.

## Phase 2.1 — Pure agents (no outbound HTTP)

| Crate | Module | Caps | Status |
| --- | --- | --- | --- |
| [x] runtara-agent-crypto | `crypto` | 2 | hash, hmac |
| [x] runtara-agent-xml | `xml` | 1 | from-xml |
| [x] runtara-agent-csv | `csv` | 3 | from-csv, to-csv, get-header |
| [x] runtara-agent-utils | `utils` | 13 | random, delay-ms, calculate, country-iso, etc. |
| [x] runtara-agent-datetime | `datetime` | 9 | format/add/subtract/round date, time-between |
| [x] runtara-agent-transform | `transform` | 16 | extract, filter, sort, group-by, map-fields, etc. |
| [x] runtara-agent-text | `text` | 28 | regex, jinja templates, base64, case ops, slugify |

## Phase 2.2 — HTTP / connection-using agents

| Crate | Module | Caps | Status |
| --- | --- | --- | --- |
| [x] runtara-agent-http | `http` | 1 | http-request — first proxy-using port, proves plumbing |
| [x] runtara-agent-mailgun | `mailgun` | 1 | send-email |
| [x] runtara-agent-slack | `slack` | 2 | send-message, upload-file (V2 three-step) |
| [x] runtara-agent-ai-tools | `ai_tools` | 5 | provider router across OpenAI/Bedrock |
| [x] runtara-agent-openai | `openai` | 8 | chat, embeddings, vision, image gen, moderation |
| [x] runtara-agent-bedrock | `bedrock` | 7 | Claude, Titan, SD, invoke-model, list-models |
| [x] runtara-agent-object-model | `object_model` | 12 | internal HTTP, CRUD + bulk + aggregate + memory |
| [x] runtara-agent-s3-storage | `s3_storage` | 10 | bucket/object CRUD; OnceLock cache dropped |
| [x] runtara-agent-azure-blob-storage | `azure_blob_storage` | 10 | container/blob CRUD; OnceLock cache dropped |
| [x] runtara-agent-sharepoint | `sharepoint` | 14 | drive/item ops + chunked upload + search |
| [x] runtara-agent-stripe | `stripe` | 26 | form-encoded bodies; customer/payment/invoice/sub |
| [x] runtara-agent-hubspot | `hubspot` | 31 | CRM CRUD + associations + search |
| [x] runtara-agent-shopify | `shopify` | 50 | Admin GraphQL — largest port (~5700 lines) |

## Phase 2.3 — Thin native wrappers

These components do NOT carry the native logic. Their `invoke()` forwards to the host's internal native agent endpoint at `$RUNTARA_AGENT_SERVICE_URL/{module}/{capability}` where the native binary owns the C-deps (libssh2, native zip libs, calamine).

| Crate | Module | Caps | Status |
| --- | --- | --- | --- |
| [x] runtara-agent-sftp | `sftp` | 4 | list/download/upload/delete |
| [x] runtara-agent-compression | `compression` | 4 | create/extract/list archive, extract single file |
| [x] runtara-agent-xlsx | `xlsx` | 2 | from-xlsx, get-sheets |

## Retired

- ~~runtara-agent-file~~ — removed. Components have no direct FS access; file content flows via base64 `FileData` records inside connection-using agents.
- ~~runtara-agent-commerce~~ — removed. Generic facade was a thin wrapper over Shopify; workflows use the shopify agent directly.

## Shared pattern

Every connection-using component:
1. Reads `connection.connection_id` from the WIT `connection: option<connection-info>` arg.
2. Calls `runtara_http::HttpClient::with_timeout(...).request(method, url).header("X-Runtara-Connection-Id", connection_id).body_bytes(...).call_agent()`.
3. `call_agent()` reads `$RUNTARA_HTTP_PROXY_URL` and forwards the JSON envelope to `/api/internal/proxy`. The proxy resolves the connection, attaches credentials (Bearer/SigV4/Entra/etc.), and forwards to the real upstream. **The component never sees secrets.**

Native wrappers use `.call()` (not `.call_agent()`) so they bypass the credential-injecting proxy and hit `$RUNTARA_AGENT_SERVICE_URL/{module}/{cap}` directly. The `_connection` envelope is re-embedded into the input JSON for the native handler.

## Verification

```bash
# Build all 23 components
./scripts/build-agent-components.sh

# Run dispatcher tests (loads every .wasm + exercises crypto/hash + error envelope)
cargo test -p runtara-component-host --tests

# Live server smoke test (after restart with RUNTARA_AGENT_COMPONENTS_DIR set):
curl -X POST 'http://127.0.0.1:7001/api/runtime/agents/crypto/capabilities/hash/test?engine=components' \
  -H 'Content-Type: application/json' \
  -H 'X-Org-Id: '"$TENANT_ID" \
  -d '{"input":{"data":"hello"}}' | jq
```

Per-engine A/B remains the gating step before Phase 3 (workflow codegen migration to WAC composition) can begin.
