// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A conversation with a workflow, over the server database and runs that
//! are stood in for: what starts a run, what waits its turn, what a run's
//! replies become, and what the reply workflow is started with.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use runtara_server::api::services::conversations::{
    AnswerError, ConversationError, ConversationEvent, Conversations, Message, MessageState,
    OpenConversation, REPLY_EVENT, Run, Runs, Said, Sender, StartError, Wait,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

async fn pool() -> PgPool {
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect("db-integration-tests requires a server database URL");
    let pool = PgPool::connect(&url)
        .await
        .expect("required server test database must accept connections");
    MIGRATOR
        .run(&pool)
        .await
        .expect("required server migrations must succeed");
    pool
}

#[derive(Clone)]
struct Started {
    tenant: String,
    workflow: String,
    instance: String,
    inputs: Value,
}

struct StoodIn {
    run: Run,
    said: Vec<Said>,
    waits: Vec<Wait>,
}

/// Runs that do what the test says.
#[derive(Default)]
struct StandIn {
    started: Mutex<Vec<Started>>,
    runs: Mutex<HashMap<String, StoodIn>>,
    answers: Mutex<Vec<(String, String, String, Value)>>,
    refused: Mutex<Vec<String>>,
    unavailable: Mutex<bool>,
    stopped: Mutex<Vec<String>>,
}

impl StandIn {
    fn started(&self, workflow: &str) -> Vec<Started> {
        self.started
            .lock()
            .unwrap()
            .iter()
            .filter(|started| started.workflow == workflow)
            .cloned()
            .collect()
    }

    fn with(&self, instance: &str, change: impl FnOnce(&mut StoodIn)) {
        change(self.runs.lock().unwrap().get_mut(instance).expect("a run"));
    }

    fn says(&self, instance: &str, kind: &str, payload: Value) {
        self.with(instance, |run| {
            run.said.push(Said {
                kind: Some(kind.to_owned()),
                payload,
            })
        });
    }

    fn ends(&self, instance: &str, output: Value) {
        self.with(instance, |run| {
            run.waits.clear();
            run.run = Run::Ended {
                completed: true,
                stopped: false,
                output: Some(output),
                error: None,
            }
        });
    }

    fn waits(&self, instance: &str, request: &str) {
        self.with(instance, |run| {
            run.waits = vec![Wait {
                request_id: request.to_owned(),
                message: "What next?".into(),
                tool_name: None,
                response_schema: None,
            }]
        });
    }
}

#[async_trait]
impl Runs for StandIn {
    async fn start(
        &self,
        tenant: &str,
        workflow: &str,
        instance: Uuid,
        inputs: Value,
    ) -> Result<(), StartError> {
        if *self.unavailable.lock().unwrap() {
            return Err(StartError::Unavailable("not now".into()));
        }
        if self.refused.lock().unwrap().iter().any(|w| w == workflow) {
            return Err(StartError::Refused(format!(
                "Workflow not found: {workflow}"
            )));
        }
        let instance = instance.to_string();
        let mut runs = self.runs.lock().unwrap();
        if runs.contains_key(&instance) {
            return Ok(());
        }
        runs.insert(
            instance.clone(),
            StoodIn {
                run: Run::Active,
                said: Vec::new(),
                waits: Vec::new(),
            },
        );
        self.started.lock().unwrap().push(Started {
            tenant: tenant.to_owned(),
            workflow: workflow.to_owned(),
            instance,
            inputs,
        });
        Ok(())
    }

    async fn look(&self, _: &str, instance: &str) -> Result<Run, String> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .get(instance)
            .map_or(Run::NotYet, |run| run.run.clone()))
    }

    async fn said(&self, _: &str, instance: &str, after: u32) -> Result<Vec<Said>, String> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .get(instance)
            .map(|run| run.said.iter().skip(after as usize).cloned().collect())
            .unwrap_or_default())
    }

    async fn waits_for(&self, _: &str, instance: &str) -> Result<Vec<Wait>, String> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .get(instance)
            .map(|run| run.waits.clone())
            .unwrap_or_default())
    }

    async fn stop(&self, _: &str, instance: &str) -> Result<(), String> {
        self.stopped.lock().unwrap().push(instance.to_owned());
        self.with(instance, |run| {
            run.waits.clear();
            run.run = Run::Ended {
                completed: false,
                stopped: true,
                output: None,
                error: None,
            }
        });
        Ok(())
    }

    async fn answer(
        &self,
        _: &str,
        instance: &str,
        request: &str,
        operation: &str,
        payload: &Value,
    ) -> Result<(), AnswerError> {
        if payload.get("refuse").is_some() {
            return Err(AnswerError::Invalid("not what was asked".into()));
        }
        let mut runs = self.runs.lock().unwrap();
        let run = runs.get_mut(instance).ok_or(AnswerError::Gone)?;
        if !run.waits.iter().any(|wait| wait.request_id == request) {
            return Err(AnswerError::Gone);
        }
        run.waits.clear();
        self.answers.lock().unwrap().push((
            instance.to_owned(),
            request.to_owned(),
            operation.to_owned(),
            payload.clone(),
        ));
        Ok(())
    }
}

struct Talk {
    conversations: Conversations,
    runs: Arc<StandIn>,
    tenant: String,
    workflow: String,
}

impl Talk {
    async fn new() -> Self {
        let runs = Arc::new(StandIn::default());
        Self {
            conversations: Conversations::new(pool().await, runs.clone()),
            runs,
            tenant: format!("tenant-{}", Uuid::new_v4()),
            workflow: format!("workflow-{}", Uuid::new_v4()),
        }
    }

    async fn send(&self, key: &str, id: &str, text: &str) -> MessageState {
        self.conversations
            .send(&self.tenant, &self.workflow, key, text_message(id, text))
            .await
            .expect("the message is taken")
            .state
    }

    async fn advance(&self, key: &str) {
        let id = self
            .conversations
            .state(&self.tenant, &self.workflow, key)
            .await
            .expect("the conversation")
            .id;
        assert!(self.conversations.advance(id).await.expect("advanced"));
    }

    async fn events(&self, key: &str) -> Vec<ConversationEvent> {
        self.conversations
            .events(&self.tenant, &self.workflow, key, 0, 500)
            .await
            .expect("the events")
    }

    async fn kinds(&self, key: &str) -> Vec<String> {
        self.events(key)
            .await
            .into_iter()
            .map(|e| e.kind().to_owned())
            .collect()
    }

    async fn replies(&self, key: &str) -> Vec<String> {
        self.events(key)
            .await
            .into_iter()
            .filter(|event| event.kind() == "reply")
            .map(|event| event.body()["text"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    fn runs_of_workflow(&self) -> Vec<Started> {
        self.runs.started(&self.workflow)
    }
}

fn text_message(id: &str, text: &str) -> Message {
    Message {
        id: id.to_owned(),
        text: Some(text.to_owned()),
        payload: None,
        sender: None,
        attachments: Vec::new(),
        in_reply_to: None,
    }
}

#[tokio::test]
async fn a_conversation_is_carried_by_one_run_after_another() {
    let talk = Talk::new().await;

    assert_eq!(talk.send("k", "m-1", "hello").await, MessageState::Started);
    let first = talk.runs_of_workflow();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].tenant, talk.tenant);
    assert_eq!(first[0].inputs["data"]["message"], "hello");
    assert_eq!(first[0].inputs["data"]["messageId"], "m-1");
    assert_eq!(first[0].inputs["data"]["conversation"]["key"], "k");

    talk.runs.ends(
        &first[0].instance,
        json!({"answer": {"response": "hi there"}}),
    );
    talk.advance("k").await;
    assert_eq!(talk.replies("k").await, ["hi there"]);
    let state = talk
        .conversations
        .state(&talk.tenant, &talk.workflow, "k")
        .await
        .unwrap();
    assert_eq!(state.instance_id, None, "the run ended");

    // The same key reaches the same conversation, which starts another run.
    assert_eq!(talk.send("k", "m-2", "again").await, MessageState::Started);
    let runs = talk.runs_of_workflow();
    assert_eq!(runs.len(), 2);
    assert_ne!(runs[0].instance, runs[1].instance);
    talk.runs
        .ends(&runs[1].instance, json!({"response": "once more"}));
    talk.advance("k").await;

    assert_eq!(talk.replies("k").await, ["hi there", "once more"]);
    // The events alone are the transcript: what was sent is in them too.
    let sent: Vec<_> = talk
        .events("k")
        .await
        .into_iter()
        .filter(|event| event.kind() == "run_started")
        .map(|event| event.body()["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(sent, ["hello", "again"]);
    assert_eq!(
        talk.kinds("k").await,
        [
            "message",
            "run_started",
            "reply",
            "run_ended",
            "message",
            "run_started",
            "reply",
            "run_ended"
        ]
    );
}

#[tokio::test]
async fn a_message_sent_while_the_run_is_busy_waits_its_turn() {
    let talk = Talk::new().await;
    assert_eq!(talk.send("k", "m-1", "first").await, MessageState::Started);
    assert_eq!(talk.send("k", "m-2", "second").await, MessageState::Queued);
    assert_eq!(talk.send("k", "m-3", "third").await, MessageState::Queued);
    assert_eq!(talk.runs_of_workflow().len(), 1, "the run is busy");

    let state = talk
        .conversations
        .state(&talk.tenant, &talk.workflow, "k")
        .await
        .unwrap();
    assert_eq!(state.queued, 2);

    // When the run ends the next message starts the next run, in order.
    talk.runs
        .ends(&talk.runs_of_workflow()[0].instance, json!({}));
    talk.advance("k").await;
    let runs = talk.runs_of_workflow();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1].inputs["data"]["message"], "second");

    talk.runs.ends(&runs[1].instance, json!({}));
    talk.advance("k").await;
    let runs = talk.runs_of_workflow();
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[2].inputs["data"]["message"], "third");
}

#[tokio::test]
async fn a_busy_conversation_refuses_a_message_past_its_bound() {
    let mut talk = Talk::new().await;
    talk.conversations = talk.conversations.clone().with_queue_bound(2);

    assert_eq!(talk.send("k", "m-1", "runs").await, MessageState::Started);
    assert_eq!(talk.send("k", "m-2", "waits").await, MessageState::Queued);
    assert_eq!(talk.send("k", "m-3", "waits").await, MessageState::Queued);
    let refused = talk
        .conversations
        .send(
            &talk.tenant,
            &talk.workflow,
            "k",
            text_message("m-4", "too many"),
        )
        .await;
    assert_eq!(refused, Err(ConversationError::QueueFull { bound: 2 }));

    // Refused, not kept: when there is room it is sent as a new message.
    talk.runs
        .ends(&talk.runs_of_workflow()[0].instance, json!({}));
    talk.advance("k").await;
    assert_eq!(talk.send("k", "m-4", "now").await, MessageState::Queued);
}

#[tokio::test]
async fn the_same_message_twice_is_one_message() {
    let talk = Talk::new().await;
    assert_eq!(talk.send("k", "m-1", "hello").await, MessageState::Started);
    let again = talk
        .conversations
        .send(
            &talk.tenant,
            &talk.workflow,
            "k",
            text_message("m-1", "hello"),
        )
        .await
        .unwrap();
    assert!(again.duplicate);
    assert_eq!(again.state, MessageState::Started);
    assert_eq!(
        again.instance_id.as_deref(),
        Some(talk.runs_of_workflow()[0].instance.as_str())
    );
    assert_eq!(talk.runs_of_workflow().len(), 1);

    // And while it waits its turn.
    assert_eq!(talk.send("k", "m-2", "next").await, MessageState::Queued);
    assert_eq!(talk.send("k", "m-2", "next").await, MessageState::Queued);
    let state = talk
        .conversations
        .state(&talk.tenant, &talk.workflow, "k")
        .await
        .unwrap();
    assert_eq!(state.queued, 1);
}

#[tokio::test]
async fn a_run_replies_from_its_middle_and_its_output_is_not_said_twice() {
    let talk = Talk::new().await;
    talk.send("k", "m-1", "work on it").await;
    let instance = talk.runs_of_workflow()[0].instance.clone();

    talk.runs
        .says(&instance, REPLY_EVENT, json!({"text": "On it."}));
    talk.advance("k").await;
    assert_eq!(
        talk.replies("k").await,
        ["On it."],
        "said while the run runs"
    );

    // Read once: advancing again says nothing more.
    talk.advance("k").await;
    assert_eq!(talk.replies("k").await, ["On it."]);

    talk.runs.says(
        &instance,
        "step_debug_start",
        json!({"step_type": "Agent", "step_id": "s1"}),
    );
    talk.runs
        .says(&instance, REPLY_EVENT, json!({"text": "Done."}));
    talk.runs
        .ends(&instance, json!({"answer": {"response": "Done."}}));
    talk.advance("k").await;

    // What the workflow said is what the user is told; a run that replied
    // is not also answered for from its output.
    assert_eq!(talk.replies("k").await, ["On it.", "Done."]);
    let kinds = talk.kinds("k").await;
    assert_eq!(kinds[..2], ["message", "run_started"]);
    assert_eq!(kinds.last().map(String::as_str), Some("run_ended"));
    assert!(kinds.iter().any(|kind| kind == "activity"));
}

#[tokio::test]
async fn a_message_answers_what_the_run_waits_for() {
    let talk = Talk::new().await;
    talk.send("k", "m-1", "start").await;
    let instance = talk.runs_of_workflow()[0].instance.clone();

    talk.runs.waits(&instance, "req-1");
    talk.advance("k").await;
    talk.advance("k").await;
    let waiting: Vec<_> = talk
        .events("k")
        .await
        .into_iter()
        .filter(|event| event.kind() == "waiting_for_input")
        .collect();
    assert_eq!(waiting.len(), 1, "said once, however often it is read");
    assert_eq!(waiting[0].body()["requestId"], "req-1");
    let state = talk
        .conversations
        .state(&talk.tenant, &talk.workflow, "k")
        .await
        .unwrap();
    assert_eq!(state.waiting_for.unwrap()["message"], "What next?");

    assert_eq!(talk.send("k", "m-2", "this").await, MessageState::Delivered);
    assert_eq!(talk.runs_of_workflow().len(), 1, "no second run");
    let answers = talk.runs.answers.lock().unwrap().clone();
    assert_eq!(
        answers,
        [(
            instance.clone(),
            "req-1".into(),
            "m-2".into(),
            json!({"message": "this"})
        )]
    );

    // A structured message is the answer as it is; one the run will not
    // take fails and does not hold up the next.
    talk.runs.waits(&instance, "req-2");
    let refused = talk
        .conversations
        .send(
            &talk.tenant,
            &talk.workflow,
            "k",
            Message {
                payload: Some(json!({"refuse": true})),
                text: None,
                ..text_message("m-3", "")
            },
        )
        .await
        .unwrap();
    assert_eq!(refused.state, MessageState::Failed);
    assert_eq!(refused.detail.as_deref(), Some("not what was asked"));
    let taken = talk
        .conversations
        .send(
            &talk.tenant,
            &talk.workflow,
            "k",
            Message {
                payload: Some(json!({"choice": 2})),
                text: None,
                ..text_message("m-4", "")
            },
        )
        .await
        .unwrap();
    assert_eq!(taken.state, MessageState::Delivered);
    assert_eq!(
        talk.runs.answers.lock().unwrap().last().unwrap().3,
        json!({"choice": 2})
    );
}

#[tokio::test]
async fn the_reply_workflow_is_started_once_for_each_reply() {
    let talk = Talk::new().await;
    let route = format!("reply-{}", Uuid::new_v4());
    talk.conversations
        .open(
            &talk.tenant,
            &talk.workflow,
            "telegram:42",
            OpenConversation {
                reply_workflow_id: Some(Some(route.clone())),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    talk.conversations
        .send(
            &talk.tenant,
            &talk.workflow,
            "telegram:42",
            Message {
                sender: Some(Sender {
                    id: "u-7".into(),
                    name: Some("Ada".into()),
                }),
                ..text_message("m-1", "hello")
            },
        )
        .await
        .unwrap();
    let instance = talk.runs_of_workflow()[0].instance.clone();
    assert_eq!(
        talk.runs_of_workflow()[0].inputs["data"]["sender"]["name"],
        "Ada"
    );

    talk.runs
        .says(&instance, REPLY_EVENT, json!({"text": "one"}));
    talk.advance("telegram:42").await;
    talk.runs
        .says(&instance, REPLY_EVENT, json!({"text": "two"}));
    talk.runs.waits(&instance, "req-1");
    talk.advance("telegram:42").await;
    talk.advance("telegram:42").await;

    let routed = talk.runs.started(&route);
    let told: Vec<_> = routed
        .iter()
        .map(|run| {
            let event = &run.inputs["data"]["event"];
            (
                event["kind"].as_str().unwrap().to_owned(),
                event["body"]["text"].as_str().map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        told,
        [
            ("reply".to_owned(), Some("one".to_owned())),
            ("reply".to_owned(), Some("two".to_owned())),
            ("waiting_for_input".to_owned(), None),
        ],
        "each reply and each wait once, in order, and nothing else"
    );
    assert!(routed.iter().all(|run| {
        run.tenant == talk.tenant && run.inputs["data"]["conversation"]["key"] == "telegram:42"
    }));

    // The reply workflow's own runs are not the conversation's.
    assert_eq!(talk.runs_of_workflow().len(), 1);
}

#[tokio::test]
async fn a_reply_waits_for_a_reply_workflow_that_cannot_be_started_now() {
    let talk = Talk::new().await;
    let route = format!("reply-{}", Uuid::new_v4());
    talk.conversations
        .open(
            &talk.tenant,
            &talk.workflow,
            "k",
            OpenConversation {
                reply_workflow_id: Some(Some(route.clone())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    talk.send("k", "m-1", "hello").await;
    let instance = talk.runs_of_workflow()[0].instance.clone();
    talk.runs
        .says(&instance, REPLY_EVENT, json!({"text": "kept"}));

    *talk.runs.unavailable.lock().unwrap() = true;
    talk.advance("k").await;
    assert!(talk.runs.started(&route).is_empty());
    // It is found again by whatever carries conversations.
    let unsettled = talk.conversations.unsettled().await.unwrap();
    assert!(unsettled.iter().any(|(_, tenant)| tenant == &talk.tenant));

    *talk.runs.unavailable.lock().unwrap() = false;
    talk.advance("k").await;
    assert_eq!(talk.runs.started(&route).len(), 1);

    // One that can never be started is said so, and not asked for again.
    talk.runs.refused.lock().unwrap().push(route.clone());
    talk.runs
        .says(&instance, REPLY_EVENT, json!({"text": "lost"}));
    talk.advance("k").await;
    talk.advance("k").await;
    let failed: Vec<_> = talk
        .events("k")
        .await
        .into_iter()
        .filter(|event| event.kind() == "reply_route_failed")
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(talk.replies("k").await, ["kept", "lost"], "still read");
}

#[tokio::test]
async fn two_tenants_conversations_with_one_key_are_two_conversations() {
    let ours = Talk::new().await;
    let theirs = Talk {
        conversations: ours.conversations.clone(),
        runs: ours.runs.clone(),
        tenant: format!("tenant-{}", Uuid::new_v4()),
        workflow: ours.workflow.clone(),
    };

    ours.send("shared", "m-1", "ours").await;
    theirs.send("shared", "m-1", "theirs").await;

    let runs = ours.runs_of_workflow();
    assert_eq!(runs.len(), 2, "the same message id, in two conversations");
    let of = |tenant: &str| {
        runs.iter()
            .find(|run| run.tenant == tenant)
            .unwrap()
            .clone()
    };
    assert_eq!(of(&ours.tenant).inputs["data"]["message"], "ours");
    assert_eq!(of(&theirs.tenant).inputs["data"]["message"], "theirs");

    ours.runs
        .ends(&of(&ours.tenant).instance, json!({"response": "for us"}));
    ours.advance("shared").await;
    theirs.advance("shared").await;
    assert_eq!(ours.replies("shared").await, ["for us"]);
    assert!(theirs.replies("shared").await.is_empty());

    // One tenant's is not found, read or deleted through the other's name.
    assert!(
        ours.conversations
            .delete(&ours.tenant, &ours.workflow, "shared")
            .await
            .unwrap()
    );
    assert_eq!(
        ours.conversations
            .state(&ours.tenant, &ours.workflow, "shared")
            .await,
        Err(ConversationError::NotFound)
    );
    assert!(
        theirs
            .conversations
            .state(&theirs.tenant, &theirs.workflow, "shared")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_message_no_run_can_be_started_for_fails_and_says_why() {
    let talk = Talk::new().await;
    talk.runs
        .refused
        .lock()
        .unwrap()
        .push(talk.workflow.clone());
    let sent = talk
        .conversations
        .send(
            &talk.tenant,
            &talk.workflow,
            "k",
            text_message("m-1", "anyone?"),
        )
        .await
        .unwrap();
    assert_eq!(sent.state, MessageState::Failed);
    assert!(sent.detail.unwrap().contains("Workflow not found"));
    assert_eq!(talk.kinds("k").await, ["message", "message_failed"]);

    // A message with nothing to say starts a run with no `message` at all,
    // not a null one: how a client lets the workflow speak first.
    talk.runs.refused.lock().unwrap().clear();
    talk.conversations
        .send(
            &talk.tenant,
            &talk.workflow,
            "silent",
            Message {
                text: None,
                payload: Some(json!({})),
                ..text_message("opening", "")
            },
        )
        .await
        .unwrap();
    let data = talk.runs_of_workflow()[0].inputs["data"].clone();
    assert!(data.get("message").is_none() && data.get("sender").is_none());
    assert_eq!(data["payload"], json!({}));
    assert_eq!(data["messageId"], "opening");

    // A message with nothing in it, or no id, is not a message.
    for message in [
        Message {
            text: None,
            ..text_message("m-2", "")
        },
        text_message("", "no id"),
    ] {
        assert!(matches!(
            talk.conversations
                .send(&talk.tenant, &talk.workflow, "k", message)
                .await,
            Err(ConversationError::Invalid(_))
        ));
    }
}

/// The log is the transcript: what was said to the workflow is in it from
/// when it was said, and every event says which message it is for.
#[tokio::test]
async fn the_log_holds_each_message_and_says_which_turn_an_event_is_of() {
    let talk = Talk::new().await;
    talk.send("k", "m-1", "start").await;
    let instance = talk.runs_of_workflow()[0].instance.clone();
    // Sent while the run is busy: it is in the log now, before its turn.
    assert_eq!(
        talk.send("k", "m-2", "and then").await,
        MessageState::Queued
    );
    talk.runs
        .says(&instance, REPLY_EVENT, json!({"text": "First."}));
    talk.runs.ends(&instance, json!({}));
    talk.advance("k").await;
    let second = talk.runs_of_workflow()[1].instance.clone();
    talk.runs
        .says(&second, REPLY_EVENT, json!({"text": "Second."}));
    talk.runs.ends(&second, json!({}));
    talk.advance("k").await;

    let log: Vec<(String, Option<String>)> = talk
        .events("k")
        .await
        .into_iter()
        .map(|event| (event.kind().to_owned(), event.turn))
        .collect();
    let of = |kind: &str, turn: &str| (kind.to_owned(), Some(turn.to_owned()));
    assert_eq!(
        log,
        [
            of("message", "m-1"),
            of("run_started", "m-1"),
            of("message", "m-2"),
            of("reply", "m-1"),
            of("run_ended", "m-1"),
            of("run_started", "m-2"),
            of("reply", "m-2"),
            of("run_ended", "m-2"),
        ]
    );
    let said = &talk.events("k").await[2];
    assert_eq!(said.body()["id"], "m-2");
    assert_eq!(said.body()["text"], "and then");
}

/// A reply that carries an id is said once for its run, however often the
/// step that says it is run; one without an id is said each time.
#[tokio::test]
async fn a_reply_with_an_id_is_said_once() {
    let talk = Talk::new().await;
    talk.send("k", "m-1", "start").await;
    let instance = talk.runs_of_workflow()[0].instance.clone();
    for _ in 0..2 {
        talk.runs.says(
            &instance,
            REPLY_EVENT,
            json!({"text": "Shipped.", "replyId": "r-1", "data": {"order": 7}, "format": "plain"}),
        );
        talk.runs
            .says(&instance, REPLY_EVENT, json!({"text": "Hm."}));
        talk.advance("k").await;
    }
    assert_eq!(talk.replies("k").await, ["Shipped.", "Hm.", "Hm."]);
    let reply = talk
        .events("k")
        .await
        .into_iter()
        .find(|event| event.kind() == "reply")
        .unwrap();
    assert_eq!(
        reply.body(),
        json!({
            "replyId": "r-1",
            "text": "Shipped.",
            "format": "plain",
            "data": {"order": 7},
            "instanceId": instance,
        })
    );
    // The events are numbered with no gap where the repeat was passed over.
    let seqs: Vec<i64> = talk
        .events("k")
        .await
        .iter()
        .map(|event| event.seq)
        .collect();
    assert_eq!(seqs, (1..=seqs.len() as i64).collect::<Vec<_>>());

    // The same id from the next run is another reply.
    talk.runs.ends(&instance, json!({}));
    talk.advance("k").await;
    talk.send("k", "m-2", "again").await;
    let next = talk.runs_of_workflow()[1].instance.clone();
    talk.runs.says(
        &next,
        REPLY_EVENT,
        json!({"text": "Shipped again.", "replyId": "r-1"}),
    );
    talk.advance("k").await;
    assert_eq!(talk.replies("k").await.last().unwrap(), "Shipped again.");
}

/// A message that names the request it answers answers that one, or fails:
/// it is not given to another question, and does not start a run.
#[tokio::test]
async fn a_message_that_names_what_it_answers_answers_that_or_fails() {
    let talk = Talk::new().await;
    let answering = |id: &str, request: &str| Message {
        payload: Some(json!({"approved": true})),
        text: None,
        in_reply_to: Some(request.to_owned()),
        ..text_message(id, "")
    };
    let send = |message: Message| {
        let talk = &talk;
        async move {
            talk.conversations
                .send(&talk.tenant, &talk.workflow, "k", message)
                .await
                .expect("the message is taken")
        }
    };
    talk.send("k", "m-1", "start").await;
    let instance = talk.runs_of_workflow()[0].instance.clone();
    talk.runs.waits(&instance, "req-1");
    talk.advance("k").await;

    // For a request that is not the one open: stale, and the run is told
    // nothing.
    let stale = send(answering("m-2", "req-0")).await;
    assert_eq!(stale.state, MessageState::Failed);
    assert!(stale.detail.unwrap().contains("no longer waited for"));
    assert!(talk.runs.answers.lock().unwrap().is_empty());
    // For the one that is open: delivered.
    assert_eq!(
        send(answering("m-3", "req-1")).await.state,
        MessageState::Delivered
    );
    assert_eq!(talk.runs.answers.lock().unwrap()[0].1, "req-1");

    // With no run to answer, it fails and starts none.
    talk.runs.ends(&instance, json!({}));
    talk.advance("k").await;
    assert_eq!(
        send(answering("m-4", "req-1")).await.state,
        MessageState::Failed
    );
    assert_eq!(talk.runs_of_workflow().len(), 1);
    let failed: Vec<_> = talk
        .events("k")
        .await
        .into_iter()
        .filter(|event| event.kind() == "message_failed")
        .map(|event| {
            (
                event.body()["messageId"].as_str().unwrap().to_owned(),
                event.turn,
            )
        })
        .collect();
    assert_eq!(
        failed,
        [
            ("m-2".to_owned(), Some("m-2".to_owned())),
            ("m-4".to_owned(), Some("m-4".to_owned()))
        ]
    );
}

/// A reader that waits is answered when the conversation says something,
/// and not on a clock; with nothing said, it is answered empty when its
/// time is up.
#[tokio::test]
async fn a_reader_that_waits_is_answered_when_something_is_said() {
    use runtara_server::api::services::conversations::ReadEvents;
    use std::time::{Duration, Instant};
    let talk = Talk::new().await;
    talk.send("k", "m-1", "start").await;
    let instance = talk.runs_of_workflow()[0].instance.clone();
    let after = talk.events("k").await.last().unwrap().seq;
    let read = |wait_ms: u32| {
        let (conversations, tenant, workflow) = (
            talk.conversations.clone(),
            talk.tenant.clone(),
            talk.workflow.clone(),
        );
        async move {
            let asked = Instant::now();
            let events = conversations
                .read(
                    &tenant,
                    &workflow,
                    "k",
                    ReadEvents {
                        after,
                        limit: 100,
                        wait_ms,
                        ..Default::default()
                    },
                )
                .await
                .expect("read");
            (events, asked.elapsed())
        }
    };

    // Nothing said: empty, when the time is up and not before.
    let (events, took) = read(400).await;
    assert!(events.is_empty());
    assert!(took >= Duration::from_millis(350), "{took:?}");

    // Something said while it waits: answered then, long before its time.
    let waiting = tokio::spawn(read(20_000));
    tokio::time::sleep(Duration::from_millis(200)).await;
    talk.runs
        .says(&instance, REPLY_EVENT, json!({"text": "Here."}));
    talk.advance("k").await;
    let (events, took) = waiting.await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].body()["text"], "Here.");
    assert!(took < Duration::from_secs(5), "{took:?}");

    // What is there already is answered at once, however long it may wait.
    let (events, took) = read(20_000).await;
    assert_eq!(events.len(), 1);
    assert!(took < Duration::from_secs(2), "{took:?}");
}

/// Sending and waiting is a whole exchange in one request: the answer
/// carries the message's turn, up to what ended it, or as far as it went
/// when the time ran out.
#[tokio::test]
async fn sending_and_waiting_answers_with_the_messages_turn() {
    use std::time::Duration;
    let talk = Arc::new(Talk::new().await);
    let send = |id: &'static str, wait: Duration| {
        let talk = talk.clone();
        async move {
            talk.conversations
                .send_and_wait(
                    &talk.tenant,
                    &talk.workflow,
                    "k",
                    text_message(id, "hello"),
                    wait,
                )
                .await
                .expect("sent")
        }
    };
    let kinds = |events: &[ConversationEvent]| -> Vec<&'static str> {
        events.iter().map(ConversationEvent::kind).collect()
    };

    // The run answers and ends while the sender waits.
    let answering = {
        let talk = talk.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if let Some(run) = talk.runs_of_workflow().first() {
                    talk.runs
                        .says(&run.instance, REPLY_EVENT, json!({"text": "Hi."}));
                    talk.runs.ends(&run.instance, json!({}));
                    talk.advance("k").await;
                    return;
                }
            }
        })
    };
    let sent = send("m-1", Duration::from_secs(20)).await;
    answering.await.unwrap();
    assert_eq!(sent.state, MessageState::Started);
    let turn = sent.turn.expect("the turn");
    assert!(turn.settled);
    assert_eq!(
        kinds(&turn.events),
        ["message", "run_started", "reply", "run_ended"]
    );
    assert_eq!(turn.last_seq, turn.events.last().unwrap().seq);

    // Asked again, the same message answers with the same turn, and is not
    // sent again.
    let again = send("m-1", Duration::from_secs(20)).await;
    assert!(again.duplicate);
    assert_eq!(again.turn.unwrap().events, turn.events);
    assert_eq!(talk.runs_of_workflow().len(), 1);

    // A turn that is not over when the time is up says so, with what there
    // is of it and where to read on from.
    let unsettled = send("m-2", Duration::from_millis(300)).await;
    let turn = unsettled.turn.unwrap();
    assert!(!turn.settled);
    assert_eq!(kinds(&turn.events), ["message", "run_started"]);
    assert_eq!(turn.last_seq, turn.events.last().unwrap().seq);

    // Without waiting there is no turn in the answer.
    assert!(send("m-3", Duration::ZERO).await.turn.is_none());
}

/// A conversation is carried when its run says it has something new, and
/// not only when the clock comes round: here the clock never does.
#[tokio::test]
async fn a_conversation_is_carried_when_its_run_says_so() {
    use runtara_server::api::services::conversations::carry;
    use std::time::Duration;
    let talk = Talk::new().await;
    talk.send("k", "m-1", "start").await;
    let instance = talk.runs_of_workflow()[0].instance.clone();
    assert_eq!(
        talk.conversations
            .carried_by(&instance)
            .map(|(_, tenant)| tenant),
        Some(talk.tenant.clone())
    );

    let shutdown = runtara_server::shutdown::ShutdownSignal::new();
    let (tell, told) = tokio::sync::mpsc::unbounded_channel();
    let carrying = tokio::spawn(carry(
        talk.conversations.clone(),
        shutdown.clone(),
        Duration::from_secs(3600),
        told,
        runtara_server::api::services::conversations::Keeping::FOR_EVER,
    ));
    // The first look, at everything, is done before anything is said.
    tokio::time::sleep(Duration::from_millis(500)).await;
    talk.runs
        .says(&instance, REPLY_EVENT, json!({"text": "Told."}));
    // A run nobody follows is told of too, and is nothing to anyone.
    tell.send("no-conversation-carries-this".into()).unwrap();
    tell.send(instance.clone()).unwrap();
    let mut said = Vec::new();
    for _ in 0..100 {
        said = talk.replies("k").await;
        if !said.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(said, ["Told."]);

    // When the run ends, the conversation no longer follows it.
    talk.runs.ends(&instance, json!({}));
    tell.send(instance.clone()).unwrap();
    for _ in 0..100 {
        if talk.conversations.carried_by(&instance).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(talk.conversations.carried_by(&instance).is_none());
    shutdown.trigger();
    carrying.await.unwrap();
}

/// Stopping a conversation stops the run that carries it, which ends as
/// stopped. The messages waiting their turn keep it, unless the queue is
/// cleared, which fails each of them.
#[tokio::test]
async fn stopping_ends_the_run_and_keeps_the_queue_unless_it_is_cleared() {
    let talk = Talk::new().await;
    let stop = |clear_queue: bool| {
        let talk = &talk;
        async move {
            talk.conversations
                .stop(&talk.tenant, &talk.workflow, "k", clear_queue)
                .await
                .expect("stopped")
        }
    };
    talk.send("k", "m-1", "long").await;
    talk.send("k", "m-2", "next").await;
    let first = talk.runs_of_workflow()[0].instance.clone();

    let stopped = stop(false).await;
    assert_eq!(stopped.instance_id.as_deref(), Some(first.as_str()));
    assert_eq!(stopped.cleared, 0);
    assert_eq!(
        *talk.runs.stopped.lock().unwrap(),
        std::slice::from_ref(&first)
    );
    talk.advance("k").await;
    let ended: Vec<_> = talk
        .events("k")
        .await
        .into_iter()
        .filter(|event| event.kind() == "run_ended")
        .map(|event| event.body()["status"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ended, ["stopped"]);
    // The message that waited has its turn: it starts the next run.
    assert_eq!(talk.runs_of_workflow().len(), 2);

    // Stopped again with the queue cleared: what waits never has its turn.
    talk.send("k", "m-3", "waits").await;
    talk.send("k", "m-4", "waits too").await;
    let stopped = stop(true).await;
    assert_eq!(stopped.cleared, 2);
    talk.advance("k").await;
    assert_eq!(
        talk.runs_of_workflow().len(),
        2,
        "no run for what was cleared"
    );
    let failed: Vec<_> = talk
        .events("k")
        .await
        .into_iter()
        .filter(|event| event.kind() == "message_failed")
        .map(|event| event.body()["messageId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(failed, ["m-3", "m-4"]);

    // With no run, nothing is stopped, and that is no error.
    let stopped = stop(false).await;
    assert_eq!(stopped.instance_id, None);
    assert!(matches!(
        talk.conversations
            .stop(&talk.tenant, &talk.workflow, "never-opened", false)
            .await,
        Err(ConversationError::NotFound)
    ));
}

/// A conversation has a title and what its client keeps about it. Each is
/// kept when an opening leaves it out.
#[tokio::test]
async fn a_conversation_keeps_a_title_and_what_its_client_keeps_about_it() {
    let talk = Talk::new().await;
    let open = |open: Value| {
        let talk = &talk;
        async move {
            talk.conversations
                .open(
                    &talk.tenant,
                    &talk.workflow,
                    "k",
                    serde_json::from_value(open).unwrap(),
                )
                .await
        }
    };
    let state = open(json!({"title": "Order 7", "metadata": {"chat": 42}}))
        .await
        .unwrap();
    assert_eq!(state.title.as_deref(), Some("Order 7"));
    assert_eq!(state.metadata, Some(json!({"chat": 42})));
    // Left out: kept.
    let state = open(json!({})).await.unwrap();
    assert_eq!(state.title.as_deref(), Some("Order 7"));
    assert_eq!(state.metadata, Some(json!({"chat": 42})));
    // Given: replaced. A title of null is removed.
    let state = open(json!({"title": null, "metadata": {"thread": "t"}}))
        .await
        .unwrap();
    assert_eq!(state.title, None);
    assert_eq!(state.metadata, Some(json!({"thread": "t"})));
    for refused in [
        json!({"metadata": "not an object"}),
        json!({"metadata": {"big": "x".repeat(5000)}}),
        json!({"title": "t".repeat(300)}),
    ] {
        assert!(matches!(
            open(refused).await,
            Err(ConversationError::Invalid(_))
        ));
    }
}

/// A workflow's conversations are listed most recently active first, a
/// page at a time, each with the last thing said in it; and read back from
/// the newest, of the kinds asked for.
#[tokio::test]
async fn conversations_are_listed_by_activity_and_read_back_from_the_newest() {
    use runtara_server::api::services::conversations::{ListConversations, ReadEvents};
    let talk = Talk::new().await;
    for (key, text) in [
        ("order_1", "first"),
        ("order_2", "second"),
        ("orderX3", "third"),
    ] {
        talk.send(key, "m-1", text).await;
        let instance = talk.runs_of_workflow().last().unwrap().instance.clone();
        talk.runs.says(
            &instance,
            REPLY_EVENT,
            json!({"text": format!("re: {text}")}),
        );
        talk.runs.ends(&instance, json!({}));
        talk.advance(key).await;
    }
    // The oldest is spoken in again: it is the most recently active now.
    talk.send("order_1", "m-2", "once more").await;
    talk.conversations
        .open(
            &talk.tenant,
            &talk.workflow,
            "order_2",
            serde_json::from_value(json!({"title": "Second"})).unwrap(),
        )
        .await
        .unwrap();
    let list = |list: ListConversations| {
        let talk = &talk;
        async move {
            talk.conversations
                .list(&talk.tenant, &talk.workflow, list)
                .await
                .expect("listed")
        }
    };
    let all = list(ListConversations::default()).await;
    let keys: Vec<&str> = all.conversations.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(keys, ["order_2", "order_1", "orderX3"]);
    assert_eq!(all.next, None);
    let first = &all.conversations[1];
    assert!(first.active, "a run carries it");
    let said = first.last_said.as_ref().unwrap();
    assert_eq!(
        (said.kind.as_str(), said.text.as_deref()),
        ("message", Some("once more"))
    );
    let third = &all.conversations[2];
    assert!(!third.active);
    assert_eq!(
        third.last_said.as_ref().unwrap().text.as_deref(),
        Some("re: third")
    );
    assert_eq!(all.conversations[0].title.as_deref(), Some("Second"));

    // A page at a time, with no conversation twice and none missed.
    let page = list(ListConversations {
        limit: 2,
        ..Default::default()
    })
    .await;
    assert_eq!(page.conversations.len(), 2);
    let rest = list(ListConversations {
        limit: 2,
        cursor: page.next.clone(),
        ..Default::default()
    })
    .await;
    assert_eq!(rest.conversations.len(), 1);
    assert_eq!(rest.conversations[0].key, "orderX3");
    assert_eq!(rest.next, None);
    // A prefix is the letters it is: `_` is not "any letter".
    let prefixed = list(ListConversations {
        prefix: Some("order_".into()),
        ..Default::default()
    })
    .await;
    assert_eq!(prefixed.conversations.len(), 2);
    assert!(matches!(
        talk.conversations
            .list(
                &talk.tenant,
                &talk.workflow,
                ListConversations {
                    cursor: Some("not a cursor".into()),
                    ..Default::default()
                }
            )
            .await,
        Err(ConversationError::Invalid(_))
    ));
    // Another tenant's list of the same workflow is empty.
    let other = talk
        .conversations
        .list(
            "another-tenant",
            &talk.workflow,
            ListConversations::default(),
        )
        .await
        .unwrap();
    assert!(other.conversations.is_empty());

    // Read back: the newest first, then further back, of the kinds asked.
    let read = |read: ReadEvents| {
        let talk = &talk;
        async move {
            talk.conversations
                .read(&talk.tenant, &talk.workflow, "order_1", read)
                .await
                .expect("read")
        }
    };
    let said = || vec!["message".to_owned(), "reply".to_owned()];
    let newest = read(ReadEvents {
        limit: 2,
        newest_first: true,
        kinds: said(),
        ..Default::default()
    })
    .await;
    let texts = |events: &[ConversationEvent]| -> Vec<String> {
        events
            .iter()
            .map(|event| event.body()["text"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(texts(&newest), ["once more", "re: first"]);
    let older = read(ReadEvents {
        limit: 10,
        before: Some(newest.last().unwrap().seq),
        kinds: said(),
        ..Default::default()
    })
    .await;
    assert_eq!(texts(&older), ["first"]);
    // Forward, of one kind.
    let replies = read(ReadEvents {
        limit: 10,
        kinds: vec!["reply".into()],
        ..Default::default()
    })
    .await;
    assert_eq!(texts(&replies), ["re: first"]);
}

/// What is past keeping is removed: a conversation long untouched, one
/// whose workflow is gone, and a run's working older than it is kept. What
/// is recent, or carried by a run, stays.
#[tokio::test]
async fn what_is_past_keeping_is_removed() {
    use std::time::Duration;
    let talk = Talk::new().await;
    let pool = pool().await;
    let day = Duration::from_secs(24 * 60 * 60);
    let settled = |key: &'static str| {
        let talk = &talk;
        async move {
            talk.send(key, "m-1", "hello").await;
            let instance = talk.runs_of_workflow().last().unwrap().instance.clone();
            talk.runs.says(
                &instance,
                "step_debug_start",
                json!({"step_id": "s", "step_type": "Agent"}),
            );
            talk.runs.ends(&instance, json!({"response": "hi"}));
            talk.advance(key).await;
        }
    };
    settled("old").await;
    settled("recent").await;
    talk.send("old-but-carried", "m-1", "still running").await;
    let aged = |key: &'static str, days: i32| {
        let (pool, tenant) = (pool.clone(), talk.tenant.clone());
        async move {
            sqlx::query(
                "UPDATE conversations SET updated_at = now() - make_interval(days => $3) \
                 WHERE tenant_id = $1 AND key = $2",
            )
            .bind(&tenant)
            .bind(key)
            .bind(days)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    aged("old", 100).await;
    aged("old-but-carried", 100).await;
    let exists = |key: &'static str| {
        let talk = &talk;
        async move {
            talk.conversations
                .state(&talk.tenant, &talk.workflow, key)
                .await
                .is_ok()
        }
    };
    let activity = |key: &'static str| {
        let talk = &talk;
        async move {
            talk.events(key)
                .await
                .iter()
                .filter(|event| event.kind() == "activity")
                .count()
        }
    };
    assert!(activity("recent").await > 0);

    // Kept for ever: nothing goes.
    talk.conversations.sweep(None, None).await.unwrap();
    assert!(exists("old").await);

    talk.conversations
        .sweep(Some(90 * day), Some(7 * day))
        .await
        .unwrap();
    assert!(!exists("old").await, "untouched for longer than it is kept");
    assert!(exists("recent").await);
    assert!(exists("old-but-carried").await, "a run carries it");
    assert!(activity("recent").await > 0, "its working is recent");

    // A run's working is kept a shorter time than what was said.
    sqlx::query(
        "UPDATE conversation_events SET created_at = now() - interval '8 days' \
         WHERE tenant_id = $1 AND kind = 'activity'",
    )
    .bind(&talk.tenant)
    .execute(&pool)
    .await
    .unwrap();
    talk.conversations
        .sweep(Some(90 * day), Some(7 * day))
        .await
        .unwrap();
    assert_eq!(activity("recent").await, 0);
    assert_eq!(talk.replies("recent").await, ["hi"], "what was said stays");

    // A conversation goes with its workflow.
    sqlx::query(
        "INSERT INTO workflows (tenant_id, workflow_id, deleted_at) VALUES ($1, $2, now())",
    )
    .bind(&talk.tenant)
    .bind(&talk.workflow)
    .execute(&pool)
    .await
    .unwrap();
    talk.conversations.sweep(None, None).await.unwrap();
    assert!(!exists("recent").await, "its workflow is gone");
    assert!(exists("old-but-carried").await, "until its run ends");
    sqlx::query("DELETE FROM workflows WHERE tenant_id = $1")
        .bind(&talk.tenant)
        .execute(&pool)
        .await
        .unwrap();
}

/// A run reads what was said in the conversation it carries, as a
/// transcript: oldest first, each with a role. A run that carries none is
/// told so.
#[tokio::test]
async fn a_run_reads_what_was_said_in_its_own_conversation() {
    let talk = Talk::new().await;
    talk.send("k", "m-1", "What is 2+2?").await;
    let first = talk.runs_of_workflow()[0].instance.clone();
    talk.runs.says(
        &first,
        REPLY_EVENT,
        json!({"text": "4.", "data": {"answer": 4}}),
    );
    talk.runs.ends(&first, json!({}));
    talk.advance("k").await;
    talk.send("k", "m-2", "And doubled?").await;
    let second = talk.runs_of_workflow()[1].instance.clone();

    let said = talk
        .conversations
        .history_of_run(&talk.tenant, &second, 20)
        .await
        .expect("the history");
    let transcript: Vec<(String, Option<String>)> = said
        .iter()
        .map(|said| (said.role.clone(), said.text.clone()))
        .collect();
    let line = |role: &str, text: &str| (role.to_owned(), Some(text.to_owned()));
    assert_eq!(
        transcript,
        [
            line("user", "What is 2+2?"),
            line("assistant", "4."),
            line("user", "And doubled?"),
        ]
    );
    assert_eq!(said[1].data, Some(json!({"answer": 4})));
    // The last few, still oldest first.
    let last = talk
        .conversations
        .history_of_run(&talk.tenant, &second, 2)
        .await
        .unwrap();
    assert_eq!(last[0].text.as_deref(), Some("4."));
    // A run that carries none, or another tenant's: none.
    for (tenant, instance) in [
        (talk.tenant.as_str(), first.as_str()),
        ("another-tenant", second.as_str()),
    ] {
        assert!(matches!(
            talk.conversations
                .history_of_run(tenant, instance, 20)
                .await,
            Err(ConversationError::NotFound)
        ));
    }
}

/// What an adapter does: a run sends a platform's message into a
/// conversation, opening it with the outbound workflow and what it keeps
/// about the chat; the outbound workflow is then started with each reply,
/// handed that, and with each kind of event the conversation asks for.
#[tokio::test]
async fn a_run_sends_into_a_conversation_and_its_reply_workflow_is_handed_what_was_kept() {
    use runtara_server::api::services::conversations::SendFromRun;
    let talk = Talk::new().await;
    let outbound = format!("out-{}", Uuid::new_v4());
    let send = |id: &str, text: &str| SendFromRun {
        workflow_id: talk.workflow.clone(),
        key: "telegram:42".into(),
        message: text_message(id, text),
        open: serde_json::from_value(json!({
            "replyWorkflowId": outbound,
            "metadata": {"chat": 42},
            "replyEvents": ["reply", "run_ended"],
        }))
        .unwrap(),
    };
    let sent = talk
        .conversations
        .send_from_run(&talk.tenant, send("update-1", "hi"))
        .await
        .unwrap();
    assert_eq!(sent.state, MessageState::Started);
    // The platform delivers it again: the same message, nothing more.
    let again = talk
        .conversations
        .send_from_run(&talk.tenant, send("update-1", "hi"))
        .await
        .unwrap();
    assert!(again.duplicate);
    assert_eq!(talk.runs_of_workflow().len(), 1);

    // The run is told the conversation, with what the adapter kept.
    let run = talk.runs_of_workflow()[0].clone();
    assert_eq!(
        run.inputs["data"]["conversation"]["metadata"],
        json!({"chat": 42})
    );
    talk.runs
        .says(&run.instance, REPLY_EVENT, json!({"text": "Hello."}));
    talk.runs.waits(&run.instance, "req-1");
    talk.advance("telegram:42").await;
    talk.runs.ends(&run.instance, json!({}));
    talk.advance("telegram:42").await;

    let routed: Vec<(String, Value)> = talk
        .runs
        .started(&outbound)
        .into_iter()
        .map(|started| {
            (
                started.inputs["data"]["event"]["kind"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                started.inputs["data"]["conversation"]["metadata"].clone(),
            )
        })
        .collect();
    // A reply and the run's end, each once, in order; not the wait, which
    // this conversation did not ask for.
    assert_eq!(
        routed,
        [
            ("reply".to_owned(), json!({"chat": 42})),
            ("run_ended".to_owned(), json!({"chat": 42})),
        ]
    );
    let state = talk
        .conversations
        .state(&talk.tenant, &talk.workflow, "telegram:42")
        .await
        .unwrap();
    assert_eq!(state.reply_events, ["reply", "run_ended"]);
    assert!(matches!(
        talk.conversations
            .open(
                &talk.tenant,
                &talk.workflow,
                "telegram:42",
                serde_json::from_value(json!({"replyEvents": ["activity"]})).unwrap()
            )
            .await,
        Err(ConversationError::Invalid(_))
    ));
}

/// A conversation token is kept as its hash, found by the token alone,
/// grants its one conversation as its sender, and is no credential once it
/// expires.
#[tokio::test]
async fn a_conversation_token_is_found_by_itself_until_it_expires() {
    use runtara_server::api::services::conversation_tokens::{self, IssueToken};
    let pool = pool().await;
    let tenant = format!("tenant-{}", Uuid::new_v4());
    let sender = Sender {
        id: "visitor".into(),
        name: Some("Visitor".into()),
    };
    let issued = conversation_tokens::issue(
        &pool,
        &tenant,
        "wf-1",
        "page",
        "member-1",
        IssueToken {
            ttl_seconds: Some(600),
            sender: Some(sender.clone()),
        },
    )
    .await
    .unwrap();
    assert!(issued.token.starts_with(conversation_tokens::PREFIX));
    let hash = conversation_tokens::hash_of(&issued.token);
    // Only the hash is kept.
    let kept: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversation_tokens WHERE token_hash = $1 OR token_hash = $2",
    )
    .bind(&hash)
    .bind(&issued.token)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(kept, 1);
    let grant = conversation_tokens::by_hash(&pool, &hash).await.unwrap();
    assert_eq!(
        (
            grant.tenant_id.as_str(),
            grant.workflow_id.as_str(),
            grant.key.as_str()
        ),
        (tenant.as_str(), "wf-1", "page")
    );
    assert_eq!(grant.sender, Some(sender));
    assert_eq!(grant.issued_by, "member-1");
    assert!(
        conversation_tokens::by_hash(&pool, &conversation_tokens::hash_of("rtc_nobodys"))
            .await
            .is_err()
    );
    for refused in [Some(10), Some(2 * 24 * 60 * 60)] {
        assert!(matches!(
            conversation_tokens::issue(
                &pool,
                &tenant,
                "wf-1",
                "page",
                "m",
                IssueToken {
                    ttl_seconds: refused,
                    sender: None,
                }
            )
            .await,
            Err(ConversationError::Invalid(_))
        ));
    }
    // Expired: no credential, and swept.
    sqlx::query("UPDATE conversation_tokens SET expires_at = now() - interval '1 second' WHERE token_hash = $1")
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();
    assert!(conversation_tokens::by_hash(&pool, &hash).await.is_err());
    assert!(conversation_tokens::sweep(&pool).await.unwrap() >= 1);
}
