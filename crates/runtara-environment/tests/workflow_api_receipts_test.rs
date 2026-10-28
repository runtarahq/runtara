// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow API idempotency receipts in the runtime database: a retried write
//! gets its first answer even from a new router over the same database, as
//! after the serving process restarts.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use runtara_environment::workflow_api_receipts::PostgresReceiptStore;
use runtara_workflow_api::server::{
    LaunchState, PostedOutcome, Receipt, ReceiptStore, WorkflowApiBackend, WorkflowApiState, router,
};
use runtara_workflow_api::{ApiError, Binding, Caps, RunInfo, headers};
use tower::ServiceExt;

async fn store() -> (PostgresReceiptStore, sqlx::PgPool) {
    let url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .expect("db-integration-tests requires TEST_ENVIRONMENT_DATABASE_URL");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    runtara_environment::migrations::run(&pool).await.unwrap();
    (PostgresReceiptStore::new(pool.clone()), pool)
}

fn fresh_launch() -> String {
    format!("launch-{}", uuid::Uuid::new_v4())
}

fn receipt(byte: u8, body: &[u8]) -> Receipt {
    Receipt {
        request_hash: [byte; 32],
        status: 204,
        headers: vec![("content-type".into(), "application/json".into())],
        body: body.to_vec(),
        body_omitted: false,
    }
}

#[tokio::test]
async fn receipts_keep_the_first_answer_until_the_launch_is_forgotten() {
    let (store, _) = store().await;
    let launch = fresh_launch();
    assert_eq!(store.get(&launch, "k1").await.unwrap(), None);
    store
        .put(&launch, "k1", receipt(1, b"first"))
        .await
        .unwrap();
    store
        .put(&launch, "k1", receipt(2, b"second"))
        .await
        .unwrap();
    assert_eq!(
        store.get(&launch, "k1").await.unwrap(),
        Some(receipt(1, b"first"))
    );
    // Keys are per launch.
    assert_eq!(store.get(&fresh_launch(), "k1").await.unwrap(), None);

    store.forget_launch(&launch).await.unwrap();
    assert_eq!(store.get(&launch, "k1").await.unwrap(), None);
}

#[tokio::test]
async fn old_receipts_are_cleaned_up() {
    let (store, pool) = store().await;
    let launch = fresh_launch();
    store.put(&launch, "k1", receipt(1, b"x")).await.unwrap();
    sqlx::query(
        "UPDATE workflow_api_receipts SET created_at = now() - interval '2 days' \
         WHERE launch_id = $1",
    )
    .bind(&launch)
    .execute(&pool)
    .await
    .unwrap();
    let deleted = store
        .delete_older_than(std::time::Duration::from_secs(86_400))
        .await
        .unwrap();
    assert!(deleted >= 1);
    assert_eq!(store.get(&launch, "k1").await.unwrap(), None);
}

/// Counts outcomes; a retry served from a receipt never reaches it.
#[derive(Default)]
struct Outcomes(AtomicUsize);

#[async_trait]
impl WorkflowApiBackend for Outcomes {
    async fn launch_state(&self, _: &Binding) -> Result<LaunchState, ApiError> {
        Ok(LaunchState::Active)
    }
    async fn run_info(&self, _: &Binding) -> Result<RunInfo, ApiError> {
        unreachable!()
    }
    async fn run_input(&self, _: &Binding) -> Result<Vec<u8>, ApiError> {
        unreachable!()
    }
    async fn post_outcome(&self, _: &Binding, _: PostedOutcome) -> Result<(), ApiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn set_deadline(&self, _: &Binding, _: u64) -> Result<(), ApiError> {
        unreachable!()
    }
}

#[tokio::test]
async fn a_retry_after_a_restart_gets_the_stored_answer() {
    let (store, _) = store().await;
    let store = Arc::new(store);
    let binding = Binding {
        tenant_id: "tenant-a".into(),
        instance_id: "instance-1".into(),
        launch_id: fresh_launch(),
        root_lease_epoch: 1,
    };
    let post = |body: &'static str| {
        Request::builder()
            .method("POST")
            .uri("/v1/run/outcome")
            .header(headers::OUTCOME, "completed")
            .header(headers::IDEMPOTENCY_KEY, "k1")
            .extension(binding.clone())
            .body(Body::from(body))
            .unwrap()
    };
    let serve = |backend: Arc<Outcomes>| {
        router(WorkflowApiState {
            backend,
            receipts: store.clone(),
            caps: Caps::default(),
        })
    };

    let before = Arc::new(Outcomes::default());
    let response = serve(before.clone()).oneshot(post("{}")).await.unwrap();
    assert_eq!(response.status(), 204);
    assert_eq!(before.0.load(Ordering::SeqCst), 1);

    // A new router and backend over the same database: the retry is answered
    // from the receipt and never reaches the backend.
    let after = Arc::new(Outcomes::default());
    let response = serve(after.clone()).oneshot(post("{}")).await.unwrap();
    assert_eq!(response.status(), 204);
    assert_eq!(after.0.load(Ordering::SeqCst), 0);

    // The same key with another request is still a conflict.
    let response = serve(after.clone())
        .oneshot(post(r#"{"other":1}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("replay-conflict"));
}

#[tokio::test]
async fn an_omitted_body_is_stored_as_omitted() {
    let (store, _) = store().await;
    let launch = fresh_launch();
    let mut omitted = receipt(3, b"");
    omitted.body_omitted = true;
    store.put(&launch, "big", omitted.clone()).await.unwrap();
    assert_eq!(store.get(&launch, "big").await.unwrap(), Some(omitted));
}

/// Two processes sharing the database (two stores on their own pools) hold a
/// key in turn: the second waits until the first lets go.
#[tokio::test]
async fn a_key_is_held_across_processes() {
    let (first, _) = store().await;
    let (second, _) = store().await;
    let launch = fresh_launch();
    let held = first.hold(&launch, "k").await.unwrap();
    let other_key = second.hold(&launch, "other").await.unwrap();
    let waiter = tokio::spawn({
        let launch = launch.clone();
        async move { second.hold(&launch, "k").await.map(|_| ()) }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !waiter.is_finished(),
        "the key is held by the other process"
    );
    drop(held);
    tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
        .await
        .expect("released when the hold dropped")
        .unwrap()
        .unwrap();
    drop(other_key);
}
