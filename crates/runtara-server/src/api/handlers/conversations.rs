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
use crate::api::services::conversation_tokens::{IssueToken, IssuedToken};
use crate::api::services::conversations::{
    ConversationError, ConversationEvent, ConversationList, ConversationState, ListConversations,
    MAX_SEND_WAIT, MAX_WAIT, Message, OpenConversation, ReadEvents, Sent, Stopped,
};
use crate::engine::Engine;
use crate::middleware::tenant_auth::OrgId;

/// How long one stream of a conversation's events is held open. A client
/// reads on from the last event it saw.
const STREAM_HELD: Duration = Duration::from_secs(10 * 60);
/// How long a client waits before it connects again, as the stream tells it.
const STREAM_RETRY: Duration = Duration::from_secs(2);
/// How many streams one tenant holds open at once, unless set otherwise
/// (`RUNTARA_CONVERSATION_STREAMS`).
const STREAMS: usize = 64;

/// The streams each tenant holds open in this process.
static OPEN_STREAMS: std::sync::Mutex<Option<std::collections::HashMap<String, usize>>> =
    std::sync::Mutex::new(None);

fn streams_bound() -> usize {
    std::env::var("RUNTARA_CONVERSATION_STREAMS")
        .ok()
        .and_then(|bound| bound.trim().parse().ok())
        .unwrap_or(STREAMS)
}

/// One of a tenant's open streams, for as long as it is held.
struct OpenStream(String);

impl OpenStream {
    /// Takes a place among the tenant's streams, if there is one.
    fn open(tenant: &str, bound: usize) -> Option<Self> {
        let mut open = OPEN_STREAMS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = open
            .get_or_insert_with(Default::default)
            .entry(tenant.to_owned())
            .or_insert(0);
        if *count >= bound {
            return None;
        }
        *count += 1;
        Some(Self(tenant.to_owned()))
    }
}

impl Drop for OpenStream {
    fn drop(&mut self) {
        let mut open = OPEN_STREAMS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(open) = open.as_mut()
            && let Some(count) = open.get_mut(&self.0)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.0);
            }
        }
    }
}

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

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Only conversations whose key begins with this.
    pub prefix: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// A workflow's conversations, most recently active first.
///
/// Each with its title, what its client keeps about it, whether a run
/// carries it, and the last thing said in it, shortened. A page says where
/// the next begins (`next`): ask again with it as `cursor`.
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{id}/conversations",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("prefix" = Option<String>, Query, description = "Only keys that begin with this"),
        ("cursor" = Option<String>, Query, description = "A page's `next`"),
        ("limit" = Option<u32>, Query, description = "At most this many, up to 200 (50 unless given)"),
    ),
    responses(
        (status = 200, description = "A page of conversations", body = ApiResponse<ConversationList>),
        (status = 400, description = "Not a cursor"),
    ),
    tag = "Conversations"
)]
pub async fn list_conversations(
    OrgId(tenant_id): OrgId,
    State(engine): State<Arc<dyn Engine>>,
    Path(workflow_id): Path<String>,
    Query(query): Query<ListQuery>,
) -> Response {
    let list = ListConversations {
        prefix: query.prefix,
        cursor: query.cursor,
        limit: query.limit.unwrap_or(0),
    };
    match engine
        .conversations_of(&tenant_id, &workflow_id, list)
        .await
    {
        Ok(list) => answered(list),
        Err(error) => refused(error),
    }
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StopConversation {
    /// Fail the messages waiting their turn too. Left out, they keep it, and
    /// the next one starts the next run.
    #[serde(default)]
    pub clear_queue: bool,
}

/// Stop the run that carries a conversation.
///
/// As stopping a run does: it ends when it has, and the conversation says
/// so then (`run_ended`, with `status: stopped`). With no run, nothing is
/// stopped and that is not an error.
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/conversations/{key}/stop",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("key" = String, Path, description = "The client's name for the conversation"),
    ),
    request_body(content = StopConversation, description = "Optional"),
    responses(
        (status = 200, description = "What was stopped", body = ApiResponse<Stopped>),
        (status = 404, description = "No such conversation"),
    ),
    tag = "Conversations"
)]
pub async fn stop_conversation(
    OrgId(tenant_id): OrgId,
    State(engine): State<Arc<dyn Engine>>,
    Path((workflow_id, key)): Path<(String, String)>,
    stop: Option<Json<StopConversation>>,
) -> Response {
    let stop = stop.map(|Json(stop)| stop).unwrap_or_default();
    match engine
        .stop_conversation(&tenant_id, &workflow_id, &key, stop.clear_queue)
        .await
    {
        Ok(stopped) => answered(stopped),
        Err(error) => refused(error),
    }
}

/// Make a credential for one conversation, for a page that talks to the
/// workflow for its end user.
///
/// The token acts as the caller who makes it, held to reading and sending
/// in this conversation and nothing else, until it expires (`ttlSeconds`,
/// an hour unless given, a day at most). Every message sent with it is from
/// `sender`, which the page cannot change. It is handed over once; present
/// it as `Authorization: Bearer <token>` to this conversation's state,
/// messages, events (a stream too) and stop. A token makes no tokens.
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/conversations/{key}/token",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("key" = String, Path, description = "The client's name for the conversation"),
    ),
    request_body(content = IssueToken, description = "Optional"),
    responses(
        (status = 200, description = "The token, once", body = ApiResponse<IssuedToken>),
        (status = 400, description = "A lifetime out of range"),
    ),
    tag = "Conversations"
)]
pub async fn issue_conversation_token(
    State(engine): State<Arc<dyn Engine>>,
    Path((workflow_id, key)): Path<(String, String)>,
    axum::Extension(caller): axum::Extension<crate::auth::AuthContext>,
    issue: Option<Json<IssueToken>>,
) -> Response {
    let issue = issue.map(|Json(issue)| issue).unwrap_or_default();
    match engine
        .issue_conversation_token(caller, &workflow_id, &key, issue)
        .await
    {
        Ok(issued) => answered(issued),
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
///
/// With `wait`, the answer waits that many seconds for the message's turn
/// to be over (the run waits to be told something, or ended, or the message
/// failed) and carries the turn's events in `turn`, with `settled` false if
/// the time ran out first. One request is then a whole exchange.
#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{id}/conversations/{key}/messages",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("key" = String, Path, description = "The client's name for the conversation"),
        ("wait" = Option<u32>, Query, description = "Wait this many seconds for the message's turn, up to 120"),
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
    Query(query): Query<SendQuery>,
    caller: Option<axum::Extension<crate::auth::AuthContext>>,
    Json(mut message): Json<Message>,
) -> Response {
    // A conversation token speaks as the sender it was made for, whoever the
    // message says it is from.
    if let Some(grant) = caller
        .as_ref()
        .and_then(|caller| caller.conversation.as_ref())
    {
        message.sender = grant.sender.clone();
    }
    let wait = Duration::from_secs(u64::from(query.wait.unwrap_or(0))).min(MAX_SEND_WAIT);
    match engine
        .send_conversation_message(
            &tenant_id,
            &workflow_id,
            &key,
            message,
            wait.as_millis() as u32,
        )
        .await
    {
        Ok(sent) => answered(sent),
        Err(error) => refused(error),
    }
}

#[derive(Debug, Deserialize)]
pub struct SendQuery {
    /// Wait this many seconds for the message's turn.
    pub wait: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// Read on from this event.
    pub after: Option<i64>,
    pub limit: Option<u32>,
    /// Wait this many seconds for an event, when there is none.
    pub wait: Option<u32>,
    /// Read back: the events before this one, newest first.
    pub before: Option<i64>,
    /// `desc` for newest first, from the end or from `before`.
    pub order: Option<String>,
    /// Only these kinds, separated by commas.
    pub kinds: Option<String>,
}

/// What a conversation has for its user: replies, that it waits for input,
/// what its run is doing, and when a run starts and ends.
///
/// Events are numbered (`seq`) and kept: read on with `after`.
///
/// **As a stream.** With `Accept: text/event-stream` this is server-sent
/// events, each `id: <seq>`, `event: <kind>`, `data: <the event>`. It sends
/// what is kept after `after` (or `Last-Event-ID`), then `caught_up`, then
/// each event as it is said: nothing is polled for. `caught_up` is not
/// kept and has no id. The stream tells the client when to connect again
/// (`retry`), sends a comment while nothing is said so that no proxy closes
/// it, and ends after ten minutes: connect again with `Last-Event-ID` and
/// nothing is missed. A browser's `EventSource` cannot send an
/// `Authorization` header; read it with `fetch`. A tenant holds a bounded
/// number of streams open (`429 CONVERSATION_STREAMS_FULL` past it).
///
/// **Replies as they are written.** A stream that asks for every kind, or
/// names `reply_delta`, is sent the pieces of a reply as the run writes them
/// (`reply_delta`: `replyId`, `offset`, `text`). They are not kept and have
/// no id; the `reply` with the same `replyId` carries the whole of it.
///
/// **Reading back.** `order=desc` reads from the newest, and `before=<seq>`
/// reads on from there, newest first: how a UI shows the last screen and
/// loads more as its user scrolls up. `kinds` names the kinds wanted,
/// separated by commas (`message,reply` for what was said and none of a
/// run's working); it holds for a stream too.
///
/// **As one request.** Without that header, `wait` holds the request that
/// many seconds (25 at most) for an event when there is none: answered at
/// once if there is something, when there is, or empty when the time is up.
/// For a client that makes one request at a time.
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{id}/conversations/{key}/events",
    params(
        ("id" = String, Path, description = "Workflow ID"),
        ("key" = String, Path, description = "The client's name for the conversation"),
        ("after" = Option<i64>, Query, description = "Read on from this event"),
        ("limit" = Option<u32>, Query, description = "At most this many, up to 500"),
        ("wait" = Option<u32>, Query, description = "Without a stream: wait this many seconds for an event, up to 25"),
        ("before" = Option<i64>, Query, description = "Read back: the events before this one, newest first"),
        ("order" = Option<String>, Query, description = "`desc` for newest first"),
        ("kinds" = Option<String>, Query, description = "Only these kinds, separated by commas"),
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
    let streamed = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/event-stream"));
    let wait = Duration::from_secs(u64::from(query.wait.unwrap_or(0))).min(MAX_WAIT);
    let kinds: Vec<String> = query
        .kinds
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .map(str::to_owned)
        .collect();
    // A stream reads forward, from where it is told to.
    let back = !streamed && (query.before.is_some() || query.order.as_deref() == Some("desc"));
    let first = ReadEvents {
        after: if back {
            query.after.unwrap_or(0)
        } else {
            after
        },
        limit,
        // A stream sends what there is and then waits; one request waits
        // as long as it asked to.
        wait_ms: if streamed { 0 } else { wait.as_millis() as u32 },
        before: query.before.filter(|_| back),
        newest_first: back,
        kinds: kinds.clone(),
        deltas_after: None,
    };
    let first = match engine
        .conversation_events(&tenant_id, &workflow_id, &key, first)
        .await
    {
        Ok(events) => events,
        Err(error) => return refused(error),
    };
    if !streamed {
        return answered(first);
    }
    let Some(open) = OpenStream::open(&tenant_id, streams_bound()) else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "success": false,
                "code": "CONVERSATION_STREAMS_FULL",
                "message": "This tenant holds as many conversation streams open as it may",
            })),
        )
            .into_response();
    };

    // The pieces of a reply as it is written go to a stream that asks for
    // every kind, or names them.
    let wants_pieces = kinds.is_empty() || kinds.iter().any(|kind| kind == "reply_delta");
    let stream = async_stream::stream! {
        // Held for as long as the stream is.
        let _open = open;
        let mut after = after;
        let mut pieces_after = 0u64;
        let mut events = first;
        let mut caught_up = false;
        let started = std::time::Instant::now();
        yield Ok::<_, Infallible>(Event::default().retry(STREAM_RETRY));
        loop {
            let read = events.len();
            for event in events {
                let data = serde_json::to_string(&event).unwrap_or_default();
                // A piece is not kept: it has no id, and a client that
                // connects again resumes from what is.
                if let crate::api::services::conversations::ConversationEventKind::ReplyDelta(
                    piece,
                ) = &event.said
                {
                    pieces_after = pieces_after.max(piece.n);
                    yield Ok(Event::default().event(event.kind()).data(data));
                    continue;
                }
                after = event.seq;
                yield Ok(Event::default()
                    .id(event.seq.to_string())
                    .event(event.kind())
                    .data(data));
            }
            // Everything kept has been sent: what follows is said now.
            let behind = !caught_up && read >= limit.clamp(1, 500) as usize;
            if !caught_up && !behind {
                caught_up = true;
                yield Ok(Event::default().event("caught_up").data(
                    json!({"kind": "caught_up", "body": {"lastSeq": after}}).to_string(),
                ));
            }
            if started.elapsed() > STREAM_HELD {
                break;
            }
            let read = ReadEvents {
                after,
                limit,
                // Held by the engine until the conversation says something.
                wait_ms: if behind { 0 } else { MAX_WAIT.as_millis() as u32 },
                kinds: kinds.clone(),
                deltas_after: wants_pieces.then_some(pieces_after),
                ..ReadEvents::default()
            };
            events = match engine
                .conversation_events(&tenant_id, &workflow_id, &key, read)
                .await
            {
                Ok(events) => events,
                // Deleted while it was read.
                Err(ConversationError::NotFound) => break,
                // The engine is away, or cannot read now: not asked again
                // at once.
                Err(_) => {
                    tokio::time::sleep(STREAM_RETRY).await;
                    Vec::new()
                }
            };
        }
    };
    // The stream asks the engine after this request has returned, for the
    // caller who asked for it and in its tenant's scope.
    let sse = Sse::new(crate::auth::presented::kept(stream)).keep_alive(KeepAlive::default());
    let headers = [
        (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        (header::HeaderName::from_static("x-accel-buffering"), "no"),
    ];
    (headers, sse).into_response()
}
