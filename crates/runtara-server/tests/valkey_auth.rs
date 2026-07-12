// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Round-trip tests for the Valkey auth-contract client
//! (`valkey::auth::{get_member_role, token_is_revoked}`).
//!
//! These need a live Valkey/Redis. They use `VALKEY_HOST` (and friends) in the same shape
//! as the server boot path and fail closed when it is unset. Run with the
//! explicit `valkey-integration-tests` feature.

use redis::AsyncCommands;
use runtara_server::authz::Role;
use runtara_server::valkey::ValkeyConfig;
use runtara_server::valkey::auth::{get_member_role, revoke_token, token_is_revoked};

/// Resolve the required Valkey URL or fail the explicit integration suite.
macro_rules! redis_url_or_skip {
    () => {
        ValkeyConfig::from_env()
            .expect("valkey-integration-tests requires VALKEY_HOST")
            .connection_url()
    };
}

/// A FRESH `ConnectionManager` bound to the calling test's runtime.
///
/// Do NOT use the process-wide `valkey::get_or_create_manager` cache here: it is a
/// `OnceCell<ConnectionManager>`, and a `ConnectionManager`'s background driver lives on the
/// tokio runtime that first created it. Each `#[tokio::test]` runs on its own throwaway runtime,
/// so a manager cached by an earlier test is driven by a now-dropped runtime and every later op
/// fails with "broken pipe". A per-test manager is bound to the current runtime and works.
async fn fresh_manager(url: &str) -> redis::aio::ConnectionManager {
    let client = runtara_server::valkey::open_client(url).expect("open valkey client");
    redis::aio::ConnectionManager::new(client)
        .await
        .expect("connect valkey")
}

/// Unique id per test run so concurrent runs / leftover state don't collide.
fn unique(prefix: &str) -> String {
    format!("{}-{}", prefix, uuid::Uuid::new_v4())
}

#[tokio::test]
async fn get_member_role_reads_each_role() {
    let url = redis_url_or_skip!();
    let manager = fresh_manager(&url).await;

    for (wire, role) in [
        ("owner", Role::Owner),
        ("admin", Role::Admin),
        ("member", Role::Member),
        ("viewer", Role::Viewer),
    ] {
        let uid = unique("auth0|test");
        let key = format!("member:{uid}");
        let value = format!(r#"{{"role":"{wire}","updated_at":"2026-05-28T12:00:00Z"}}"#);
        let mut conn = manager.clone();
        let _: () = conn.set(&key, &value).await.expect("set member");

        let got = get_member_role(&manager, &uid).await.expect("read role");
        assert_eq!(got, Some(role));

        let _: () = conn.del(&key).await.expect("cleanup");
    }
}

#[tokio::test]
async fn get_member_role_absent_is_none() {
    let url = redis_url_or_skip!();
    let manager = fresh_manager(&url).await;

    let uid = unique("auth0|missing");
    let got = get_member_role(&manager, &uid).await.expect("read role");
    assert_eq!(got, None, "absent member key must read as not-a-member");
}

#[tokio::test]
async fn get_member_role_malformed_fails_closed() {
    let url = redis_url_or_skip!();
    let manager = fresh_manager(&url).await;

    let uid = unique("auth0|bad");
    let key = format!("member:{uid}");
    let mut conn = manager.clone();
    let _: () = conn
        .set(&key, r#"{"role":"superuser"}"#)
        .await
        .expect("set");

    assert!(
        get_member_role(&manager, &uid).await.is_err(),
        "unknown role must surface an error, not a default"
    );

    let _: () = conn.del(&key).await.expect("cleanup");
}

#[tokio::test]
async fn token_is_revoked_reflects_presence() {
    let url = redis_url_or_skip!();
    let manager = fresh_manager(&url).await;

    let jti = unique("jti");
    assert!(
        !token_is_revoked(&manager, &jti).await.expect("check"),
        "absent denylist key means not revoked"
    );

    let key = format!("token:revoked:{jti}");
    let mut conn = manager.clone();
    let _: () = conn
        .set(&key, r#"{"revoked_at":"2026-05-28T12:00:00Z"}"#)
        .await
        .expect("set");

    assert!(
        token_is_revoked(&manager, &jti).await.expect("check"),
        "present denylist key means revoked"
    );

    let _: () = conn.del(&key).await.expect("cleanup");
}

#[tokio::test]
async fn revoke_token_then_seen_as_revoked() {
    let url = redis_url_or_skip!();
    let manager = fresh_manager(&url).await;

    let jti = unique("jti");
    assert!(
        !token_is_revoked(&manager, &jti).await.expect("check"),
        "not revoked before write"
    );

    revoke_token(&manager, &jti, Some(60))
        .await
        .expect("revoke");
    assert!(
        token_is_revoked(&manager, &jti).await.expect("check"),
        "revoked after write"
    );

    let mut conn = manager.clone();
    let _: () = conn
        .del(format!("token:revoked:{jti}"))
        .await
        .expect("cleanup");
}
