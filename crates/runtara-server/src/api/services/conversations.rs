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

/// The longest a sender waits for its message's turn in one request.
pub const MAX_SEND_WAIT: Duration = Duration::from_secs(120);

/// How long a run may take to appear after it was queued, before the
/// conversation gives it up.
const START_ALLOWED: Duration = Duration::from_secs(10 * 60);

/// Why a message waiting its turn fails when the queue is cleared.
const STOPPED: &str = "the conversation was stopped before this message's turn";

/// Why a message that names what it answers fails when that is not open.
const STALE: &str = "what this message answers is no longer waited for";

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
    /// The request this message answers: the `requestId` of a
    /// `waiting_for_input`. With it, the message answers that request or
    /// fails as stale; without it, it answers what the run waits for when
    /// its turn comes, or starts the next run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
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
    /// What came of it, where the sender asked to wait for that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<TurnSoFar>,
}

/// A message's turn, as far as it had gone when its sender stopped waiting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnSoFar {
    /// The events of the turn, in order: the replies, and what ended it.
    pub events: Vec<ConversationEvent>,
    /// Whether the turn is over: the run waits to be told something, or
    /// ended, or the message failed. `false` when the time ran out first.
    pub settled: bool,
    /// The last event read: read on from it.
    pub last_seq: i64,
}

/// What is asked of a conversation's events.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadEvents {
    /// Those after this one.
    pub after: i64,
    pub limit: u32,
    /// How long to wait for one, in milliseconds, when there is none.
    #[serde(default)]
    pub wait_ms: u32,
    /// Those before this one, newest first: for reading back from the end.
    /// Nothing is waited for.
    #[serde(default)]
    pub before: Option<i64>,
    /// Newest first, from the end or from `before`.
    #[serde(default)]
    pub newest_first: bool,
    /// Only these kinds. Empty for all.
    #[serde(default)]
    pub kinds: Vec<String>,
}

/// One of a workflow's conversations, as a list shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: Uuid,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Whether a run carries it now.
    pub active: bool,
    /// Whether that run waits to be told something.
    pub waiting: bool,
    /// The last thing said in it, by either side, shortened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_said: Option<LastSaid>,
    pub created_at: DateTime<Utc>,
    /// When it was last active.
    pub updated_at: DateTime<Utc>,
}

/// The last thing said in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LastSaid {
    /// `message` or `reply`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub at: DateTime<Utc>,
}

/// A page of a workflow's conversations, most recently active first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationList {
    pub conversations: Vec<ConversationSummary>,
    /// Ask again with this as `cursor` for the next page. Absent on the
    /// last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// What is asked of a workflow's conversations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListConversations {
    /// Only those whose key begins with this.
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: u32,
}

/// What stopping a conversation did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    /// The run that was asked to stop, if one carried the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// The messages that were waiting their turn and now never have it.
    pub cleared: i64,
}

/// The most a client keeps about a conversation.
pub const MAX_METADATA_BYTES: usize = 4 * 1024;
const MAX_TITLE: usize = 255;
/// How much of the last thing said a list shows.
const LAST_SAID_CHARS: i32 = 200;

/// The longest one ask waits for an event. A reader asks again.
pub const MAX_WAIT: Duration = Duration::from_secs(25);

/// Who is waiting for what a conversation says next, and which conversation
/// each run carries: what lets a conversation be read when there is
/// something to read, and not on a clock.
///
/// It is this process's alone. The engine is one process; conversations are
/// carried and read in it.
#[derive(Default)]
struct Wakes {
    readers: std::sync::Mutex<std::collections::HashMap<Uuid, Arc<tokio::sync::Notify>>>,
    /// The run that carries each conversation, by the run's instance id.
    runs: std::sync::Mutex<std::collections::HashMap<String, (Uuid, String)>>,
}

impl Wakes {
    fn reader(&self, conversation: Uuid) -> Arc<tokio::sync::Notify> {
        self.readers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(conversation)
            .or_default()
            .clone()
    }

    /// Forgets a conversation nobody is waiting on any more.
    fn left(&self, conversation: Uuid, held: Arc<tokio::sync::Notify>) {
        let mut readers = self
            .readers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(held);
        if readers
            .get(&conversation)
            .is_some_and(|notify| Arc::strong_count(notify) == 1)
        {
            readers.remove(&conversation);
        }
    }

    /// The conversation said something: whoever waits on it reads.
    fn said(&self, conversation: Uuid) {
        let waiting = self
            .readers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&conversation)
            .cloned();
        if let Some(waiting) = waiting {
            waiting.notify_waiters();
        }
    }

    fn carried_by(&self, conversation: Uuid, tenant: &str, instance: Option<&str>) {
        let mut runs = self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runs.retain(|_, (carried, _)| *carried != conversation);
        if let Some(instance) = instance {
            runs.insert(instance.to_owned(), (conversation, tenant.to_owned()));
        }
    }

    fn conversation_of(&self, instance: &str) -> Option<(Uuid, String)> {
        self.runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(instance)
            .cloned()
    }
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
    /// What a person calls it. Left out, it is kept; `null` removes it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schema(value_type = Option<String>)]
    pub title: Option<Option<String>>,
    /// What the client keeps about it: an object of its own, up to 4 KB.
    /// Left out, it is kept; given, it replaces what was there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// What the client keeps about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
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

/// Something a conversation has for its user: one entry of its log.
///
/// Every event has the same envelope. `kind` says which it is and `body`
/// what it carries; the two are one type, [`ConversationEventKind`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationEvent {
    /// Its place in the log, and the stream's event id.
    pub seq: i64,
    /// The id of the message this event is for: the one that started the
    /// run, or the one that answered what it waited for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    #[serde(flatten)]
    pub said: ConversationEventKind,
    pub created_at: DateTime<Utc>,
}

impl ConversationEvent {
    /// The event's kind, as it is written.
    pub fn kind(&self) -> &'static str {
        self.said.kind()
    }

    /// What the event carries, as JSON.
    pub fn body(&self) -> Value {
        self.said.body()
    }
}

/// What a text is written in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextFormat {
    #[default]
    Markdown,
    Plain,
}

/// A run began to carry the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunStarted {
    pub instance_id: String,
    /// The message that started it.
    pub message_id: String,
    /// The message's text. Read the `message` event for it: this is kept
    /// for clients written before that event, and will go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The message's sender. As `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<Sender>,
}

/// What a workflow told its user.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    /// The workflow's own id for it: a reply with an id is said once for
    /// its run, however often the step that says it is run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// What `text` is written in.
    #[serde(default)]
    pub format: TextFormat,
    /// Structured data beside the text, for a client that acts on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// The run that said it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
}

/// The run waits to be told something.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WaitingForInput {
    /// Name it as a message's `inReplyTo` to answer this and no other.
    pub request_id: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// The JSON Schema of the answer, where the run asks for a structured
    /// one: send it as the message's `payload`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<Value>,
    pub instance_id: String,
}

/// A message answered what the run waited for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessageDelivered {
    pub message_id: String,
    pub request_id: String,
    pub instance_id: String,
    /// As [`RunStarted::text`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// As [`RunStarted::sender`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<Sender>,
}

/// No run would take a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessageFailed {
    pub message_id: String,
    pub reason: String,
}

/// The run that carried the conversation ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunEnded {
    pub instance_id: String,
    /// `completed`, `failed` or `stopped`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The reply workflow could not be started for an event, and never can be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplyRouteFailed {
    /// The event it was to be started for.
    pub seq: i64,
    pub reply_workflow_id: String,
    pub reason: String,
}

/// What an event says: its `kind`, and the `body` that kind carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum ConversationEventKind {
    /// Someone said this to the workflow.
    Message(Message),
    RunStarted(RunStarted),
    Reply(Reply),
    WaitingForInput(WaitingForInput),
    MessageDelivered(MessageDelivered),
    MessageFailed(MessageFailed),
    RunEnded(RunEnded),
    ReplyRouteFailed(ReplyRouteFailed),
    /// What a run is doing: a model call, a tool call, a step. Its own
    /// `type` says which.
    Activity(Value),
}

impl ConversationEventKind {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Message(_) => "message",
            Self::RunStarted(_) => "run_started",
            Self::Reply(_) => "reply",
            Self::WaitingForInput(_) => "waiting_for_input",
            Self::MessageDelivered(_) => "message_delivered",
            Self::MessageFailed(_) => "message_failed",
            Self::RunEnded(_) => "run_ended",
            Self::ReplyRouteFailed(_) => "reply_route_failed",
            Self::Activity(_) => "activity",
        }
    }

    pub fn body(&self) -> Value {
        serde_json::to_value(self)
            .ok()
            .and_then(|said| said.get("body").cloned())
            .unwrap_or(Value::Null)
    }

    /// The event a stored `kind` and `body` are. `None` for a kind this
    /// build does not know, or a body that is not that kind's.
    pub fn stored(kind: &str, body: Value) -> Option<Self> {
        serde_json::from_value(json!({"kind": kind, "body": body})).ok()
    }
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
        /// It was stopped, and neither completed nor failed.
        stopped: bool,
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
    /// Asks the run to stop. It ends when it has, as any stopped run does.
    async fn stop(&self, tenant: &str, instance: &str) -> Result<(), String>;
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
    wakes: Arc<Wakes>,
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
    /// The message the conversation's events are for now.
    turn: Option<String>,
    title: Option<String>,
    metadata: Option<Value>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "conversation_id, tenant_id, workflow_id, key, reply_workflow_id, \
    instance_id, run_started_at, events_read, run_replied, waiting, next_seq, turn, title, \
    metadata, created_at, updated_at";

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
        turn: row.try_get("turn")?,
        title: row.try_get("title")?,
        metadata: row.try_get("metadata")?,
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
            wakes: Arc::default(),
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
        if let Some(Some(title)) = &open.title {
            checked("the title", title, MAX_TITLE)?;
        }
        if let Some(metadata) = &open.metadata {
            if !metadata.is_object() {
                return Err(ConversationError::Invalid("metadata is an object".into()));
            }
            if metadata.to_string().len() > MAX_METADATA_BYTES {
                return Err(ConversationError::Invalid(format!(
                    "metadata is over {MAX_METADATA_BYTES} bytes"
                )));
            }
        }
        let (sets_route, route) = match open.reply_workflow_id {
            Some(route) => (true, route),
            None => (false, None),
        };
        let (sets_title, title) = match open.title {
            Some(title) => (true, title),
            None => (false, None),
        };
        let changes = sets_route || sets_title || open.metadata.is_some();
        sqlx::query(
            "INSERT INTO conversations (tenant_id, workflow_id, key, reply_workflow_id, title, metadata) \
             VALUES ($1, $2, $3, $5, $7, $8) \
             ON CONFLICT (tenant_id, workflow_id, key) DO UPDATE SET \
                 reply_workflow_id = CASE WHEN $4 THEN $5 ELSE conversations.reply_workflow_id END, \
                 title = CASE WHEN $6 THEN $7 ELSE conversations.title END, \
                 metadata = COALESCE($8, conversations.metadata), \
                 updated_at = CASE WHEN $9 THEN now() ELSE conversations.updated_at END",
        )
        .bind(tenant)
        .bind(workflow)
        .bind(key)
        .bind(sets_route)
        .bind(route)
        .bind(sets_title)
        .bind(title)
        .bind(open.metadata)
        .bind(changes)
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
            title: row.title,
            metadata: row.metadata,
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
        if let Some(request) = &message.in_reply_to {
            checked("what the message answers", request, MAX_KEY)?;
        }
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
        .bind(&body)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        // It is in the log from when it was said, whenever its turn comes.
        sqlx::query(
            "WITH taken AS ( \
                 UPDATE conversations SET next_seq = next_seq + 1, updated_at = now() \
                 WHERE conversation_id = $1 RETURNING next_seq - 1 AS seq) \
             INSERT INTO conversation_events (conversation_id, tenant_id, seq, kind, body, turn) \
             SELECT $1, $2, seq, 'message', $3, $4 FROM taken",
        )
        .bind(conversation)
        .bind(tenant)
        .bind(&body)
        .bind(&message.id)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        self.wakes.said(conversation);

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
        let read = ReadEvents {
            after,
            limit,
            ..ReadEvents::default()
        };
        self.events_of(tenant, workflow, key, &read).await
    }

    /// The events `read` asks for, as they stand: after `after` in order,
    /// or newest first from the end or from `before`, of the kinds it names.
    async fn events_of(
        &self,
        tenant: &str,
        workflow: &str,
        key: &str,
        read: &ReadEvents,
    ) -> ConversationResult<Vec<ConversationEvent>> {
        let row = self.find(tenant, workflow, key).await?;
        let back = read.newest_first || read.before.is_some();
        let sql = format!(
            "SELECT seq, kind, body, turn, created_at FROM conversation_events \
             WHERE conversation_id = $1 AND seq > $2 AND ($3::bigint IS NULL OR seq < $3) \
               AND (cardinality($4::text[]) = 0 OR kind = ANY($4)) \
             ORDER BY seq {} LIMIT $5",
            if back { "DESC" } else { "ASC" }
        );
        let rows = sqlx::query(&sql)
            .bind(row.id)
            .bind(read.after)
            .bind(read.before)
            .bind(&read.kinds)
            .bind(i64::from(read.limit.clamp(1, 500)))
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        let mut events = Vec::with_capacity(rows.len());
        for row in &rows {
            let seq: i64 = row.try_get("seq").map_err(storage)?;
            let kind: String = row.try_get("kind").map_err(storage)?;
            // A kind a later build wrote, read by this one after a rollback,
            // is passed over and not an error.
            let Some(said) =
                ConversationEventKind::stored(&kind, row.try_get("body").map_err(storage)?)
            else {
                tracing::debug!(%seq, %kind, "A conversation's event is of no kind known here");
                continue;
            };
            events.push(ConversationEvent {
                seq,
                turn: row.try_get("turn").map_err(storage)?,
                said,
                created_at: row.try_get("created_at").map_err(storage)?,
            });
        }
        Ok(events)
    }

    /// The events after `read.after`, waiting up to `read.wait_ms` for one
    /// when there is none. It answers as soon as the conversation says
    /// something, and empty when the time is up.
    pub async fn read(
        &self,
        tenant: &str,
        workflow: &str,
        key: &str,
        read: ReadEvents,
    ) -> ConversationResult<Vec<ConversationEvent>> {
        let wait = Duration::from_millis(u64::from(read.wait_ms)).min(MAX_WAIT);
        // What is read back is there already: nothing is waited for.
        if wait.is_zero() || read.newest_first || read.before.is_some() {
            return self.events_of(tenant, workflow, key, &read).await;
        }
        let conversation = self.find(tenant, workflow, key).await?.id;
        let until = tokio::time::Instant::now() + wait;
        let waiting = self.wakes.reader(conversation);
        let answered = async {
            loop {
                // Listening before looking: what is said between the two is
                // not missed.
                let woken = waiting.notified();
                tokio::pin!(woken);
                woken.as_mut().enable();
                let events = self.events_of(tenant, workflow, key, &read).await?;
                if !events.is_empty() {
                    return Ok(events);
                }
                if tokio::time::timeout_at(until, woken).await.is_err() {
                    return Ok(Vec::new());
                }
            }
        }
        .await;
        self.wakes.left(conversation, waiting);
        answered
    }

    /// Sends a message and waits up to `wait` for its turn to be over: for
    /// the run to wait to be told something, or to end, or for the message
    /// to fail. The answer carries the turn's events as far as they went.
    pub async fn send_and_wait(
        &self,
        tenant: &str,
        workflow: &str,
        key: &str,
        message: Message,
        wait: Duration,
    ) -> ConversationResult<Sent> {
        let id = message.id.clone();
        let mut sent = self.send(tenant, workflow, key, message).await?;
        if wait.is_zero() {
            return Ok(sent);
        }
        let conversation = self.find(tenant, workflow, key).await?.id;
        // From the message itself: its turn is what follows it.
        let said: Option<i64> = sqlx::query_scalar(
            "SELECT min(seq) FROM conversation_events \
             WHERE conversation_id = $1 AND kind = 'message' AND turn = $2",
        )
        .bind(conversation)
        .bind(&id)
        .fetch_one(&self.pool)
        .await
        .map_err(storage)?;
        let mut after = said.map_or(0, |seq| seq - 1);
        let until = tokio::time::Instant::now() + wait.min(MAX_SEND_WAIT);
        let mut turn = TurnSoFar {
            events: Vec::new(),
            settled: false,
            last_seq: after,
        };
        while !turn.settled {
            let left = until.saturating_duration_since(tokio::time::Instant::now());
            let read = ReadEvents {
                after,
                limit: EVENTS_PAGE,
                wait_ms: left.min(MAX_WAIT).as_millis() as u32,
                ..ReadEvents::default()
            };
            let events = self.read(tenant, workflow, key, read).await?;
            if events.is_empty() && left.is_zero() {
                break;
            }
            for event in events {
                after = event.seq;
                turn.last_seq = event.seq;
                if event.turn.as_deref() != Some(id.as_str()) {
                    continue;
                }
                turn.settled = matches!(
                    event.said,
                    ConversationEventKind::WaitingForInput(_)
                        | ConversationEventKind::RunEnded(_)
                        | ConversationEventKind::MessageFailed(_)
                );
                turn.events.push(event);
                if turn.settled {
                    break;
                }
            }
        }
        // What became of the message, now that it has had its turn.
        let mut tx = self.pool.begin().await.map_err(storage)?;
        if let Some(now) = self::sent(&mut tx, conversation, &id, sent.duplicate).await? {
            sent = now;
        }
        sent.turn = Some(turn);
        Ok(sent)
    }

    /// The conversation the run `instance` carries, and its tenant, if this
    /// process knows it to carry one.
    pub fn carried_by(&self, instance: &str) -> Option<(Uuid, String)> {
        self.wakes.conversation_of(instance)
    }

    /// A workflow's conversations, most recently active first.
    pub async fn list(
        &self,
        tenant: &str,
        workflow: &str,
        list: ListConversations,
    ) -> ConversationResult<ConversationList> {
        checked("the workflow", workflow, MAX_KEY)?;
        let limit = i64::from(match list.limit {
            0 => 50,
            limit => limit.min(200),
        });
        // A cursor is where the last page ended: when, and which.
        let from = match list.cursor.as_deref() {
            None => None,
            Some(cursor) => Some(
                cursor
                    .split_once('_')
                    .and_then(|(at, id)| {
                        Some((
                            DateTime::<Utc>::from_timestamp_micros(at.parse().ok()?)?,
                            id.parse::<Uuid>().ok()?,
                        ))
                    })
                    .ok_or_else(|| ConversationError::Invalid("not a cursor".into()))?,
            ),
        };
        // `%` and `_` in a prefix are themselves, not wildcards.
        let prefix = list
            .prefix
            .filter(|prefix| !prefix.is_empty())
            .map(|prefix| {
                format!(
                    "{}%",
                    prefix
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
            });
        let rows = sqlx::query(
            "SELECT c.conversation_id, c.key, c.title, c.metadata, c.instance_id, c.waiting, \
                    c.created_at, c.updated_at, said.kind, said.text, said.created_at AS said_at \
             FROM conversations c \
             LEFT JOIN LATERAL ( \
                 SELECT e.kind, left(e.body->>'text', $6) AS text, e.created_at \
                 FROM conversation_events e \
                 WHERE e.conversation_id = c.conversation_id AND e.kind IN ('message', 'reply') \
                 ORDER BY e.seq DESC LIMIT 1) said ON true \
             WHERE c.tenant_id = $1 AND c.workflow_id = $2 \
               AND ($3::text IS NULL OR c.key LIKE $3) \
               AND ($4::timestamptz IS NULL OR (c.updated_at, c.conversation_id) < ($4, $5)) \
             ORDER BY c.updated_at DESC, c.conversation_id DESC LIMIT $7",
        )
        .bind(tenant)
        .bind(workflow)
        .bind(prefix)
        .bind(from.map(|(at, _)| at))
        .bind(from.map(|(_, id)| id))
        .bind(LAST_SAID_CHARS)
        .bind(limit + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        let mut conversations = Vec::with_capacity(rows.len());
        for row in &rows {
            let said_at: Option<DateTime<Utc>> = row.try_get("said_at").map_err(storage)?;
            let last_said = match said_at {
                None => None,
                Some(at) => Some(LastSaid {
                    kind: row.try_get("kind").map_err(storage)?,
                    text: row.try_get("text").map_err(storage)?,
                    at,
                }),
            };
            let instance: Option<String> = row.try_get("instance_id").map_err(storage)?;
            let waiting: Option<Value> = row.try_get("waiting").map_err(storage)?;
            conversations.push(ConversationSummary {
                id: row.try_get("conversation_id").map_err(storage)?,
                key: row.try_get("key").map_err(storage)?,
                title: row.try_get("title").map_err(storage)?,
                metadata: row.try_get("metadata").map_err(storage)?,
                active: instance.is_some(),
                waiting: waiting.is_some(),
                last_said,
                created_at: row.try_get("created_at").map_err(storage)?,
                updated_at: row.try_get("updated_at").map_err(storage)?,
            });
        }
        let more = conversations.len() as i64 > limit;
        conversations.truncate(limit as usize);
        let next = conversations
            .last()
            .filter(|_| more)
            .map(|last| format!("{}_{}", last.updated_at.timestamp_micros(), last.id));
        Ok(ConversationList {
            conversations,
            next,
        })
    }

    /// Stops the run that carries the conversation, as stopping an instance
    /// does: it ends when it has, and the conversation says so then. The
    /// messages waiting their turn keep it, and the next starts the next
    /// run, unless `clear_queue`, which fails each of them now.
    pub async fn stop(
        &self,
        tenant: &str,
        workflow: &str,
        key: &str,
        clear_queue: bool,
    ) -> ConversationResult<Stopped> {
        let conversation = self.find(tenant, workflow, key).await?.id;
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM conversations WHERE conversation_id = $1 FOR UPDATE"
        ))
        .bind(conversation)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .ok_or(ConversationError::NotFound)?;
        let mut at = Carried {
            row: read(&row).map_err(storage)?,
            tx,
        };
        let mut cleared = 0;
        if clear_queue {
            while let Some((message, _)) = at.next_message().await? {
                at.fail(&message, None, STOPPED).await?;
                cleared += 1;
            }
        }
        let instance = at.row.instance_id.clone();
        at.save().await?;
        at.tx.commit().await.map_err(storage)?;
        if cleared > 0 {
            self.wakes.said(conversation);
        }
        if let Some(instance) = &instance {
            self.runs
                .stop(tenant, instance)
                .await
                .map_err(ConversationError::Storage)?;
        }
        Ok(Stopped {
            instance_id: instance,
            cleared,
        })
    }

    /// Removes what is past keeping, across tenants: a conversation nothing
    /// has touched for `kept` and that nothing carries, one whose workflow
    /// is gone, and a run's working (`activity`) older than `activity_kept`.
    /// `None` keeps for ever. Returns how many conversations and how many
    /// events were removed.
    pub async fn sweep(
        &self,
        kept: Option<Duration>,
        activity_kept: Option<Duration>,
    ) -> ConversationResult<(u64, u64)> {
        let seconds = |kept: Duration| kept.as_secs() as f64;
        let mut conversations = sqlx::query(
            "DELETE FROM conversations c WHERE c.instance_id IS NULL AND EXISTS ( \
                 SELECT 1 FROM workflows w \
                 WHERE w.tenant_id = c.tenant_id AND w.workflow_id = c.workflow_id \
                   AND w.deleted_at IS NOT NULL)",
        )
        .execute(&self.pool)
        .await
        .map_err(storage)?
        .rows_affected();
        if let Some(kept) = kept {
            conversations += sqlx::query(
                "DELETE FROM conversations c \
                 WHERE c.instance_id IS NULL \
                   AND c.updated_at < now() - make_interval(secs => $1) \
                   AND NOT EXISTS (SELECT 1 FROM conversation_messages m \
                                   WHERE m.conversation_id = c.conversation_id \
                                     AND m.state = 'queued')",
            )
            .bind(seconds(kept))
            .execute(&self.pool)
            .await
            .map_err(storage)?
            .rows_affected();
        }
        let mut events = 0;
        if let Some(kept) = activity_kept {
            events = sqlx::query(
                "DELETE FROM conversation_events \
                 WHERE kind = 'activity' AND created_at < now() - make_interval(secs => $1)",
            )
            .bind(seconds(kept))
            .execute(&self.pool)
            .await
            .map_err(storage)?
            .rows_affected();
        }
        Ok((conversations, events))
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
            "SELECT c.conversation_id, c.tenant_id, c.instance_id FROM conversations c \
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
        let mut unsettled = Vec::with_capacity(rows.len());
        for row in &rows {
            let conversation: Uuid = row.try_get(0).map_err(storage)?;
            let tenant: String = row.try_get(1).map_err(storage)?;
            let instance: Option<String> = row.try_get(2).map_err(storage)?;
            // Which run carries it is kept in mind, for a run's notice to
            // find its conversation by: here for one this process did not
            // start itself (it was restarted since).
            self.wakes
                .carried_by(conversation, &tenant, instance.as_deref());
            unsettled.push((conversation, tenant));
        }
        Ok(unsettled)
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
        let row_next_seq = at.row.next_seq;
        for _ in 0..ROUNDS {
            if !self.round(&mut at).await? {
                break;
            }
        }
        let said_before = row_next_seq;
        self.route(&mut at).await?;
        at.save().await?;
        let (tenant, instance, said_now) = (
            at.row.tenant_id.clone(),
            at.row.instance_id.clone(),
            at.row.next_seq,
        );
        at.tx.commit().await.map_err(storage)?;
        // Told once it can be read: after the commit, never before.
        self.wakes
            .carried_by(conversation, &tenant, instance.as_deref());
        if said_now != said_before {
            self.wakes.said(conversation);
        }
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
            at.ended(&instance, "failed", Some("the run did not start".into()))
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
                    // What a run says is its own to shape: what is not a
                    // reply's is left out, not refused.
                    let mut reply: Reply = serde_json::from_value(said.payload).unwrap_or_default();
                    reply.instance_id = Some(instance.clone());
                    at.say(ConversationEventKind::Reply(reply)).await?;
                    continue;
                }
                for activity in crate::api::services::run_activity::parse_debug_event(
                    said.kind.as_deref(),
                    &said.payload,
                ) {
                    // What a run waits for is said from its open requests,
                    // which are current; this is a record of one.
                    if matches!(
                        activity,
                        crate::api::services::run_activity::Activity::WaitingForInput { .. }
                    ) {
                        continue;
                    }
                    if let Ok(body) = serde_json::to_value(&activity) {
                        at.say(ConversationEventKind::Activity(body)).await?;
                    }
                }
            }
            if read < EVENTS_PAGE as usize {
                break;
            }
        }

        if let Run::Ended {
            completed,
            stopped,
            output,
            error,
        } = run
        {
            // A run that said nothing answers with what it returned.
            if completed
                && !at.row.run_replied
                && let Some(text) = answer_in(output.as_ref())
            {
                at.say(ConversationEventKind::Reply(Reply {
                    text: Some(text),
                    instance_id: Some(instance.clone()),
                    ..Reply::default()
                }))
                .await?;
            }
            let status = match (completed, stopped) {
                (true, _) => "completed",
                (false, true) => "stopped",
                (false, false) => "failed",
            };
            at.ended(&instance, status, error).await?;
            return Ok(true);
        }

        let waits = match self.runs.waits_for(&tenant, &instance).await {
            Ok(waits) => waits,
            Err(error) => {
                tracing::debug!(%instance, %error, "What a conversation's run waits for could not be read");
                return Ok(false);
            }
        };
        let Some(first) = waits.first().cloned() else {
            at.row.waiting = None;
            return Ok(false);
        };
        let announced = at
            .row
            .waiting
            .as_ref()
            .and_then(|waiting| waiting.get("requestId"))
            .and_then(Value::as_str)
            == Some(first.request_id.as_str());
        if !announced {
            let waiting = WaitingForInput {
                request_id: first.request_id.clone(),
                message: first.message.clone(),
                tool_name: first.tool_name.clone(),
                response_schema: first.response_schema.clone(),
                instance_id: instance.clone(),
            };
            at.row.waiting = serde_json::to_value(&waiting).ok();
            at.say(ConversationEventKind::WaitingForInput(waiting))
                .await?;
        }

        let Some((message_id, message)) = at.next_message().await? else {
            return Ok(false);
        };
        // A message that names what it answers answers that, or nothing: an
        // answer written for one question is not given to another.
        let wait = match &message.in_reply_to {
            None => first,
            Some(request) => match waits.into_iter().find(|wait| wait.request_id == *request) {
                Some(wait) => wait,
                None => {
                    at.fail(&message_id, Some(&instance), STALE).await?;
                    return Ok(true);
                }
            },
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
                at.row.turn = Some(message_id.clone());
                at.say(ConversationEventKind::MessageDelivered(MessageDelivered {
                    message_id,
                    request_id: wait.request_id,
                    instance_id: instance,
                    text: message.text,
                    sender: message.sender,
                }))
                .await?;
                at.row.waiting = None;
                Ok(true)
            }
            Err(AnswerError::Invalid(reason)) => {
                at.fail(&message_id, Some(&instance), &reason).await?;
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
        // What it answers was a run's that is gone.
        if message.in_reply_to.is_some() {
            at.fail(&message_id, None, STALE).await?;
            return Ok(true);
        }
        // The run's id is made from the message's, so starting it again
        // after a failure here starts the same run.
        let instance = Uuid::new_v5(&at.row.id, format!("message:{message_id}").as_bytes());
        // What a message does not carry is left out, not sent as null: a
        // workflow's input schema that says `message` is a string would
        // refuse a null one.
        let mut data = serde_json::Map::new();
        data.insert("messageId".into(), json!(message_id));
        data.insert("conversation".into(), at.describe());
        data.insert("attachments".into(), json!(message.attachments));
        for (name, value) in [
            ("message", json!(message.text)),
            ("payload", json!(message.payload)),
            ("sender", json!(message.sender)),
        ] {
            if !value.is_null() {
                data.insert(name.into(), value);
            }
        }
        let inputs = json!({"data": data, "variables": {}});
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
                at.row.turn = Some(message_id.clone());
                at.say(ConversationEventKind::RunStarted(RunStarted {
                    instance_id: instance,
                    message_id,
                    text: message.text,
                    sender: message.sender,
                }))
                .await?;
                Ok(true)
            }
            Err(StartError::Refused(reason)) => {
                at.fail(&message_id, None, &reason).await?;
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
                    at.say(ConversationEventKind::ReplyRouteFailed(ReplyRouteFailed {
                        seq,
                        reply_workflow_id: route.clone(),
                        reason,
                    }))
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

    /// Says `said` in the turn the conversation is in.
    async fn say(&mut self, said: ConversationEventKind) -> ConversationResult<()> {
        let turn = self.row.turn.clone();
        self.say_for(turn.as_deref(), said).await
    }

    /// Says `said`, for the message `turn` names.
    async fn say_for(
        &mut self,
        turn: Option<&str>,
        said: ConversationEventKind,
    ) -> ConversationResult<()> {
        // A reply that carries an id is said once for its run: the same one
        // again conflicts and is passed over.
        let written = sqlx::query(
            "INSERT INTO conversation_events (conversation_id, tenant_id, seq, kind, body, turn) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(self.row.id)
        .bind(&self.row.tenant_id)
        .bind(self.row.next_seq)
        .bind(said.kind())
        .bind(said.body())
        .bind(turn)
        .execute(&mut *self.tx)
        .await
        .map_err(storage)?;
        self.row.next_seq += written.rows_affected() as i64;
        Ok(())
    }

    /// No run takes `message`: it fails, and its sender is told why.
    async fn fail(
        &mut self,
        message: &str,
        instance: Option<&str>,
        reason: &str,
    ) -> ConversationResult<()> {
        self.settle(message, "failed", instance, Some(reason))
            .await?;
        self.say_for(
            Some(message),
            ConversationEventKind::MessageFailed(MessageFailed {
                message_id: message.to_owned(),
                reason: reason.to_owned(),
            }),
        )
        .await
    }

    /// The run ended: the conversation has none, until its next message.
    async fn ended(
        &mut self,
        instance: &str,
        status: &str,
        error: Option<String>,
    ) -> ConversationResult<()> {
        self.say(ConversationEventKind::RunEnded(RunEnded {
            instance_id: instance.to_owned(),
            status: status.to_owned(),
            error,
        }))
        .await?;
        self.row.instance_id = None;
        self.row.run_started_at = None;
        self.row.events_read = 0;
        self.row.run_replied = false;
        self.row.waiting = None;
        self.row.turn = None;
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
                 run_replied = $5, waiting = $6, next_seq = $7, turn = $8, \
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
        .bind(&self.row.turn)
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
        turn: None,
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

/// How long what was said is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keeping {
    /// A conversation, from when it was last active. `None` for ever.
    pub conversations: Option<Duration>,
    /// A run's working (`activity` events). `None` for ever.
    pub activity: Option<Duration>,
}

impl Keeping {
    /// For ever: nothing is removed for its age.
    pub const FOR_EVER: Self = Self {
        conversations: None,
        activity: None,
    };

    /// As the deployment set it: `RUNTARA_CONVERSATION_RETENTION_DAYS` (90)
    /// and `RUNTARA_CONVERSATION_ACTIVITY_DAYS` (7), each `0` for ever.
    pub fn from_env() -> Self {
        let days = |name: &str, unless_set: u64| {
            let days = std::env::var(name)
                .ok()
                .and_then(|days| days.trim().parse::<u64>().ok())
                .unwrap_or(unless_set);
            (days > 0).then(|| Duration::from_secs(days * 24 * 60 * 60))
        };
        Self {
            conversations: days("RUNTARA_CONVERSATION_RETENTION_DAYS", 90),
            activity: days("RUNTARA_CONVERSATION_ACTIVITY_DAYS", 7),
        }
    }
}

/// How often what is past keeping is removed.
const SWEPT_EVERY: Duration = Duration::from_secs(60 * 60);

/// How often every conversation that has something to do is looked at,
/// whether or not anything said so: what catches a notice that was missed.
pub const LOOKED_AT_EVERY: Duration = Duration::from_secs(5);

/// Carries every conversation that has something to do, until the server
/// stops. The conversations are found across tenants; each is carried in
/// its own tenant's scope.
///
/// A conversation is carried when its run says it has something new
/// (`told`, the instance ids of such runs), and all of them are looked at
/// every `every`, for what no notice said.
pub async fn carry(
    conversations: Conversations,
    shutdown: crate::shutdown::ShutdownSignal,
    every: Duration,
    mut told: tokio::sync::mpsc::UnboundedReceiver<String>,
    keeping: Keeping,
) {
    use runtara_store_postgres::tenant_scope::{scoped, system};
    let mut telling = true;
    let mut swept: Option<tokio::time::Instant> = None;
    loop {
        // What is past keeping goes, now and then.
        if swept.is_none_or(|swept| swept.elapsed() >= SWEPT_EVERY) {
            swept = Some(tokio::time::Instant::now());
            match system(conversations.sweep(keeping.conversations, keeping.activity)).await {
                Ok((0, 0)) => {}
                Ok((gone, events)) => tracing::info!(
                    conversations = gone,
                    activity_events = events,
                    "Removed conversations past keeping"
                ),
                Err(error) => tracing::warn!(%error, "Conversations past keeping were not removed"),
            }
        }
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
        let look_again = tokio::time::sleep(every);
        tokio::pin!(look_again);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.clone().wait() => return,
                instance = told.recv(), if telling => {
                    let Some(instance) = instance else {
                        // Nothing tells any more: the clock alone remains.
                        telling = false;
                        continue;
                    };
                    // Several notices of one run are one reading of it.
                    let mut instances = std::collections::BTreeSet::from([instance]);
                    while let Ok(more) = told.try_recv() {
                        instances.insert(more);
                    }
                    for instance in instances {
                        let Some((conversation, tenant)) = conversations.carried_by(&instance)
                        else {
                            continue;
                        };
                        if let Err(error) =
                            scoped(tenant, conversations.advance(conversation)).await
                        {
                            tracing::warn!(%conversation, %error, "A conversation was not advanced");
                        }
                    }
                }
                _ = &mut look_again => break,
            }
        }
    }
}

/// Listens for the runs of the runtime database that have something new,
/// and hands each one's instance id on, until the server stops. A listener
/// that loses its connection takes another; what was said meanwhile is
/// found by the look every [`LOOKED_AT_EVERY`].
pub async fn hear_runs(
    runtime: PgPool,
    shutdown: crate::shutdown::ShutdownSignal,
    tell: tokio::sync::mpsc::UnboundedSender<String>,
) {
    loop {
        let heard = async {
            let mut listener = runtara_store_postgres::notices::listen(&runtime).await?;
            loop {
                let notice = listener.recv().await?;
                if tell.send(notice.payload().to_owned()).is_err() {
                    return Ok::<_, sqlx::Error>(());
                }
            }
        };
        tokio::select! {
            biased;
            _ = shutdown.clone().wait() => return,
            heard = heard => match heard {
                Ok(()) => return,
                Err(error) => {
                    tracing::warn!(%error, "The runs' notices are not heard; listening again");
                }
            },
        }
        tokio::select! {
            biased;
            _ = shutdown.clone().wait() => return,
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
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
                stopped: info.status == InstanceStatus::Cancelled,
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

    async fn stop(&self, tenant: &str, instance: &str) -> Result<(), String> {
        self.executions
            .stop(tenant, instance)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind an event may be, with a body of that kind.
    fn one_of_each() -> Vec<ConversationEventKind> {
        vec![
            ConversationEventKind::Message(Message {
                id: "m-1".into(),
                text: Some("hello".into()),
                payload: None,
                sender: None,
                attachments: Vec::new(),
                in_reply_to: None,
            }),
            ConversationEventKind::RunStarted(RunStarted {
                instance_id: "i".into(),
                message_id: "m-1".into(),
                text: None,
                sender: None,
            }),
            ConversationEventKind::Reply(Reply::default()),
            ConversationEventKind::WaitingForInput(WaitingForInput {
                request_id: "r".into(),
                message: "?".into(),
                tool_name: None,
                response_schema: None,
                instance_id: "i".into(),
            }),
            ConversationEventKind::MessageDelivered(MessageDelivered {
                message_id: "m-2".into(),
                request_id: "r".into(),
                instance_id: "i".into(),
                text: None,
                sender: None,
            }),
            ConversationEventKind::MessageFailed(MessageFailed {
                message_id: "m-3".into(),
                reason: "no".into(),
            }),
            ConversationEventKind::RunEnded(RunEnded {
                instance_id: "i".into(),
                status: "completed".into(),
                error: None,
            }),
            ConversationEventKind::ReplyRouteFailed(ReplyRouteFailed {
                seq: 4,
                reply_workflow_id: "out".into(),
                reason: "gone".into(),
            }),
            ConversationEventKind::Activity(json!({"type": "tool_call", "tool_name": "t"})),
        ]
    }

    /// An event is one envelope: its place, its turn, its kind and that
    /// kind's body. What is stored is read back as what was said, and a kind
    /// this build does not know is passed over.
    #[test]
    fn an_event_is_one_envelope_and_is_read_back_as_it_was_said() {
        for said in one_of_each() {
            let event = ConversationEvent {
                seq: 7,
                turn: Some("m-1".into()),
                said: said.clone(),
                created_at: Utc::now(),
            };
            let written = serde_json::to_value(&event).unwrap();
            assert_eq!(written["seq"], 7);
            assert_eq!(written["turn"], "m-1");
            assert_eq!(written["kind"], said.kind());
            assert_eq!(written["body"], said.body());
            assert!(written.get("createdAt").is_some());
            // As the engine's wire carries it, and as the table holds it.
            assert_eq!(
                serde_json::from_value::<ConversationEvent>(written).unwrap(),
                event
            );
            assert_eq!(
                ConversationEventKind::stored(said.kind(), said.body()),
                Some(said)
            );
        }
        assert_eq!(
            ConversationEventKind::stored("of_a_later_build", json!({})),
            None
        );
        assert_eq!(
            ConversationEventKind::stored("run_ended", json!("not a body")),
            None
        );
        // A reply a run shaped loosely is still a reply.
        let reply: Reply = serde_json::from_value(json!({"text": "hi", "more": 1})).unwrap();
        assert_eq!(reply.format, TextFormat::Markdown);
    }

    /// The API's document says each kind and its body, so a client's types
    /// are generated and not written from this file.
    #[test]
    fn the_api_document_names_every_kind_of_event() {
        use utoipa::OpenApi;
        let document = serde_json::to_value(crate::server::ApiDoc::openapi()).unwrap();
        let schemas = &document["components"]["schemas"];
        let kinds = serde_json::to_string(&schemas["ConversationEventKind"]).unwrap();
        for said in one_of_each() {
            assert!(
                kinds.contains(&format!("\"{}\"", said.kind())),
                "{} is not in the document: {kinds}",
                said.kind()
            );
        }
        for body in [
            "RunStarted",
            "Reply",
            "WaitingForInput",
            "MessageDelivered",
            "MessageFailed",
            "RunEnded",
            "ReplyRouteFailed",
            "Message",
            "TextFormat",
        ] {
            assert!(schemas.get(body).is_some(), "{body} is not in the document");
        }
        let event = serde_json::to_string(&schemas["ConversationEvent"]).unwrap();
        for field in ["seq", "turn", "createdAt", "ConversationEventKind"] {
            assert!(
                event.contains(field),
                "{field} is not in the event: {event}"
            );
        }
    }
}
