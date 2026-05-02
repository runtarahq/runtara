---
name: inspect-connection
description: Use when a workflow that uses a connection misbehaves — to see stored params (secrets masked), token state, rate-limit history, and which integration it belongs to. Catches stale OAuth tokens and misconfigured connection params before you blame the agent.
---

# Inspect a connection

Pulls connection state from the runtime API (port 7001). Connections are how integrations get credentials at runtime; if a HubSpot/Shopify/etc. agent is failing, this is usually the first thing to check.

## 1. Find the connection

By integration:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/connections?integrationId=<integration_id>" \
  | jq '.items[] | {id, name, integrationId, status, createdAt, updatedAt}'
```

Examples of `integration_id`: `shopify_access_token`, `openai_api_key`, `hubspot_private_app`, `mailgun`. Full list at [crates/runtara-agents/src/agents/integrations/connection_types.rs](../../../crates/runtara-agents/src/agents/integrations/connection_types.rs).

By status (find broken ones):

```bash
curl -s "http://127.0.0.1:7001/api/runtime/connections?status=REQUIRES_RECONNECTION" | jq '.items'
curl -s "http://127.0.0.1:7001/api/runtime/connections?status=INVALID_CREDENTIALS" | jq '.items'
```

By operator (which connections does this agent use):

```bash
curl -s "http://127.0.0.1:7001/api/runtime/connections/operator/<agent_module_name>" | jq '.items'
```

## 2. Get a specific connection

```bash
curl -s "http://127.0.0.1:7001/api/runtime/connections/<CONNECTION_ID>" | jq .
```

What to look at:
- `status` — `ACTIVE` is good; `REQUIRES_RECONNECTION` means the OAuth token expired and the user needs to redo the auth dance; `INVALID_CREDENTIALS` means the stored creds were rejected
- `parameters` — secret fields will be masked (`***`); non-secret fields like `shop_domain`, `base_url`, `region` are visible — verify they're what you expect
- `tokenExpiresAt` — for OAuth connections, when the access token expires
- `lastRefreshedAt` — when the runtime last exchanged the refresh token

## 3. Rate-limit history (if you suspect throttling)

```bash
curl -s "http://127.0.0.1:7001/api/runtime/connections/<CONNECTION_ID>/rate-limit-history?limit=50" \
  | jq '.events[] | {createdAt, event_type, payload}'
```

Filter to a window:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/connections/<CONNECTION_ID>/rate-limit-history?from=2026-05-01T00:00:00Z&to=2026-05-02T00:00:00Z" | jq .
```

For aggregate stats, ask the list endpoint:

```bash
curl -s "http://127.0.0.1:7001/api/runtime/connections?includeRateLimitStats=true&interval=24h" \
  | jq '.items[] | {id, name, rateLimitStats}'
```

## 4. Verify the connection schema is what you think

If you just added an integration and it's not showing the right fields:

```bash
# list all registered integration types
curl -s "http://127.0.0.1:7001/api/runtime/connections/types" | jq '.items[] | {integrationId, displayName, category, authType}'

# get a specific one's schema
curl -s "http://127.0.0.1:7001/api/runtime/connections/types/<integration_id>" | jq .
```

If your new integration isn't listed: the `inventory::submit!` for the extractor might be gated behind `#[cfg(not(target_family = "wasm"))]` and you forgot to declare it on the native side, or the agent module isn't re-exported in [lib.rs](../../../crates/runtara-agents/src/lib.rs). Re-check `add-integration` step 4.

## Common diagnostic patterns

**"Workflow fails with `MISSING_FIELD: domain` (or similar)"** → the connection was created with an old schema; the params struct was updated since. Recreate the connection.

**"OAuth integration suddenly returning 401s"** → check `status` and `tokenExpiresAt`. If expired and refresh failed, the user needs to re-authorize through the UI.

**"Connection says ACTIVE but the call fails"** → confirm the extractor's `url_prefix` and headers match what the upstream API actually expects. The extractor lives next to the params struct in [connection_types.rs](../../../crates/runtara-agents/src/agents/integrations/connection_types.rs); test it independently by checking `parameters` and walking through what the extractor would build.

**"Secret value seems wrong but I can't see it"** → masked by design. Recreate the connection with a known value or add a one-off log line in the extractor (and remove it before commit).
