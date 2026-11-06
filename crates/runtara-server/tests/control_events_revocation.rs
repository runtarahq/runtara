//! Integration tests for the `member_removed` control-event revocation path (SYN-939).
//!
//! These drive the *real* consumer ([`runtara_server::valkey::control_events::run`]) against a
//! live Postgres and a live Valkey, covering the two things unit tests cannot:
//!
//!   1. the end-to-end stream round trip — `XADD` a `member_removed` event → the consumer reads it
//!      → `revoke_user_keys_before` flips `is_revoked` and writes the Valkey denylist → the entry
//!      is `XACK`ed off the consumer group's pending list; and
//!   2. the startup reconcile sweep — a user who still holds an active key but is no longer a
//!      member (their `{tenant}:member:{sub}` key is gone) has that key revoked at startup, while a
//!      user who *is* still a member keeps theirs.
//!
//! Gated behind `--features db-integration-tests,valkey-integration-tests` (see Cargo.toml). Run
//! with a live Postgres + Valkey, e.g.:
//!
//! ```text
//! TEST_RUNTARA_SERVER_DATABASE_URL=postgres://postgres:postgres@localhost:5432/runtara_test \
//! VALKEY_HOST=localhost VALKEY_PORT=6379 \
//! cargo test -p runtara-server --features db-integration-tests,valkey-integration-tests \
//!   --test control_events_revocation -- --test-threads=1
//! ```
//!
//! Every run uses a fresh random tenant id, so the Valkey keys (`{tenant}:control`,
//! `{tenant}:member:{sub}`, `{tenant}:token:revoked:{jti}`) and the `api_keys` rows are isolated
//! and cleaned up afterward — concurrent runs don't collide.

use std::time::Duration;

use chrono::Utc;
use redis::aio::ConnectionManager;
use runtara_server::shutdown::ShutdownSignal;
use runtara_server::valkey::control_events::{self, CONTROL_CONSUMER_GROUP, control_stream_key};
use runtara_server::valkey::{ValkeyConfig, open_client};
use sqlx::PgPool;
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

async fn test_pool() -> PgPool {
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect(
            "db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL \
             or RUNTARA_SERVER_DATABASE_URL",
        );
    let pool = PgPool::connect(&url)
        .await
        .expect("connect to the test Postgres");
    MIGRATOR.run(&pool).await.expect("apply server migrations");
    pool
}

fn valkey_config() -> ValkeyConfig {
    ValkeyConfig::from_env().expect("valkey-integration-tests requires VALKEY_HOST")
}

async fn valkey_manager(config: &ValkeyConfig) -> ConnectionManager {
    let client = open_client(&config.connection_url()).expect("open valkey client");
    ConnectionManager::new(client)
        .await
        .expect("connect valkey manager")
}

fn unique_tenant() -> String {
    // Tenant ids are lowercase letters and digits; keep that shape so key handling matches prod.
    format!("t{}", Uuid::new_v4().simple())
}

/// Seed one active (non-revoked) key for `user` in `tenant`, created an hour ago so any removal
/// timestamp "now" is after it. Returns the key's `jti`.
async fn seed_active_key(pool: &PgPool, tenant: &str, user: &str) -> String {
    let jti = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO api_keys \
         (org_id, name, key_prefix, key_hash, issuing_user_id, jti, created_at, is_revoked) \
         VALUES ($1, 'control-test', 'rt_test', $2, $3, $4, NOW() - INTERVAL '1 hour', FALSE)",
    )
    .bind(tenant)
    .bind(format!("hash-{jti}"))
    .bind(user)
    .bind(&jti)
    .execute(pool)
    .await
    .expect("insert api key");
    jti
}

async fn is_revoked(pool: &PgPool, tenant: &str, jti: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT is_revoked FROM api_keys WHERE org_id = $1 AND jti = $2")
        .bind(tenant)
        .bind(jti)
        .fetch_one(pool)
        .await
        .expect("read is_revoked")
}

/// Poll until the key reads revoked, or the deadline passes.
async fn wait_until_revoked(pool: &PgPool, tenant: &str, jti: &str, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if is_revoked(pool, tenant, jti).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn key_exists(conn: &mut ConnectionManager, key: &str) -> bool {
    redis::cmd("EXISTS")
        .arg(key)
        .query_async::<bool>(conn)
        .await
        .expect("EXISTS")
}

/// XPENDING summary reply: `[count, min-id, max-id, consumers]` (ids/consumers are nil when the
/// count is 0). `consumers` is kept as a raw `Value` since we only read the count.
type XPendingSummary = (i64, Option<String>, Option<String>, Option<redis::Value>);

/// Count of entries still pending (delivered but un-acked) for the control consumer group.
async fn group_pending_count(conn: &mut ConnectionManager, stream: &str) -> i64 {
    // When the group doesn't exist yet Valkey errors with NOGROUP — treat that as "nothing pending".
    let reply: redis::RedisResult<XPendingSummary> = redis::cmd("XPENDING")
        .arg(stream)
        .arg(CONTROL_CONSUMER_GROUP)
        .query_async(conn)
        .await;
    match reply {
        Ok((count, _, _, _)) => count,
        Err(_) => 0,
    }
}

/// Wait until the consumer group has acked everything (0 pending), or the deadline passes.
async fn wait_until_acked(conn: &mut ConnectionManager, stream: &str, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if group_pending_count(conn, stream).await == 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn cleanup(pool: &PgPool, conn: &mut ConnectionManager, tenant: &str, keys: &[String]) {
    let _ = sqlx::query("DELETE FROM api_keys WHERE org_id = $1")
        .bind(tenant)
        .execute(pool)
        .await;
    for key in keys {
        let _: redis::RedisResult<()> = redis::cmd("DEL").arg(key).query_async(conn).await;
    }
}

/// XADD a `member_removed` event exactly as `runtara-cloud`'s `publish_member_removed` does:
/// fields `type`, `sub`, `at` (unix millis).
async fn publish_member_removed(conn: &mut ConnectionManager, tenant: &str, sub: &str, at_ms: i64) {
    let _: String = redis::cmd("XADD")
        .arg(control_stream_key(tenant))
        .arg("*")
        .arg("type")
        .arg("member_removed")
        .arg("sub")
        .arg(sub)
        .arg("at")
        .arg(at_ms)
        .query_async(conn)
        .await
        .expect("XADD member_removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn control_event_revokes_user_keys_round_trip() {
    let pool = test_pool().await;
    let config = valkey_config();
    let mut conn = valkey_manager(&config).await;

    let tenant = unique_tenant();
    let user = "auth0|roundtrip-user";
    let jti = seed_active_key(&pool, &tenant, user).await;

    let stream = control_stream_key(&tenant);
    let denylist = format!("{tenant}:token:revoked:{jti}");

    // Start the real consumer for this tenant.
    let shutdown = ShutdownSignal::new();
    let consumer = tokio::spawn(control_events::run(
        pool.clone(),
        config.clone(),
        tenant.clone(),
        shutdown.clone(),
    ));

    // Give it a beat to create the consumer group, then publish the removal event.
    sleep(Duration::from_millis(300)).await;
    publish_member_removed(&mut conn, &tenant, user, Utc::now().timestamp_millis()).await;

    // The key must become revoked (DB) ...
    assert!(
        wait_until_revoked(&pool, &tenant, &jti, Duration::from_secs(10)).await,
        "key should be revoked after the member_removed event"
    );
    // ... its jti written to the Valkey denylist ...
    assert!(
        key_exists(&mut conn, &denylist).await,
        "revoked key's jti should be on the Valkey denylist"
    );
    // ... and the stream entry XACKed off the group's pending list.
    assert!(
        wait_until_acked(&mut conn, &stream, Duration::from_secs(5)).await,
        "the processed entry should be acknowledged (no pending entries left)"
    );

    shutdown.trigger();
    let _ = timeout(Duration::from_secs(5), consumer).await;
    cleanup(&pool, &mut conn, &tenant, &[stream, denylist]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn startup_reconcile_revokes_former_member_keys() {
    let pool = test_pool().await;
    let config = valkey_config();
    let mut conn = valkey_manager(&config).await;

    let tenant = unique_tenant();
    // `gone` has an active key but no membership (removal event was lost to stream trimming).
    let gone = "auth0|former-member";
    let gone_jti = seed_active_key(&pool, &tenant, gone).await;
    // `present` has an active key and a live membership key — must be left alone.
    let present = "auth0|current-member";
    let present_jti = seed_active_key(&pool, &tenant, present).await;
    let present_member_key = format!("{tenant}:member:{present}");
    let _: () = redis::cmd("SET")
        .arg(&present_member_key)
        .arg(r#"{"role":"member"}"#)
        .query_async(&mut conn)
        .await
        .expect("SET membership key");

    let stream = control_stream_key(&tenant);
    let gone_denylist = format!("{tenant}:token:revoked:{gone_jti}");

    // Starting the consumer runs the reconcile sweep once, before it listens on the stream.
    let shutdown = ShutdownSignal::new();
    let consumer = tokio::spawn(control_events::run(
        pool.clone(),
        config.clone(),
        tenant.clone(),
        shutdown.clone(),
    ));

    // The former member's key must be revoked by the sweep ...
    assert!(
        wait_until_revoked(&pool, &tenant, &gone_jti, Duration::from_secs(10)).await,
        "the reconcile sweep should revoke a former member's active key"
    );
    assert!(
        key_exists(&mut conn, &gone_denylist).await,
        "the swept key's jti should be on the Valkey denylist"
    );
    // ... while the current member's key is preserved. The sweep has no code path that revokes a
    // current member, so this holds regardless of the order the two users were processed in.
    assert!(
        !is_revoked(&pool, &tenant, &present_jti).await,
        "the reconcile sweep must not touch a current member's key"
    );

    shutdown.trigger();
    let _ = timeout(Duration::from_secs(5), consumer).await;
    cleanup(
        &pool,
        &mut conn,
        &tenant,
        &[stream, gone_denylist, present_member_key],
    )
    .await;
}
