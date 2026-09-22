# Outbound HTTP without an internal HTTP hop

Status: implemented and locally verified (2026-09-19). Verification and the
unrun platform-specific check are recorded below. This supersedes
the outbound HTTP portion of [the broader host-interface plan](wasm-host-interfaces-plan.md).

## Decision

Replace the local proxy request with a direct call to an injected native outbound
HTTP service:

```text
Before: agent -> host-io HTTP import -> local HTTP proxy -> external HTTP(S)
After:  agent -> outbound HTTP import -> native outbound service -> external HTTP(S)
```

Reuse the existing connection authentication, OAuth refresh, request signing,
destination checks, mTLS, and rate limiting. This is a transport migration, not
a new policy engine. Provider request construction remains in agents. External
HTTP(S), OAuth token exchange/callbacks, public APIs, and administrative APIs stay
as they are. Trusted WASM presigning is unchanged.

Remove `/api/internal/proxy`; do not retain a legacy endpoint or silently fall
back to it. The internal listener still serves connection administration and
other routes: removing this hop does not remove that listener wholesale.

## Migration baseline

| Location | Previous responsibility | Change |
| --- | --- | --- |
| `runtara-http/src/lib.rs` | `call_agent[_async]` reads proxy/tenant environment variables, extracts control headers, creates a proxy JSON request, and decodes its response | Construct one outbound request with explicit fields |
| `runtara-http/src/host_io.rs` | Encodes that request again as JSON/base64 for `runtara:host-io/http@0.1.0` | Use the new typed outbound import with raw body bytes |
| `runtara-component-host/src/host_io.rs` | Concurrent host import performs an HTTP request to the proxy | Inject/call the native outbound service; keep the independent timer bindings |
| `runtara-server/src/api/handlers/internal_proxy.rs` | `execute_proxy_request` already contains most outbound behavior, but accepts/returns Axum-shaped DTOs | Extract that implementation into a transport-independent server service |
| `runtara-connections` and `runtara-server/src/egress_client.rs` | Credential resolution, OAuth, signing, mTLS, pooled clients and DNS/redirect policy | Reuse them directly |

The existing concurrent host import is important: it suspends only the calling
guest task. Do not replace it with blocking I/O or a store-wide WASI HTTP wait.

## Interface

Introduce `runtara:outbound-http/client@0.1.0` with one async `request` operation.
Use WIT records/variants for stable fields and `list<u8>` for body bytes, without
JSON/base64 transport envelopes. Put the canonical WIT in the existing
`runtara-workflow-wit` package, following the connection/database interface
registration pattern. Generate bindings for `runtara-http` and the host from the
same source; use the existing WIT dependency tooling for resolved dependencies.
No new service framework or generic dispatch interface is needed.

Conceptually:

```text
request({ destination, method, headers, body?, timeout-ms?, max-response-bytes? })
  -> result<{ status, headers, body }, outbound-error>

destination =
  connection { connection-id, url-or-path, endpoint?, endpoint-ref?,
               ai-provider?, aws-service? }
  | public { url }
```

- Connection IDs are opaque. Preserve relative paths, validated absolute URLs,
  named endpoints, Teams endpoint references, AI-provider compatibility, and AWS
  service selection. These become explicit controls, not `X-Runtara-*` headers.
- Public requests cover ordinary connectionless HTTP and provider-issued signed
  URLs used by agents such as Slack/SharePoint. They use the same native egress
  service without stored-credential injection. Do not require a connection ID
  for these requests or rewrite signed query strings.
- Tenant, instance, trace attribution, active deadline, and restriction state
  come from the host's execution context. Guest headers/environment/request
  fields do not establish authority. Upstream headers and host identity are
  separate concepts.
- Headers are ordered name/value pairs in WIT. Preserve the existing public
  `HttpResponse` mapping and header lookup behavior at the client adapter; test
  duplicate-header and credential-header precedence explicitly.
- Preserve absent versus empty body, query encoding, binary bytes, and JSON
  serialization/content-type behavior. Serialize a JSON body once, before
  signing/sending those same bytes.
- Upstream statuses, including 4xx/5xx, are HTTP responses. Keep synthetic
  preflight 429 responses and their retry headers. Connection/auth/transport
  failures have stable, sanitized error codes; retain current agent-facing
  status/retry behavior through an explicit adapter rather than changing retry
  classification as a side effect of removing HTTP.

## Native service and injection

1. Add an `OutboundHttpHost` trait and concurrent linker adapter to
   `runtara-component-host`, using generated contract types. The component host
   must not depend on the server or credential implementation.
2. Extract `execute_proxy_request` and its helpers into a server service, for
   example `api/services/outbound_http.rs`. Remove Axum `Json`/`StatusCode` from
   its internal result contract. Reuse the existing shared reqwest client,
   connection facade, mTLS clients, signing functions, rate-limit accounting,
   named-endpoint/ref handling, and URL/DNS/redirect behavior. Move the tests with
   the logic. Do not copy it into the component host.
3. Inject the service beside the existing database and connection services in
   standalone dispatcher calls, interactive agent testing, `WorkflowRunSpec` /
   `WorkflowState`, embedded runners, scoped child Stores, and workflows used as
   agents. Shared clients/pools live at service scope; identity belongs to each
   invocation.
4. Restricted trusted instances deny outbound requests before credential lookup
   or I/O. Metadata enumeration needs no live outbound backend; attempting I/O
   there fails explicitly. Missing service/tenant cannot select a direct-network
   fallback.

OAuth acquisition/refresh and outgoing AWS/Azure request signing remain native
and use the existing implementations. They are distinct from presigned URL
generation in trusted agents. Connection resource resolution already makes
native provider requests and does not need a replacement HTTP hop.

## Execution behavior

Apply one absolute deadline to connection lookup, OAuth refresh/rate-limit
checks, DNS/connect/TLS, sending, response headers, and consuming the body. Use
the minimum of the active execution deadline and the request deadline. Preserve
the current proxied-call default of 30 seconds and the existing 120-second host
ceiling; do not accidentally switch all requests to host-io's 120-second default.

Keep `func_wrap_concurrent`/Component Model async cancellation. Dropping the
guest task must stop the awaited request/body stream, without serializing sibling
Split tasks or keeping detached request work alive. Preserve the existing retry
ownership in agents; the host adds no automatic request retry. A timeout after
sending a mutation is not proof the provider did not apply it.

Carry forward the nominal 64 MiB request and 8 MiB response budgets as explicit
native-boundary byte budgets, including metadata. Check guest lengths before
copying large bodies/headers; bound streamed response collection even without a
truthful Content-Length. The old limits measured nested encoded envelopes, so
removing base64/duplicate JSON changes the effective payload capacity. Document
and test the new raw-byte accounting rather than treating the previous overhead
as an API contract. Keep the existing 5 MiB agent download limit in this change.

Retain existing destination/credential policy; do not add new allowlists,
provider restrictions, or payload parsing. Do not introduce a raw networking
escape hatch: WASM `call`, `call_async`, `call_agent`, and `call_agent_async` all
use the same service, with explicit public/connection destinations. Raw
`wasi:http` remains denied.

## Implementation sequence

1. **Characterize the existing behavior.** Capture method/URL/query/body/header
   handling, status/error mapping, defaults, rate-limit replies, and binary and
   JSON responses. Cover auth variants and connectionless signed URLs. Use local
   mock providers and synthetic credentials; no shadowing live writes.
2. **Extract the native service.** Move current logic and tests, preserving
   behavior. The old handler can delegate during development, but is deleted
   before the migration is delivered. Update the mTLS integration target to test
   the service directly. Public callers, if found, remain thin authenticated
   adapters with their existing contracts.
3. **Add the host contract and all injection paths.** Bind an injected fake
   service first, then the real service. Verify authoritative identity, scoped
   children, restrictions, concurrency, deadlines, and cancellation with actual
   components. Keep `runtara:host-io/timers@0.1.0` unchanged.
4. **Migrate the HTTP client and callers.** Give `RequestBuilder` explicit
   connection/endpoint/provider/response-limit setters. Update agents, AI/MCP
   helpers, download helpers, and tests that currently set magic headers. Keep
   workflow-facing capability schemas/output shapes stable. The native SDK's
   ordinary direct HTTP client can remain; native connection-aware execution
   must use an explicitly supplied service or fail clearly, never depend on a
   removed endpoint or silently send an unauthenticated request.
5. **Remove the hop and obsolete wiring.** Delete the proxy route/handler DTOs,
   proxy wrapping/decoding, old HTTP host transport, `RUNTARA_HTTP_PROXY_URL`,
   `CallContext.proxy_url/proxy_host`, dispatcher/config/runtime-client injection,
   and proxy-address fixtures. Retain any policy configuration still consumed by
   the native service; do not combine this with a configuration rename. Update
   docs and the removed-bridge E2E test to expect 404 for the proxy route too.
6. **Rebuild and verify together.** Rebuild all agents/shared components and
   recompose workflows. Remove the old `runtara:host-io/http@0.1.0` binding from
   the new host, retaining timers. Old HTTP-importing artifacts require rebuild;
   do not silently reinterpret their old envelopes. Drain old running/suspended
   artifacts on their matching release or explicitly restart them. Roll back
   matching host/component bundles together.

## Acceptance and tests

- A real HTTP agent, a provider agent, AI, remote MCP, and a download run through
  the injected service with no internal listener and no proxy URL configured.
  Network traffic goes to mock upstream providers only.
- Both connection and public/signed-URL destinations work. Bodies and signed
  URLs preserve their bytes; query strings, content types, response headers,
  statuses, and provider-specific error/retry behavior match the fixtures.
- Existing OAuth refresh/write-back, auth-header injection, AWS/Azure outgoing
  signing, mTLS, destination pinning, endpoint refs, DNS/redirect restrictions,
  and synthetic/upstream rate-limit cases pass through the new service.
- Forged guest tenant/env/headers do not change authority; connection ownership
  is checked. Restricted trusted instances and missing-service contexts perform
  no network or credential access. There is no old HTTP or WASI HTTP bypass.
- Actual composed parallel branches overlap I/O. Cancellation/deadline tests
  cover lookup/auth, pending headers, trickled bodies, and oversized responses,
  then prove the runner/client can be reused. Parent/child and workflow-as-agent
  paths use the same context and service.
- `/api/internal/proxy` returns 404. Inspect rebuilt imports for the new outbound
  interface and absence of the old HTTP transport. Search source/config/tests
  for leftover proxy URL injection and wrapping. Administrative/public HTTP
  routes and OAuth callbacks continue to work.

Run pinned-toolchain formatting and strict affected-crate Clippy; focused
`runtara-http` native/WASM client tests; server outbound/mTLS/provider tests with
their CI feature gates and isolated fixtures; `scripts/build-agent-components.sh`;
the component-host integration suite; direct-WASM and deadline suites; and
serialized scoped-runner/cooperative-stop tests from CI. Verify actual imports
and the listener-free E2E separately from unit tests. Report unrun checks rather
than treating a build as end-to-end evidence.

Acceptance: agent HTTP reaches the native service through a host function, all
existing outbound behavior is accounted for, and no workflow execution requires
the local HTTP proxy.


## Implementation and verification

The canonical contract is in
[`runtara-outbound-http.wit`](../crates/runtara-workflow-wit/wit/outbound-http/runtara-outbound-http.wit).
The [concurrent host adapter](../crates/runtara-component-host/src/outbound_http.rs)
calls the injected [native service](../crates/runtara-server/src/api/services/outbound_http.rs).
Dispatchers, embedded workflows, and scoped children receive that service with
host-established tenant/instance context. All guest HTTP entry points use the
same contract; the old route, envelope, import, and proxy-URL injection are gone.

Verified with the pinned Rust toolchain and rebuilt components:

- `scripts/build-agent-components.sh`: all 26 agents and both shared components.
  Import inspection of all 28 found 17 new outbound imports and zero old HTTP
  imports. Composed workflow tests recompose against this bundle.
- Component-host integration suite: 255 passed; one manual capacity soak ignored.
  Real HTTP, provider, AI, MCP, and download components use injected services
  without an internal listener. Restricted, missing-backend, identity, and
  cancellation paths are covered.
- `direct_wasm_execute` with `direct-wasm-integration-tests`: 400 passed;
  three manual measurements ignored. Feature-gated `agent_deadline_tests`:
  all 119 passed, including overlapping I/O, lookup/preparation, body cancellation,
  retries, parent/child execution, and reuse.
- Serialized scoped-runner/cooperative-stop tests: all 13 passed against an
  isolated runtime database, including authoritative root identity despite
  forged guest environment.
- Server `outbound_http` with `db-integration-tests`, `valkey-integration-tests`,
  and `component-integration-tests`: all nine passed against isolated PostgreSQL,
  Valkey, and local providers. These cover actual WASM credentialed HTTP, OAuth
  refresh cancellation/rotation/reuse, exact binary AWS/Azure signing, mTLS,
  signed public URLs, URL pinning, endpoint references, synthetic/upstream 429s,
  lookup/body deadlines, redirects, and response bounds.
- Extracted service regression tests: all 30 passed, including named endpoints,
  endpoint-reference validation, credential errors, and streaming limits.
- Native/WASM HTTP adapter unit tests: 15/17 passed. WIT tests: eight passed.
  Formatting, diff whitespace checks, and strict affected-crate all-target
  Clippy passed with the integration feature gates, including the new server
  component gate.
- Live isolated server: `/api/internal/proxy` returns 404; removed dispatch,
  presign, and Object Model bridges also remain absent. Public catalogs,
  connection administration, and the OAuth callback remain available.
- Source/config/test searches found no remaining proxy URL injection or old
  request wrapping. Historical migration prose and explicit 404 assertions
  still name the removed route/import.

CI runs the real WASM/native-provider test in the component-build job with a
separate server-schema database. Ordinary native provider tests remain in the
server job; the component feature prevents an undeclared artifact dependency.

Not run: the full `e2e/test_workflow_attachments.py` script on Linux. Its migrated
local HTTPS fixture requires Linux/OpenSSL process-local CA trust; only syntax
was checked on macOS. Component attachment tests, native provider tests, and
live route checks above passed. No deployment or manual capacity soak was run.
