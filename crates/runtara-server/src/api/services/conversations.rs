// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A conversation with a workflow (`docs/native-chat.md`).
//!
//! A conversation is a workflow, a key its client chose, and what has been
//! said. It is found by `(tenant, workflow, key)`, kept in the server
//! database, and carried by one run after another: a message starts a run
//! when none is active, answers what a run waits for, and otherwise waits
//! its turn in a bounded queue.
//!
//! [`Conversations::advance`] is the whole loop, and every step of it is
//! safe to repeat: it reads what the active run said, says what it waits
//! for, hands it the next message, starts the next run, and starts the
//! reply workflow for what the conversation has for its user. It is run
//! when a message is sent and by the conversation worker, under the
//! conversation's row lock, so two of them never carry one conversation.
//!
//! Nothing here names a chat platform. What a run is, and how one is
//! started or answered, is behind [`Runs`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use utoipa::ToSchema;
use uuid::Uuid;

/// How many messages wait their turn in one conversation, unless set
/// otherwise. One past it is refused.
pub const QUEUE_BOUND: usize = 32;

/// The bound an operator set (`RUNTARA_CONVERSATION_QUEUE_BOUND`), or
/// [`QUEUE_BOUND`].
pub fn queue_bound_from_env() -> usize {
    std::env::var("RUNTARA_CONVERSATION_QUEUE_BOUND")
        .ok()
        .and_then(|bound| bound.trim().parse().ok())
        .unwrap_or(QUEUE_BOUND)
}

/// The kind of event a `conversation` agent's `reply` leaves on its run.
pub const REPLY_EVENT: &str = "conversation_reply";

/// How long a run may take to appear after it was queued, before the
/// conversation gives it up.
const START_ALLOWED: Duration = Duration::from_secs(10 * 60);

const MAX_KEY: usize = 255;
const MAX_MESSAGE_ID: usize = 128;
const EVENTS_PAGE: u32 = 100;
/// How many times one `advance` goes round: a run ending, the next one
/// starting, and what it said at once.
const ROUNDS: usize = 4;

/// Who is speaking, as the client says. The core makes nothing of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Sender {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Something sent with a message or a reply: where to fetch it, not it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub url: String,
}

/// A message into a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// The sender's own id for it. The same id again is the same message.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A structured message, for a run that waits for one. Used as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<Sender>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

/// What became of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    /// Waiting its turn.
    Queued,
    /// It started a run.
    Started,
    /// It answered what a run waited for.
    Delivered,
    /// No run would take it.
    Failed,
}

impl MessageState {
    fn parse(text: &str) -> Self {
        match text {
            "started" => Self::Started,
            "delivered" => Self::Delivered,
            "failed" => Self::Failed,
            _ => Self::Queued,
        }
    }
}

/// The answer to sending a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub message_id: String,
    pub state: MessageState,
    /// The run it started or answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Whether this id had been sent before: nothing was done again.
    pub duplicate: bool,
}

/// What is asked when a conversation is opened.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenConversation {
    /// The workflow to start with each reply and each "waiting for input".
    /// Left out, the route is kept as it is; `null` removes it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schema(value_type = Option<String>)]
    pub reply_workflow_id: Option<Option<String>>,
}

/// Tells a field given as `null` from one left out.
fn present<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(de).map(Some)
}

/// A conversation, as it stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationState {
    pub id: Uuid,
    pub workflow_id: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_workflow_id: Option<String>,
    /// The run that carries it now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// What that run waits to be told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<Value>,
    /// Messages waiting their turn.
    pub queued: i64,
    /// The last event's number: read on from it.
    pub last_seq: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Something a conversation has for its user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationEvent {
    pub seq: i64,
    /// `reply`, `waiting_for_input`, `activity`, `run_started`,
    /// `run_ended`, `message_delivered`, `message_failed` or
    /// `reply_route_failed`. `run_started` and `message_delivered` carry
    /// the message's text and sender, so the events alone are the
    /// transcript.
    pub kind: String,
    pub body: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum ConversationError {
    #[error("no such conversation")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    /// The conversation is busy and holds as many messages as it may.
    #[error("the conversation already holds {bound} messages waiting their turn")]
    QueueFull { bound: usize },
    #[error("{0}")]
    Storage(String),
}

pub type ConversationResult<T> = Result<T, ConversationError>;

fn storage(error: sqlx::Error) -> ConversationError {
    ConversationError::Storage(error.to_string())
}

/// A run, as the conversation needs to know it.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    /// Queued, and not yet known to the runtime.
    NotYet,
    Active,
    Ended {
        completed: bool,
        output: Option<Value>,
        error: Option<String>,
    },
}

/// An event a run left: its kind and what it carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Said {
    pub kind: Option<String>,
    pub payload: Value,
}

/// What a run waits to be told.
#[derive(Debug, Clone, PartialEq)]
pub struct Wait {
    pub request_id: String,
    pub message: String,
    pub tool_name: Option<String>,
    pub response_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// The workflow cannot be run: asking again changes nothing.
    Refused(String),
    /// Not now.
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerError {
    /// The run would not take this answer.
    Invalid(String),
    /// The request is no longer open: the message is another's to take.
    Gone,
    Unavailable(String),
}

/// The runs a conversation is carried by. The engine's is [`EngineRuns`].
#[async_trait]
pub trait Runs: Send + Sync {
    /// Starts `workflow` as run `instance`, once: the same `instance` again
    /// starts nothing more.
    async fn start(
        &self,
        tenant: &str,
        workflow: &str,
        instance: Uuid,
        inputs: Value,
    ) -> Result<(), StartError>;
    async fn look(&self, tenant: &str, instance: &str) -> Result<Run, String>;
    /// The events the run left, in order, from the `after`th.
    async fn said(&self, tenant: &str, instance: &str, after: u32) -> Result<Vec<Said>, String>;
    async fn waits_for(&self, tenant: &str, instance: &str) -> Result<Vec<Wait>, String>;
    /// Answers `request`, once for `operation`.
    async fn answer(
        &self,
        tenant: &str,
        instance: &str,
        request: &str,
        operation: &str,
        payload: &Value,
    ) -> Result<(), AnswerError>;
}

/// Conversations, over the server database and the runs that carry them.
#[derive(Clone)]
pub struct Conversations {
    pool: PgPool,
    runs: Arc<dyn Runs>,
    queue_bound: usize,
}

struct Row_ {
    id: Uuid,
    tenant_id: String,
    workflow_id: String,
    key: String,
    reply_workflow_id: Option<String>,
    instance_id: Option<String>,
    run_started_at: Option<DateTime<Utc>>,
    events_read: i32,
    run_replied: bool,
    waiting: Option<Value>,
    next_seq: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "conversation_id, tenant_id, workflow_id, key, reply_workflow_id, \
    instance_id, run_started_at, events_read, run_replied, waiting, next_seq, created_at, updated_at";

fn read(row: &sqlx::postgres::PgRow) -> Result<Row_, sqlx::Error> {
    Ok(Row_ {
        id: row.try_get("conversation_id")?,
        tenant_id: row.try_get("tenant_id")?,
        workflow_id: row.try_get("workflow_id")?,
        key: row.try_get("key")?,
        reply_workflow_id: row.try_get("reply_workflow_id")?,
        instance_id: row.try_get("instance_id")?,
        run_started_at: row.try_get("run_started_at")?,
        events_read: row.try_get("events_read")?,
        run_replied: row.try_get("run_replied")?,
        waiting: row.try_get("waiting")?,
        next_seq: row.try_get("next_seq")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn checked<'a>(what: &str, value: &'a str, max: usize) -> ConversationResult<&'a str> {
    if value.trim().is_empty() {
        return Err(ConversationError::Invalid(format!("{what} is empty")));
    }
    if value.len() > max {
        return Err(ConversationError::Invalid(format!(
            "{what} is longer than {max} bytes"
        )));
    }
    Ok(value)
}

impl Conversations {
    pub fn new(pool: PgPool, runs: Arc<dyn Runs>) -> Self {
        Self {
            pool,
            runs,
            queue_bound: QUEUE_BOUND,
        }
    }

    /// With another bound on the messages one conversation holds.
    pub fn with_queue_bound(mut self, bound: usize) -> Self {
        self.queue_bound = bound;
        self
    }

    /// Opens the conversation `key` names, or finds it.
    pub async fn open(
        &self,
        tenant: &str,
        workflow: &str,
        key: &str,
        open: OpenConversation,
    ) -> ConversationResult<ConversationState> {
        checked("the workflow", workflow, MAX_KEY)?;
        checked("the key", key, MAX_KEY)?;
        if let Some(Some(route)) = &open.reply_workflow_id {
            checked("the reply workflow", route, MAX_KEY)?;
        }
        let (sets, route) = match open.reply_workflow_id {
            Some(route) => (true, route),
            None => (false, None),
        };
        sqlx::query(
            "INSERT INTO conversations (tenant_id, workflow_id, key, reply_workflow_id) \
             VALUES ($1, $2, $3, $5) \
             ON CONFLICT (tenant_id, workflow_id, key) DO UPDATE SET \
                 reply_workflow_id = CASE WHEN $4 THEN $5 ELSE conversations.reply_workflow_id END, \
                 updated_at = CASE WHEN $4 THEN now() ELSE conversations.updated_at END",
        )
        .bind(tenant)
        .bind(workflow)
        .bind(key)
        .bind(sets)
        .bind(route)
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        self.state(tenant, workflow, key).await
    }

    async fn find(&self, tenant: &str, workflow: &str, key: &str) -> ConversationResult<Row_> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM conversations \
             WHERE tenant_id = $1 AND workflow_id = $2 AND key = $3"
        ))
        .bind(tenant)
        .bind(workflow)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .ok_or(ConversationError::NotFound)?;
        read(&row).map_err(storage)
    }

    pub async fn state(
        &self,
        tenant: &str,
        workflow: &str,
        key: &str,
    ) -> ConversationResult<ConversationState> {
        let row = self.find(tenant, workflow, key).await?;
        let queued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM conversation_messages \
             WHERE conversation_id = $1 AND state = 'queued'",
        )
        .bind(row.id)
        .fetch_one(&self.pool)
        .await
        .map_err(storage)?;
        Ok(ConversationState {
            id: row.id,
            workflow_id: row.workflow_id,
            key: row.key,
            reply_workflow_id: row.reply_workflow_id,
            instance_id: row.instance_id,
            waiting_for: row.waiting,
            queued,
            last_seq: row.next_seq - 1,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }

    /// Sends a message. The conversation is opened if it was not. A message
    /// whose id was sent before is answered as it stands and not sent again.
    pub async fn send(
        &self,
        tenant: &str,
        workflow: &str,
        key: &str,
        message: Message,
    ) -> ConversationResult<Sent> {
        checked("the message's id", &message.id, MAX_MESSAGE_ID)?;
        if message.text.is_none() && message.payload.is_none() && message.attachments.is_empty() {
            return Err(ConversationError::Invalid(
                "a message carries text, a payload or an attachment".into(),
            ));
        }
        let conversation = self
            .open(tenant, workflow, key, OpenConversation::default())
            .await?
            .id;

        let mut tx = self.pool.begin().await.map_err(storage)?;
        // Held against another message being counted at the same moment.
        sqlx::query("SELECT 1 FROM conversations WHERE conversation_id = $1 FOR UPDATE")
            .bind(conversation)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or(ConversationError::NotFound)?;
        if let Some(sent) = sent(&mut tx, conversation, &message.id, true).await? {
            return Ok(sent);
        }
        let queued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM conversation_messages \
             WHERE conversation_id = $1 AND state = 'queued'",
        )
        .bind(conversation)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if queued as usize >= self.queue_bound {
            return Err(ConversationError::QueueFull {
                bound: self.queue_bound,
            });
        }
        let body = serde_json::to_value(&message)
            .map_err(|error| ConversationError::Invalid(error.to_string()))?;
        sqlx::query(
            "INSERT INTO conversation_messages (conversation_id, tenant_id, message_id, body) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(conversation)
        .bind(tenant)
        .bind(&message.id)
        .bind(body)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        tx.commit().await.map_err(storage)?;

        // The sender is told what became of it, where that is known at once.
        if let Err(error) = self.advance(conversation).await {
            tracing::warn!(%conversation, %error, "A conversation was not advanced after a message");
        }
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sent(&mut tx, conversation, &message.id, false)
            .await?
            .ok_or(ConversationError::NotFound)
    }

    /// What the conversation has for its user after event `after`, in order.
    pub async fn events(
        &self,
        tenant: &str,
        workflow: &str,
        key: &str,
        after: i64,
        limit: u32,
    ) -> ConversationResult<Vec<ConversationEvent>> {
        let row = self.find(tenant, workflow, key).await?;
        let rows = sqlx::query(
            "SELECT seq, kind, body, created_at FROM conversation_events \
             WHERE conversation_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
        )
        .bind(row.id)
        .bind(after)
        .bind(i64::from(limit.clamp(1, 500)))
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.iter()
            .map(|row| {
                Ok(ConversationEvent {
                    seq: row.try_get("seq")?,
                    kind: row.try_get("kind")?,
                    body: row.try_get("body")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()
            .map_err(storage)
    }

    /// Deletes a conversation and what was said in it. A run that carries it
    /// is left to end: nothing reads what it says afterwards.
    pub async fn delete(
        &self,
        tenant: &str,
        workflow: &str,
        key: &str,
    ) -> ConversationResult<bool> {
        let deleted = sqlx::query(
            "DELETE FROM conversations WHERE tenant_id = $1 AND workflow_id = $2 AND key = $3",
        )
        .bind(tenant)
        .bind(workflow)
        .bind(key)
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(deleted.rows_affected() > 0)
    }

    /// The conversations that have something to do, across tenants: one
    /// with a run, a message waiting, or something not yet sent to its
    /// reply workflow.
    pub async fn unsettled(&self) -> ConversationResult<Vec<(Uuid, String)>> {
        let rows = sqlx::query(
            "SELECT c.conversation_id, c.tenant_id FROM conversations c \
             WHERE c.instance_id IS NOT NULL \
                OR EXISTS (SELECT 1 FROM conversation_messages m \
                           WHERE m.conversation_id = c.conversation_id AND m.state = 'queued') \
                OR (c.reply_workflow_id IS NOT NULL AND EXISTS ( \
                        SELECT 1 FROM conversation_events e \
                        WHERE e.conversation_id = c.conversation_id AND NOT e.routed \
                          AND e.kind IN ('reply', 'waiting_for_input'))) \
             ORDER BY c.updated_at LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.iter()
            .map(|row| Ok((row.try_get(0)?, row.try_get(1)?)))
            .collect::<Result<_, sqlx::Error>>()
            .map_err(storage)
    }

    /// Carries the conversation as far as it goes now. Returns whether it
    /// did: `false` when another holds it.
    pub async fn advance(&self, conversation: Uuid) -> ConversationResult<bool> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM conversations WHERE conversation_id = $1 \
             FOR UPDATE SKIP LOCKED"
        ))
        .bind(conversation)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?;
        let Some(row) = row else {
            return Ok(false);
        };
        let mut at = Carried {
            row: read(&row).map_err(storage)?,
            tx,
        };
        for _ in 0..ROUNDS {
            if !self.round(&mut at).await? {
                break;
            }
        }
        self.route(&mut at).await?;
        at.save().await?;
        at.tx.commit().await.map_err(storage)?;
        Ok(true)
    }

    /// One round: what the active run has to say, or the next run. Returns
    /// whether another round may find more.
    async fn round(&self, at: &mut Carried) -> ConversationResult<bool> {
        let tenant = at.row.tenant_id.clone();
        let Some(instance) = at.row.instance_id.clone() else {
            return self.start_next(at).await;
        };
        let run = match self.runs.look(&tenant, &instance).await {
            Ok(run) => run,
            Err(error) => {
                tracing::debug!(%instance, %error, "A conversation's run could not be read");
                return Ok(false);
            }
        };
        if run == Run::NotYet {
            let overdue = at.row.run_started_at.is_some_and(|started| {
                Utc::now().signed_duration_since(started).to_std().ok() > Some(START_ALLOWED)
            });
            if !overdue {
                return Ok(false);
            }
            at.ended(&instance, false, Some("the run did not start".into()))
                .await?;
            return Ok(true);
        }

        // What it said, before what became of it: a reply made just before
        // the run ended is still its reply.
        loop {
            let said = match self
                .runs
                .said(&tenant, &instance, at.row.events_read as u32)
                .await
            {
                Ok(said) => said,
                Err(error) => {
                    tracing::debug!(%instance, %error, "A conversation's run's events could not be read");
                    return Ok(false);
                }
            };
            let read = said.len();
            for said in said {
                at.row.events_read += 1;
                if said.kind.as_deref() == Some(REPLY_EVENT) {
                    at.row.run_replied = true;
                    let mut body = said.payload;
                    if let Some(body) = body.as_object_mut() {
                        body.insert("instanceId".into(), json!(instance));
                    }
                    at.say("reply", body).await?;
                    continue;
                }
                for activity in crate::api::handlers::chat::parse_debug_event(
                    said.kind.as_deref(),
                    &said.payload,
                ) {
                    // What a run waits for is said from its open requests,
                    // which are current; this is a record of one.
                    if matches!(
                        activity,
                        crate::api::handlers::chat::ChatEvent::WaitingForInput { .. }
                    ) {
                        continue;
                    }
                    if let Ok(body) = serde_json::to_value(&activity) {
                        at.say("activity", body).await?;
                    }
                }
            }
            if read < EVENTS_PAGE as usize {
                break;
            }
        }

        if let Run::Ended {
            completed,
            output,
            error,
        } = run
        {
            // A run that said nothing answers with what it returned.
            if completed
                && !at.row.run_replied
                && let Some(text) = answer_in(output.as_ref())
            {
                at.say("reply", json!({"text": text, "instanceId": instance}))
                    .await?;
            }
            at.ended(&instance, completed, error).await?;
            return Ok(true);
        }

        let waits = match self.runs.waits_for(&tenant, &instance).await {
            Ok(waits) => waits,
            Err(error) => {
                tracing::debug!(%instance, %error, "What a conversation's run waits for could not be read");
                return Ok(false);
            }
        };
        let Some(wait) = waits.into_iter().next() else {
            at.row.waiting = None;
            return Ok(false);
        };
        let announced = at
            .row
            .waiting
            .as_ref()
            .and_then(|waiting| waiting.get("requestId"))
            .and_then(Value::as_str)
            == Some(wait.request_id.as_str());
        if !announced {
            let waiting = json!({
                "requestId": wait.request_id,
                "message": wait.message,
                "toolName": wait.tool_name,
                "responseSchema": wait.response_schema,
                "instanceId": instance,
            });
            at.say("waiting_for_input", waiting.clone()).await?;
            at.row.waiting = Some(waiting);
        }

        let Some((message_id, message)) = at.next_message().await? else {
            return Ok(false);
        };
        let payload = match (&message.payload, &message.text) {
            (Some(payload), _) => payload.clone(),
            (None, text) => json!({"message": text.clone().unwrap_or_default()}),
        };
        match self
            .runs
            .answer(&tenant, &instance, &wait.request_id, &message_id, &payload)
            .await
        {
            Ok(()) => {
                at.settle(&message_id, "delivered", Some(&instance), None)
                    .await?;
                at.say(
                    "message_delivered",
                    json!({
                        "messageId": message_id,
                        "requestId": wait.request_id,
                        "instanceId": instance,
                        "text": message.text,
                        "sender": message.sender,
                    }),
                )
                .await?;
                at.row.waiting = None;
                Ok(true)
            }
            Err(AnswerError::Invalid(reason)) => {
                at.settle(&message_id, "failed", Some(&instance), Some(&reason))
                    .await?;
                at.say(
                    "message_failed",
                    json!({"messageId": message_id, "reason": reason}),
                )
                .await?;
                Ok(true)
            }
            // The request closed between being read and being answered:
            // the message keeps its turn.
            Err(AnswerError::Gone) => Ok(true),
            Err(AnswerError::Unavailable(error)) => {
                tracing::debug!(%instance, %error, "A conversation's message could not be delivered");
                Ok(false)
            }
        }
    }

    /// Starts a run with the next message, when no run carries the
    /// conversation.
    async fn start_next(&self, at: &mut Carried) -> ConversationResult<bool> {
        let Some((message_id, message)) = at.next_message().await? else {
            return Ok(false);
        };
        // The run's id is made from the message's, so starting it again
        // after a failure here starts the same run.
        let instance = Uuid::new_v5(&at.row.id, format!("message:{message_id}").as_bytes());
        let said = json!({"text": message.text, "sender": message.sender});
        let inputs = json!({
            "data": {
                "message": message.text,
                "payload": message.payload,
                "messageId": message_id,
                "sender": message.sender,
                "attachments": message.attachments,
                "conversation": at.describe(),
            },
            "variables": {},
        });
        match self
            .runs
            .start(&at.row.tenant_id, &at.row.workflow_id, instance, inputs)
            .await
        {
            Ok(()) => {
                let instance = instance.to_string();
                at.settle(&message_id, "started", Some(&instance), None)
                    .await?;
                at.row.instance_id = Some(instance.clone());
                at.row.run_started_at = Some(Utc::now());
                at.row.events_read = 0;
                at.row.run_replied = false;
                at.row.waiting = None;
                at.say(
                    "run_started",
                    json!({
                        "instanceId": instance,
                        "messageId": message_id,
                        "text": said["text"],
                        "sender": said["sender"],
                    }),
                )
                .await?;
                Ok(true)
            }
            Err(StartError::Refused(reason)) => {
                at.settle(&message_id, "failed", None, Some(&reason))
                    .await?;
                at.say(
                    "message_failed",
                    json!({"messageId": message_id, "reason": reason}),
                )
                .await?;
                Ok(true)
            }
            Err(StartError::Unavailable(error)) => {
                tracing::debug!(conversation = %at.row.id, %error, "A conversation's run could not be started");
                Ok(false)
            }
        }
    }

    /// Starts the reply workflow for what it has not been started for.
    async fn route(&self, at: &mut Carried) -> ConversationResult<()> {
        let Some(route) = at.row.reply_workflow_id.clone() else {
            return Ok(());
        };
        let rows = sqlx::query(
            "SELECT seq, kind, body FROM conversation_events \
             WHERE conversation_id = $1 AND NOT routed \
               AND kind IN ('reply', 'waiting_for_input') ORDER BY seq LIMIT 50",
        )
        .bind(at.row.id)
        .fetch_all(&mut *at.tx)
        .await
        .map_err(storage)?;
        for row in rows {
            let seq: i64 = row.try_get("seq").map_err(storage)?;
            let kind: String = row.try_get("kind").map_err(storage)?;
            let body: Value = row.try_get("body").map_err(storage)?;
            let instance = Uuid::new_v5(&at.row.id, format!("event:{seq}").as_bytes());
            let inputs = json!({
                "data": {
                    "conversation": at.describe(),
                    "event": {"seq": seq, "kind": kind, "body": body},
                },
                "variables": {},
            });
            match self
                .runs
                .start(&at.row.tenant_id, &route, instance, inputs)
                .await
            {
                Ok(()) => {}
                Err(StartError::Refused(reason)) => {
                    at.say(
                        "reply_route_failed",
                        json!({"seq": seq, "replyWorkflowId": route, "reason": reason}),
                    )
                    .await?;
                }
                // In order, or not yet: what follows waits for this one.
                Err(StartError::Unavailable(error)) => {
                    tracing::debug!(conversation = %at.row.id, %error, "A reply workflow could not be started");
                    return Ok(());
                }
            }
            sqlx::query(
                "UPDATE conversation_events SET routed = true \
                 WHERE conversation_id = $1 AND seq = $2",
            )
            .bind(at.row.id)
            .bind(seq)
            .execute(&mut *at.tx)
            .await
            .map_err(storage)?;
        }
        Ok(())
    }
}

/// A conversation while it is carried: its row, locked, and what was
/// changed in it.
struct Carried {
    row: Row_,
    tx: Transaction<'static, Postgres>,
}

impl Carried {
    fn describe(&self) -> Value {
        json!({"id": self.row.id, "workflowId": self.row.workflow_id, "key": self.row.key})
    }

    async fn say(&mut self, kind: &str, body: Value) -> ConversationResult<()> {
        sqlx::query(
            "INSERT INTO conversation_events (conversation_id, tenant_id, seq, kind, body) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(self.row.id)
        .bind(&self.row.tenant_id)
        .bind(self.row.next_seq)
        .bind(kind)
        .bind(body)
        .execute(&mut *self.tx)
        .await
        .map_err(storage)?;
        self.row.next_seq += 1;
        Ok(())
    }

    /// The run ended: the conversation has none, until its next message.
    async fn ended(
        &mut self,
        instance: &str,
        completed: bool,
        error: Option<String>,
    ) -> ConversationResult<()> {
        self.say(
            "run_ended",
            json!({
                "instanceId": instance,
                "status": if completed { "completed" } else { "failed" },
                "error": error,
            }),
        )
        .await?;
        self.row.instance_id = None;
        self.row.run_started_at = None;
        self.row.events_read = 0;
        self.row.run_replied = false;
        self.row.waiting = None;
        Ok(())
    }

    async fn next_message(&mut self) -> ConversationResult<Option<(String, Message)>> {
        let row = sqlx::query(
            "SELECT message_id, body FROM conversation_messages \
             WHERE conversation_id = $1 AND state = 'queued' ORDER BY position LIMIT 1",
        )
        .bind(self.row.id)
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(storage)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let id: String = row.try_get("message_id").map_err(storage)?;
        let body: Value = row.try_get("body").map_err(storage)?;
        let message = serde_json::from_value(body)
            .map_err(|error| ConversationError::Storage(error.to_string()))?;
        Ok(Some((id, message)))
    }

    async fn settle(
        &mut self,
        message: &str,
        state: &str,
        instance: Option<&str>,
        detail: Option<&str>,
    ) -> ConversationResult<()> {
        sqlx::query(
            "UPDATE conversation_messages SET state = $3, instance_id = $4, detail = $5 \
             WHERE conversation_id = $1 AND message_id = $2",
        )
        .bind(self.row.id)
        .bind(message)
        .bind(state)
        .bind(instance)
        .bind(detail)
        .execute(&mut *self.tx)
        .await
        .map_err(storage)?;
        Ok(())
    }

    async fn save(&mut self) -> ConversationResult<()> {
        sqlx::query(
            "UPDATE conversations SET instance_id = $2, run_started_at = $3, events_read = $4, \
                 run_replied = $5, waiting = $6, next_seq = $7, \
                 updated_at = CASE WHEN next_seq = $7 AND instance_id IS NOT DISTINCT FROM $2 \
                                   THEN updated_at ELSE now() END \
             WHERE conversation_id = $1",
        )
        .bind(self.row.id)
        .bind(&self.row.instance_id)
        .bind(self.row.run_started_at)
        .bind(self.row.events_read)
        .bind(self.row.run_replied)
        .bind(&self.row.waiting)
        .bind(self.row.next_seq)
        .execute(&mut *self.tx)
        .await
        .map_err(storage)?;
        Ok(())
    }
}

/// What became of a message, if it was sent.
async fn sent(
    tx: &mut Transaction<'static, Postgres>,
    conversation: Uuid,
    message: &str,
    duplicate: bool,
) -> ConversationResult<Option<Sent>> {
    let row = sqlx::query(
        "SELECT state, instance_id, detail FROM conversation_messages \
         WHERE conversation_id = $1 AND message_id = $2",
    )
    .bind(conversation)
    .bind(message)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let state: String = row.try_get("state").map_err(storage)?;
    Ok(Some(Sent {
        message_id: message.to_owned(),
        state: MessageState::parse(&state),
        instance_id: row.try_get("instance_id").map_err(storage)?,
        detail: row.try_get("detail").map_err(storage)?,
        duplicate,
    }))
}

/// The text a run answered with, when it returned one: `answer.response`
/// (an AI agent's outputs mapped to `answer`), or `answer`, `response` or
/// `message` as a string.
pub fn answer_in(output: Option<&Value>) -> Option<String> {
    let output = output?.as_object()?;
    let answer = output.get("answer");
    answer
        .and_then(|answer| answer.get("response"))
        .and_then(Value::as_str)
        .or_else(|| answer.and_then(Value::as_str))
        .or_else(|| output.get("response").and_then(Value::as_str))
        .or_else(|| output.get("message").and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
}

/// Carries every conversation that has something to do, until the server
/// stops. The conversations are found across tenants; each is carried in
/// its own tenant's scope.
pub async fn carry(
    conversations: Conversations,
    shutdown: crate::shutdown::ShutdownSignal,
    every: Duration,
) {
    use runtara_store_postgres::tenant_scope::{scoped, system};
    loop {
        match system(conversations.unsettled()).await {
            Ok(unsettled) => {
                for (conversation, tenant) in unsettled {
                    if let Err(error) = scoped(tenant, conversations.advance(conversation)).await {
                        tracing::warn!(%conversation, %error, "A conversation was not advanced");
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "The conversations to advance were not read"),
        }
        tokio::select! {
            biased;
            _ = shutdown.clone().wait() => return,
            _ = tokio::time::sleep(every) => {}
        }
    }
}

/// The engine's runs: started through its execution engine, like any other
/// run nobody is waiting on, and read from its runtime.
pub struct EngineRuns {
    runtime: Option<Arc<crate::runtime_client::RuntimeClient>>,
    executions: Arc<crate::workers::execution_engine::ExecutionEngine>,
}

impl EngineRuns {
    pub fn new(
        runtime: Option<Arc<crate::runtime_client::RuntimeClient>>,
        executions: Arc<crate::workers::execution_engine::ExecutionEngine>,
    ) -> Self {
        Self {
            runtime,
            executions,
        }
    }

    fn runtime(&self) -> Result<&crate::runtime_client::RuntimeClient, String> {
        self.runtime
            .as_deref()
            .ok_or_else(|| "the workflow runtime is not available".to_owned())
    }
}

#[async_trait]
impl Runs for EngineRuns {
    async fn start(
        &self,
        tenant: &str,
        workflow: &str,
        instance: Uuid,
        inputs: Value,
    ) -> Result<(), StartError> {
        use crate::workers::execution_engine::{ExecutionError, QueueRequest, TriggerSource};
        let queued = self
            .executions
            .queue(QueueRequest {
                run_label: None,
                tenant_id: tenant,
                workflow_id: workflow,
                version: None,
                inputs,
                debug: false,
                correlation_id: None,
                idempotency_key: Some(format!("conversation:{instance}")),
                trigger_source: TriggerSource::Chat,
                instance_id: Some(instance),
            })
            .await;
        match queued {
            Ok(_) => Ok(()),
            Err(
                error @ (ExecutionError::ValidationError(_)
                | ExecutionError::RunLabelConflict
                | ExecutionError::WorkflowValidationError { .. }
                | ExecutionError::NotFound(_)
                | ExecutionError::WorkflowNotFound(_)
                | ExecutionError::BinaryNotFound(_)
                | ExecutionError::CompilationFailed(_)
                | ExecutionError::WorkflowNotRunnable { .. }),
            ) => Err(StartError::Refused(error.to_string())),
            Err(error) => Err(StartError::Unavailable(error.to_string())),
        }
    }

    async fn look(&self, tenant: &str, instance: &str) -> Result<Run, String> {
        use crate::runtime_client::RuntimeError;
        use crate::runtime_types::InstanceStatus;
        use runtara_store_postgres::tenant_scope::scoped;
        let runtime = self.runtime()?;
        match scoped(tenant, runtime.get_instance_info(instance)).await {
            Ok(info) if info.status.is_terminal() => Ok(Run::Ended {
                completed: info.status == InstanceStatus::Completed,
                output: info.output,
                error: info.error,
            }),
            Ok(_) => Ok(Run::Active),
            Err(RuntimeError::InstanceNotFound(_)) => Ok(Run::NotYet),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn said(&self, tenant: &str, instance: &str, after: u32) -> Result<Vec<Said>, String> {
        use crate::runtime_types::{EventSortOrder, ListEventsOptions};
        use runtara_store_postgres::tenant_scope::scoped;
        let runtime = self.runtime()?;
        let page = scoped(
            tenant,
            runtime.list_events(
                instance,
                Some(ListEventsOptions {
                    event_type: Some("custom".to_owned()),
                    sort_order: Some(EventSortOrder::Asc),
                    limit: Some(EVENTS_PAGE),
                    offset: Some(after),
                    ..Default::default()
                }),
            ),
        )
        .await
        .map_err(|error| error.to_string())?;
        Ok(page
            .events
            .into_iter()
            .map(|event| Said {
                kind: event.subtype,
                payload: event.payload.unwrap_or(Value::Null),
            })
            .collect())
    }

    async fn waits_for(&self, tenant: &str, instance: &str) -> Result<Vec<Wait>, String> {
        let page = crate::api::services::pending_inputs::pending_input_page(
            self.runtime()?,
            tenant,
            instance,
        )
        .await
        .map_err(|error| error.to_string())?;
        Ok(page
            .pending_inputs
            .into_iter()
            .map(|input| Wait {
                request_id: input.request_id,
                message: input.message,
                tool_name: input.tool_name,
                response_schema: input.response_schema,
            })
            .collect())
    }

    async fn answer(
        &self,
        tenant: &str,
        instance: &str,
        request: &str,
        operation: &str,
        payload: &Value,
    ) -> Result<(), AnswerError> {
        use runtara_core::persistence::inputs::InputError;
        let runtime = self.runtime().map_err(AnswerError::Unavailable)?;
        match runtime
            .submit_input_response(tenant, instance, request, operation, payload)
            .await
        {
            Ok(_) => Ok(()),
            Err(InputError::InvalidPayload(reason)) => Err(AnswerError::Invalid(reason)),
            Err(error @ (InputError::InvalidRequest | InputError::OperationConflict)) => {
                Err(AnswerError::Invalid(error.to_string()))
            }
            Err(
                InputError::NotFound
                | InputError::Inactive
                | InputError::AlreadyAnswered
                | InputError::FenceRejected,
            ) => Err(AnswerError::Gone),
            Err(error) => Err(AnswerError::Unavailable(error.to_string())),
        }
    }
}
