# What to create on the Azure/Teams side

Setup guide for connecting a custom (non-SDK, Bot Connector REST) bot to Microsoft Teams as a **single-tenant** bot. Assumes your bot backend is already running and exposes a webhook.

> Companion to [`teams-setup.md`](teams-setup.md) (the runtara side). For a local test, your **public host** is a tunnel to the running server (e.g. `cloudflared tunnel --url http://127.0.0.1:7010`), and `<connectionId>` comes from creating the connection (step 4).

## You will end up with

Three values to paste into the connection form (plus `app_type = single_tenant`):

- **App ID** (`MicrosoftAppId`) — the Entra app registration's Application (client) ID
- **App Password** — the **client secret Value** (`MicrosoftAppPassword`), *not* the Secret ID
- **Tenant ID** (`MicrosoftAppTenantId`) — the Directory (tenant) ID

## Steps (in order)

Ordering matters: capture identity + secret before endpoint config; validate the endpoint; enable the channel before packaging; install before any test send.

### 1. Create the Azure Bot resource (single-tenant)

Go to [portal.azure.com](https://portal.azure.com/) → **Create a resource** → type `bot` → Enter → select the **Azure Bot** card → **Create**.

On the create blade:
- **Project details**: **Bot handle** (display/handle name), **Subscription**, **Resource group**, data residency (Global unless you need Local — only `westeurope`/`centralindia`), pricing tier (F0 or S1).
- **Microsoft App ID** section → **Type of App** = **Single Tenant** → **Creation type** = **Create new Microsoft App ID** (Azure creates the Entra app in this tenant), or **Use existing app registration** if you pre-registered a single-tenant app (supply **App ID** + **App tenant ID**).

**Review + create** → **Create** → **Go to resource**. [abs-quickstart](https://learn.microsoft.com/en-us/azure/bot-service/abs-quickstart?view=azure-bot-service-4.0) · [registration](https://learn.microsoft.com/en-us/azure/bot-service/bot-service-quickstart-registration?view=azure-bot-service-4.0)

Do **not** create a "Web App Bot" or "Bot Channels Registration" — those are legacy and can no longer be created. [FAQ](https://learn.microsoft.com/en-us/azure/bot-service/bot-service-resources-faq-azure?view=azure-bot-service-4.0)

### 2. Capture App ID + Tenant ID

Azure Bot resource → left nav **Settings → Configuration**. Copy **Microsoft App ID** (= App ID) and **App Tenant ID** (= Tenant ID; present because single-tenant). Same values appear on the Entra app **Overview** page as **Application (client) ID** and **Directory (tenant) ID**. [registration (single-tenant tab)](https://learn.microsoft.com/en-us/azure/bot-service/bot-service-quickstart-registration?view=azure-bot-service-4.0&tabs=singletenant)

### 3. Create the client secret (App Password)

On the **Configuration** blade, next to **Microsoft App ID** select **Manage** → opens the Entra app's **Certificates & secrets** blade → **Client secrets** tab → **New client secret** → enter a **Description**, set **Expires** → **Add**.

Copy the string in the **Value** column immediately — it is shown **once** and is unrecoverable later (the **Secret ID** is a GUID, not the password). If lost, create a new secret. [add-authentication](https://learn.microsoft.com/en-us/microsoftteams/platform/bots/how-to/authentication/add-authentication)

### 4. Create the runtara connection, then set the messaging endpoint

The messaging-endpoint URL embeds the runtara **connectionId**, so create the connection first (paste App ID, App Password, Tenant ID from steps 2–3, `app_type = single_tenant`). It returns a `connectionId`.

Back in Azure Bot → **Settings → Configuration** → **Messaging endpoint** field, enter:

```
https://<your-public-host>/api/runtime/events/webhook/teams/<connectionId>
```

→ **Apply** (there is no separate Save). The endpoint must be a public **HTTPS** URL with a valid, publicly-trusted cert; keep **TLS 1.2** enabled (do not lock to TLS 1.3-only). Only one messaging endpoint is allowed per bot. [manage-settings](https://learn.microsoft.com/en-us/azure/bot-service/bot-service-manage-settings?view=azure-bot-service-4.0)

If you must set the endpoint before the connection exists, enter a placeholder and return here to update it once you have the connectionId.

### 5. Enable the Microsoft Teams channel

Azure Bot → **Settings → Channels** → select **Microsoft Teams** → **agree to the Terms of Service** → on the **Messaging** tab pick the cloud environment (**Microsoft Teams Commercial** for standard M365; the Government option only for GCC/GCC-High/DoD) → **Apply**.

The **Publish** tab is informational — you do not publish there for the bot to work. Don't casually delete/re-add the channel: re-enabling regenerates keys and invalidates stored `29:`/`a:` IDs. [channel-connect-teams](https://learn.microsoft.com/en-us/azure/bot-service/channel-connect-teams?view=azure-bot-service-4.0)

### 6. Build the Teams app package

Enabling the channel is necessary but **not sufficient** — users need an installable app package. Easiest route: [Teams Developer Portal](https://dev.teams.microsoft.com/) → **Apps** → create app → **Configure** → fill **Basic information**, add **Color icon** (192×192 PNG) + **Outline icon** (32×32 transparent PNG) under **Branding**, then **App features → Bot** and paste your **existing** Microsoft App ID as the botId (do not create a new bot). Download via **Publish → Publish to Store → Download app package**. [Developer Portal](https://learn.microsoft.com/en-us/microsoftteams/platform/concepts/build-and-test/manage-your-apps-in-developer-portal)

The zip holds `manifest.json` + both PNGs flat at the root. Minimal `bots` block: `botId` = your App ID, `scopes` include `personal`/`team`/`groupChat` as needed. Target `manifestVersion` 1.19+ (avoid `devPreview`); author Adaptive Cards at v1.2 for mobile. [apps-package](https://learn.microsoft.com/en-us/microsoftteams/platform/concepts/build-and-test/apps-package) · [schema](https://learn.microsoft.com/en-us/microsoft-365/extensibility/schema/)

### 7. Allow custom app upload (Teams admin center)

For a test user to sideload: **[Teams admin center](https://admin.teams.microsoft.com/) → Teams apps → Setup policies** → Global (Org-wide default) or a new policy → turn **Upload custom apps** ON → assign to the user. Also **Teams apps → Manage apps → Org-wide app settings** → **Let users interact with custom apps in preview** = ON. If org-wide is Off, sideload is unavailable regardless of the other toggles. [custom-app-policies](https://learn.microsoft.com/en-us/microsoftteams/teams-custom-app-policies-and-settings)

### 8. Install + test

In the Teams client → **Apps → Manage your apps → Upload an app → Upload a custom app** → select the .zip → **Add** → **Open** (personal) or pick a channel/chat then **Go**. [apps-upload](https://learn.microsoft.com/en-us/microsoftteams/platform/concepts/deploy-and-publish/apps-upload)

Test: in personal chat the bot receives every message. In a channel/group chat you must **@mention** the bot (`@botname`) or it gets nothing. Portal **Test in Web Chat** also works (the Bot Framework Emulator does not support single-tenant).

## Values → connection fields

| Azure artifact | Our field | Value |
|---|---|---|
| Configuration → **Microsoft App ID** (= Entra Application (client) ID) | `app_id` | `MicrosoftAppId` |
| Certificates & secrets → Client secret **Value** column | `app_password` | `MicrosoftAppPassword` (not the Secret ID) |
| Configuration → **App Tenant ID** (= Directory (tenant) ID) | `azure_tenant_id` | `MicrosoftAppTenantId` |
| — (fixed) | `app_type` | `single_tenant` |

## Gotchas

- **Multi-tenant creation is deprecated after 2025-07-31.** New bots must be Single Tenant or User-Assigned Managed Identity; MSI has no client secret, so Single Tenant is the only password-based choice. [manage-settings](https://learn.microsoft.com/en-us/azure/bot-service/bot-service-manage-settings?view=azure-bot-service-4.0)
- **Single-tenant token authority.** Your backend must mint outbound tokens against `https://login.microsoftonline.com/<azure_tenant_id>/oauth2/v2.0/token` (scope `https://api.botframework.com/.default`) — **not** the multi-tenant `botframework.com` authority. Wrong authority → **AADSTS700016** → no token → 401 on send. [connector-auth](https://learn.microsoft.com/en-us/bot-framework/rest-api/bot-framework-rest-connector-authentication)
- **@mention required in channels/group chats.** Bots receive channel/group messages only when directly @mentioned, unless you declare RSC `ChannelMessage.Read.Group` (consented by a team owner at install). Parse the `mention` entity in `entities[]`, not raw `text`.
- **Custom-app-upload org policy.** If user sideload is blocked, a Global/Teams admin uploads the zip directly via **Manage apps → Upload new app** (available org-wide after a few hours).
- **Error meanings.** **401** = auth failure (wrong authority, expired secret, Secret ID used as password, or your inbound validator rejecting the token). **403 errorCode 209 (MessageWritesBlocked)** = bot uninstalled/blocked or not in the conversation roster — install precedes any send. **403 InvalidBotApiHost** = commercial-vs-Government cloud mismatch (only set Gov authorities for GCC-High/DoD).
- **Verify the inbound issuer empirically.** Microsoft documents the inbound Teams token as `iss = https://api.botframework.com`, `aud = <app_id>`, signed via `https://login.botframework.com/v1/.well-known/keys` — identical for single- and multi-tenant. Threads mentioning `sts.windows.net/{tid}` describe the *outbound* or *Emulator* path. Log the first real inbound Activity's `iss`/`aud`/`serviceUrl` in staging before hard-coding validator rules. [connector-auth](https://learn.microsoft.com/en-us/bot-framework/rest-api/bot-framework-rest-connector-authentication)