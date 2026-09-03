# HTTP Egress Fallback Build-Gate Planning Brief

## Objective

Make proxyless HTTP fallback an explicit, opt-in build choice for all
Runtara-managed agent egress—not a behavior hidden behind the misleading
`call_agent()` name.

By default, a component or native agent making a policy-managed outbound HTTP
request must use the internal HTTP proxy. If no proxy is configured, the
request must fail before any direct HTTP request is attempted.

## Current State

`runtara-http::RequestBuilder::call_agent()` does not invoke an agent. It is a
shared outbound-HTTP helper used by the generic HTTP agent and provider agents
to call third-party APIs.

Today it behaves as follows:

```text
RUNTARA_HTTP_PROXY_URL configured  -> forward request to the internal proxy
RUNTARA_HTTP_PROXY_URL absent       -> call() directly
```

For a WASM component, that direct branch uses `wasi:http/outgoing-handler`.
For a native caller, it uses the native HTTP backend. The fallback currently
affects all callers of `call_agent()`, including credentialed provider API
calls and connectionless signed-URL transfers.

## Naming Decision Required

`call_agent()` is misleading because it describes neither the transport nor
the operation: it does not invoke an agent. The API needs distinct, clear names
for:

- an explicit direct target request; and
- a policy-managed outbound request that uses the internal proxy by default.

The exact public API names, compatibility aliases, deprecation period, and
caller-migration strategy are deliberately not decided by this brief. They
must be agreed separately before implementation.

## Proposed Default Behavior

The policy-managed egress request resolves the proxy URL once and then follows
this policy:

```text
proxy URL configured       -> forward_to_proxy()
proxy URL absent, default  -> ProxyNotConfigured error; do not call direct HTTP
proxy URL absent, opt-in   -> execute a direct request for legacy local/development use
```

`ProxyNotConfigured` is a distinct `HttpError` variant. Agent-facing adapters
must convert it to a permanent, actionable error such as:

```text
EGRESS_PROXY_REQUIRED: Outbound agent HTTP requires RUNTARA_HTTP_PROXY_URL; this build disallows direct HTTP fallback.
```

A configured but unreachable proxy is a transport failure, not a missing
configuration error. It must not trigger direct fallback in either build.

## Build Argument Contract

The build-time Boolean is:

```text
RUNTARA_BUILD_ALLOW_DIRECT_EGRESS_FALLBACK=false
```

| Build input | Policy-managed egress when no proxy URL is configured |
|---|---|
| Unset or `false` | Returns `ProxyNotConfigured`; no native or WASI direct request. |
| `true` | Permits the historical direct fallback for controlled local/development builds. |

The implementation maps this input to a default-off Cargo feature on
`runtara-http`, named `allow-direct-egress-fallback`. The feature must be
enabled consistently in both native and WASM artifacts; the confirmed scope is
all traffic currently using `call_agent()`, not only the generic HTTP agent.

`scripts/build-agent-components.sh` should accept
`--allow-direct-egress-fallback`, and `scripts/build-bundle.sh` should forward
the same option. The bundle manifest should record the selected policy. A
prebuilt bundle cannot change this behavior at deployment time, and
`--skip-build` cannot retroactively alter it.

The current Dockerfile only packages a prebuilt bundle. A Docker `ARG` alone
therefore cannot select this policy; a source-building Dockerfile, if added,
must forward its argument to the same bundle-build input.

## Scope

This policy covers every Runtara-owned caller of the policy-managed egress API
across both component and native execution. That includes the generic HTTP
agent, provider agents, shared S3 client flows, AI-provider proxy flows, and
signed-URL transfers that currently use `call_agent()`.

The policy does not change deliberately explicit direct-request uses. It also
cannot prevent an independently authored WASM component from importing
`wasi:http` directly; enforcing that broader platform guarantee requires a
separate component-host capability/linker policy.

## Compatibility and Deployment Requirements

- Managed workflow/component runs already receive the proxy URL and should
  continue to work without behavioral change.
- Standalone, local, and misconfigured runs will fail closed in the default
  build. Operators who intentionally need direct development egress must use
  the opt-in build variant.
- Native shared callers must consume the server's resolved proxy configuration
  or explicitly require `RUNTARA_HTTP_PROXY_URL`; they must not accidentally
  bypass the policy because they read a different configuration source.
- Do not treat a missing proxy as retryable `NETWORK_ERROR`.
- Preserve the proxy's URL validation, credential injection, signing,
  redirect, DNS-rebinding, private-address, rate-limit, and audit behavior.

## Verification

- Default native and WASM builds: a policy-managed egress request with no
  proxy URL returns `ProxyNotConfigured` and produces no outbound request.
- Opt-in native and WASM builds: the same no-proxy request uses the legacy
  direct backend.
- Both build variants: a configured proxy receives the envelope request; the
  target is never contacted directly by the component.
- Verify permanent agent error mapping for the missing-proxy case and
  transient classification for an unreachable configured proxy.
- Exercise at least the generic HTTP agent, a connection-bound provider agent,
  a signed-URL transfer, and a native shared S3-client path.
- Verify the release bundle defaults to the fail-closed variant and that its
  manifest records the chosen policy.
- Add API-level tests once the naming and compatibility strategy is decided.

## Exit Criteria

- Default release artifacts contain no implicit direct fallback from the
  policy-managed egress API.
- The agreed API names make direct and policy-managed transport distinguishable
  during review.
- The build and bundle paths make the exceptional fallback variant deliberate
  and auditable.
