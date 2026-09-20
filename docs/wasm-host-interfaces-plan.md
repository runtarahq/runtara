# Primary host interfaces for WASM modules

Status: proposed implementation plan. No runtime changes are part of this document.

The focused [outbound HTTP migration plan](outbound-http-host-calls-plan.md)
supersedes this document's outbound transport migration details, including its
legacy HTTP compatibility proposal.

Companion decision: [trusted capabilities](trusted-capabilities-plan.md) defines
the `trusted` flag for built-in capabilities executed in fresh restricted WASM
instances with credentials for their own connection types. Host interfaces remain
the authority boundary; approved provider code may perform presigning inside
these isolated instances rather than in native code.

## Decision and scope

Make versioned Runtara host interfaces the primary contract between WASM and
platform services. The embedded deployment implements them through native Rust
service calls. Credentials, connection ownership, destination restrictions,
entitlements, rate limits, and auditing remain platform responsibilities.

The five surfaces are runtime, connections, outbound HTTP, Object Model, and
presigning. Agents retain their provider-specific request construction and
workflow-facing capability schemas. They no longer know internal service URLs,
internal HTTP routes, or how to supply tenant identity to those services.

```text
WASM workflow / agent
  -> versioned host interface
  -> run-scoped adapter with trusted identity and execution limits
  -> native service
     -> persistence / PostgreSQL / Redis / external HTTPS

Public HTTP / MCP client
  -> authenticated API adapter
  -> the same native service
```

HTTP remains an adapter for external clients and, during migration, older
artifacts. A future remote service implementation can sit behind a host trait;
implementing a distributed gateway is not required for this migration.

Success means new embedded workflow and agent execution needs no internal HTTP
listener for these five surfaces. It does not mean external HTTPS disappears,
the server loses its public APIs, or every other internal server endpoint can
automatically be deleted.

## Current implementation and migration boundaries

| Surface | Current path | Required change |
| --- | --- | --- |
| Runtime | `runtime@0.4.0` host imports, implemented by `PersistenceRuntimeHost`; production workflow entry uses `lifecycle@0.2.0.invoke` | Preserve existing native behavior and lifecycle semantics; include it in the common context and compatibility model |
| Connections | Async `resolver@0.2.0` imports call `NativeConnectionResolver` and `ConnectionsFacade` directly; existing `0.1.0` artifacts use the same native backend | Implemented: host-owned tenant, per-run caches, interactive and workflow execution, no internal connection HTTP routes or URL configuration |
| Outbound HTTP | Async `outbound-http/client@0.1.0` imports call `NativeOutboundHttp` with host-owned identity | Implemented: explicit connection/public destinations, shared credential and egress policy, no internal proxy endpoint; see [the outbound migration plan](outbound-http-host-calls-plan.md) |
| Object Model | Migrating to native SQL imports; schema/CRUD/memory run in the agent | See [the superseding SQL host-call plan](object-model-host-calls-plan.md); no domain host operations |
| Presigning | S3/Azure trusted capabilities execute their agent-owned signers in restricted WASM; the legacy HTTP endpoint is removed | Authorize on the host and invoke the approved built-in provider's `trusted` capability in a restricted instance |

Source anchors:

- [Runtime contract](../crates/runtara-workflow-wit/wit/runtime/runtara-workflow-runtime.wit),
  [runtime implementation](../crates/runtara-environment/src/runtime_host.rs), and
  [compiler ABI choices](../crates/runtara-workflows/src/direct_wasm/component.rs).
- [Resolver host](../crates/runtara-component-host/src/connection_resolver_host.rs)
  and [connection facade](../crates/runtara-connections/src/facade.rs).
- [Guest HTTP client](../crates/runtara-http/src/lib.rs),
  [outbound host interface](../crates/runtara-component-host/src/outbound_http.rs),
  [native outbound service](../crates/runtara-server/src/api/services/outbound_http.rs),
  and [hardened client](../crates/runtara-server/src/egress_client.rs).
- [Object Model agent](../crates/agents/runtara-agent-object-model/src/lib.rs),
  [native database adapter](../crates/runtara-server/src/api/services/database.rs),
  and [services](../crates/runtara-server/src/api/services/object_model.rs).
- [Trusted credential adapter](../crates/runtara-server/src/api/services/trusted.rs)
  and [signing implementations](../crates/runtara-connections/src/auth/mod.rs).

Important existing behavior to carry across:

- Object Model's `database` entitlement and 64 MiB internal request limit are
  installed in [server routing](../crates/runtara-server/src/server.rs), outside
  the ObjectStore. Direct calls must not bypass them. Bulk and SQL limits,
  product events, and raw-SQL audit behavior also need service-level owners.
- Host HTTP currently caps responses at 8 MiB and applies an absolute deadline
  capped at 120 seconds. Native egress must bound upstream body collection,
  not just the final value returned to WASM. Document limits on decoded bytes
  when replacing nested JSON/base64 envelopes; do not silently make them unlimited.
- The [MCP agent](../crates/agents/runtara-agent-mcp/src/lib.rs) has a legacy
  `resolve_connection_params` path that fetches connection parameters over
  internal HTTP. Replace it with explicitly safe metadata; never reproduce a
  raw-parameter resolver in the host ABI.
- Provider-issued URLs are used by Slack and SharePoint without an attached
  connection. Anonymous/public HTTP and signed-URL consumption need an explicit
  governed path; requiring a connection ID on every request would break them.
- Standalone agent testing uses a different store/linker from composed workflow
  execution. Both must receive the same service bindings and policies.

## Contract design

### Ownership and dependency direction

Add a small, WASM-compatible `runtara-host-wit` crate for the new shared WIT
contracts and shared serialized DTOs where dynamic JSON is required. It must
not depend on SQLx, Tokio, Axum, Wasmtime, or credential implementations.
Existing workflow runtime/lifecycle WIT stays in `runtara-workflow-wit`; existing
agent invocation WIT stays in `runtara-agent-wit`. Do not rename stable runtime
interfaces just to put every package under one name.

Define service traits and linker adapters in `runtara-component-host`, following
the existing `RuntimeHost` dependency direction. The component host must not
depend on `runtara-server`, `runtara-connections`, or `runtara-object-store`.

Implement adapters in the server's composition layer and inject them through
the environment runner and component dispatcher. Keep runtime persistence in
`runtara-environment`. Initially extract egress, presigning, and workflow Object
Model orchestration into server service modules; no additional service crate is
needed merely to call them from the host. HTTP handlers translate wire DTOs and
statuses to/from those services.

### Proposed interface surfaces

Names below are proposed contracts, not interfaces that already exist.

| Interface | Operations and data |
| --- | --- |
| Existing runtime/lifecycle | Preserve checkpoint, signal, heartbeat, event, sleep, cancellation, and invoke input/outcome contracts |
| `runtara:connection-resolver/resolver@0.2.0` | Async `describe(connection-id)` and `resolve-resource(connection-id, request)`; safe descriptor and paginated resource page |
| `runtara:outbound-http/client@0.1.0` | Async `request(request)`; explicit destination, headers, raw body bytes, timeout; HTTP status, headers, raw response bytes |
| `runtara:object-model/store@0.1.0` | Async named schema/instance/query/mutation operations with connection ID and operation DTOs |
| `runtara:presigning/signer@0.1.0` | Async facade selecting an approved built-in trusted signer by connection type, returning URL and effective expiry; provider capability calls can use the generic trusted host invocation directly |

Use WIT records/variants for stable controls, connection IDs, HTTP headers,
binary bodies, and errors. Keep dynamic object properties, conditions, SQL
parameters/results, and provider metadata as documented JSON bytes where that
matches existing contracts. Share their DTO definitions rather than allowing
host and guest JSON schemas to diverge. Do not introduce a generic
`invoke-service(name, arbitrary-json)` replacement for all five interfaces.

Object Model initially exposes the operations the agent already needs:

- Get/create schema, including the schema bootstrap used by memory capabilities.
- Create/query/check-exists/create-if-not-exists/update/delete instance.
- Bulk create/update/delete, preserving conflict and validation modes.
- Aggregate, guarded SQL query, and guarded SQL execute.

`load_memory` and `save_memory` remain agent orchestration over these primitives.
Preserve existing behavior and concurrency semantics; atomicity improvements to
multi-call operations are separate changes. Do not expose every administrative
ObjectStore method just because it exists.

Outbound destinations are an explicit variant:

- Connection request: opaque connection ID, relative path or validated absolute
  URL, named endpoint or opaque endpoint reference, and validated provider hints.
- Public request: explicit absolute URL, without stored-credential injection,
  subject to the platform's destination and request policy. This covers ordinary
  unauthenticated HTTP and provider-issued signed URLs.

Carry existing provider selectors, AWS service hints, named endpoints, and
tenant/connection-bound endpoint references as validated fields rather than
magic `X-Runtara-*` headers. Connection-specific credentials and mTLS material
remain native. Public URL handling must preserve signed query strings and never
attach credentials from a different connection.

### Trusted context, policy, errors, and execution

Provide a run-scoped service context containing trusted tenant identity,
optional instance ID, cancellation, active deadline, resource limits, and trace
context. Obtain identity from the launch/test request's authenticated host
context, not guest environment variables, headers, or request JSON. Never infer
a tenant from a connection ID. Every connection lookup verifies ownership.

Use the same service policy decisions from HTTP and host adapters. Preserve
entitlement checks, URL/base-path restrictions, endpoint-reference validation,
DNS/IP restrictions, redirect behavior, auth refresh, signing, mTLS, adaptive
rate limiting, and credential-request accounting. Audit connection resource
discovery's own provider requests as well; routing through a host trait alone
does not establish egress policy coverage.

For new interfaces, define a structured host error with stable code, sanitized
message, category, retryability, and optional retry-after. Reuse the existing
agent error vocabulary where practical. Distinguish policy denial, missing
connection, invalid input, upstream failure, rate limit, timeout, cancellation,
and resource exhaustion. Preserve workflow-facing error codes through guest
adapters; do not rewrite the existing runtime ABI as part of error unification.

Valid upstream HTTP responses, including non-2xx responses, remain responses;
transport and host policy failures are errors. Preserve each agent's current
status-to-error behavior. A timed-out mutation may have reached the provider or
database: do not automatically replay it or treat it as definitely unexecuted.
Keep durable retries in the workflow runtime rather than adding a second host
retry loop. Preserve existing credential refresh behavior separately.

Use async-typed new WIT operations and concurrent host bindings for independent
I/O so one blocked request does not stall sibling Split tasks. Clone the service
handle and context before awaiting; do not hold a Wasmtime store borrow or a
global lock across I/O. Existing runtime operations retain their proven ordering
and durable suspension behavior. Check cancellation/deadlines through connection
lookup, pool acquisition, provider I/O, and response collection; database-side
statement limits remain necessary when dropping a future cannot stop work.

Keep host-side request/response bounds, bounded concurrency, and backend pool
limits. WIT byte lists still allocate/copy; direct calls remove HTTP envelopes,
not every boundary cost. Streaming is a later contract unless a measured need
requires it during migration.

Presigning authorizes creation of a scoped bearer URL. It must validate tenant,
connection, object path, permitted operation, and expiry. Signing keys stay out
of ordinary workflow/agent instances; only an approved built-in capability marked
`trusted` receives its compatible connection credentials in a fresh restricted
instance. Preserve current provider behavior, including expiry clamping and
provider-specific content-type semantics; do not promise an S3 content-type
binding the current signer does not implement. Consumption outside Runtara is
not subject to Runtara's per-request proxy checks. Exclude URL signatures,
credentials, and auth headers from new diagnostic logging.

## Implementation sequence

### 1. Establish contracts and service injection

Add the shared contract crate and host traits. Introduce an injected service
bundle/factory with per-run context, keeping shared connection pools and clients
separate from per-run resolver caches. Thread it through `WorkflowRunSpec`,
`WorkflowState`, the environment runner, and dispatcher `HostState`/`CallContext`.
Cover all workflow execution constructors and standalone agent testing.

Register new bindings without changing old imports. Missing required services
produce clear configuration errors before execution where artifact inspection
allows it; never fall back to unrestricted networking. Fixtures can inject fake
services. Metadata loading must not require live credentials or execute I/O.

Exit: a small real component can invoke an injected fake service in both execution
paths; old artifacts still link; the shared contract crate builds for WASM.

### 2. Extract transport-independent native services

Extract `execute_proxy_request` and its policy/error mapping from Axum-shaped
returns into an egress service. Extract presign orchestration similarly. Move
workflow Object Model orchestration, relevant entitlement enforcement, events,
audit, and limits out of route-only code. Existing HTTP handlers delegate to
these services without changing their public response contracts.

Use `ConnectionsFacade::describe_connection` and
`resolve_connection_resource` as the connection adapter entry points. Extend
safe metadata via explicit integration-owned projections where agents need it.
Do not import host-only connection/database crates into guest components.

Exit: existing handler behavior passes against the extracted services, including
denials, rate limits, SQL guardrails, and errors. Policy logic has one owner.

### 3. Migrate connections and verify runtime consistency

Inject native connection resolution instead of constructing an HTTP resolver
from `WorkflowRunSpec.env`. Add the new async resolver import to compiler WIT,
core import/lowering code, and composition. Preserve per-run cache behavior and
tenant separation. Bind the old resolver version to the same native service
where its signature permits, retaining its legacy synchronous semantics.

Replace the MCP parameter-fetch fallback with safe descriptor metadata for
non-secret tool hints/scope and host-controlled endpoint selection. Keep extra
authentication headers native. Inventory other guest connection lookups, plus
compile-time and agent-testing lookups, before removing URL configuration.

Runtime already uses native calls: retain current versions, checkpoint IDs,
replay rules, signal acknowledgement, terminal outcomes, and suspend/resume
semantics. Preserve production `lifecycle.invoke` and workflow-as-agent behavior;
do not re-enable rejected legacy CLI entrypoints.

Exit: connection-aware workflows and standalone MCP agent tests run without a
connection-service listener or URL in the guest environment; runtime replay and
suspension regression tests remain green.

### 4. Migrate outbound HTTP and presigning

Implement outbound HTTP using the extracted native service. Implement presigning
through the trusted-capability executor in the companion plan, retaining a host
facade for callers that select storage by connection type. Change the WASM
backend of `runtara-http` to use these host paths. Keep its request-builder API
initially to minimize provider-agent churn; translate existing control headers
into contract fields, then migrate first-party callers to explicit methods.
Keep native HTTP-client usage separate from the WASM capability path.

Classify every `.call()`, `.call_agent()`, and `presign()` caller. Internal calls
must migrate to their domain interface; public/signed URL calls use governed
egress. Do not repurpose a missing proxy URL as permission to send directly.
Register equivalent bindings in workflow and standalone agent linkers.

Mark S3/Azure presigning capabilities `trusted` and move their pure signing logic
into WASM-compatible provider code. Their ordinary dispatcher entries forward to
the host; the host invokes the approved component's trusted export in a separate
restricted instance. Preserve provider request bytes, query
encoding, endpoint selection, and status/error behavior. No provider integration
needs its protocol implementation moved wholesale into the host.

Exit: HTTP-agent and provider fixtures, S3/Azure URL generation, Slack upload,
SharePoint signed-URL flows, and parallel Split I/O run with no proxy/presign
listener. Tests assert policies still apply in both execution paths.

### 5. Migrate Object Model

Implement named host operations over the extracted workflow Object Model
service. Replace agent HTTP helpers with host clients while retaining capability
IDs, inputs/outputs, memory schema behavior, bulk semantics, and error mapping.
Keep raw SQL routed through the guarded workflow service methods rather than
unguarded ObjectStore query/execute helpers.

Exit: CRUD, schema bootstrap, memory, aggregation, bulk modes, and raw SQL work
against isolated PostgreSQL without Object Model HTTP routes. Disabled database
entitlements, cross-tenant connections, oversized payloads, read-only query
violations, statement deadlines, and result limits are rejected consistently.

### 6. Switch defaults and retire compatibility dependencies

Rebuild the agent bundle and composed workflows with the new imports. Update WIT
source/templates, compiler composition, build scripts, checksums, sidecars, and
test harnesses through their generators. Never hand-edit generated agent worlds,
metadata, frontend clients, or resolved WIT dependencies.

Inspect real component imports and validate required interface versions at
registration/preparation. Keep existing import names bound while supported
persisted images still require them; a host upgrade cannot rewrite an already
compiled component. Bind compatible old resolver calls natively; isolate legacy
HTTP transport support to legacy artifacts. New artifacts must not acquire the
old generic HTTP import or internal URLs as an escape path.

Deploy compatible hosts before publishing new bundles. Preserve immutable old
artifacts for already-running/suspended executions; drain or explicitly migrate
them before deleting required bindings/routes. Do not silently recompile a
suspended run against different workflow code. Rollback must retain a host that
understands imports already published, not just restore an older binary.

Remove guest dependence on `RUNTARA_HTTP_PROXY_URL`, `RUNTARA_OBJECT_MODEL_URL`,
`CONNECTION_SERVICE_URL`, and runtime HTTP URLs for the migrated artifact profile.
Remove now-unused fields from dispatcher/runner context and configuration only
after checking native tooling, compile-time clients, debug routes, and tests.
Audit the legacy internal-agent route and remaining SDK paths separately before
claiming the entire internal listener can be removed.

Exit: no supported new artifact imports the legacy transport for these surfaces;
the five-interface integration suite runs with the internal listener disabled.
The presign endpoint is already removed; workflows using it require recompilation.
For the remaining migrations, delete compatibility routes/bindings once their supported-consumer inventory
is empty. Public HTTP and MCP APIs continue to call shared services.

## Verification and acceptance

Start with focused contract/service tests, then validate the actual WASM boundary.
Required behavior tests include:

- Old/new import linking; unsupported version errors; standalone agents,
  composed workflows, and published workflow-as-agent components.
- Tenant identity cannot be overridden by guest headers, environment, JSON, or
  forged connection metadata; credentials never appear in safe descriptors.
- Connection ownership, entitlement denials, destination/base-path restrictions,
  forged endpoint references, redirect/DNS rules, rate limits, and signed-URL
  public requests. Resource discovery must follow its declared egress policy.
- Cancellation during I/O, stalled response bodies, body/row/request limits,
  malformed JSON, pool exhaustion, concurrent requests, and bounded memory.
- Retry-after and permanent/transient classification, including no automatic
  replay of an indeterminate mutation; unchanged durable checkpoint/signal and
  lifecycle outcomes after restart/resume.
- Object Model data/results/events and guarded SQL behavior against isolated
  databases. Use deterministic fixtures for HTTP-versus-host comparisons; never
  shadow live mutating requests by executing them twice.
- S3/Azure presign outputs verified by a compatible provider/emulator, with
  injected time where needed. Exercise object/operation/expiry restrictions and
  keep current provider-specific content-type behavior explicit.
- Trusted execution rejects non-built-ins, forged metadata, and incompatible
  connection types before credential resolution. Verify restricted imports,
  per-call instance teardown, secret-free diagnostics, and standalone/workflow
  parity using the companion plan's execution and denial tests.
- A real compiled workflow covering all five surfaces plus standalone agent
  tests with no internal HTTP listener. External provider stubs and isolated
  PostgreSQL/Valkey remain available as required.

Use the pinned toolchains and the current [CI matrix](../.github/workflows/ci.yml).
Relevant commands, run when the corresponding implementation changes land:

```sh
cargo fmt --all -- --check
cargo test -p runtara-host-wit
cargo test -p runtara-workflow-wit
cargo test -p runtara-component-host
cargo test -p runtara-workflows
scripts/build-agent-components.sh
cargo test -p runtara-component-host --features component-integration-tests --tests
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute
cargo test -p runtara-object-store --features db-integration-tests --test integration -- --test-threads=1
cargo test -p runtara-environment --features db-integration-tests -- --test-threads=1
cargo test -p runtara-server --features db-integration-tests,valkey-integration-tests -- --test-threads=1
cargo test -p runtara-connections -- --test-threads=1
```

`runtara-host-wit` is proposed and does not exist yet. Run focused agent/client
tests and clippy for each affected crate as well; use CI's feature-gated lint
matrix for the final cross-crate change. Component integration checks require
the generated bundle and its configured directory. Database/Valkey checks need
isolated services; provider signing validation needs emulators or explicitly
configured test accounts. Report unavailable checks rather than claiming them.
Regenerate the OpenAPI client only if public runtime API contracts change.

Measure equivalent HTTP-backed and native fixtures for latency, allocations,
bytes copied, parallel throughput, and cancellation latency. The architectural
acceptance gate is policy/behavior parity and absence of internal HTTP dependency;
performance gains are measured outcomes, not assumed guarantees.

## Completion criteria

1. All five host surfaces are available consistently to the appropriate workflow
   and agent execution paths, with trusted context injected by the host.
2. New artifacts run in the embedded deployment without internal HTTP for these
   services and without a silent raw-network fallback.
3. Shared native services enforce the same policies for host and API callers;
   workflow-specific SQL and entitlement restrictions remain intact.
4. Capability inputs/outputs and durable execution behavior remain compatible.
5. Supported persisted artifacts have a tested compatibility path, and obsolete
   routes/configuration are removed only after the final consumer is retired.
