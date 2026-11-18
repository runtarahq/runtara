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
        self.events(key).await.into_iter().map(|e| e.kind).collect()
    }

    async fn replies(&self, key: &str) -> Vec<String> {
        self.events(key)
            .await
            .into_iter()
            .filter(|event| event.kind == "reply")
            .map(|event| event.body["text"].as_str().unwrap_or_default().to_owned())
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
    assert_eq!(
        talk.kinds("k").await,
        [
            "run_started",
            "reply",
            "run_ended",
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
    assert_eq!(kinds.first().map(String::as_str), Some("run_started"));
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
        .filter(|event| event.kind == "waiting_for_input")
        .collect();
    assert_eq!(waiting.len(), 1, "said once, however often it is read");
    assert_eq!(waiting[0].body["requestId"], "req-1");
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
        .filter(|event| event.kind == "reply_route_failed")
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
    assert_eq!(talk.kinds("k").await, ["message_failed"]);

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
