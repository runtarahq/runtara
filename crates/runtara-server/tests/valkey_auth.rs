// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Round-trip tests for the SYN-437 Valkey auth-contract client
//! (`valkey::auth::{get_member_role, token_is_revoked}`).
//!
//! These need a live Valkey/Redis. They use `VALKEY_HOST` (and friends) in the same shape
//! as the server boot path and skip cleanly when it is unset, mirroring `redis_isolation.rs`.
//! Run with:
//!   `VALKEY_HOST=localhost cargo test -p runtara-server --test valkey_auth`

use redis::AsyncCommands;
use runtara_server::authz::Role;
use runtara_server::valkey::auth::{get_member_role, token_is_revoked};
use runtara_server::valkey::{ValkeyConfig, get_or_create_manager};

/// Skip the test if Valkey is not configured in the environment.
macro_rules! redis_url_or_skip {
    () => {
        match ValkeyConfig::from_env() {
            Some(cfg) => cfg.connection_url(),
            None => {
                eprintln!("Skipping test: VALKEY_HOST not set");
                return;
            }
        }
    };
}

/// Unique id per test run so concurrent runs / leftover state don't collide.
fn unique(prefix: &str) -> String {
    format!("{}-{}", prefix, uuid::Uuid::new_v4())
}

#[tokio::test]
async fn get_member_role_reads_each_role() {
    let url = redis_url_or_skip!();
    let manager = get_or_create_manager(&url).await.expect("connect valkey");

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
    let manager = get_or_create_manager(&url).await.expect("connect valkey");

    let uid = unique("auth0|missing");
    let got = get_member_role(&manager, &uid).await.expect("read role");
    assert_eq!(got, None, "absent member key must read as not-a-member");
}

#[tokio::test]
async fn get_member_role_malformed_fails_closed() {
    let url = redis_url_or_skip!();
    let manager = get_or_create_manager(&url).await.expect("connect valkey");

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
    let manager = get_or_create_manager(&url).await.expect("connect valkey");

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
