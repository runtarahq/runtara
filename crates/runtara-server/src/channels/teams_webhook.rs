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
        // Authenticated but not yet handled (conversationUpdate,
        // installationUpdate, messageReaction, invoke, ...): acknowledge.
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

    // Deduplicate at-least-once redeliveries by activity id. Teams retries the
    // webhook if processing exceeds ~15s; a genuine redelivery within the Bot
    // Framework token lifetime would otherwise start a second session.
    let activity_id = payload.get("id").and_then(|v| v.as_str());
    if let Some(activity_id) = activity_id
        && !router.reserve_activity(connection_id, activity_id).await
    {
        debug!(
            connection_id = %connection_id,
            activity_id = %activity_id,
            "Dropping duplicate Teams activity"
        );
        return StatusCode::OK.into_response();
    }

    // The serviceUrl is stored only now: after full JWT validation, which
    // includes the serviceurl-claim cross-check where the token carries one.
    if let Some(svc_url) = service_url {
        router.set_teams_service_url(conversation_id, svc_url);
    }

    // Strip bot mentions from text (Teams includes <at>BotName</at> in text).
    let clean_text = strip_teams_mentions(text);
    let clean_text = clean_text.trim();
    if clean_text.is_empty() {
        return StatusCode::OK.into_response();
    }

    let sender_id = from_id.unwrap_or(conversation_id).to_string();

    // Build the curated, credential-free reply target (an opaque signed
    // endpoint ref + conversation identifiers) for the workflow's data.target.
    let target = service_url.and_then(|svc_url| {
        build_conversation_target(connection_id, conversation_id, svc_url, payload)
    });

    let msg = InboundMessage {
        text: clean_text.to_string(),
        sender_id,
        conv_id: conversation_id.to_string(),
        channel: "teams".into(),
        attachments: vec![],
        original_message: payload.clone(),
        target,
        activity_id: activity_id.map(str::to_string),
    };

    debug!(
        connection_id = %connection_id,
        conversation = %conversation_id,
        "Teams message received"
    );

    if let Err(e) = router.handle_message(connection_id, &msg).await {
        warn!(
            connection_id = %connection_id,
            error = %e,
            "Failed to handle Teams message"
        );
    }

    StatusCode::OK.into_response()
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

    let keyring = EndpointRefKeyring::from_env().ok()?;
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
