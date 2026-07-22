# Microsoft Teams Messaging — Implementation Plan

Status: proposed · 2026-07-22
Companion to [teams-messaging-planning-brief.md](teams-messaging-planning-brief.md). This plan is grounded in a full audit of the existing prototype, the Slack integration as parity template, the credential proxy, the channel-session subsystem, and current Microsoft Bot Framework / Teams documentation (citations inline). It corrects several factual claims in the brief, answers the brief's open planning decisions, and sequences the work into landable slices.

---

## 1. Summary

The brief's architecture is sound and matches repo conventions (WASM agent, credentials never in the guest, no Teams-specific gateway). Research changed four things:

1. **The outbound send path is broken for all new bots, not merely unhardened.** Microsoft deprecated multi-tenant bot *creation* after 2025-07-31; `TeamsChannel::get_token` hardcodes the multi-tenant authority (`channel.rs:193`) and `azure_tenant_id` is never read anywhere at runtime. Any bot registered since mid-2025 (necessarily SingleTenant) cannot authenticate. Auth is the first functional slice, not a hardening afterthought.
2. **Inbound JWT validation is better than the brief claims** — issuer, audience, `exp`, and signature-vs-JWKS are already enforced (`teams_webhook.rs:165-180`). The real gaps are narrower and different: the `serviceurl` claim is never compared to the activity (the token-exfiltration/SSRF hole), channel endorsements are unchecked, `channelData.tenant.id` is unchecked, **single-tenant issuer variants are unsupported** (would 403 the MVP's own traffic), and **non-`message` activities are ACKed 200 before any JWT validation**.
3. **Proxy redirect/SSRF hardening is already done** (`net.rs:133-146`, `Policy::none()` + GuardedResolver, enforce-by-default pinning). The exposure is exclusively the channel adapters' plain `reqwest::Client::new()` (`session.rs:103`). Moving replies onto the hardened path makes the brief's redirect item disappear as a separate task.
4. **A dynamic-endpoint precedent already exists in the proxy**: `apply_aws_service_override` (`internal_proxy.rs:245-259`) driven by the agent-declared `X-Runtara-Aws-Service` header. The new endpoint-binding facility should mirror this shape (`X-Runtara-Endpoint-Ref` header → lookup → pin → hardened egress).

The MVP boundary from the brief stands, with two amendments: (a) validate the JWT for **all** activity types and opportunistically capture conversation references from `conversationUpdate`/`installationUpdate` (cheap now, prerequisite for proactive later); (b) include Adaptive Card **sending** (an attachment on the same POST — near-free) while explicitly deferring all card interactivity (`Action.Execute` is a synchronous invoke the async session architecture cannot answer).

---

## 2. Corrections to the brief (verified against code)

| Brief claim | Verdict | Reality |
|---|---|---|
| `teams_bot` connection type exists (app ID, secret, optional tenant) | Confirmed | `TeamsBotParams` at `connection_types.rs:459-492`; field is `app_password`, not `client_secret`; `azure_tenant_id` has **zero runtime consumers** |
| Webhook accepts activities, extracts text, starts/resumes sessions | Confirmed | `teams_webhook.rs:53-145`; sessions in-memory only |
| `TeamsChannel` does its own token acquisition | Confirmed | Private per-session cache, hardcoded multi-tenant authority, plain reqwest (`channel.rs:181-223`) |
| Trigger form supports Teams connections | Confirmed | `ChannelConnectionField.tsx:13-18` |
| Slack agent = send-message, add-reaction, upload-file | Confirmed | `runtara-agent-slack/src/lib.rs:850-853` |
| Proxy assumes one static base URL per connection | **Partial** | True that nothing like `endpoint_ref` exists; but AWS service override is a per-request multi-endpoint precedent, and Shopify/QuickBooks/Azure derive single bases from params |
| Webhook handles only non-empty text messages | Confirmed | Three gates at `teams_webhook.rs:64-69, 98-100, 108-112`; attachments always dropped (`:126`); **non-message types ACKed pre-auth** |
| Non-success Bot Connector responses swallowed | Confirmed | `channel.rs:267-279` warns and returns `Ok(())` (Slack/Telegram/Mailgun share the bug) |
| JWT validation trusts header alg, doesn't validate issuer/audience/lifetime/signature/serviceUrl/endorsements | **Partial** | Issuer, audience, exp, signature ARE enforced. Missing: `serviceurl` claim check, endorsements, tenant check, single-tenant issuers. Header-alg trust (`:170`) is real but largely defanged by jsonwebtoken 10.4 (alg=none unrepresentable, HS-downgrade fails on RSA key) — pin RS256 anyway |
| Redirects followed on credentialed requests | **Partial** | False for the proxy (hardened, `Policy::none()`); true only for the channel adapters' direct egress, which bypasses the proxy entirely |
| No idempotency on activity IDs | Confirmed | Activity `id` is never even read (`teams_webhook.rs:53-145`); channel path passes `instance_id: None` so environment-level dedup can't fire (`session.rs:410-418`) |
| serviceUrl stored without validation against token claims | Confirmed | Decoded claims discarded (`teams_webhook.rs:179-180`); payload serviceUrl stored keyed by bare conversation_id in a process-global map (`session.rs:85,124-127`) and later receives a real Bearer token — token exfiltration via any validly-signed Bot Framework JWT |

---

## 3. Decisions (answers to "Planning Decisions Still Needed")

| Question | Decision | Rationale |
|---|---|---|
| Reactive-only or proactive first release? | **Reactive-only**, but validate JWT on all activity types and capture conversation references from `conversationUpdate`/`installationUpdate` now | Capture-at-install is the canonical moment; deferring capture (not just send) would force users to re-trigger the bot after upgrade |
| One opaque target = conversation, reply chain, or message? | **Conversation-level target**; activity IDs are plain data fields | In Teams the reply chain *is* the conversation id (`;messageid=` suffix); reply-to-activity degrades to send-to-conversation; update/delete take an explicit activity id returned by send. Avoids Slack's fragile `channel:thread_ts` string-splitting |
| Retention/expiry of stored targets? | **No expiry.** Every authenticated inbound activity mints a fresh ref; stale targets surface reactively as `403 errorCode 209 subCode MessageWritesBlocked` send errors | serviceUrl has no documented expiry; Microsoft's own guidance is "save the value"; conversationIds are stable per bot per channel |
| Postgres, Valkey, or both? | **Neither, for the MVP: endpoint refs are stateless signed tokens (§4.3); the only new state is the Valkey `SET NX EX` activity-dedup window.** A *generic* (never Teams-specific) binding table is deferred until proactive enumeration / central staleness / diagnostics need it | Refs ride durable workflow state, so there is nothing to persist server-side; no integration-specific tables |
| Survive secret rotation / reinstall / restart? | Rotation: **yes** — refs are signed with a server key (versioned `kid`, accept N−1 during rollover), not derived from connection secrets. Reinstall: **don't expire proactively** (detect via 403/209). Restart: **yes** — refs live inside the durable workflow input envelope | |
| Which scopes initially? | **All three** (personal, groupChat, team) | Activity shape is uniform. Document: group/team requires @mention absent RSC; bots cannot post in private channels; General channel id == team id |
| Adaptive Cards in first send-message? | **Yes for sending, no for interactivity** | A card is an attachment (`application/vnd.microsoft.card.adaptive`) on the same POST. `Action.Execute` requires a synchronous HTTP-response card the queue-and-poll session architecture cannot produce — defer, including `Action.Submit` handling. Document v1.5 desktop / v1.2 mobile limits |
| Files in parity scope? | **Defer.** One decision to make explicit: if inbound inline images are accepted at all, their `contentUrl` download requires the bot token — that is credentialed egress and must use the hardened path (Slack's raw-token download is a do-not-copy) | Bot file APIs are personal-scope-only, need `supportsFiles`, unavailable in GCC High/DoD; channel files are SharePoint/Graph |
| RSC all-message ingestion? | **Defer receipt.** Decide the manifest question at manifest-authoring time: declaring RSC permissions unused in v1 avoids forced re-installs later (`ChatMessage.Read.Chat` takes effect only on new install); omitting keeps store review simpler | Product call; it's manifest versioning, not code |
| Historical reads? | **Separate Microsoft Graph agent**, definitively | RSC delivery is live-only; history is Graph REST. Keep out of the Bot Connector agent |

Additional decisions this plan makes (the brief left them implicit):

- **D1 — How server-side replies use "the generic path":** a shared **in-process host-side egress helper** — `ConnectionsFacade::resolve_connection_auth` (new `teams_bot` arm) + target-token verification + the pure `pin`/`reject_private_url` fns + `build_hardened_client`. The refactored `TeamsChannel` must NOT loop through `POST /api/internal/proxy` over HTTP (that would be a de-facto internal gateway), and must not spin a WASM component per reply.
- **D2 — Reply seams:** the session event-relay (AiAgent responses) remains the sanctioned in-session reply; the `teams.send-message` capability is for explicit workflow sends (using the trigger's opaque target). Document the double-send hazard (an AiAgent response relayed by the session AND an explicit send step) in the capability docs.
- **D3 — Rate limiting:** MVP relies on 429-driven durable retries. The agent maps 429 → transient with `retry_after_ms` when `Retry-After` is present, else the executor's backoff+jitter; **412, 502, 504 are also transient** (Teams-documented retryables). Teams's dominant limit is per-thread (7 msg/s, 1800/hr) which `rate_limit_config` cannot express — accepted for MVP; a secondary rate-key-by-header extension is a recorded follow-up.
- **D4 — No integration-specific storage; no storage at all for the MVP:** endpoint refs are self-contained signed tokens (§4.3), so the MVP adds zero tables. When proactive messaging or ops diagnostics eventually need enumeration and central staleness, add one **generic** `connection_endpoint_bindings` table — `(tenant_id, connection_id, endpoint_ref) → base_url, status, last_seen_at, metadata JSONB` — with all provider specifics in `metadata`, never in columns. The proxy resolution seam is a small trait/enum over "ref → validated base URL" so a DB-backed resolver and a per-cloud well-known-host allowlist (AWS-synthesized-endpoint analogue) slot in beside the token verifier additively.

---

## 4. Target architecture

### 4.1 Outbound authentication (workstream 1)

Add a `teams_bot` arm to `describe_connection_auth` (`crates/runtara-connections/src/auth/provider_auth.rs:99-351`), modeled on `microsoft_entra_client_credentials` (`:395-432`), returning `DeferredAuth::OAuth2ClientCredentials`:

- `token_url`: `https://login.microsoftonline.com/{azure_tenant_id}/oauth2/v2.0/token` (single-tenant) or `https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token` (legacy multi-tenant only).
- `scope`: `https://api.botframework.com/.default` (fixed; not substitutable).
- `cache_key`: `[integration, connection_id, authority, client_id, scope]`. Honor `expires_in` — never hardcode lifetime.
- No static `base_url` — the endpoint binding supplies it (4.3).

Connection schema (`TeamsBotParams`, JSONB — no migration needed): add optional `app_type` (`single_tenant` default | `multi_tenant` legacy). Validation: require `azure_tenant_id` unless `app_type = multi_tenant`; warn on multi-tenant (Microsoft deprecated creation 2025-07-31). Flip the `azure_tenant_id` field description — its current "leave empty for multi-tenant bots" encodes the deprecated default. Accept-and-warn for existing stored connections.

Delete `TeamsChannel::get_token` and its private cache (`channel.rs:157-223`); both the agent (via proxy) and the session-reply helper (via facade) use the shared token cache with its single-flight and negative-cache semantics.

### 4.2 Inbound authentication (workstream 1)

Rework `validate_teams_jwt` (`teams_webhook.rs:148-183`) into a proper validator:

1. **Pin RS256** (from the OpenID metadata's `id_token_signing_alg_values_supported`); never take the algorithm from the token header.
2. **Dual-issuer dispatch on `iss`:**
   - `https://api.botframework.com` → Bot Framework JWKS (`https://login.botframework.com/v1/.well-known/keys`) **+ channel-endorsement check for `msteams`** (JWKs carry an `endorsements` array — extend `JwkKey` beyond kid/n/e).
   - `https://sts.windows.net/{tid}/` or `https://login.microsoftonline.com/{tid}/v2.0` → Entra tenant JWKS, accepted only when `tid == connection.azure_tenant_id`. **This is load-bearing:** single-tenant bots receive tenant-issued tokens (SDK constants confirm; learn.microsoft.com does not document which issuer Teams SMBA uses for single-tenant — verify empirically, see §6).
3. `aud == app_id`; `exp`/`nbf` with 5-minute skew (industry standard per the auth doc).
4. **`serviceurl` claim (lowercase) must equal the activity's `serviceUrl`** — enforce when the claim is present; empirically confirm presence on single-tenant tokens.
5. `channelData.tenant.id` must match `azure_tenant_id` for single-tenant connections.
6. **Validate before ACK for ALL activity types** — remove the pre-auth 200 for non-message activities (`teams_webhook.rs:66-69`). Reject failures with **403** (docs mandate 403; current code returns 401).
7. JWKS caches: keep ≤24 h refresh (current 6 h is fine); fetch via the hardened client, not plain reqwest.

Only after all checks pass may serviceUrl be persisted (brief's rule, now enforceable).

### 4.3 Conversation targets + generic endpoint binding (workstreams 2–3)

**Endpoint refs are stateless signed tokens — no new tables, no integration-specific storage.** Everything a target must carry is known when the webhook finishes validating an inbound activity, and serviceUrl is documented stable per conversation, so the opaque ref is self-contained rather than a row id. Precedent: the stateless HMAC session tokens (`api/services/session_token.rs:47-60`).

**Mint (writer):** after full validation (4.2), the webhook validates the authenticated serviceUrl (`net::validate_public_url` — HTTPS, public host), then signs and base64url-encodes:

```json
{ "v": 1, "kid": "…", "tenant_id": "…", "connection_id": "…",
  "base_url": "<validated serviceUrl>", "conversation_id": "…",
  "conversation_type": "personal|groupChat|channel", "ms_tenant_id": "…", "iat": … }
```

HMAC-SHA256 with a server key (versioned `kid`; accept N−1 during rotation). This replaces `ChannelRouter::set_teams_service_url` and both in-memory DashMaps (`session.rs:85`, `channel.rs:160`). A fresh ref is minted on every authenticated activity, so the latest trigger always carries the current serviceUrl.

**Verify (proxy contract, mirrors the AWS override precedent):**

1. Agent sets `X-Runtara-Endpoint-Ref: <token>`; `runtara-http::call_via_proxy` lifts it into a new `ProxyRequest.endpoint_ref` field and strips the header (`crates/runtara-http/src/lib.rs:191-240`), like `x-runtara-aws-service`.
2. In `execute_proxy_request`, between `resolve_connection_auth` and the pin (`internal_proxy.rs:304-355`): verify the signature, require `token.tenant_id == X-Org-Id` and `token.connection_id == ProxyRequest.connection_id`; any mismatch or unknown `kid` → fail closed. `token.base_url` becomes the pin base. Hardening: require the request path to target `token.conversation_id`. No DB lookup in the hot path.
3. **URL join semantics:** the agent cannot know the base path (`https://smba.trafficmanager.net/amer/` vs `/teams/`), so when `endpoint_ref` is present the agent's URL host is a sentinel and the final URL = token base joined with the agent's path+query, with strict normalization (reject `..`, decode-once traversal — reuse the `path_within_base` machinery post-join). Then `reject_private_url` + the hardened no-redirect client run unchanged. Pin-then-egress ordering is preserved.
4. Integration-neutral: the proxy sees only "verified ref → validated base URL". The resolution seam is a small trait/enum so future resolvers slot in additively: the deferred generic `connection_endpoint_bindings` table (D4) and a per-cloud well-known-host allowlist for proactive create-conversation.

**Durability without storage:** refs ride the durable workflow input envelope (restart-safe); they aren't derived from connection secrets (rotation-safe); revocation is inherent — the proxy resolves `(connection_id, tenant)` for auth on every send, so a deleted/disabled connection kills its refs. Trade-off accepted for MVP: no central staleness marking (403/209 surfaces as a permanent send error to the workflow) and no target enumeration (only needed for deferred proactive messaging). The only new server state anywhere in the MVP is the Valkey activity-dedup window (4.6).

**Workflow-visible contract:** trigger data gains a curated block alongside the existing raw payload:

```json
"data": {
  "sessionId": "...", "channel": "teams", "userMessage": "...",
  "target": {
    "ref": "<opaque signed endpoint-ref token>",
    "conversationId": "<full id incl. ;messageid= for channel threads>",
    "conversationType": "channel",
    "replyToActivityId": "<inbound activity id>",
    "teamId": "...", "channelId": "...", "msTenantId": "..."
  },
  "attachments": [], "originalMessage": { ... }
}
```

No credentials, no serviceUrl. Mid-session signal payloads gain the same `target` field.

### 4.4 Teams agent (workstream 4)

New crate `crates/agents/runtara-agent-teams` (model: slack; header precedent: sqs). Full packaging checklist is in §5 slice 3. Capability **`send-message`**:

- Inputs: `target` (opaque ref, required), `text` (required unless `card` given), `card` (optional Adaptive Card JSON pass-through), `reply_to_activity_id` (optional), hidden `#[field(skip)] _connection`.
- Operation: `POST {base}/v3/conversations/{conversationId}/activities` (or `.../activities/{activityId}` for reply-to) through the proxy with `X-Runtara-Connection-Id` + `X-Runtara-Endpoint-Ref`. **Percent-encode the conversation id** (contains `;messageid=`, `@`, `:` — the prototype interpolates it raw, `channel.rs:248-252`). Card as attachment `application/vnd.microsoft.card.adaptive`. 4000-char text splitting as in the prototype. The conversation id needed for the URL path is a plain non-sensitive input (the trigger's `target.conversationId`); the proxy cross-checks it against the ref token's embedded `conversation_id` (4.3).
- Output: `{ok, conversation_id, activity_id}` from the `ResourceResponse` — API-confirmed, not input-echoed (fixes Slack's add-reaction anti-pattern).
- Errors (three-layer taxonomy per the Slack template, `lib.rs:189-280`): transport → transient `TEAMS_NETWORK_ERROR`; 401 → `TEAMS_AUTH_ERROR` (permanent); 403 with `errorCode 209 / MessageWritesBlocked` → `TEAMS_TARGET_BLOCKED` (permanent — surfaces to the workflow; central staleness tracking arrives with the deferred binding store, D4); 403 other → `TEAMS_PERMISSION_ERROR`; 404 → `TEAMS_TARGET_NOT_FOUND`; 429 → transient with `retry_after_ms` when present, else backoff+jitter; **412/502/504 → transient**; other 4xx → permanent `TEAMS_HTTP_{status}`; attach the upstream body as `attributes.response`.

Follow-up capabilities (planned separately, same contract shapes): `update-message` (PUT), `delete-message` (DELETE), reactions (note: Teams bots cannot add reactions via Bot Connector — receiving them is inbound work; validate before promising parity), file send (personal scope, FileConsentCard, requires the synchronous-invoke seam).

### 4.5 Channel-session integration (workstream 5)

- New shared host-side egress helper (e.g., `crates/runtara-server/src/channels/egress.rs`): facade auth (4.1) + target-token verification (4.3) + hardened client + **error propagation** (delete the log-and-`Ok(())` swallow at `channel.rs:267-279`).
- `TeamsChannel` reduces to a thin wrapper over the helper; the adapter-construction arm (`session.rs:283-299`) stops copying serviceUrl snapshots and stops receiving raw secrets.
- Session replies keep precise reply targets: the session stores the inbound target ref; replies go to the same conversation (thread) rather than always conversation-root.
- Known accepted limitation (documented, not fixed here): sessions themselves are not durable — MVP durability = reply targets, not session actors. The 600 s session cap (`session.rs:444`) and orphaned-WaitForSignal-on-restart behavior are pre-existing platform issues tracked separately.

### 4.6 Inbound parity (workstream 6)

MVP scope: `message` (all three scopes, mention-stripped via `entities` mentions rather than the hand-rolled `<at>` parser — docs say don't trust `<at>` text), `conversationUpdate` + `installationUpdate` (JWT-validated, target capture/cleanup, no session start), **ack-fast + dedup**:

- Respond 200 immediately after validation; process asynchronously (Teams retries at-least-once if handling exceeds ~15 s; the current handler does JWKS fetch, DB load, trigger scan, and execution queueing synchronously).
- Dedup: Valkey `SET NX EX 600` on `sha256(tenant:connection:conversation:activity_id)` (report-action precedent, `reports.rs:409-457`) + **UUIDv5 deterministic instance id** when an activity starts an execution, so redelivery after Valkey loss still cannot double-fire (environment dedups starts by instance PK; the channel path currently passes `instance_id: None`, `session.rs:410-418`).
- Loop prevention: drop activities where `from.id == recipient.id` (bot-originated).

Post-MVP: `messageReaction` (six documented types; expanded-emoji payload undocumented — verify), `messageUpdate`/`messageDelete` (`channelData.eventType` disambiguates edit/undelete/softDelete), attachments/inline images (credentialed download through the hardened path + S3 persistence, Slack pattern minus the raw-token hack), RSC receipt, invoke handling (requires a new synchronous seam — architecture decision, not an activity type).

### 4.7 Ops & UX (workstream 7)

- Teams app manifest template (bot id, all three scopes; decide RSC declaration per §3; `supportsFiles` false for MVP) + setup doc (Azure Bot resource, single-tenant registration, messaging endpoint).
- **Verify the gateway rewrite is platform-generic**: advertised `{WEBHOOK_BASE_URL}/api/events/{tenant}/webhook/{platform}/{id}` (`webhook_manager.rs:112-115`) vs served `/api/runtime/events/webhook/{platform}/{id}` (`server.rs:2315-2318`). Teams imposes no URL shape (the Azure Bot messaging endpoint is one arbitrary HTTPS URL); the platform segment is runtara's own dispatch convention. Requirement: the out-of-repo SMO gateway carries exactly one wildcard rule over `{platform}` — **no per-platform (teams-specific) gateway entries**. If the current rule enumerates platforms, generalize it once.
- Connection validation: reject or explicitly scope out Azure Government/GCC clouds (different token bases and serviceUrl hosts; GCC returns `403 InvalidBotApiHost` if the public host is used).
- UI: `teams` entry in `SERVICE_ICONS` (`service-icon.tsx:21-41`) + logo SVG; empty-state copy in `ChannelConnectionField.tsx:50-53` mentions Teams; webhook diagnostics surface (last delivery, last validation failure).

---

## 5. Delivery slices (each lands independently; e2e-verify before done)

**Slice 1 — Auth foundation (P0, gates everything).**
`provider_auth.rs` `teams_bot` arm; `TeamsBotParams.app_type` + validation + description flip; inbound validator rework per 4.2 (dual-issuer, RS256 pin, `serviceurl` claim, endorsements, tenant check, all-activity validation, 403); propagate send failures (drop the swallow). Interim: `TeamsChannel` token via facade (correct authority) even before the full egress refactor.
*Accept:* a SingleTenant dev-tenant bot round-trips a message; forged-issuer/forged-audience/mismatched-serviceUrl tokens are 403'd (unit tests with a mock JWKS); non-message activities require valid JWTs.

**Slice 2 — Endpoint-ref tokens + proxy binding.**
Signed-ref mint/verify module (shared by webhook and proxy; versioned `kid`); webhook mints post-validation; `ProxyRequest.endpoint_ref` + header lift + resolution seam + join semantics; remove both in-memory serviceUrl maps. No migration — the MVP stores nothing.
*Accept:* proxy-policy tests — forged-signature, foreign-tenant, foreign-connection, and unknown-`kid` refs fail closed; private-IP/http serviceUrl rejected at mint and at use; path traversal (incl. encoded `%3B`/`..`) rejected; conversation-id cross-check enforced; a workflow started before a server restart sends successfully after it (the ref rides the durable input envelope).

**Slice 3 — `runtara-agent-teams`.**
Crate (Cargo/build.rs/lib.rs per slack template); workspace member (build script greps members); `runtara-agent-bundle-emit` entry (build hard-fails on mismatch); `send-message` per 4.4; catalog pickup is automatic from meta.json; frontend `npm run generate-api-runtime-local`; icons.
*Accept:* in-crate unit tests (agent_info shape, pure body-builders/error mappers, missing-connection error); mocked Bot Connector via local `TcpListener` behind `RUNTARA_HTTP_PROXY_URL` (pattern: `direct_wasm_execute.rs:658-709`); e2e script `e2e/test_teams_send_message.sh` (compile → execute → assert activity POST + returned activity id).

**Slice 4 — Session egress + trigger contract.**
Shared egress helper; `TeamsChannel` refactor; curated `data.target` block in trigger/signal payloads; ack-fast + activity dedup + UUIDv5 instance ids; reply threading.
*Accept:* restart test — inbound message, restart server, workflow `send-message` using the previously minted target ref succeeds; duplicate webhook delivery produces one execution.

**Slice 5 — Inbound parity increment** (reactions, update/delete, installationUpdate cleanup, attachments decision per §3). Each type: normalize, JWT-validated, loop-guarded, tested.

**Slice 6 — Ops/UX** (manifest, setup docs, gateway-rewrite verification, cloud scoping, diagnostics).

**Slice 7 — Verification hardening**: full suite green — unit, proxy-policy, mocked-connector, restart/recovery, and the real dev-tenant checklist (§6).

---

## 6. Empirical unknowns — verify on the dev tenant (documentation is silent)

1. Which issuer Teams SMBA uses for SingleTenant bots (`api.botframework.com` vs `sts.windows.net/{tid}/`) — decides the default path in the dual-issuer validator.
2. Whether single-tenant tokens carry the `serviceurl` claim (enforce-when-present until confirmed).
3. Whether Teams timeout-retries reuse the same `activity.id` (practitioner reports say yes; spec says receivers MAY dedup but senders SHOULDN'T rely on it).
4. What a bot receives for expanded-emoji reactions (docs enumerate only angry/heart/laugh/like/sad/surprised).
5. That the SMO gateway's webhook rewrite is a `{platform}` wildcard (so Teams — and any future channel — needs no gateway change); generalize the rule if it enumerates platforms.
6. Real token `expires_in` for the single-tenant authority (honor it either way).

---

## 7. Updated MVP boundary

As the brief's "Suggested MVP Boundary", amended:

- Single-tenant Teams bot connection (**explicit `app_type`; multi-tenant = legacy accept-and-warn**).
- Fully validated inbound activities — **all types validated**, text messages in all three scopes processed, `conversationUpdate`/`installationUpdate` captured for targets.
- Tamper-proof, tenant- and connection-bound conversation targets as stateless signed refs riding durable workflow state (no new tables; 403/209 staleness surfaces as send errors).
- Generic connection-bound endpoint enforcement in the proxy (`endpoint_ref` per 4.3).
- `teams.send-message` with text, **optional Adaptive Card (send-only)**, optional reply target, activity-ID result, full error taxonomy.
- Channel-session replies through the shared hardened egress path with error propagation and thread-precise targets.
- **Ack-fast + activity-ID dedup + deterministic instance ids** (promoted into MVP: at-least-once redelivery is documented Teams behavior, not an edge case).
- Manifest/setup docs and the automated test suite of §5.

Explicitly out: reactions, files, RSC ingestion, card interactivity (`Action.Execute`/`Action.Submit`), proactive create-conversation, historical reads (separate Graph agent), session durability (platform-level, tracked separately).

---

## 8. References

- Bot Connector auth (in/outbound, JWKS, endorsements, serviceUrl claim): learn.microsoft.com/azure/bot-service/rest-api/bot-framework-rest-connector-authentication
- Connector REST (send/reply/update/delete/create-conversation, ResourceResponse): …/bot-framework-rest-connector-api-reference
- Proactive messaging + serviceUrl storage + 403/209: learn.microsoft.com/microsoftteams/platform/bots/how-to/conversations/send-proactive-messages
- Rate limits (per-thread, 50 RPS/tenant, retry 412/502/504): …/bots/how-to/rate-limit
- Activity types, conversation events, RSC, files, cards: …/bots/build-conversational-capability, …/bots/how-to/conversations/*, …/task-modules-and-cards/cards/cards-reference
- App types & multi-tenant deprecation: learn.microsoft.com/azure/bot-service/bot-service-quickstart-registration
- Single-tenant issuer constants: github.com/microsoft/botbuilder-dotnet — `AuthenticationConstants.cs`
