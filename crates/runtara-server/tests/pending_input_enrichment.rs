//! Integration tests for `enrich_pending_input` — the executions-list pass that
//! flags running rows still waiting on human input.
//!
//! These drive the real `RuntimeClient` -> SDK -> HTTP path against `wiremock`
//! standing in for the environment service, so they cover the wiring rather
//! than the pairing logic alone (that has its own unit tests next to it).
//!
//! Every response is served with a fixed delay, which makes the enrichment's
//! request pattern observable: the elapsed time of the whole pass says whether
//! the per-instance lookups were issued together or one after another.

use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use runtara_server::api::dto::workflows::{InstanceInputs, WorkflowInstanceDto};
use runtara_server::runtime_client::RuntimeClient;
use runtara_server::types::ExecutionStatus;
use runtara_server::workers::runtara_dto::enrich_pending_input;

/// How long the stand-in environment service takes to answer one events query.
const RESPONSE_DELAY: Duration = Duration::from_millis(200);

/// Running rows in the fixture. Matches the enrichment's concurrency bound, so
/// a correctly batched pass resolves all of them in a single wave.
const RUNNING_ROWS: usize = 8;

/// One event in the environment service's wire shape: the payload travels as
/// base64-encoded JSON.
fn event(id: i64, instance_id: &str, subtype: &str, payload: Value) -> Value {
    json!({
        "id": id,
        "instance_id": instance_id,
        "event_type": "custom",
        "subtype": subtype,
        "payload": STANDARD.encode(payload.to_string()),
        "created_at_ms": 1_700_000_000_000i64,
    })
}

fn events_body(events: Vec<Value>) -> Value {
    let total_count = events.len() as u32;
    json!({
        "events": events,
        "total_count": total_count,
        "limit": 100,
        "offset": 0,
    })
}

fn instance(id: &str, status: ExecutionStatus) -> WorkflowInstanceDto {
    WorkflowInstanceDto {
        id: id.to_string(),
        created: "2026-08-21T00:00:00Z".to_string(),
        updated: "2026-08-21T00:00:00Z".to_string(),
        status,
        termination_type: None,
        error: None,
        workflow_id: "workflow-1".to_string(),
        workflow_name: None,
        inputs: InstanceInputs {
            data: Value::Null,
            variables: Value::Null,
        },
        outputs: None,
        tags: vec![],
        used_version: 1,
        steps: vec![],
        execution_duration_seconds: None,
        max_memory_mb: None,
        queue_duration_seconds: None,
        processing_overhead_seconds: None,
        has_pending_input: false,
    }
}

/// Serve both event queries the enrichment makes for one instance.
async fn mount_instance(
    server: &MockServer,
    instance_id: &str,
    input_events: Vec<Value>,
    end_events: Vec<Value>,
) {
    let events_path = format!("/api/v1/instances/{}/events", instance_id);

    Mock::given(method("GET"))
        .and(path(events_path.clone()))
        .and(query_param("subtype", "external_input_requested"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(events_body(input_events))
                .set_delay(RESPONSE_DELAY),
        )
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path(events_path))
        .and(query_param("subtype", "step_debug_end"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(events_body(end_events))
                .set_delay(RESPONSE_DELAY),
        )
        .mount(server)
        .await;
}

/// A page of running executions is enriched concurrently, and each row still
/// gets the flag its own events imply.
///
/// The pass previously walked the page one row at a time, awaiting both event
/// queries per row in turn, so this fixture cost `RUNNING_ROWS * 2 *
/// RESPONSE_DELAY` (~3.2s) of serialized round trips. Batched, it is one wave
/// (~200ms). The one-second bound sits well clear of both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enriches_a_page_of_running_instances_concurrently() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "healthy": true,
            "version": "test",
            "uptime_ms": 0,
        })))
        .mount(&server)
        .await;

    let mut instances = Vec::new();

    // One row whose request has already been answered: its `step_debug_end`
    // carries the same signal id, so nothing is left open.
    mount_instance(
        &server,
        "resolved",
        vec![event(
            1,
            "resolved",
            "external_input_requested",
            json!({ "signal_id": "signal-resolved" }),
        )],
        vec![event(
            2,
            "resolved",
            "step_debug_end",
            json!({
                "step_id": "step-1",
                "outputs": { "signal_id": "signal-resolved" },
            }),
        )],
    )
    .await;
    instances.push(instance("resolved", ExecutionStatus::Running));

    // The rest are genuinely waiting: a request with no matching end event.
    for index in 0..RUNNING_ROWS - 1 {
        let id = format!("open-{}", index);
        mount_instance(
            &server,
            &id,
            vec![event(
                10 + index as i64,
                &id,
                "external_input_requested",
                json!({ "signal_id": format!("signal-{}", index) }),
            )],
            vec![],
        )
        .await;
        instances.push(instance(&id, ExecutionStatus::Running));
    }

    // A finished row must not be looked up at all — wiremock asserts the
    // expectation on drop.
    Mock::given(method("GET"))
        .and(path("/api/v1/instances/finished/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(events_body(vec![])))
        .expect(0)
        .mount(&server)
        .await;
    instances.push(instance("finished", ExecutionStatus::Completed));

    let client = RuntimeClient::with_address(&server.address().to_string());
    // Connect up front so the measured window covers only the event lookups,
    // not a one-off health check racing across every future.
    client.connect().await.expect("connect to mock environment");

    let started = Instant::now();
    enrich_pending_input(&mut instances, &client).await;
    let elapsed = started.elapsed();

    let by_id = |id: &str| {
        instances
            .iter()
            .find(|instance| instance.id == id)
            .unwrap_or_else(|| panic!("instance {} missing", id))
            .has_pending_input
    };

    assert!(!by_id("resolved"), "answered request must not stay pending");
    for index in 0..RUNNING_ROWS - 1 {
        let id = format!("open-{}", index);
        assert!(by_id(&id), "{} still has an unanswered request", id);
    }
    assert!(!by_id("finished"), "a finished run has nothing pending");

    assert!(
        elapsed < Duration::from_secs(1),
        "expected the page to resolve in roughly one round trip, took {:?} \
         (serialized would be about {:?})",
        elapsed,
        RESPONSE_DELAY * (RUNNING_ROWS as u32) * 2,
    );
}
