//! Integration test for the pair of event queries behind one instance's
//! pending-input state.
//!
//! Resolving whether a run is still waiting needs both the
//! `external_input_requested` stream and the `step_debug_end` stream that
//! resolves it. Neither depends on the other, so they go out together — one
//! instance costs one round trip, not two. The executions list multiplies this
//! by the number of running rows on the page, so the saving is not marginal.
//!
//! This lives in its own test binary because pointing `RuntimeClient` at a mock
//! sets a process-global environment variable; a second test in the same
//! process would race with it.

use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use runtara_server::api::services::pending_inputs::has_open_inputs;
use runtara_server::runtime_client::RuntimeClient;

/// Deliberately coarse: one delay and two must stay far apart under CI
/// scheduling noise for the assertion below to mean anything.
const RESPONSE_DELAY: Duration = Duration::from_millis(500);

const INSTANCE_ID: &str = "instance-1";

fn events_body(events: Vec<Value>) -> Value {
    let total_count = events.len() as u32;
    json!({
        "events": events,
        "total_count": total_count,
        "limit": 100,
        "offset": 0,
    })
}

/// One event in the environment service's wire shape: the payload travels as
/// base64-encoded JSON.
fn event(id: i64, subtype: &str, payload: Value) -> Value {
    json!({
        "id": id,
        "instance_id": INSTANCE_ID,
        "event_type": "custom",
        "subtype": subtype,
        "payload": STANDARD.encode(payload.to_string()),
        "created_at_ms": 1_700_000_000_000i64,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_event_queries_for_one_instance_are_issued_together() {
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

    let events_path = format!("/api/v1/instances/{}/events", INSTANCE_ID);

    // An unanswered request, so the lookup has to consult both streams before
    // it can conclude anything.
    Mock::given(method("GET"))
        .and(path(events_path.clone()))
        .and(query_param("subtype", "external_input_requested"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(events_body(vec![event(
                    1,
                    "external_input_requested",
                    json!({ "signal_id": "signal-1" }),
                )]))
                .set_delay(RESPONSE_DELAY),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(events_path))
        .and(query_param("subtype", "step_debug_end"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(events_body(vec![]))
                .set_delay(RESPONSE_DELAY),
        )
        .mount(&server)
        .await;

    let client = RuntimeClient::with_address(&server.address().to_string());
    // Connect up front so the measured window covers only the two event
    // queries, not the one-off health check.
    client.connect().await.expect("connect to mock environment");

    let started = Instant::now();
    let has_open = has_open_inputs(&client, INSTANCE_ID)
        .await
        .expect("pending input lookup succeeds");
    let elapsed = started.elapsed();

    assert!(has_open, "the request was never answered, so it stays open");

    // Halfway between one round trip and two: reachable only if the queries
    // overlapped, and clear of both by a wide margin.
    assert!(
        elapsed < RESPONSE_DELAY * 3 / 2,
        "expected both queries in about one round trip ({:?}), took {:?} \
         (back to back would be about {:?})",
        RESPONSE_DELAY,
        elapsed,
        RESPONSE_DELAY * 2,
    );
}
