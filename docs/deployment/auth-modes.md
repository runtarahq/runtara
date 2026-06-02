# Authentication modes

RUNTARA ships three deploy-time authentication modes. Pick one with the `AUTH_PROVIDER` environment variable; all other auth-related env vars are specific to the chosen mode.

| `AUTH_PROVIDER` | Who validates the caller | When to use |
|---|---|---|
| `oidc` (default) | RUNTARA, via OIDC discovery + JWKS | You want RUNTARA to act as a relying-party against a hosted IdP (Auth0, Okta, Entra ID, Keycloak Cloud, Zitadel Cloud, etc.). |
| `trust_proxy` | Your reverse proxy (nginx, Caddy, Traefik, oauth2-proxy) | You already run an IdP and want to terminate authentication at the perimeter. |
| `local` | Nobody | Single-user airgapped / developer workstation. No user attribution. |

## Shared requirements

`TENANT_ID` is required in every mode — RUNTARA is single-tenant per process and will panic at startup if it is unset.

RUNTARA-issued API keys (`rt_*` / `smo_*` prefixes) continue to work in every mode. They are validated against the local database independently of the provider, so operators always have a direct-access path that does not depend on the perimeter.

Auth answers "who is calling"; **entitlements answer "what they can do"**. Both are env-driven and resolve at startup. See [`entitlements.md`](entitlements.md) for the feature gates, agent allowlist, and tier limits — common questions like "why is the Reports menu hidden" or "why does this workflow fail with `AGENT_NOT_ENABLED`" live there.

For SaaS multi-tenant deployments, per-user **roles and permissions** (Owner / Admin / Member / Viewer) are resolved per request from the tenant's Valkey rather than from the JWT. The cross-service contract — JWT claim shape, Valkey key schema, the static permission map, and the audit-event shape — is specified in [`../security/user-management-contracts.md`](../security/user-management-contracts.md).

The MCP Streamable HTTP endpoint validates the inbound `Host` header before MCP auth and tool dispatch. Local loopback hosts are allowed by default. Public or proxied deployments must set `RUNTARA_MCP_ALLOWED_HOSTS` to the comma-separated public host authorities clients use, for example `runtara.example.com,runtara.example.com:7001`.

MCP session recovery uses Valkey by default (`RUNTARA_MCP_SESSION_STORE=valkey`). Valkey mode requires `VALKEY_HOST` and a working shared Valkey connection at startup; the server exits instead of falling back to process-local sessions. Set `RUNTARA_MCP_SESSION_STORE=local` only for explicit single-process development. `RUNTARA_MCP_SESSION_TTL_SECONDS` controls the persisted recovery-state TTL and defaults to `86400`.

## `AUTH_PROVIDER=oidc`

Backwards-compatible with the previous RUNTARA behaviour.

| Var | Required | Notes |
|---|---|---|
| `OAUTH2_JWKS_URI` | yes | RUNTARA fetches this on startup and refreshes hourly. Unreachable → server exits. |
| `OAUTH2_ISSUER` | yes | Validated against the `iss` claim. |
| `OAUTH2_AUDIENCE` | no | If set, validated against the `aud` claim on API routes. |
| `OAUTH2_MCP_AUDIENCE` | no | Same, but for MCP routes. |
| `RUNTARA_AUTH_REQUIRE_JTI` | no | When `true`, reject any JWT lacking a `jti` claim. Defaults to `false` for backwards compatibility; flip to `true` once the Auth0 Action emits `jti` on every token, since `jti` is the key the revocation denylist relies on. See [`../security/user-management-contracts.md`](../security/user-management-contracts.md). |
| `RUNTARA_AUTH_MEMBERSHIP_POLICY` | no | `disabled` \| `logging` \| `required` — how the per-tenant Valkey membership/revocation lookup is treated. Default: `logging` for `oidc` when Valkey is configured, else `disabled`. `required` fails requests closed on missing membership, revoked token, or unreachable Valkey. Enforcement lands with the SYN-437 rollout. |

The runtime expects every JWT to carry an `org_id` claim equal to `TENANT_ID`; a mismatch returns `403 Forbidden`. `sub` becomes `AuthContext.user_id`. Auth0-namespaced custom claims (e.g. `https://runtara.io/org_id`) are accepted and normalized to their raw names.

## `AUTH_PROVIDER=trust_proxy`

RUNTARA performs no in-process authentication. A reverse proxy is expected to:

1. Terminate TLS and authenticate the caller against whatever IdP the customer already operates.
2. **Strip every `Authorization`, `X-Forwarded-*`, and `X-User-*` header the client sent.**
3. Forward the validated end-user identity in `X-Forwarded-User` (or a custom header — see below).
4. Proxy to RUNTARA on a loopback address.

| Var | Required | Notes |
|---|---|---|
| `TRUST_PROXY_USER_HEADER` | no | Override the header name. Default `x-forwarded-user`. Case-insensitive. |
| `SERVER_HOST` | must be loopback | `127.0.0.1`, `::1`, or `localhost`. Enforced at startup. |

If the user header is absent, `AuthContext.user_id` falls back to the literal `"proxy"` so audit logs still record that a proxy-terminated request landed.

Reference configs: [`docs/reference/proxy/`](../reference/proxy/).

## `AUTH_PROVIDER=local`

No authentication at all. Every request is served as the configured tenant with `user_id = "local"`. This is intended for:

- Single-user airgapped deployments where perimeter auth is physical (the box is on a closed network).
- Developer workstations and local CI.

`SERVER_HOST` must be a loopback address; RUNTARA refuses to start otherwise.

## Startup safety check

In `local` and `trust_proxy` modes, RUNTARA validates the public bind address before opening the listener. Example error on misconfiguration:

```
❌ Configuration error: AUTH_PROVIDER=trust_proxy requires SERVER_HOST to be a
   loopback address (127.0.0.1, ::1, or localhost); got '0.0.0.0'. Unauthenticated
   modes must not accept non-local connections — bind RUNTARA to loopback and put
   a reverse proxy in front of it. See docs/deployment/auth-modes.md and
   docs/reference/proxy/.
```

The process exits with status `1` so systemd / container orchestrators surface a clean failure instead of silently exposing an unauthenticated listener.
