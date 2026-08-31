// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The instance HTTP API, driven end to end against a real Postgres.
//!
//! `runtara-core` is a library; this crate owns the socket its instance
//! protocol is served on. What is asserted here is the seam between the two —
//! that the knobs `CoreRuntime` accepts are actually enforced by the handlers
//! underneath, and that the drain sequence refuses new work while continuing
//! to serve the work already in flight.
//!
//! These properties used to be covered by `e2e/test_core_sigterm_drain.sh`
//! driving the standalone `runtara-core` binary. That binary is gone — core no
//! longer speaks HTTP — so the coverage moves here, where it also runs under
//! the CI gate the shell suite never ran in. The assertions that script made
//! about SIGTERM handling belonged to `runtara-core/src/main.rs`; the process
//! lifecycle is `runtara-server`'s own now.
//!
//! Gated by `db-integration-tests` and fails closed when the database is
//! unavailable.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use runtara_core::migrations;
use runtara_core::persistence::{Persistence, PostgresPersistence};
use runtara_server::core_runtime::CoreRuntime;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// A fresh pool against the runtime database, migrated.
async fn test_pool() -> PgPool {
    let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_DATABASE_URL"))
        .expect("db-integration-tests requires TEST_RUNTARA_DATABASE_URL");
    let pool = PgPool::connect(&url)
        .await
        .expect("required runtime test database must accept connections");
    migrations::run_postgres(&pool)
        .await
        .expect("required core migrations must succeed");
    pool
}

/// Bind an ephemeral port and release it, so the runtime can take it.
async fn free_port() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap()
}

/// Start a runtime with the given concurrency cap.
async fn start(persistence: Arc<dyn Persistence>, cap: u32) -> (CoreRuntime, SocketAddr) {
    let addr = free_port().await;
    let runtime = CoreRuntime::builder()
        .persistence(persistence)
        .bind_addr(addr)
        .max_concurrent_instances(cap)
        .shutdown_grace(Duration::from_secs(5))
        .build()
        .expect("builder")
        .start()
        .await
        .expect("start");

    // The listener is bound before `start` returns, but poll `/health` anyway
    // so a slow CI box cannot turn a connection refusal into a bogus failure.
    for _ in 0..100 {
        if reqwest::get(format!("http://{addr}/health")).await.is_ok() {
            return (runtime, addr);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("instance server never became healthy on {addr}");
}

/// Register one instance; returns the HTTP status.
async fn register(addr: SocketAddr, instance_id: &str, tenant_id: &str) -> u16 {
    reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/v1/instances/{instance_id}/register"
        ))
        .json(&json!({ "tenant_id": tenant_id }))
        .send()
        .await
        .expect("register request")
        .status()
        .as_u16()
}

/// The cap is enforced by the handlers, not merely accepted by the builder —
/// and it counts running work only, so parking an instance frees its slot.
///
/// Both halves matter. A cap that is logged but never applied lets a host
/// overcommit silently; a cap that counted `suspended` instances too would let
/// work parked in a durable sleep — possibly for days — wedge the runtime shut.
#[tokio::test(flavor = "multi_thread")]
async fn the_concurrency_cap_bounds_running_work_only() {
    let pool = test_pool().await;
    let persistence: Arc<dyn Persistence> = Arc::new(PostgresPersistence::new(pool));

    // The cap counts every `running` instance in the table, so this test needs
    // a tenant of its own and a cap it can reach on its own terms. Count the
    // rows already running and aim just past them.
    let tenant = format!("cap-{}", Uuid::new_v4());
    let already_running: i64 = persistence
        .count_active_instances()
        .await
        .expect("count active instances");
    let cap = u32::try_from(already_running).expect("active count fits a cap") + 2;

    let (runtime, addr) = start(Arc::clone(&persistence), cap).await;

    let first = format!("{tenant}-1");
    let second = format!("{tenant}-2");
    assert_eq!(register(addr, &first, &tenant).await, 200, "first register");
    assert_eq!(
        register(addr, &second, &tenant).await,
        200,
        "second register"
    );

    let over = format!("{tenant}-3");
    assert_eq!(
        register(addr, &over, &tenant).await,
        429,
        "registration past the cap must be refused, not merely logged"
    );

    // Suspend one, and the slot comes back.
    let suspended = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/instances/{first}/suspended"))
        .send()
        .await
        .expect("suspend request")
        .status()
        .as_u16();
    assert_eq!(suspended, 200, "suspend");

    assert_eq!(
        register(addr, &over, &tenant).await,
        200,
        "a parked instance must not hold a concurrency slot"
    );

    runtime.shutdown().await.expect("shutdown");
}

/// Draining refuses new registrations while the server keeps serving.
///
/// The ordering is the whole point of a two-phase shutdown: an instance that is
/// already running has to be able to reach a checkpoint and suspend, which it
/// can only do if the server is still answering after the door closes to new
/// arrivals. A drain that stopped the listener outright would sever exactly the
/// instances it exists to protect.
#[tokio::test(flavor = "multi_thread")]
async fn draining_refuses_new_instances_but_keeps_serving() {
    let pool = test_pool().await;
    let persistence: Arc<dyn Persistence> = Arc::new(PostgresPersistence::new(pool));
    let tenant = format!("drain-{}", Uuid::new_v4());

    let (runtime, addr) = start(persistence, 0).await;

    let early = format!("{tenant}-1");
    assert_eq!(
        register(addr, &early, &tenant).await,
        200,
        "before draining"
    );

    runtime.set_draining();

    let late = format!("{tenant}-2");
    assert_eq!(
        register(addr, &late, &tenant).await,
        503,
        "a draining runtime must refuse fresh registrations"
    );

    // Still serving: the instance that got in can still report its own state.
    let status = reqwest::get(format!("http://{addr}/api/v1/instances/{early}/status"))
        .await
        .expect("status request")
        .status()
        .as_u16();
    assert_eq!(
        status, 200,
        "draining severed an instance that had already registered"
    );

    runtime.shutdown().await.expect("shutdown");
}

/// A suspend event that carries a payload parks the instance without arming a
/// wake, and says so in the log.
///
/// The generic `/events` endpoint takes an arbitrary base64 payload, so an
/// out-of-tree client can still post the `{wake_at_ms, state}` shape that core
/// used to sniff for sleep data. Nothing in this repo produces it — durable
/// sleep goes through `/sleep` — and core no longer parses it. The risk that
/// buys is silence: a client that believes it armed a wake would park forever.
///
/// This runs through the real router rather than calling the handler directly,
/// because the payload only reaches core by way of the endpoint's base64
/// decode, and that seam is the part a unit test cannot see.
#[tokio::test(flavor = "multi_thread")]
async fn a_suspend_event_with_a_sleep_payload_arms_no_wake() {
    use base64::Engine;

    let pool = test_pool().await;
    let persistence: Arc<dyn Persistence> = Arc::new(PostgresPersistence::new(pool));
    let tenant = format!("payload-suspend-{}", Uuid::new_v4());
    let instance = format!("{tenant}-1");
    let checkpoint = "sleep-cp-1";

    let (runtime, addr) = start(persistence.clone(), 8).await;
    assert_eq!(register(addr, &instance, &tenant).await, 200, "register");

    let sleep_shape = json!({
        "wake_at_ms": (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp_millis(),
        "state": base64::engine::general_purpose::STANDARD.encode(b"test checkpoint state"),
    })
    .to_string();

    let status = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/instances/{instance}/events"))
        .json(&json!({
            "event_type": "suspended",
            "checkpoint_id": checkpoint,
            "payload": base64::engine::general_purpose::STANDARD.encode(&sleep_shape),
        }))
        .send()
        .await
        .expect("events request")
        .status()
        .as_u16();
    assert_eq!(status, 200, "a payload-bearing suspend is still accepted");

    let record = persistence
        .get_instance(&instance)
        .await
        .expect("get_instance")
        .expect("instance must exist");
    assert_eq!(record.status, "suspended", "the instance still parks");
    assert!(
        record.sleep_until.is_none(),
        "no wake may be armed from a payload core no longer parses"
    );
    assert_ne!(
        record.termination_reason.as_deref(),
        Some("sleeping"),
        "parking is a plain suspend, not a durable sleep"
    );
    assert!(
        persistence
            .load_checkpoint(&instance, checkpoint)
            .await
            .expect("load_checkpoint")
            .is_none(),
        "no checkpoint may be reconstructed out of the payload"
    );

    runtime.shutdown().await.expect("shutdown");
}
