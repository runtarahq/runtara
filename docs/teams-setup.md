# Microsoft Teams Bot — Setup

This guide connects a Microsoft Teams bot to Runtara for channel triggers
(inbound messages start/continue a workflow session) and the `teams.send-message`
workflow capability. It covers the public Microsoft cloud only; Government/GCC
clouds are not yet supported.

## 1. Register an Azure Bot (single-tenant)

1. In the Azure portal, create an **Azure Bot** resource.
2. For **Type of App**, choose **Single Tenant** (Microsoft deprecated creation
   of multi-tenant bots after 2025-07-31). Note the **App ID** and the
   **Microsoft Entra Tenant ID** of the app registration.
3. Under the app registration's **Certificates & secrets**, create a **client
   secret** — this is the bot's *App Password*.
4. Under the Azure Bot's **Configuration**, set the **Messaging endpoint** to
   your Runtara webhook URL (see step 3).

## 2. Create the Runtara connection

Create a connection of type **Microsoft Teams Bot** (`teams_bot`):

- **App ID** — the Azure Bot App ID.
- **App Password** — the client secret.
- **Tenant ID** — the Microsoft Entra tenant ID (required for single-tenant).
- **App Type** — `Single Tenant` (default). Use `Multi Tenant` only for a legacy
  multi-tenant registration.

No serviceUrl is configured: it is per-conversation and captured automatically
from authenticated inbound activities.

## 3. Wire the webhook

Create a **Channel** trigger and select the Teams connection. Runtara shows the
webhook URL. The externally advertised form is:

```
{WEBHOOK_BASE_URL}/api/events/{tenant_id}/webhook/teams/{connection_id}
```

The public gateway rewrites `/api/events/{tenant}/webhook/{platform}/{id}` to the
served `/api/runtime/events/webhook/{platform}/{id}` via a single `{platform}`
wildcard rule — no Teams-specific gateway entry is required. Paste the advertised
URL into the Azure Bot's **Messaging endpoint**.

Teams imposes no URL shape; the path convention is Runtara's own.

## 4. Install the app in Teams

Package the bot into a Teams app (see `docs/teams-app-manifest.example.json`) and
install it into the target personal chat, group chat, or team. In group and team
scopes the bot only receives messages that @mention it (RSC-based all-message
receipt is not part of this release).

## Security model (how it works)

- **Inbound**: every activity's Bot Framework JWT is validated (RS256 pinned,
  issuer/audience/`exp`+`nbf`, `serviceurl` claim vs the activity, `msteams`
  channel endorsement for Bot Framework tokens, and the connection's tenant).
  Failures return HTTP 403. Nothing from an unauthenticated activity is stored.
  The webhook **acks fast**: it validates, returns 200 within Teams' ~15s retry
  window, then processes on a background task where the redelivery dedup runs
  (a failed handoff releases the reservation so the redelivery is not lost). The
  JWKS fetch is bounded by a timeout, single-flighted per authority, and
  negative-caches unknown `kid`s so a bogus-token flood cannot hammer Microsoft.
- **Outbound**: the bot token is minted through Runtara's connection-auth token
  cache (single-tenant authority) — the workflow/agent never sees the secret.
  The per-conversation serviceUrl is delivered to the `teams.send-message`
  capability as an opaque signed *endpoint ref* in the trigger's `data.target`;
  the credential proxy verifies it (tenant + connection match, exact
  conversation path segment) and pins egress to that serviceUrl. A ref is only
  minted for a serviceUrl on a public Bot Connector host, and the connection's
  `authority_host` must be exactly the public Microsoft cloud — both close
  credential-exfiltration paths.

## Environment

- `RUNTARA_ENDPOINT_REF_SECRET` — HMAC key that signs conversation targets.
  Required for the `teams.send-message` workflow path (server-side session
  replies work without it). Rotate by moving the old value to
  `RUNTARA_ENDPOINT_REF_SECRET_PREV`. The webhook logs a one-time warning if it
  is unset while Teams activities arrive.
- `RUNTARA_TEAMS_OPENID_CONFIG_URL` / `RUNTARA_TEAMS_ENTRA_OPENID_URL_TEMPLATE`
  — override the Bot Framework / Entra OpenID metadata endpoints. Production
  defaults to Microsoft; tests point these at a mock authority.
- `RUNTARA_TEAMS_ALLOW_INSECURE_SERVICE_URL` — **testing only.** Permits an
  endpoint ref to be minted for a `127.0.0.1`/`localhost` serviceUrl (a mock Bot
  Connector). Never set in production; real serviceUrls are always public Bot
  Connector hosts.

## Not in this release

Reactions, message update/delete, files/inline images, Adaptive Card
interactivity (`Action.Execute`/`Action.Submit`), proactive create-conversation,
RSC all-message ingestion, and historical reads (a separate Microsoft Graph
concern). Adaptive Cards can be **sent** (as an attachment) but card actions are
not handled.
