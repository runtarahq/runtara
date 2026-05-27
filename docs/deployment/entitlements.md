# Entitlements

RUNTARA ships a per-process entitlement system that gates product features (Reports, Database, API access, MCP), the agent allowlist, and a handful of numeric tier limits. Entitlements are env-driven, resolved once at startup, and enforced on every authenticated entry point — REST, MCP tools, and the internal routes the WASM workflow runtime calls.

This document is the operator-facing reference. For engineering details — the data model, enforcement points, error codes, and the rationale for each design choice — see [`docs/entitlements.md`](../entitlements.md).

## Quick start

The default (no entitlement env set) is **everything enabled, no limits**. New deployments work out of the box without any of the variables on this page. You only need to read further when you want to *restrict* what a tenant can do.

To confirm the resolved snapshot at any time, check the server's startup log. There is one line per boot in the form:

```
INFO  entitlement snapshot resolved
  tenant_id=org_p0IkAFnrVqVOvQw9 pricing_tier=Default
  features_enabled=reports,database,api,mcp features_disabled=
  agents_explicit=false agents_allowlist_size=23
  max_workflows=None max_object_schemas=None max_api_keys=None
  object_model_bulk_request_limit=None max_concurrent_executions=None
```

`features_disabled` is empty and limits are all `None` — that's the all-on default. If anything on this line surprises you, your env is doing something you didn't expect; re-read it before debugging downstream.

## Environment variables

| Var | Required | Notes |
|---|---|---|
| `RUNTARA_PRICING_TIER` | no | Selects a built-in baseline. One of `default`, `starter`, `premium`, `enterprise`. Unset → `default`. Tier definitions are currently placeholders pending product input. |
| `RUNTARA_ENTITLEMENTS_JSON` | no | Per-tenant overrides on top of the tier baseline. Partial; missing keys inherit the baseline. |
| `RUNTARA_ENTITLEMENT_OVERRIDES_JSON` | no | A second, higher-precedence override layer. Same shape as above. Useful for environment-specific tweaks layered on top of a tenant-stable config. |
| `TENANT_ID` | yes | Already required by `AUTH_PROVIDER`; the entitlement system uses the same value to stamp the snapshot. |

### Precedence

Lower-numbered layers are applied first, then overridden by higher-numbered ones:

1. Built-in tier defaults from `RUNTARA_PRICING_TIER`.
2. `RUNTARA_ENTITLEMENTS_JSON`.
3. `RUNTARA_ENTITLEMENT_OVERRIDES_JSON`.

`null` in a JSON layer means **inherit from below**, not "uncap". To remove a cap a lower layer imposes, restate it as a large explicit value — see [`docs/entitlements.md`](../entitlements.md#limit-merge-semantics-current-state) for the rationale.

### JSON shape

All three of `features`, `agents`, and `limits` are optional inside a JSON layer.

```json
{
  "features": {
    "reports": true,
    "database": true,
    "api": true,
    "mcp": true
  },
  "agents": ["http", "csv", "xml", "openai"],
  "limits": {
    "maxWorkflows": 100,
    "maxObjectSchemas": 50,
    "maxApiKeys": 10,
    "objectModelBulkRequestLimit": 1000,
    "maxConcurrentExecutions": 8
  }
}
```

`agents` semantics:

- **Omitted** → all known agent modules are enabled (preserves the default).
- **Present, including `[]`** → exact allowlist. `[]` disables every agent.
- Each ID is validated against the dispatcher's registered agent modules at startup. Unknown IDs fail with `ConfigError::Invalid`.

To discover which agent modules are registered in your build, start the server once with the default config and read the `agents_allowlist_size` field in the startup log — the materialised list of every module is what the implicit-all default expands to.

## Worked example: disable Reports for this tenant

Goal: ship a server where the Reports UI is hidden, the report REST routes return 403, and the report MCP tools refuse.

Start the server with:

```bash
RUNTARA_ENTITLEMENTS_JSON='{"features":{"reports":false}}' \
  cargo run -p runtara-server --features embed-ui
```

(Or via your dev wrapper of choice. The point is the env var on the process.)

### Verify the startup log

The startup log should show:

```
INFO  entitlement snapshot resolved
  ... features_enabled=database,api,mcp features_disabled=reports ...
```

If `features_disabled` doesn't contain `reports`, your env didn't make it to the process — fix that before testing further.

### Verify the REST gate

Public API on `:7001`:

```bash
curl -i -X GET http://localhost:7001/api/runtime/reports
```

Expected:

```
HTTP/1.1 403 Forbidden
content-type: application/json

{"error":"Entitlement required","code":"ENTITLEMENT_REQUIRED","feature":"reports","message":"Reports is not enabled for this tenant."}
```

And in the server log:

```
WARN  entitlement denial
  code=ENTITLEMENT_REQUIRED tenant_id=<your tenant> feature=Some("reports") ...
```

### Verify the SPA

Open the UI. The Reports menu item is hidden. Direct navigation to `/ui/<tenant>/reports` shows the "Feature not enabled" page rather than the report list.

## Public vs. internal API

The server binds **two ports** by default:

| Port | Role | Reached by | Auth | Gated since |
|---|---|---|---|---|
| `7001` (`SERVER_PORT`) | Public API + embedded SPA | Browsers, API-key callers | JWT / API key / proxy header (depends on `AUTH_PROVIDER`) | Phase 3 |
| `7002` (`INTERNAL_PORT`) | Internal routes | WASM workflow binaries on the same machine | None — localhost-only | Phase 5 |

The two share the same entitlement snapshot. A feature you disable applies to both — the gates are mounted on each sub-router individually so a request hitting either port lands at the same `ENTITLEMENT_REQUIRED` body.

Internal routes are bound to `127.0.0.1` by default (`INTERNAL_HOST`). Do not expose this port externally; entitlement gating is the second line of defense, not the first.

## Troubleshooting

### Server refuses to start

Most likely `ConfigError::Invalid` from a malformed env. Expected output looks like:

```
❌ Configuration error: Invalid value for RUNTARA_ENTITLEMENTS_JSON: ...
```

Common shapes:

- **Unknown feature key.** `{"features": {"workflows": false}}` — `workflows` isn't a feature key. Use one of `reports`, `database`, `api`, `mcp`.
- **Unknown agent module.** `{"agents": ["does-not-exist"]}` — the value is validated against the dispatcher's registered modules at startup. Check `sample.json` for the full list, or read the log line at startup that reports the materialised allowlist size.
- **Non-boolean feature value.** `{"features": {"reports": "yes"}}` — features are strict booleans.
- **Negative limit.** `{"limits": {"maxApiKeys": -1}}` — caps must be non-negative.
- **Plain bad JSON.** Missing commas, unquoted keys, etc. Run your value through `jq` first.

### Tenant says "I can't save my workflow"

If the tenant's workflow uses an agent that's no longer in their allowlist, the management plane will reject the save with `AGENT_NOT_ENABLED`. This is intentional: the workflow becomes uneditable until either the entitlement is restored or the forbidden step is removed (which requires temporarily restoring the entitlement). See [`docs/entitlements.md`](../entitlements.md#stale-workflows-after-entitlement-changes-expected-behavior) for the full table of behaviors.

Server-side, you'll see one `WARN entitlement denial code=AGENT_NOT_ENABLED agent=Some("<module>")` line per blocked save attempt — grep for the tenant's id and the agent name to confirm.

### Tenant says "my workflow used to run, now it errors"

Same root cause as above, different surface. A workflow compiled while the agent was allowed keeps its cached binary; when the binary runs and hits `/api/internal/agents/<module>/<capability>`, the entitlement gate denies. The response stays HTTP 200 (by design — preserves the WASM runtime's existing failure envelope) but the body carries `{"success": false, "code": "AGENT_NOT_ENABLED"}`. The execution log will show that error string.

Restoring the agent to the allowlist and restarting the server clears the state — no workflow mutation needed.

### "Reports is hidden in the SPA but the env says it should be enabled"

Two checks:

1. **Hard-reload the browser.** The entitlement snapshot is inlined into `index.html` at serve time; a stale tab from before the restart will still have the previous snapshot.
2. **Confirm the env actually reached the process.** The startup log line is authoritative — if it says `features_disabled=reports`, the SPA is correct and the env was wrong.

## Audit logging

Every entitlement denial — REST, MCP, internal — emits one structured `WARN` line. The fields are split across two layers: ones on the denial event itself, and ones inherited from surrounding tracing spans.

### On the denial event

Always present:

- `code` — one of `ENTITLEMENT_REQUIRED`, `AGENT_NOT_ENABLED`, `ENTITLEMENT_LIMIT_EXCEEDED`. Same stable string the client sees in the 403/MCP error body, so a tenant report with a specific code maps directly to log lines.
- `tenant_id` — the process's configured tenant.

Variant-specific (exactly one set populates per denial):

- `feature` — for `ENTITLEMENT_REQUIRED`.
- `agent` — for `AGENT_NOT_ENABLED`.
- `limit` + `maximum` — for `ENTITLEMENT_LIMIT_EXCEEDED`.

### From surrounding spans (subscriber-dependent)

These travel in parent spans created by other middleware. JSON formatters and OTLP exporters typically flatten them onto each emitted event; the default text formatter does not. If your subscriber doesn't flatten, correlate via request-id instead.

- `method`, `uri` — from `TraceLayer`'s per-request span.
- `user_id`, `auth_method` — from the `request_auth` span the auth middleware wraps every authenticated request future with.
- MCP tool name is not yet on the line — each tool function knows its name but doesn't attach it to a span today. Tracked as a follow-up if denial-line tool attribution becomes a real support need.

### Level + correlation

The denial log is at `WARN` level by design — operators can grep / dashboard for it without lowering log verbosity globally. For long-running diagnostics, join `code` + `tenant_id` (per-line) with `user_id` + `auth_method` + `uri` (parent span) via request-id from the structured-log envelope.

### Privacy note

The audit fields include `tenant_id` by default. If your log sink is shared across tenants and that's a concern, redact at the log filter — RUNTARA does not pre-redact.

## Beyond this doc

- Engineering reference: [`docs/entitlements.md`](../entitlements.md) — data model, enforcement points, full error codes, sub-phase history.
- Authentication: [`docs/deployment/auth-modes.md`](auth-modes.md) — `AUTH_PROVIDER` and related env. Pairs with entitlements (auth answers "who", entitlements answer "what can they do").
