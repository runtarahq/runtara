// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A process that claims only its served tenants' work
//! (`RUNTARA_CLAIM_ONLY_SERVED_TENANTS`) relays only their execution
//! requests: another tenant's trigger stream is not its to publish to (M7.13,
//! seen while a shared service handed over to a process serving one more
//! tenant). Its own test binary, as the claim scope is set once per process.
//!
//! Requires the `db-integration-tests` and `valkey-integration-tests`
//! features, a live Postgres and Valkey.

use std::sync::Arc;
use std::time::Duration;

use runtara_server::api::dto::trigger_event::TriggerEvent;
use runtara_server::api::repositories::trigger_stream::TriggerStreamPublisher;
use runtara_server::valkey::ValkeyConfig;
use runtara_server::workers::execution_outbox::{
    ExecutionOutbox, ExecutionOutboxPolicy, ExecutionOutboxRelay, source_idempotency_key,
};
use sqlx::PgPool;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

fn event(tenant_id: &str) -> TriggerEvent {
    TriggerEvent::http_api(
        Uuid::new_v4().to_string(),
        tenant_id.to_string(),
        "outbox-workflow".to_string(),
        Some(1),
        serde_json::json!({"data": {}, "variables": {}}),
        false,
        None,
        false,
    )
}

#[tokio::test]
async fn the_relay_publishes_only_the_served_tenants_requests() {
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .expect("db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    let tag = Uuid::new_v4().simple().to_string();
    let (served, other) = (format!("claim-served-{tag}"), format!("claim-other-{tag}"));
    runtara_environment::claim_scope::set_claim_tenants(vec![served.clone()]);

    let mut config =
        ValkeyConfig::from_env().expect("valkey-integration-tests requires VALKEY_HOST");
    config.trigger_stream_prefix = format!("runtara:test:outbox:claim-scope:{tag}");
    let manager =
        redis::aio::ConnectionManager::new(redis::Client::open(config.connection_url()).unwrap())
            .await
            .unwrap();
    let publisher = Arc::new(TriggerStreamPublisher::new(manager, config));
    let outbox = ExecutionOutbox::with_policy(
        pool.clone(),
        ExecutionOutboxPolicy {
            request_deadline: Duration::from_secs(60),
            lease_duration: Duration::from_secs(5),
            retry_delay: Duration::from_millis(10),
            poll_interval: Duration::from_millis(10),
            batch_size: 50,
        },
    );
    let mut requests = Vec::new();
    for tenant in [&served, &other] {
        let accepted = outbox
            .enqueue(
                tenant,
                &event(tenant),
                &source_idempotency_key("http-api", &format!("claim-scope-{tenant}")),
                1,
            )
            .await
            .unwrap();
        requests.push(accepted.request_id);
    }

    ExecutionOutboxRelay::new(outbox, publisher)
        .run_once()
        .await
        .unwrap();
    let state = |request: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM execution_outbox WHERE request_id = $1",
            )
            .bind(request)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(state(requests[0]).await, "delivered", "the served tenant's");
    assert_eq!(
        state(requests[1]).await,
        "pending",
        "another's is left to its own process"
    );

    for tenant in [&served, &other] {
        for table in [
            "execution_admission_reservations",
            "execution_admission_tenants",
            "execution_requests",
        ] {
            sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id = $1"))
                .bind(tenant)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
}
