// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database boundary tests for durable execution intake.
//!
//! These intentionally test the transaction boundary rather than a mocked
//! repository: the critical guarantees are PostgreSQL uniqueness/atomic
//! counter behaviour after a process dies between commit and stream delivery.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use runtara_environment::launch_dispatcher::LaunchLifecycleObserver;
use runtara_server::api::dto::trigger_event::TriggerEvent;
use runtara_server::workers::execution_outbox::{
    DurableLaunchClaim, ExecutionAdmissionLifecycleObserver, ExecutionOutbox, ExecutionOutboxError,
    ExecutionOutboxPolicy, source_idempotency_key,
};
use sqlx::PgPool;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

// The relay and deadline reaper intentionally operate across all server
// tenants. Serialize this integration target so a concurrent test cannot
// claim or expire another test's deliberately pending source request.
static OUTBOX_TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

async fn test_pool() -> PgPool {
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect("db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL or RUNTARA_SERVER_DATABASE_URL");
    let pool = PgPool::connect(&url)
        .await
        .expect("server test database must accept connections");
    MIGRATOR
        .run(&pool)
        .await
        .expect("server migrations must succeed");
    pool
}

fn policy() -> ExecutionOutboxPolicy {
    ExecutionOutboxPolicy {
        request_deadline: Duration::from_secs(60),
        lease_duration: Duration::from_secs(5),
        retry_delay: Duration::from_millis(10),
        poll_interval: Duration::from_millis(10),
        batch_size: 50,
    }
}

fn event(tenant_id: &str, instance_id: Uuid) -> TriggerEvent {
    TriggerEvent::http_api(
        instance_id.to_string(),
        tenant_id.to_string(),
        "outbox-workflow".to_string(),
        Some(1),
        serde_json::json!({"data": {}, "variables": {}}),
        false,
        None,
        false,
    )
}

async fn reserved_count(pool: &PgPool, tenant_id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT reserved_count FROM execution_admission_tenants WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .expect("query reservation counter")
    .unwrap_or(0)
}

async fn cleanup(pool: &PgPool, tenant_id: &str) {
    // Keep cleanup explicitly tenant-scoped: integration runs may share a
    // database with other suites and this table owns only its test tenant.
    let _ = sqlx::query(
        r#"
        DELETE FROM execution_outbox
        WHERE request_id IN (
            SELECT request_id FROM execution_requests WHERE tenant_id = $1
        )
        "#,
    )
    .bind(tenant_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM execution_admission_reservations WHERE tenant_id = $1")
        .bind(tenant_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM execution_requests WHERE tenant_id = $1")
        .bind(tenant_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM execution_admission_tenants WHERE tenant_id = $1")
        .bind(tenant_id)
        .execute(pool)
        .await;
}

#[tokio::test]
async fn committed_request_survives_a_restart_before_stream_delivery() {
    let _guard = OUTBOX_TEST_LOCK.lock().await;
    let pool = test_pool().await;
    let tenant_id = format!("outbox-restart-{}", Uuid::new_v4());
    let key = source_idempotency_key("http-api", "request-1");

    let first = ExecutionOutbox::with_policy(pool.clone(), policy());
    let accepted = first
        .enqueue(&tenant_id, &event(&tenant_id, Uuid::new_v4()), &key, 4)
        .await
        .expect("commit source request");
    assert!(!accepted.duplicate);
    drop(first); // Simulates a process dying after commit and before XADD.

    let restarted = ExecutionOutbox::with_policy(pool.clone(), policy());
    let recovered = restarted
        .find_by_idempotency(&tenant_id, &key)
        .await
        .expect("find committed request after restart")
        .expect("outbox request remains durable");
    assert_eq!(recovered.request_id, accepted.request_id);
    assert!(recovered.duplicate);
    assert_eq!(reserved_count(&pool, &tenant_id).await, 1);

    let state: String =
        sqlx::query_scalar("SELECT state FROM execution_outbox WHERE request_id = $1")
            .bind(accepted.request_id)
            .fetch_one(&pool)
            .await
            .expect("outbox row exists after restart");
    assert_eq!(state, "pending");

    cleanup(&pool, &tenant_id).await;
}

#[tokio::test]
async fn concurrent_sources_never_exceed_the_durable_admission_bound() {
    let _guard = OUTBOX_TEST_LOCK.lock().await;
    let pool = test_pool().await;
    let tenant_id = format!("outbox-bound-{}", Uuid::new_v4());
    let outbox = Arc::new(ExecutionOutbox::with_policy(pool.clone(), policy()));
    const CAP: u64 = 3;

    let mut tasks = Vec::new();
    for index in 0..12 {
        let outbox = Arc::clone(&outbox);
        let tenant_id = tenant_id.clone();
        tasks.push(tokio::spawn(async move {
            outbox
                .enqueue(
                    &tenant_id,
                    &event(&tenant_id, Uuid::new_v4()),
                    &source_idempotency_key("mixed-source", &index.to_string()),
                    CAP,
                )
                .await
        }));
    }

    let mut accepted = Vec::new();
    let mut denied = 0;
    for task in tasks {
        match task.await.expect("enqueue task did not panic") {
            Ok(enqueued) => accepted.push(enqueued),
            Err(ExecutionOutboxError::AdmissionFull { limit }) => {
                assert_eq!(limit, CAP);
                denied += 1;
            }
            Err(error) => panic!("unexpected enqueue error: {error}"),
        }
    }

    assert_eq!(accepted.len(), CAP as usize);
    assert_eq!(denied, 12 - CAP as usize);
    assert_eq!(reserved_count(&pool, &tenant_id).await, CAP as i64);

    // A lifecycle callback may be delivered more than once. Exactly one call
    // decrements the durable counter, so a duplicate terminal/suspend signal
    // cannot free another request's capacity.
    assert!(
        outbox
            .release_admission(accepted[0].request_id, "terminal")
            .await
            .expect("first release")
    );
    assert!(
        !outbox
            .release_admission(accepted[0].request_id, "terminal_retry")
            .await
            .expect("duplicate release")
    );
    assert_eq!(reserved_count(&pool, &tenant_id).await, (CAP - 1) as i64);

    cleanup(&pool, &tenant_id).await;
}

#[tokio::test]
async fn expired_undelivered_request_releases_admission_once() {
    let _guard = OUTBOX_TEST_LOCK.lock().await;
    let pool = test_pool().await;
    let tenant_id = format!("outbox-expiry-{}", Uuid::new_v4());
    let outbox = ExecutionOutbox::with_policy(pool.clone(), policy());
    let accepted = outbox
        .enqueue(
            &tenant_id,
            &event(&tenant_id, Uuid::new_v4()),
            &source_idempotency_key("cron", "expired-tick"),
            1,
        )
        .await
        .expect("enqueue request");

    sqlx::query("UPDATE execution_requests SET deadline_at = NOW() - INTERVAL '1 second' WHERE request_id = $1")
        .bind(accepted.request_id)
        .execute(&pool)
        .await
        .expect("make request overdue");

    assert_eq!(outbox.expire_due().await.expect("expire request"), 1);
    assert_eq!(outbox.expire_due().await.expect("repeat expiry"), 0);
    assert_eq!(reserved_count(&pool, &tenant_id).await, 0);

    let state: String =
        sqlx::query_scalar("SELECT state FROM execution_outbox WHERE request_id = $1")
            .bind(accepted.request_id)
            .fetch_one(&pool)
            .await
            .expect("read terminal outbox state");
    assert_eq!(state, "expired");

    cleanup(&pool, &tenant_id).await;
}

#[tokio::test]
async fn delivered_request_expires_before_worker_handoff_and_cannot_start_later() {
    let _guard = OUTBOX_TEST_LOCK.lock().await;
    let pool = test_pool().await;
    let tenant_id = format!("outbox-delivered-expiry-{}", Uuid::new_v4());
    let outbox = ExecutionOutbox::with_policy(pool.clone(), policy());
    let instance_id = Uuid::new_v4();
    let accepted = outbox
        .enqueue(
            &tenant_id,
            &event(&tenant_id, instance_id),
            &source_idempotency_key("http-event", "delivery-before-worker"),
            1,
        )
        .await
        .expect("enqueue request");

    // Simulate a successful XADD followed by workers being unavailable. The
    // source record is no longer queued, but its original deadline must still
    // free admission and fence a later PEL replay before Environment starts.
    sqlx::query(
        r#"
        UPDATE execution_requests
        SET state = 'delivered', deadline_at = NOW() - INTERVAL '1 second'
        WHERE request_id = $1
        "#,
    )
    .bind(accepted.request_id)
    .execute(&pool)
    .await
    .expect("make delivered request overdue");
    sqlx::query("UPDATE execution_outbox SET state = 'delivered' WHERE request_id = $1")
        .bind(accepted.request_id)
        .execute(&pool)
        .await
        .expect("mark stream delivery");

    assert_eq!(outbox.expire_due().await.expect("expire request"), 1);
    assert_eq!(reserved_count(&pool, &tenant_id).await, 0);
    assert_eq!(
        outbox
            .claim_for_launch(
                accepted.request_id,
                &tenant_id,
                &instance_id.to_string(),
                "test-worker",
            )
            .await
            .expect("inspect expired stream replay"),
        DurableLaunchClaim::Expired
    );

    let request_state: String =
        sqlx::query_scalar("SELECT state FROM execution_requests WHERE request_id = $1")
            .bind(accepted.request_id)
            .fetch_one(&pool)
            .await
            .expect("read expired request state");
    assert_eq!(request_state, "expired");

    cleanup(&pool, &tenant_id).await;
}

#[tokio::test]
async fn consumer_claims_the_handoff_before_the_relay_commits_its_delivery() {
    let _guard = OUTBOX_TEST_LOCK.lock().await;
    let pool = test_pool().await;
    let tenant_id = format!("outbox-delivery-race-{}", Uuid::new_v4());
    let outbox = ExecutionOutbox::with_policy(pool.clone(), policy());
    let instance_id = Uuid::new_v4();
    let accepted = outbox
        .enqueue(
            &tenant_id,
            &event(&tenant_id, instance_id),
            &source_idempotency_key("http-api", "delivery-race"),
            1,
        )
        .await
        .expect("enqueue request");

    // The relay leases the row, publishes it, and has not yet committed its
    // delivery mark. A trigger worker woken by that XADD arrives here first —
    // this is the ordinary case, because the blocking stream read returns in
    // about a millisecond while the relay still has a transaction to commit.
    sqlx::query(
        r#"
        UPDATE execution_outbox
        SET state = 'leased',
            lease_owner = 'relay-1',
            lease_expires_at = NOW() + INTERVAL '30 seconds'
        WHERE request_id = $1
        "#,
    )
    .bind(accepted.request_id)
    .execute(&pool)
    .await
    .expect("simulate relay lease held across its own XADD");

    // Holding the stream entry proves the relay published it, so the consumer
    // must be able to fence and launch now. Before this was allowed the claim
    // returned InProgress, the entry stayed unacked in the PEL, and every
    // launch waited for an idle-based XAUTOCLAIM reclaim.
    assert_eq!(
        outbox
            .claim_for_launch(
                accepted.request_id,
                &tenant_id,
                &instance_id.to_string(),
                "trigger-worker-1",
            )
            .await
            .expect("claim handoff inside the relay's delivery window"),
        DurableLaunchClaim::Claimed
    );

    let (request_state, outbox_state, lease_owner): (String, String, Option<String>) =
        sqlx::query_as(
            r#"
            SELECT r.state, o.state, o.lease_owner
            FROM execution_requests AS r
            INNER JOIN execution_outbox AS o ON o.request_id = r.request_id
            WHERE r.request_id = $1
            "#,
        )
        .bind(accepted.request_id)
        .fetch_one(&pool)
        .await
        .expect("read state after the consumer claimed the handoff");
    assert_eq!(request_state, "launching");
    assert_eq!(outbox_state, "leased");
    assert_eq!(lease_owner.as_deref(), Some("trigger-worker-1"));

    // The handoff the consumer owns still completes, so the launch is fenced
    // exactly once rather than being replayed by the retry path.
    assert!(
        outbox
            .mark_launch_accepted(accepted.request_id, "trigger-worker-1")
            .await
            .expect("accept the environment handoff")
    );

    cleanup(&pool, &tenant_id).await;
}

#[tokio::test]
async fn accepted_environment_handoff_retains_admission_until_lifecycle_release() {
    let _guard = OUTBOX_TEST_LOCK.lock().await;
    let pool = test_pool().await;
    let tenant_id = format!("outbox-handoff-{}", Uuid::new_v4());
    let outbox = ExecutionOutbox::with_policy(pool.clone(), policy());
    let instance_id = Uuid::new_v4();
    let accepted = outbox
        .enqueue(
            &tenant_id,
            &event(&tenant_id, instance_id),
            &source_idempotency_key("http-api", "accepted-handoff"),
            1,
        )
        .await
        .expect("enqueue request");

    sqlx::query("UPDATE execution_requests SET state = 'delivered' WHERE request_id = $1")
        .bind(accepted.request_id)
        .execute(&pool)
        .await
        .expect("mark source delivered");
    sqlx::query("UPDATE execution_outbox SET state = 'delivered' WHERE request_id = $1")
        .bind(accepted.request_id)
        .execute(&pool)
        .await
        .expect("mark outbox delivered");

    assert_eq!(
        outbox
            .claim_for_launch(
                accepted.request_id,
                &tenant_id,
                &instance_id.to_string(),
                "test-worker",
            )
            .await
            .expect("claim launch"),
        DurableLaunchClaim::Claimed
    );
    assert!(
        outbox
            .mark_launch_accepted(accepted.request_id, "test-worker")
            .await
            .expect("confirm Environment handoff")
    );

    // Once Environment owns a durable launch, the source deadline does not
    // release it. A suspended/terminal/cancelled lifecycle event does.
    sqlx::query("UPDATE execution_requests SET deadline_at = NOW() - INTERVAL '1 second' WHERE request_id = $1")
        .bind(accepted.request_id)
        .execute(&pool)
        .await
        .expect("make accepted request old");
    assert_eq!(outbox.expire_due().await.expect("reap old requests"), 0);
    assert_eq!(reserved_count(&pool, &tenant_id).await, 1);
    // Exercise the installed Environment-to-server bridge rather than
    // invoking the repository directly. Its idempotent callback makes an
    // Environment transition release source admission exactly once.
    let observer = ExecutionAdmissionLifecycleObserver::new(outbox.clone());
    observer
        .release_admission(&tenant_id, &instance_id.to_string(), "suspended")
        .await
        .expect("lifecycle observer release");
    observer
        .release_admission(&tenant_id, &instance_id.to_string(), "suspended_retry")
        .await
        .expect("duplicate lifecycle observer release");
    assert_eq!(reserved_count(&pool, &tenant_id).await, 0);

    cleanup(&pool, &tenant_id).await;
}

#[cfg(feature = "valkey-integration-tests")]
#[tokio::test]
async fn relay_redelivers_after_stream_write_before_delivery_mark() {
    use redis::AsyncCommands;
    use runtara_server::api::repositories::trigger_stream::TriggerStreamPublisher;
    use runtara_server::valkey::ValkeyConfig;
    use runtara_server::workers::execution_outbox::ExecutionOutboxRelay;

    let _guard = OUTBOX_TEST_LOCK.lock().await;
    let pool = test_pool().await;
    let tenant_id = format!("outbox-publish-crash-{}", Uuid::new_v4());
    let mut config =
        ValkeyConfig::from_env().expect("valkey-integration-tests requires VALKEY_HOST");
    config.trigger_stream_prefix = format!("runtara:test:outbox:publish-crash:{}", Uuid::new_v4());
    let stream_key = config.trigger_stream_key(&tenant_id);
    // Do not use the process-wide manager in a Tokio integration test: its
    // reconnect driver belongs to the runtime that initialized it, while each
    // `#[tokio::test]` owns and tears down a separate runtime.
    let manager = redis::aio::ConnectionManager::new(
        redis::Client::open(config.connection_url()).expect("build Valkey client"),
    )
    .await
    .expect("connect Valkey");
    let publisher = Arc::new(TriggerStreamPublisher::new(manager.clone(), config));
    let outbox = ExecutionOutbox::with_policy(pool.clone(), policy());
    let instance_id = Uuid::new_v4();
    let source_event = event(&tenant_id, instance_id);
    let accepted = outbox
        .enqueue(
            &tenant_id,
            &source_event,
            &source_idempotency_key("http-api", "publish-before-mark"),
            1,
        )
        .await
        .expect("enqueue request");

    // Simulate a relay process that successfully wrote XADD, then died before
    // it could mark the outbox delivery. Its expired lease lets a replacement
    // relay publish a second, safe-at-least-once copy.
    sqlx::query(
        r#"
        UPDATE execution_outbox
        SET state = 'leased', lease_owner = 'crashed-relay',
            lease_expires_at = NOW() - INTERVAL '1 second'
        WHERE request_id = $1
        "#,
    )
    .bind(accepted.request_id)
    .execute(&pool)
    .await
    .expect("simulate relay claim before crash");
    let mut first_event = source_event.clone();
    first_event.request_id = Some(accepted.request_id);
    publisher
        .publish_with_request_id(&tenant_id, &first_event, accepted.request_id)
        .await
        .expect("simulate successful XADD before mark");

    let relay = ExecutionOutboxRelay::new(outbox.clone(), publisher);
    let replay = relay.run_once().await.expect("recover relay lease");
    assert_eq!(replay.delivered, 1);
    assert_eq!(reserved_count(&pool, &tenant_id).await, 1);

    let mut redis = manager.clone();
    let entries: redis::streams::StreamRangeReply = redis
        .xrange_all(&stream_key)
        .await
        .expect("read redelivered stream");
    assert_eq!(entries.ids.len(), 2);
    for entry in entries.ids {
        let json_data: String = redis::from_redis_value(entry.map.get("data").expect("data field"))
            .expect("decode event JSON");
        let replayed: TriggerEvent = serde_json::from_str(&json_data).expect("decode relay event");
        assert_eq!(replayed.request_id, Some(accepted.request_id));
        assert_eq!(replayed.instance_id, instance_id.to_string());
    }

    let _: () = redis.del(&stream_key).await.expect("remove test stream");
    cleanup(&pool, &tenant_id).await;
}

#[cfg(feature = "valkey-integration-tests")]
#[tokio::test]
async fn relay_writes_request_id_into_json_and_recovers_an_expired_worker_handoff() {
    use redis::AsyncCommands;
    use runtara_server::api::repositories::trigger_stream::TriggerStreamPublisher;
    use runtara_server::valkey::ValkeyConfig;
    use runtara_server::workers::execution_outbox::ExecutionOutboxRelay;

    let _guard = OUTBOX_TEST_LOCK.lock().await;
    let pool = test_pool().await;
    let tenant_id = format!("outbox-relay-{}", Uuid::new_v4());
    let mut config =
        ValkeyConfig::from_env().expect("valkey-integration-tests requires VALKEY_HOST");
    config.trigger_stream_prefix = format!("runtara:test:outbox:{}", Uuid::new_v4());
    let stream_key = config.trigger_stream_key(&tenant_id);
    // Use an integration-test-local manager; see the equivalent crash test
    // above for why a process-wide manager is invalid across test runtimes.
    let manager = redis::aio::ConnectionManager::new(
        redis::Client::open(config.connection_url()).expect("build Valkey client"),
    )
    .await
    .expect("connect Valkey");
    let publisher = Arc::new(TriggerStreamPublisher::new(manager.clone(), config));
    let outbox = ExecutionOutbox::with_policy(pool.clone(), policy());
    let accepted = outbox
        .enqueue(
            &tenant_id,
            &event(&tenant_id, Uuid::new_v4()),
            &source_idempotency_key("cron", "relay-recovery"),
            1,
        )
        .await
        .expect("enqueue request");
    let relay = ExecutionOutboxRelay::new(outbox.clone(), publisher);

    let first = relay.run_once().await.expect("relay first delivery");
    assert_eq!(first.delivered, 1);
    assert_eq!(reserved_count(&pool, &tenant_id).await, 1);

    let mut redis = manager.clone();
    let entries: redis::streams::StreamRangeReply = redis
        .xrange_all(&stream_key)
        .await
        .expect("read relay stream");
    assert_eq!(entries.ids.len(), 1);
    let entry = &entries.ids[0];
    let stream_request_id: String = redis::from_redis_value(
        entry
            .map
            .get("request_id")
            .expect("request_id stream field"),
    )
    .expect("decode stream request ID");
    assert_eq!(stream_request_id, accepted.request_id.to_string());
    let json_data: String = redis::from_redis_value(entry.map.get("data").expect("data field"))
        .expect("decode event JSON");
    let event: TriggerEvent = serde_json::from_str(&json_data).expect("decode relay event");
    assert_eq!(event.request_id, Some(accepted.request_id));

    // Mimic a worker crash after it has claimed the event but before
    // Environment accepts a durable launch. The relay repairs the expired
    // handoff by publishing an at-least-once copy; the same instance ID makes
    // a later Environment start idempotent.
    sqlx::query(
        r#"
        UPDATE execution_requests
        SET state = 'launching'
        WHERE request_id = $1
        "#,
    )
    .bind(accepted.request_id)
    .execute(&pool)
    .await
    .expect("simulate worker launch claim");
    sqlx::query(
        r#"
        UPDATE execution_outbox
        SET state = 'leased', lease_owner = 'crashed',
            lease_expires_at = NOW() - INTERVAL '1 second'
        WHERE request_id = $1
        "#,
    )
    .bind(accepted.request_id)
    .execute(&pool)
    .await
    .expect("expire worker claim");

    let recovered = relay.run_once().await.expect("recover worker handoff");
    assert_eq!(recovered.recovered, 1);
    assert_eq!(recovered.delivered, 1);
    assert_eq!(reserved_count(&pool, &tenant_id).await, 1);

    let entries: redis::streams::StreamRangeReply = redis
        .xrange_all(&stream_key)
        .await
        .expect("read recovered relay stream");
    assert_eq!(entries.ids.len(), 2);

    let _: () = redis.del(&stream_key).await.expect("remove test stream");
    cleanup(&pool, &tenant_id).await;
}
