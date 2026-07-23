// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Microsoft Teams Bot Framework webhook handler.
//!
//! Receives Activity objects from the Bot Framework. EVERY activity type is
//! authenticated (JWT validation per `teams_auth`) before it is acknowledged
//! — an unauthenticated caller learns nothing and changes nothing, and no
//! field of an unauthenticated activity (notably `serviceUrl`) is ever
//! stored. Rejections use HTTP 403 per the Bot Connector spec.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use tracing::{debug, warn};

use super::session::{ChannelRouter, InboundMessage};
use super::teams_auth::{TeamsAuthEndpoints, TeamsTokenContext, validate_teams_request};

/// POST /api/runtime/events/webhook/teams/{connection_id}
pub async fn teams_webhook(
    State(router): State<Arc<ChannelRouter>>,
    Path(connection_id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Load the connection's auth material first: validation applies to every
    // activity type, before any acknowledgement or processing.
    let conn = match load_teams_connection(&router, &connection_id).await {
        Ok(c) => c,
        Err(e) => {
            warn!(connection_id = %connection_id, error = %e, "Failed to load Teams connection");
            return StatusCode::FORBIDDEN.into_response();
        }
    };

    let activity_service_url = payload.get("serviceUrl").and_then(|s| s.as_str());
    let token_ctx = TeamsTokenContext {
        app_id: &conn.app_id,
        azure_tenant_id: conn.azure_tenant_id.as_deref(),
        activity_service_url,
    };
    if let Err(e) =
        validate_teams_request(&headers, &token_ctx, TeamsAuthEndpoints::from_env()).await
    {
        warn!(connection_id = %connection_id, error = %e, "Teams JWT validation failed");
        return StatusCode::FORBIDDEN.into_response();
    }

    // Single-tenant connections only accept activities from their own
    // Microsoft tenant.
    if let Some(expected_tenant) = conn.enforced_activity_tenant() {
        let activity_tenant = payload
            .pointer("/channelData/tenant/id")
            .or_else(|| payload.pointer("/conversation/tenantId"))
            .and_then(|v| v.as_str());
        if activity_tenant != Some(expected_tenant) {
            warn!(
                connection_id = %connection_id,
                activity_tenant = activity_tenant.unwrap_or("<none>"),
                "Teams activity tenant does not match the connection's tenant"
            );
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    let activity_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match activity_type {
        "message" => handle_message_activity(&router, &connection_id, &payload).await,
        // Installation / membership events: capture the conversation's
        // serviceUrl so a proactive target is warm even before the first
        // message. No session is started. (Proactive send itself is a later
        // slice; this only persists the reference.)
        "conversationUpdate" | "installationUpdate" => {
            if is_bot_removal(&conn.app_id, &payload) {
                clear_conversation_reference(&router, &connection_id, &payload);
            } else {
                capture_conversation_reference(&router, &connection_id, &payload);
            }
            StatusCode::OK.into_response()
        }
        // Authenticated but not yet handled (messageReaction, invoke,
        // messageUpdate/Delete, ...): acknowledge. These are post-MVP.
        other => {
            debug!(
                connection_id = %connection_id,
                activity_type = %other,
                "Ignoring unhandled Teams activity type"
            );
            StatusCode::OK.into_response()
        }
    }
}

/// Persist the serviceUrl from an authenticated installation/membership event
/// so a later proactive send has a warm conversation reference. Bot-added is
/// the canonical capture moment (membersAdded contains the bot's recipient id),
/// but we capture on any such event since all are authenticated.
fn capture_conversation_reference(
    router: &Arc<ChannelRouter>,
    connection_id: &str,
    payload: &Value,
) {
    let conversation_id = payload.pointer("/conversation/id").and_then(|v| v.as_str());
    let service_url = payload.get("serviceUrl").and_then(|v| v.as_str());
    if let (Some(conversation_id), Some(service_url)) = (conversation_id, service_url) {
        router.set_teams_service_url(connection_id, conversation_id, service_url);
        debug!(
            connection_id = %connection_id,
            conversation = %conversation_id,
            "Captured Teams conversation reference from installation/membership event"
        );
    }
}

/// Drop the stored serviceUrl for a conversation the bot was just removed from,
/// so a stale reference is not left behind after uninstall.
fn clear_conversation_reference(router: &Arc<ChannelRouter>, connection_id: &str, payload: &Value) {
    if let Some(conversation_id) = payload.pointer("/conversation/id").and_then(|v| v.as_str()) {
        router.remove_teams_service_url(connection_id, conversation_id);
        debug!(
            connection_id = %connection_id,
            conversation = %conversation_id,
            "Cleared Teams conversation reference after bot removal"
        );
    }
}

/// Whether this conversationUpdate/installationUpdate signals the BOT being
/// removed from the conversation — either an `installationUpdate` with
/// `action: "remove"`, or a `conversationUpdate` whose `membersRemoved` list
/// contains the bot's own id (`recipient.id`).
fn is_bot_removal(bot_app_id: &str, payload: &Value) -> bool {
    if payload.get("type").and_then(|t| t.as_str()) == Some("installationUpdate") {
        let action = payload.get("action").and_then(|a| a.as_str()).unwrap_or("");
        if action.eq_ignore_ascii_case("remove") || action.eq_ignore_ascii_case("remove-upgrade") {
            return true;
        }
    }
    // The bot's channel account id is `28:{app_id}`; recipient.id is the most
    // reliable self-identifier on the activity.
    let self_id = payload
        .pointer("/recipient/id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("28:{bot_app_id}"));
    payload
        .get("membersRemoved")
        .and_then(|m| m.as_array())
        .is_some_and(|members| {
            members.iter().any(|m| {
                m.get("id").and_then(|v| v.as_str()) == Some(self_id.as_str())
                    || m.get("id").and_then(|v| v.as_str()) == Some(&format!("28:{bot_app_id}"))
            })
        })
}

/// Handle an authenticated `message` activity: normalize and route it into
/// the channel-session machinery.
async fn handle_message_activity(
    router: &Arc<ChannelRouter>,
    connection_id: &str,
    payload: &Value,
) -> Response {
    let conversation_id = payload
        .pointer("/conversation/id")
        .and_then(|id| id.as_str());
    let text = payload.get("text").and_then(|t| t.as_str());
    let service_url = payload.get("serviceUrl").and_then(|s| s.as_str());

    let (Some(conversation_id), Some(text)) = (conversation_id, text) else {
        return StatusCode::OK.into_response();
    };

    // Loop prevention: drop activities the bot itself authored.
    let from_id = payload.pointer("/from/id").and_then(|v| v.as_str());
    let recipient_id = payload.pointer("/recipient/id").and_then(|v| v.as_str());
    if from_id.is_some() && from_id == recipient_id {
        return StatusCode::OK.into_response();
    }

    // Strip @mentions using the activity's `entities` (exact markup), with the
    // `<at>…</at>` scanner as a fallback. Done before the ack so a mention-only
    // message is acknowledged without spinning up any work.
    let clean_text = strip_teams_mentions_with_entities(text, payload);
    let clean_text = clean_text.trim().to_string();
    if clean_text.is_empty() {
        return StatusCode::OK.into_response();
    }

    let activity_id = payload.get("id").and_then(|v| v.as_str());
    let sender_id = from_id.unwrap_or(conversation_id).to_string();

    // Ack-fast: Teams retries the webhook if it does not receive a 2xx within
    // ~15s, and every DB lookup + workflow enqueue happens below. Move all of
    // that OFF the response path and return 200 now; the redelivery dedup then
    // runs INSIDE the task (SET NX still serializes concurrent redeliveries).
    let router = router.clone();
    let connection_id = connection_id.to_string();
    let conversation_id = conversation_id.to_string();
    let service_url = service_url.map(str::to_string);
    let activity_id = activity_id.map(str::to_string);
    let payload = payload.clone();

    tokio::spawn(async move {
        if let Some(activity_id) = activity_id.as_deref()
            && !router.reserve_activity(&connection_id, activity_id).await
        {
            debug!(
                connection_id = %connection_id,
                activity_id = %activity_id,
                "Dropping duplicate Teams activity"
            );
            return;
        }

        // The serviceUrl is stored only now: after full JWT validation, which
        // includes the serviceurl-claim cross-check where the token carries one.
        if let Some(svc_url) = service_url.as_deref() {
            router.set_teams_service_url(&connection_id, &conversation_id, svc_url);
        }

        // Curated, credential-free reply target (opaque signed endpoint ref +
        // conversation identifiers) for the workflow's data.target.
        let target = service_url.as_deref().and_then(|svc_url| {
            build_conversation_target(&connection_id, &conversation_id, svc_url, &payload)
        });

        let msg = InboundMessage {
            text: clean_text,
            sender_id,
            conv_id: conversation_id.clone(),
            channel: "teams".into(),
            attachments: vec![],
            original_message: payload,
            target,
            activity_id: activity_id.clone(),
        };

        debug!(
            connection_id = %connection_id,
            conversation = %conversation_id,
            "Teams message received"
        );

        if let Err(e) = router.handle_message(&connection_id, &msg).await {
            warn!(
                connection_id = %connection_id,
                error = %e,
                "Failed to handle Teams message"
            );
            // Release the reservation: with ack-fast we already returned 200,
            // so a message we failed to handle must not be tombstoned against a
            // future redelivery.
            if let Some(activity_id) = activity_id.as_deref() {
                router.release_activity(&connection_id, activity_id).await;
            }
        }
    });

    StatusCode::OK.into_response()
}

/// Strip Teams @mention markup from `text`, preferring the activity's
/// `entities` array — each `mention` entity carries the exact `text` markup as
/// it appears in the message, so removing it is precise. The `<at>…</at>`
/// scanner then mops up any residual/malformed markup (or a mention entity that
/// carried no `text`).
fn strip_teams_mentions_with_entities(text: &str, payload: &Value) -> String {
    let mut out = text.to_string();
    if let Some(entities) = payload.get("entities").and_then(|e| e.as_array()) {
        for entity in entities {
            if entity.get("type").and_then(|t| t.as_str()) == Some("mention")
                && let Some(mention_text) = entity.get("text").and_then(|t| t.as_str())
                && !mention_text.is_empty()
            {
                out = out.replace(mention_text, "");
            }
        }
    }
    strip_teams_mentions(&out)
}

/// Mint the opaque endpoint ref and assemble the workflow-visible `data.target`
/// block. Returns `None` if the ref key is unconfigured (the target is optional;
/// server-side session replies still work via the in-process serviceUrl map).
fn build_conversation_target(
    connection_id: &str,
    conversation_id: &str,
    service_url: &str,
    payload: &Value,
) -> Option<Value> {
    use crate::api::services::endpoint_ref::{EndpointBinding, EndpointRefKeyring, sign};

    let ms_tenant_id = payload
        .pointer("/channelData/tenant/id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let conversation_type = payload
        .pointer("/conversation/conversationType")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let reply_to_activity_id = payload
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Defense in depth: the endpoint ref pins the outbound Bearer token to this
    // serviceUrl. The inbound JWT already cross-checks the `serviceurl` claim,
    // but Entra tenant-issued tokens may omit that claim (the check is skipped),
    // so a bad serviceUrl could otherwise slip through to token egress. Refuse
    // to mint a ref for a host that is not a public Bot Connector endpoint.
    if let Err(reason) = validate_teams_service_url(service_url) {
        warn!(
            connection_id = %connection_id,
            service_url = %service_url,
            reason,
            "Refusing to mint a Teams endpoint ref for a non-Bot-Connector serviceUrl"
        );
        return None;
    }

    let keyring = match EndpointRefKeyring::from_env() {
        Ok(k) => k,
        Err(e) => {
            // The signing secret is unconfigured. Server-side session replies
            // still work via the in-process serviceUrl map, but a workflow that
            // wants to reply via the teams agent cannot (no ref to pin the
            // token). Surface this once so it is diagnosable in production.
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                warn!(
                    error = %e,
                    "RUNTARA_ENDPOINT_REF_SECRET is not configured; Teams workflows \
                     cannot mint reply refs (set it to enable agent-driven replies)"
                );
            });
            return None;
        }
    };
    let binding = EndpointBinding {
        v: EndpointBinding::CURRENT_VERSION,
        tenant_id: crate::config::tenant_id().to_string(),
        connection_id: connection_id.to_string(),
        base_url: service_url.trim_end_matches('/').to_string(),
        conversation_id: Some(conversation_id.to_string()),
        conversation_type: conversation_type.clone(),
        ms_tenant_id: ms_tenant_id.clone(),
        iat: chrono::Utc::now().timestamp(),
    };
    let reference = sign(keyring, &binding);

    Some(serde_json::json!({
        "ref": reference,
        "conversationId": conversation_id,
        "conversationType": conversation_type,
        "replyToActivityId": reply_to_activity_id,
        "teamId": payload.pointer("/channelData/team/id").and_then(|v| v.as_str()),
        "channelId": payload.pointer("/channelData/channel/id").and_then(|v| v.as_str()),
        "msTenantId": ms_tenant_id,
    }))
}

/// Validate an inbound Teams `serviceUrl` before it is pinned into an endpoint
/// ref. Public-cloud Bot Connector serviceUrls are always `https` on a
/// Microsoft-owned host — the regional traffic-manager fronts
/// (`smba.trafficmanager.net`, `<region>.smba.trafficmanager.net`) or the
/// global connector (`*.botframework.com`). Anything else is refused so the
/// pinned Bearer token can never egress to an attacker-chosen host.
///
/// Loopback hosts are accepted only when
/// `RUNTARA_TEAMS_ALLOW_INSECURE_SERVICE_URL` is set, for local mock testing.
fn validate_teams_service_url(service_url: &str) -> Result<(), &'static str> {
    let url = url::Url::parse(service_url).map_err(|_| "serviceUrl is not a valid URL")?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("serviceUrl must not contain userinfo");
    }
    let host = url
        .host_str()
        .ok_or("serviceUrl has no host")?
        .to_ascii_lowercase();

    let loopback = host == "localhost" || host == "127.0.0.1";
    if loopback {
        let allowed = std::env::var("RUNTARA_TEAMS_ALLOW_INSECURE_SERVICE_URL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        return if allowed {
            Ok(())
        } else {
            Err("loopback serviceUrl requires RUNTARA_TEAMS_ALLOW_INSECURE_SERVICE_URL")
        };
    }

    if url.scheme() != "https" {
        return Err("serviceUrl must use https");
    }
    let is_bot_connector = host == "smba.trafficmanager.net"
        || host.ends_with(".smba.trafficmanager.net")
        || host == "botframework.com"
        || host.ends_with(".botframework.com");
    if is_bot_connector {
        Ok(())
    } else {
        Err("serviceUrl host is not a public Bot Connector endpoint")
    }
}

/// Auth-relevant fields of a teams_bot connection.
struct TeamsConnectionAuth {
    app_id: String,
    /// Configured Microsoft tenant, when present and non-empty.
    azure_tenant_id: Option<String>,
    /// Explicit legacy multi-tenant registration.
    multi_tenant: bool,
}

impl TeamsConnectionAuth {
    /// The tenant every inbound activity must belong to, when the connection
    /// is single-tenant. Legacy multi-tenant connections accept any tenant.
    fn enforced_activity_tenant(&self) -> Option<&str> {
        if self.multi_tenant {
            return None;
        }
        self.azure_tenant_id.as_deref()
    }
}

/// Load the connection's auth material (tenant-scoped lookup).
async fn load_teams_connection(
    router: &ChannelRouter,
    connection_id: &str,
) -> anyhow::Result<TeamsConnectionAuth> {
    let expected_tenant = crate::config::tenant_id();
    let conn = router
        .connections()
        .get_with_parameters(connection_id, expected_tenant)
        .await
        .map_err(|e| anyhow::anyhow!("DB error: {}", e))?
        .ok_or_else(|| anyhow::anyhow!("Connection not found"))?;

    let params = conn
        .connection_parameters
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Connection has no parameters"))?;

    let app_id = params["app_id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing app_id"))?;
    let azure_tenant_id = params["azure_tenant_id"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let multi_tenant = params["app_type"].as_str() == Some("multi_tenant");

    Ok(TeamsConnectionAuth {
        app_id,
        azure_tenant_id,
        multi_tenant,
    })
}

/// Strip Teams @mention markup (`<at>BotName</at>`) from message text.
fn strip_teams_mentions(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' && chars.peek() == Some(&'a') {
            // Check for <at> tag
            let mut tag = String::from('<');
            let mut found_at = false;
            for inner in chars.by_ref() {
                tag.push(inner);
                if inner == '>' {
                    if tag.starts_with("<at>") || tag.starts_with("</at>") {
                        found_at = true;
                    }
                    break;
                }
            }
            if found_at {
                // Skip content between <at> and </at>
                if tag.starts_with("<at>") {
                    // Consume until </at>
                    let mut depth = String::new();
                    for inner in chars.by_ref() {
                        depth.push(inner);
                        if depth.ends_with("</at>") {
                            break;
                        }
                    }
                }
                // </at> by itself — already consumed
            } else {
                // Not an <at> tag, keep the text
                result.push_str(&tag);
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bot_removal_events() {
        let app_id = "11111111-2222-3333-4444-555555555555";
        let self_id = format!("28:{app_id}");

        // installationUpdate remove.
        assert!(is_bot_removal(
            app_id,
            &serde_json::json!({ "type": "installationUpdate", "action": "remove" })
        ));
        // conversationUpdate with the bot in membersRemoved.
        assert!(is_bot_removal(
            app_id,
            &serde_json::json!({
                "type": "conversationUpdate",
                "recipient": { "id": self_id },
                "membersRemoved": [{ "id": self_id }],
            })
        ));
        // membersRemoved by 28:{app_id} even without recipient.
        assert!(is_bot_removal(
            app_id,
            &serde_json::json!({
                "type": "conversationUpdate",
                "membersRemoved": [{ "id": self_id }],
            })
        ));

        // Not a removal: install add.
        assert!(!is_bot_removal(
            app_id,
            &serde_json::json!({ "type": "installationUpdate", "action": "add" })
        ));
        // A different member removed — not the bot.
        assert!(!is_bot_removal(
            app_id,
            &serde_json::json!({
                "type": "conversationUpdate",
                "recipient": { "id": self_id },
                "membersRemoved": [{ "id": "29:some-user" }],
            })
        ));
        // membersAdded is not a removal.
        assert!(!is_bot_removal(
            app_id,
            &serde_json::json!({
                "type": "conversationUpdate",
                "recipient": { "id": self_id },
                "membersAdded": [{ "id": self_id }],
            })
        ));
    }

    #[test]
    fn service_url_allows_public_bot_connector_hosts() {
        for ok in [
            "https://smba.trafficmanager.net/amer/",
            "https://smba.trafficmanager.net/emea/",
            "https://uk.smba.trafficmanager.net/uk/",
            "https://api.botframework.com",
            "https://europe.botframework.com/v3",
        ] {
            assert!(
                validate_teams_service_url(ok).is_ok(),
                "expected {ok} to be allowed"
            );
        }
    }

    #[test]
    fn service_url_rejects_non_connector_and_tricks() {
        for bad in [
            // Attacker host with a connector-looking prefix/suffix.
            "https://smba.trafficmanager.net.attacker.example/amer/",
            "https://botframework.com.evil.example/",
            "https://smba.trafficmanager.net@attacker.example/amer/",
            // Plain http on the real host (would leak the token in cleartext).
            "http://smba.trafficmanager.net/amer/",
            // Government cloud, not yet supported.
            "https://smba.infra.gcc.teams.microsoft.com/",
            // Totally unrelated.
            "https://attacker.example/",
            "not a url",
        ] {
            assert!(
                validate_teams_service_url(bad).is_err(),
                "expected {bad} to be rejected"
            );
        }
    }

    #[test]
    fn service_url_loopback_gated_by_env() {
        // SAFETY: single-threaded test; we set then clear the flag.
        unsafe { std::env::remove_var("RUNTARA_TEAMS_ALLOW_INSECURE_SERVICE_URL") };
        assert!(validate_teams_service_url("http://127.0.0.1:3999/amer/").is_err());
        unsafe { std::env::set_var("RUNTARA_TEAMS_ALLOW_INSECURE_SERVICE_URL", "1") };
        assert!(validate_teams_service_url("http://127.0.0.1:3999/amer/").is_ok());
        assert!(validate_teams_service_url("http://localhost:3999/amer/").is_ok());
        unsafe { std::env::remove_var("RUNTARA_TEAMS_ALLOW_INSECURE_SERVICE_URL") };
    }

    #[test]
    fn strips_at_mentions() {
        assert_eq!(
            strip_teams_mentions("<at>Bot</at> hello there").trim(),
            "hello there"
        );
        assert_eq!(strip_teams_mentions("no mentions"), "no mentions");
        assert_eq!(
            strip_teams_mentions("a <b>bold</b> claim"),
            "a <b>bold</b> claim"
        );
    }

    #[test]
    fn entities_strip_uses_exact_mention_text() {
        let payload = serde_json::json!({
            "text": "<at>My Bot</at> what's the weather?",
            "entities": [{
                "type": "mention",
                "text": "<at>My Bot</at>",
                "mentioned": { "id": "28:bot", "name": "My Bot" }
            }],
        });
        let text = payload.get("text").and_then(|t| t.as_str()).unwrap();
        assert_eq!(
            strip_teams_mentions_with_entities(text, &payload).trim(),
            "what's the weather?"
        );
    }

    #[test]
    fn entities_strip_falls_back_to_at_scanner() {
        // No entities array at all — the <at> scanner still cleans the markup.
        let payload = serde_json::json!({ "text": "<at>Bot</at> hi" });
        let text = payload.get("text").and_then(|t| t.as_str()).unwrap();
        assert_eq!(
            strip_teams_mentions_with_entities(text, &payload).trim(),
            "hi"
        );

        // Mention entity with no `text` — fallback scanner handles residual markup.
        let payload = serde_json::json!({
            "text": "<at>Bot</at> ping",
            "entities": [{ "type": "mention", "mentioned": { "id": "28:bot" } }],
        });
        let text = payload.get("text").and_then(|t| t.as_str()).unwrap();
        assert_eq!(
            strip_teams_mentions_with_entities(text, &payload).trim(),
            "ping"
        );
    }

    #[test]
    fn enforced_tenant_only_for_single_tenant_connections() {
        let single = TeamsConnectionAuth {
            app_id: "app".into(),
            azure_tenant_id: Some("tid".into()),
            multi_tenant: false,
        };
        assert_eq!(single.enforced_activity_tenant(), Some("tid"));

        let multi = TeamsConnectionAuth {
            app_id: "app".into(),
            azure_tenant_id: Some("tid".into()),
            multi_tenant: true,
        };
        assert_eq!(multi.enforced_activity_tenant(), None);

        let legacy = TeamsConnectionAuth {
            app_id: "app".into(),
            azure_tenant_id: None,
            multi_tenant: false,
        };
        assert_eq!(legacy.enforced_activity_tenant(), None);
    }
}
