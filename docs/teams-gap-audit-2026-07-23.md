# Teams Messaging — Verified Gap Audit

Date: 2026-07-23 · Baseline: `main @ e149d1bf` (post live validation: bidirectional echo working against a real Teams tenant)
Method: 13-agent audit — 6 dimension finders, each adversarially re-verified finding-by-finding against the code, plus a completeness critic. 72 findings survived verification; ~33 unique after de-duplication. Every claim below carries file:line evidence in the underlying reports; only key citations are repeated here.

Companion docs: [teams-messaging-implementation-plan.md](teams-messaging-implementation-plan.md) (the plan this audits against), [teams-setup.md](teams-setup.md), [teams-azure-setup.md](teams-azure-setup.md).

---

## 0. Calibration: what is solid

The audit is a gap list, so first the verified strengths — the core is in good shape:

- **The inbound JWT validator is the strongest part of the feature.** Dual-issuer dispatch, RS256 pin (header alg never trusted), endorsement enforcement incl. no-endorsement rejection, alg-confusion rejection, serviceurl-claim cross-check, tenant enforcement, all-activity-type validation before ACK, 403 semantics — with 14 mock-JWKS unit tests matching slice-1 acceptance almost verbatim (`teams_auth.rs`).
- **Endpoint-ref crypto is sound**: HMAC with constant-time verify, kid rotation, fail-closed proxy tests (forged/foreign-tenant/foreign-connection/unknown-kid). Both egress paths use the hardened no-redirect DNS-guarded client, closing the validate→send TOCTOU.
- **inputHint, percent-encoding, and the 429 retry chain verified correct** end-to-end (retry_after_ms rides ErrorInfo into the durable sleep and the rate-limit budget).
- Cloud scoping, `app_type` validation, and connection-form gating landed as planned; CI exercises the agent's tests and wasm build automatically; no frontend API regen was needed (catalog and forms are served-schema-driven).

Every real hole is in the **perimeter**: before validation (DB load, JWKS fetch), after it (mint, dedup, dispatch), and in the error contract with the proxy.

---

## 1. Fix first (priority order)

1. **Typed proxy credential-resolution errors** — *platform-wide, the highest leverage per line.*
   A failed token mint (e.g. wrong client secret → Azure 401 `invalid_client`) is stringly-typed (`token_cache.rs:356-366`), blanket-mapped to **502** by the proxy (`internal_proxy.rs:405-410`), and classified **transient** by every integration agent (teams `lib.rs:243`, slack, http, mailgun, quickbooks all map 5xx→transient) → durable retry storms on a permanent auth error, observed live. `maybe_flip_needs_reauth` cannot fire for client-credentials types (no `reauth_on_error_codes`). The agent test suite *asserts* 502→transient, enshrining the bug.
   **Fix:** proxy returns a structured permanent error (401 pass-through / `{"code":"CREDENTIAL_RESOLUTION_FAILED","permanent":true}`) for token-endpoint 4xx, keeping 502 for transport failures — all agents already treat 401/4xx as permanent, so no agent changes needed; add an agent-side envelope guard as defense in depth; extend needs-reauth flipping to client-credentials connection types; fix the test.

2. **Webhook pipeline restructure (ack-fast + dedup ordering + JWKS hardening)** — fixes three verified defects in one seam:
   - *Ack-fast not implemented* (plan §4.6 promoted it into the MVP): the handler synchronously does a pre-auth DB connection load, a potentially **un-timeouted** cold JWKS fetch (the hardened client sets **no HTTP timeout** — `net.rs:133-139`), a full tenant trigger-table scan, and an mpsc handoff that can block, all before the 200 (`teams_webhook.rs:38-206`). Teams redelivers at ~15s.
   - *At-most-once inversion*: the dedup key is reserved **before** processing; a `handle_message` failure still returns 200 and the retained key swallows the redelivery — **message permanently lost** (`teams_webhook.rs:148-158` + `:198-206`).
   - *JWKS amplification*: unknown-kid tokens force 2 outbound fetches with no single-flight, negative kid cache, or route rate limit (`teams_auth.rs:255-289`); needs a valid connection id, but that's in the Azure-visible endpoint URL.
   **Fix:** validate → spawn (reserve inside the task, unreserve on failure) → 200; add client timeout; single-flight + negative-cache the JWKS; body-limit/rate-limit layer parity on channel routes.

3. **`authority_host` allowlist is a prefix check — client-secret exfiltration** *(HIGH security, ~5 lines).*
   `starts_with("https://login.microsoftonline.com")` accepts `https://login.microsoftonline.com.attacker.example` and `https://login.microsoftonline.com@attacker.example` (`connections.rs:111-126`); the URL receives the client-credentials POST containing `app_password`. A connection *editor* (who cannot read the masked secret) can exfiltrate it. **Fix:** parse with `url::Url`, require exact host equality against the allowlist, reject userinfo.

4. **serviceUrl snapshot-copy → silent total reply loss** *(HIGH, one-line mitigation).*
   The adapter snapshot-copies the router map at construction (`session.rs:320-322`); existing-session messages never touch the adapter, so in default `per_sender` mode a mid-session conversation switch deterministically kills all replies (error then discarded by `let _ =` at `session.rs:795`). **Fix now:** share the `Arc<DashMap>` instead of copying. **Fix properly:** move session replies onto the minted endpoint-ref (plan §4.5 as written — the promised `channels/egress.rs` was never extracted, and both DashMaps the plan said slice 2 would remove still exist). **Design decision underneath (critic):** `per_sender` keys on Teams `from.id`, which is stable across *all* of a user's conversations — context bleeds between a user's personal chat and channel threads; session identity should likely be `(from.id, conversation.id)`.

5. **Mint-time `validate_public_url` + Bot Connector host allowlist** *(closes the remaining serviceUrl-exfil residual).*
   `build_conversation_target` signs the payload serviceUrl with **no validation** (`teams_webhook.rs:238`), contradicting the plan and `endpoint_ref.rs:48-49`'s own doc-comment invariant ("validated before minting"). Residual risk: a tenant-issued token *without* the `serviceurl` claim skips the cross-check (`teams_auth.rs:221` no-op arm) → attacker serviceUrl signed into a never-expiring ref. Use-time pins hold the https line, but the double barrier the plan mandated is absent. **Fix:** validate at mint (both mint sites) + positive allowlist (`*.botframework.com`, `smba.trafficmanager.net`) at mint and egress.

**Near-misses (do soon):**
- **Dedup TTL 600s ≪ ~1h token window, and a post-TTL replay re-sends the entire past reply transcript**: the deterministic-instance backstop covers only new-session first executions; a replayed finished activity creates a fresh session whose `flush_events` re-dispatches every past bot reply from offset 0 (`session.rs:437-453`, `:506-517`; the engine's `Deduplicated` flag is dropped at the boundary, `execution_engine.rs:1042-1049`). Raise TTL to ≥4h, surface the flag, skip flush on deduplicated-terminal.
- **Conversation-id binding uses substring containment** (`internal_proxy.rs:324-329`): a workflow holding a ref for conversation A can target conversation B by stuffing A into `reply_to_activity_id` — per-conversation confinement fully defeated within a connection/serviceUrl. Extract the `/v3/conversations/{seg}/` segment and compare exactly.
- **Guarded `invocation_trigger.name` reconciliation migration** (`ALTER … DROP NOT NULL` behind a column-existence check): drift originates from a never-merged branch's migration; `set_ignore_missing(true)` hides it forever; drifted DBs 500 on *any* trigger creation.
- **Rename the UUIDv5 dedup namespace** from `teams-activity:{org}:{id}` to a channel-parameterized form *before* a second producer freezes it into persisted instance ids (`session.rs:437-442`); add conversation_id (and connection) to both the dedup key and the namespace per the plan's key shape.
- **`RUNTARA_ENDPOINT_REF_SECRET` silent degradation**: unset → `data.target` silently null (`.ok()?` at `teams_webhook.rs:233`, memoized `None` for process lifetime) → `send-message` fails later with a misleading message. Boot-time loud warn when teams connections exist + rate-limited warn at mint; add to install/deployment docs and the SMO conf template.

---

## 2. Teams-specific gaps (verified)

### Agent (`runtara-agent-teams`)
| Sev | Gap | Fix |
|---|---|---|
| high | 502-transient misclassification (see §1.1) | proxy-primary + envelope guard |
| med | 403/209 blocked-target detection is a single string match; `errorCode:209` numeric shape falls to PERMISSION_ERROR; dead `error_subcode` stub (`lib.rs:216-239`) | add numeric/top-level checks, delete dead code, capture real body on dev tenant |
| med | Chunked send not idempotent under durable retry (re-sends delivered chunks); only last activity id returned; conversation_id input-echoed | `activity_ids` array; document duplicate-on-retry; chunk pacing for 7 msg/s |
| med | Card sends skip text splitting (card + >4000 chars can exceed ~28KB) and never set `summary` (empty toast previews) | split with card-on-first-chunk; optional `summary` input |
| low | Retry-After HTTP-date form ignored (integer-only parse); hour-scale Teams limits exceed default 60s rate budget, undocumented | accept HTTP-date; document `rate_limit_budget_ms` |
| low | Step-level `timeout` ignored — fixed 30s per POST, W071 warns | optional `timeout_ms` input |

### Inbound (`teams_webhook.rs` / `teams_auth.rs`)
| Sev | Gap | Fix |
|---|---|---|
| high | Ack-fast missing + no-timeout JWKS + at-most-once inversion (see §1.2) | pipeline restructure |
| med | Mention stripping kept the hand-rolled `<at>` parser the plan replaced: strips **user** mentions, mishandles `<at id=…>`, an unclosed `<at>` swallows the rest (after consuming the dedup key) | entities-based stripping, keep parser as fallback |
| med | JWKS refetch amplification (see §1.2) | single-flight + negative cache |
| low | Success-path validation logs nothing — plan §6 unknowns #1/#2 (which issuer, serviceurl-claim presence) remain unanswerable even from live traffic | info-log trust domain/issuer/claim-presence |
| low | `nbf` not validated (jsonwebtoken default off; plan required it) | `validate_nbf = true` + test |
| low | `installationUpdate action=remove` **refreshes** the captured reference instead of cleaning up | read `action`/`membersRemoved` |
| low | Dedup key omits conversation id (epoch-ms activity ids can collide across conversations) | add conversation to identity |
| low | Per-request pre-auth DB load; second load + full trigger-table scan per message | short-TTL auth-material cache; targeted trigger query |
| low | No body-size limit layer on channel routes (axum 2MB default only); serviceUrl claim compare normalizes only trailing slash (case/port variance → spurious 403) | DefaultBodyLimit parity; normalize host case |

### Sessions / adapters
| Sev | Gap | Fix |
|---|---|---|
| high | Snapshot-copy reply loss (see §1.4) | share Arc now; ref-based replies proper |
| med | Propagated send errors discarded at every session call site (`let _ =` ×6); a 429/403 on the relay drops the reply silently | session-layer retry/degrade policy; at minimum stop discarding on relay+prompt paths |
| med | `TeamsChannel::send_text` (session egress: facade mint, re-validation, error bail) has **zero** tests and was not exercised live (echo went via agent path) | mocked-connector unit test + one live AiAgent echo |
| low | Cross-connection serviceUrl bleed (map keyed by bare conversation id) + unbounded growth | key by (connection, conversation) or delete maps |
| low | Idle-phase re-queues bypass deterministic-id backstop and drop activity_id | carry activity_id through queued events |
| low | D2 double-send hazard (AiAgent relay + explicit send step) documented nowhere despite plan requiring it | capability description + docs note |
| low | "No matching trigger" ACKs 200, consumes the dedup key, logs invisibly — "bot installed but silent" (critic) | distinct outcome in the diagnostics ring (below) |

---

## 3. Platform-wide gaps — what the next channel agent (WhatsApp/Discord/Google Chat/SMS) would hit

1. **Proxy error contract** (§1.1) — every new agent re-inherits the 502 conflation and re-hand-rolls status classification and Retry-After parsing. Needed: typed proxy error envelope + a shared classifier/Retry-After helper in `runtara-http` or an agent-support crate.
2. **Channel onboarding is a 6-file hand-wired checklist with no doc**: route (`server.rs:~2306`), platform match + register arm (`webhook_manager.rs:69-139` — **unregister has only a Telegram arm**), handler module, adapter arm (`session.rs:290-341`), `CHANNEL_INTEGRATION_IDS` (frontend), `SERVICE_ICONS`. Needed: a `ChannelPlugin` descriptor (validate/normalize/register hooks); interim `docs/add-channel.md`.
3. **Inbound auth machinery is per-channel bespoke**: Discord (Ed25519), WhatsApp (HMAC), Google Chat (JWT) would each rebuild keyed-fetch caching — with the same single-flight/negative-cache/timeout gaps Teams has. Needed: shared `channels/webhook_auth.rs` + a uniform body-limit/rate-limit tower layer on channel routes.
4. **Ack-fast/dedup harness**: generic wrapper (validate → hand off → reserve-with-unreserve → 200). `reserve_activity_dedup` is already generic; but **only Teams populates `activity_id`** — Slack's documented redelivery **double-fires sessions today** (no `event_id`/`X-Slack-Retry-Num` handling), Telegram and Mailgun likewise. Populate per-channel ids + rename the UUIDv5 namespace first.
5. **Shared egress helper (`channels/egress.rs`)**: promised by plan D1/§4.5, never extracted. TeamsChannel inlines the hardened pattern while Slack/Telegram/Mailgun remain the raw-secret, error-swallowing template — a new channel author has a 75% chance of copying the wrong adapter. (Other-adapter parity itself is explicitly out of the Teams plan's scope — track as its own slice.)
6. **Endpoint-ref generalization**: Teams fields (`conversation_type`, `ms_tenant_id`) sit in the "neutral" struct — move to a `meta` map; no resolver seam in the proxy (plan D4 promised trait/enum — bundle with the §1.1 rework); per-conversation confinement is a Teams-URL-shape substring hack — needs a provider `confine(path, binding)` hook (or Discord's `/channels/{id}/messages` gets substring matching too); mint-time validation absent (§1.5). Refs have no expiry (documented deferral) — honor `iat` with a max-age when revisited, and scrub `data.target.ref` from persisted summaries/logs.
7. **Secondary rate keys (plan D3, recorded deferral)**: WhatsApp per-recipient / Discord per-route buckets need the `X-Runtara-Rate-Key` extension; `rate_limit_config` is per-connection only.
8. **Webhook diagnostics surface (slice-6 deliverable, unbuilt)**: per-connection last-N delivery/validation-failure ring (Valkey or small table), surfaced on the connection/trigger page, channel-generic, including a distinct "no trigger matched" outcome. Inbound failures today are warn!-logs invisible to tenants — the live bring-up demonstrated the debugging cost.
9. **`invocation_trigger.name` reconciliation migration** (§1 near-miss) — protects every SMO-lineage deployment, invisible to fresh-DB tests by construction.

---

## 4. Tests & docs

**Tests (the inbound half is nearly test-free):**
- No inbound e2e for Teams **or Slack** (mock-authority seam `RUNTARA_TEAMS_OPENID_CONFIG_URL` was built for it and is unused); webhook handler logic (dedup, tenant gate, target mint, loop prevention, capture) has 2 trivial unit tests; `reserve_activity_dedup` and the UUIDv5 backstop untested.
- Three plan acceptance criteria never became tests: restart-recovery of a ref (slices 2/4), duplicate-delivery-yields-one-execution (slice 4), private-URL/traversal at mint+use composition (slice 2 — pin-layer tests exist; the composed ref path and mint-time validation don't).
- The outbound e2e (`test_teams_send_message.sh`) is in **neither CI nor run_all.sh**; the slice-3 in-crate mocked-connector cargo test was never written.
- The agent suite *asserts* the 502-transient bug green; no test anywhere exercises a failed token mint.

**Docs (every live lesson from the bring-up is missing):**
- Manifest example pins 1.16 while its companion says "1.19+" (internal contradiction); no `supportsChannelFeatures:"tier1"` (portal default 1.25 requires it for team scope); no isNotificationOnly warning (silently disables the compose box); org-catalog publish requirement for channel installs absent (setup doc implies sideload suffices); usage-location, app-cache version-bump, AADSTS7000215 (secret Value-vs-ID) gotchas absent.
- The two setup docs give **different messaging-endpoint URLs** (gateway vs direct/tunnel) without stating when each applies → silent 404s for the wrong topology.
- Plan still says "Status: proposed"; §6 unannotated with live results; slice bookkeeping overstates slice 5 (install-capture only) and slice 2 ("remove both maps" not met); gateway-wildcard claim asserted as fact but never verified (live test used a tunnel).
- Missing author-facing notes the plan itself committed to: card v1.5 desktop / v1.2 mobile, bots can't post in private channels, General channel id == team id, 600s session cap/restart behavior, `rate_limit_budget_ms` for hour-scale Teams limits, test-only OpenID override env vars.

---

## 5. Working-as-designed (planned deferrals — no action)

Reactions / messageUpdate / messageDelete / invoke handling (invoke gets a bare 200 — any card Action.Execute shows a client error; acceptable while deferred), inbound files/attachments (add drop-logs within the deferral), RSC ingestion, proactive create-conversation + D4 bindings table + per-cloud allowlist, D3 secondary rate keys, ref expiry, session durability (600s cap, in-memory actors — accepted platform limitation, tracked separately).

## 6. Still-open empirical unknowns (need logging or a dev-tenant probe)

1. Which issuer Teams SMBA sends for single-tenant bots (blocked on the success-path log). 2. serviceurl-claim presence on tenant-issued tokens (same). 3. Whether Teams redelivery reuses the activity id. 4. Expanded-emoji reaction payload. 5. Real wire shape of the blocked-bot 403 body. 6. SMO gateway `{platform}` wildcard (inspect the gateway config once and record it).
