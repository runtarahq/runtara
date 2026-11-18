// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Conversations with a workflow, over the API (`docs/native-chat.md`).
//!
//! Thin: each asks the engine, which keeps the conversations and carries
//! them.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::dto::common::ApiResponse;
use crate::api::services::conversations::{
    ConversationError, ConversationEvent, ConversationState, Message, OpenConversation, Sent,
};
use crate::engine::Engine;
use crate::middleware::tenant_auth::OrgId;

/// How long one stream of a conversation's events is held open. A client
/// reads on from the last event it saw.
const STREAM_HELD: Duration = Duration::from_secs(10 * 60);
const STREAM_EVERY: Duration = Duration::from_millis(500);

fn refused(error: ConversationError) -> Response {
    let (status, code) = match &error {
        ConversationError::NotFound => (StatusCode::NOT_FOUND, "CONVERSATION_NOT_FOUND"),
        ConversationError::Invalid(_) => (StatusCode::BAD_REQUEST, "CONVERSATION_INVALID"),
        ConversationError::QueueFull { .. } => {
            (StatusCode::TOO_MANY_REQUESTS, "CONVERSATION_QUEUE_FULL")
        }
        ConversationError::Storage(_) => {
            (StatusCode::SERVICE_UNAVAILABLE, "CONVERSATION_UNAVAILABLE")
        }
    };
    let message = match &error {
        // What failed in storage is the server's to know.
        ConversationError::Storage(detail) => {
            tracing::warn!(%detail, "A conversation could not be read or written");
            "The conversation is not available now".to_owned()
        }
        other => other.to_string(),
    };
    (
        status,
        Json(json!({"success": false, "code": code, "message": message})),
    )
        .into_response()
}

fn answered<T: serde::Serialize>(data: T) -> Response {
    Json(ApiResponse::success(data)).into_response()
}

/// Open the conversation a key names with a workflow, or find it.
///
/// The key is the client's: the same key always reaches the same
/// conversation. `replyWorkflowId` sets the workflow started with each
/// reply and each "waiting for input"; `null` removes it; left out, it is
/// kept.
#[utoipa::path(
    put,
    path = "/api/runtime/workflows/{id}/conversations/{key}",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("key" = String, Path, description = "The client's name for the conversation"),
    ),
    request_body(content = OpenConversation, description = "Optional"),
    responses(
        (status = 200, description = "The conversation", body = ApiResponse<ConversationState>),
        (status = 400, description = "Invalid key or reply workflow"),
    ),
    tag = "Conversations"
)]
pub async fn open_conversation(
    OrgId(tenant_id): OrgId,
    State(engine): State<Arc<dyn Engine>>,
    Path((workflow_id, key)): Path<(String, String)>,
    open: Option<Json<OpenConversation>>,
) -> Response {
    let open = open.map(|Json(open)| open).unwrap_or_default();
    match engine
        .open_conversation(&tenant_id, &workflow_id, &key, open)
        .await
    {
        Ok(state) => answered(state),
        Err(error) => refused(error),
    }
}

/// A conversation as it stands: its active run, what that run waits for,
/// and how many messages wait their turn.
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{id}/conversations/{key}",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("key" = String, Path, description = "The client's name for the conversation"),
    ),
    responses(
        (status = 200, description = "The conversation", body = ApiResponse<ConversationState>),
        (status = 404, description = "No such conversation"),
    ),
    tag = "Conversations"
)]
pub async fn get_conversation(
    OrgId(tenant_id): OrgId,
    State(engine): State<Arc<dyn Engine>>,
    Path((workflow_id, key)): Path<(String, String)>,
) -> Response {
    match engine.conversation(&tenant_id, &workflow_id, &key).await {
        Ok(state) => answered(state),
        Err(error) => refused(error),
    }
}

/// Delete a conversation and what was said in it.
#[utoipa::path(
    delete,
    path = "/api/runtime/workflows/{id}/conversations/{key}",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("key" = String, Path, description = "The client's name for the conversation"),
    ),
    responses(
        (status = 200, description = "Deleted"),
        (status = 404, description = "No such conversation"),
    ),
    tag = "Conversations"
)]
pub async fn delete_conversation(
    OrgId(tenant_id): OrgId,
    State(engine): State<Arc<dyn Engine>>,
    Path((workflow_id, key)): Path<(String, String)>,
) -> Response {
    match engine
        .delete_conversation(&tenant_id, &workflow_id, &key)
        .await
    {
        Ok(true) => answered(Value::Null),
        Ok(false) => refused(ConversationError::NotFound),
        Err(error) => refused(error),
    }
}

/// Send a message into a conversation, at any time.
///
/// With no active run the message starts one. If the run waits for input
/// the message answers it. If the run is busy the message waits its turn;
/// a conversation holds a bounded number of them and refuses one past it
/// with `429 CONVERSATION_QUEUE_FULL`. The same `id` again is the same
/// message: nothing is done twice, and `duplicate` says so.
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/conversations/{key}/messages",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("key" = String, Path, description = "The client's name for the conversation"),
    ),
    request_body = Message,
    responses(
        (status = 200, description = "What became of the message", body = ApiResponse<Sent>),
        (status = 400, description = "Not a message"),
        (status = 429, description = "The conversation holds as many messages as it may"),
    ),
    tag = "Conversations"
)]
pub async fn send_message(
    OrgId(tenant_id): OrgId,
    State(engine): State<Arc<dyn Engine>>,
    Path((workflow_id, key)): Path<(String, String)>,
    Json(message): Json<Message>,
) -> Response {
    match engine
        .send_conversation_message(&tenant_id, &workflow_id, &key, message)
        .await
    {
        Ok(sent) => answered(sent),
        Err(error) => refused(error),
    }
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// Read on from this event.
    pub after: Option<i64>,
    pub limit: Option<u32>,
}

/// What a conversation has for its user: replies, that it waits for input,
/// what its run is doing, and when a run starts and ends.
///
/// Events are numbered (`seq`) and kept: read on with `after`. With
/// `Accept: text/event-stream` this is a stream of them as they come, each
/// with its `seq` as the event id, so a client that reconnects with
/// `Last-Event-ID` misses nothing.
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{id}/conversations/{key}/events",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("key" = String, Path, description = "The client's name for the conversation"),
        ("after" = Option<i64>, Query, description = "Read on from this event"),
        ("limit" = Option<u32>, Query, description = "At most this many, up to 500"),
    ),
    responses(
        (status = 200, description = "The events, or a stream of them", body = ApiResponse<Vec<ConversationEvent>>),
        (status = 404, description = "No such conversation"),
    ),
    tag = "Conversations"
)]
pub async fn conversation_events(
    OrgId(tenant_id): OrgId,
    State(engine): State<Arc<dyn Engine>>,
    Path((workflow_id, key)): Path<(String, String)>,
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
) -> Response {
    let last_seen = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok());
    let after = query.after.or(last_seen).unwrap_or(0);
    let limit = query.limit.unwrap_or(100);

    let first = match engine
        .conversation_events(&tenant_id, &workflow_id, &key, after, limit)
        .await
    {
        Ok(events) => events,
        Err(error) => return refused(error),
    };
    let streamed = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/event-stream"));
    if !streamed {
        return answered(first);
    }

    let stream = async_stream::stream! {
        let mut after = after;
        let mut events = first;
        let started = std::time::Instant::now();
        loop {
            for event in events {
                after = event.seq;
                yield Ok::<_, Infallible>(
                    Event::default()
                        .id(event.seq.to_string())
                        .event(event.kind.clone())
                        .data(serde_json::to_string(&event).unwrap_or_default()),
                );
            }
            if started.elapsed() > STREAM_HELD {
                break;
            }
            tokio::time::sleep(STREAM_EVERY).await;
            events = match engine
                .conversation_events(&tenant_id, &workflow_id, &key, after, limit)
                .await
            {
                Ok(events) => events,
                // Deleted while it was read.
                Err(ConversationError::NotFound) => break,
                Err(_) => Vec::new(),
            };
        }
    };
    // The stream asks the engine after this request has returned, for the
    // caller who asked for it.
    let sse = Sse::new(crate::auth::presented::kept(stream)).keep_alive(KeepAlive::default());
    let headers = [
        (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        (header::HeaderName::from_static("x-accel-buffering"), "no"),
    ];
    (headers, sse).into_response()
}
