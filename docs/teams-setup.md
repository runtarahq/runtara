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
  issuer/audience/expiry, `serviceurl` claim vs the activity, `msteams` channel
  endorsement for Bot Framework tokens, and the connection's tenant). Failures
  return HTTP 403. Nothing from an unauthenticated activity is stored.
- **Outbound**: the bot token is minted through Runtara's connection-auth token
  cache (single-tenant authority) — the workflow/agent never sees the secret.
  The per-conversation serviceUrl is delivered to the `teams.send-message`
  capability as an opaque signed *endpoint ref* in the trigger's `data.target`;
  the credential proxy verifies it and pins egress to that serviceUrl.

## Environment

- `RUNTARA_ENDPOINT_REF_SECRET` — HMAC key that signs conversation targets.
  Required for the `teams.send-message` workflow path (server-side session
  replies work without it). Rotate by moving the old value to
  `RUNTARA_ENDPOINT_REF_SECRET_PREV`.

## Not in this release

Reactions, message update/delete, files/inline images, Adaptive Card
interactivity (`Action.Execute`/`Action.Submit`), proactive create-conversation,
RSC all-message ingestion, and historical reads (a separate Microsoft Graph
concern). Adaptive Cards can be **sent** (as an attachment) but card actions are
not handled.
