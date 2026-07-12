// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Live TLS round-trip tests for the Valkey connection layer.
//!
//! These need a TLS-enabled Valkey/Redis and fail closed unless both
//! `VALKEY_HOST` and `VALKEY_TLS` are set. Trust the server's self-signed
//! certificate either via `VALKEY_TLS_CA_CERT=/path/to/cert.pem` (verified)
//! or `VALKEY_TLS_INSECURE=1` (encrypted, unverified). Run with:
//!   `VALKEY_HOST=localhost VALKEY_PORT=6390 VALKEY_TLS=1 \
//!    VALKEY_TLS_CA_CERT=/path/server.crt \
//!    cargo test -p runtara-server --test valkey_tls`
//!
//! `wrong_ca_fails_verification` additionally needs `VALKEY_TLS_WRONG_CA`
//! pointing at a PEM certificate that is NOT the server's.
//!
//! Certificate gotcha: rustls rejects a server certificate carrying
//! `basicConstraints CA:TRUE` (`CaUsedAsEndEntity`) — which is what a bare
//! `openssl req -x509` produces. Either use a private CA (CA cert signs a
//! CA:FALSE server cert; point `VALKEY_TLS_CA_CERT` at the CA cert) or
//! generate the single self-signed server cert with
//! `-addext "basicConstraints=critical,CA:FALSE"` and trust it directly.

use redis::AsyncCommands;
use runtara_server::valkey::{ValkeyConfig, open_client};
use std::time::Duration;

/// Resolve the required TLS Valkey config or fail the explicit suite.
macro_rules! tls_config_or_skip {
    () => {
        ValkeyConfig::from_env()
            .filter(|cfg| cfg.tls)
            .expect("valkey-tls-integration-tests requires VALKEY_HOST and VALKEY_TLS=1")
    };
}

/// A fresh `ConnectionManager` bound to the calling test's runtime (see
/// `valkey_auth.rs` for why the process-wide cache must not be used here).
async fn fresh_manager(url: &str) -> redis::aio::ConnectionManager {
    let client = open_client(url).expect("open valkey client");
    redis::aio::ConnectionManager::new(client)
        .await
        .expect("connect valkey over TLS")
}

#[tokio::test]
async fn tls_round_trip() {
    let cfg = tls_config_or_skip!();
    let url = cfg.connection_url();
    assert!(
        url.starts_with("rediss://"),
        "VALKEY_TLS must produce a rediss:// URL, got {url}"
    );

    let mut conn = fresh_manager(&url).await;

    let pong: String = redis::cmd("PING")
        .query_async(&mut conn)
        .await
        .expect("PING over TLS");
    assert_eq!(pong, "PONG");

    let key = format!("tls-test-{}", uuid::Uuid::new_v4());
    let _: () = conn.set(&key, "encrypted").await.expect("SET over TLS");
    let got: String = conn.get(&key).await.expect("GET over TLS");
    assert_eq!(got, "encrypted");
    let _: () = conn.del(&key).await.expect("cleanup");
}

#[tokio::test]
async fn tls_stream_round_trip() {
    // XADD/XRANGE mirror what the trigger publisher and workers actually do.
    let cfg = tls_config_or_skip!();
    let mut conn = fresh_manager(&cfg.connection_url()).await;

    let stream = format!("tls-test-stream-{}", uuid::Uuid::new_v4());
    let id: String = redis::cmd("XADD")
        .arg(&stream)
        .arg("*")
        .arg("k")
        .arg("v")
        .query_async(&mut conn)
        .await
        .expect("XADD over TLS");
    assert!(!id.is_empty());

    let entries: redis::streams::StreamRangeReply =
        conn.xrange_all(&stream).await.expect("XRANGE over TLS");
    assert_eq!(entries.ids.len(), 1);
    let _: () = conn.del(&stream).await.expect("cleanup");
}

#[tokio::test]
async fn plaintext_to_tls_port_fails() {
    // A plaintext client pointed at the TLS port must not be able to complete
    // a command — proof the port actually requires TLS. Both an outright
    // error and a hang (killed by the timeout) count as failure to
    // communicate; only a successful PING fails the test.
    let cfg = tls_config_or_skip!();
    let plain_url = format!("redis://{}:{}", cfg.host, cfg.port);

    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let client = open_client(&plain_url)?;
        let mut conn = client.get_multiplexed_async_connection().await?;
        redis::cmd("PING").query_async::<String>(&mut conn).await
    })
    .await;

    match result {
        Err(_elapsed) => {}       // timed out: no plaintext service on the TLS port
        Ok(Err(_redis_err)) => {} // rejected: also fine
        Ok(Ok(pong)) => panic!(
            "plaintext PING to the TLS port unexpectedly succeeded ({pong}) — \
             the endpoint is not actually TLS-only"
        ),
    }
}

#[tokio::test]
async fn wrong_ca_fails_verification() {
    // With a CA that did not sign the server's certificate, the TLS handshake
    // must fail — proof that certificate verification is actually happening
    // (i.e. the CA path is not silently falling back to insecure mode).
    let cfg = tls_config_or_skip!();
    let wrong_ca = std::env::var("VALKEY_TLS_WRONG_CA")
        .expect("valkey-tls-integration-tests requires VALKEY_TLS_WRONG_CA");

    let url = format!("rediss://{}:{}", cfg.host, cfg.port);
    let root_cert = std::fs::read(&wrong_ca).expect("read wrong-CA pem");
    let client = redis::Client::build_with_tls(
        url.as_str(),
        redis::TlsCertificates {
            client_tls: None,
            root_cert: Some(root_cert),
        },
    )
    .expect("client builds fine; failure must happen at handshake");

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        client.get_multiplexed_async_connection(),
    )
    .await;

    // Only a successful connection fails the test: an error or a timeout both
    // mean the handshake was rejected.
    if let Ok(Ok(_)) = result {
        panic!(
            "TLS handshake with a non-matching CA unexpectedly succeeded — \
             server certificate verification is not effective"
        );
    }
}
