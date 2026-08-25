//! JWKS startup-recovery and cache-miss behaviour.
//!
//! A failed startup fetch used to be terminal — one unlucky request during boot left the
//! process unable to validate any token for its whole life. These tests pin the recovery
//! behaviour that replaced it, and the rate limiting that keeps a permanently-empty cache from
//! hammering the endpoint that is already failing.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rsa::RsaPrivateKey;
use rsa::traits::PublicKeyParts;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use runtara_server::auth::jwks::{JwksCache, RetryPolicy};

const JWKS_PATH: &str = "/.well-known/jwks.json";
const KID: &str = "test-kid";

/// A JWKS body with one usable RSA signing key. The key never signs anything here — these
/// tests care about whether the cache populates, not about validation.
fn jwks_body() -> serde_json::Value {
    let mut rng = rand::thread_rng();
    let key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
    let public = key.to_public_key();
    json!({
        "keys": [{
            "kty": "RSA",
            "use": "sig",
            "kid": KID,
            "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
            "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be()),
        }]
    })
}

/// Timings small enough to run in a test. `refresh_interval`/`empty_retry_interval` are long
/// by default so the background task cannot perturb request counts; tests that exercise
/// healing shorten them explicitly.
fn fast_policy() -> RetryPolicy {
    RetryPolicy {
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
        startup_budget: Duration::from_millis(500),
        refresh_interval: Duration::from_secs(300),
        empty_retry_interval: Duration::from_secs(300),
        miss_cooldown: Duration::from_millis(50),
    }
}

async fn mount_jwks(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(JWKS_PATH))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn request_count(server: &MockServer) -> usize {
    server.received_requests().await.map_or(0, |r| r.len())
}

fn uri(server: &MockServer) -> String {
    format!("{}{}", server.uri(), JWKS_PATH)
}

/// The regression: a JWKS endpoint that fails and then recovers must leave the cache
/// populated. On the old code the first failure panicked and the process exited 101.
#[tokio::test]
async fn transient_startup_failure_is_retried_until_it_succeeds() {
    let server = MockServer::start().await;

    // First two attempts fail; the third serves a valid document.
    Mock::given(method("GET"))
        .and(path(JWKS_PATH))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(JWKS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(jwks_body()))
        .with_priority(2)
        .mount(&server)
        .await;

    let cache = JwksCache::with_policy(uri(&server), fast_policy()).await;

    assert!(
        cache.is_ready().await,
        "cache should be populated after the endpoint recovered"
    );
    assert_eq!(
        request_count(&server).await,
        3,
        "expected two failures then one success"
    );
}

/// An endpoint that never recovers must not panic and must not block startup — the process
/// comes up unready rather than dying, which is what lets the refresher heal it later.
#[tokio::test]
async fn permanent_startup_failure_leaves_an_empty_cache_without_panicking() {
    let server = MockServer::start().await;
    mount_jwks(&server, ResponseTemplate::new(500)).await;

    let cache = JwksCache::with_policy(uri(&server), fast_policy()).await;

    assert!(
        !cache.is_ready().await,
        "cache must report unready when no key was ever fetched"
    );
    assert!(
        request_count(&server).await > 1,
        "the startup budget should have covered more than a single attempt"
    );
}

/// The property the whole design rests on: a process that started with an empty cache heals
/// itself, with no restart, once the endpoint comes back.
#[tokio::test]
async fn background_refresher_heals_an_empty_cache() {
    let server = MockServer::start().await;
    mount_jwks(&server, ResponseTemplate::new(500)).await;

    let policy = RetryPolicy {
        empty_retry_interval: Duration::from_millis(50),
        ..fast_policy()
    };
    let cache = JwksCache::with_policy(uri(&server), policy).await;
    assert!(!cache.is_ready().await, "precondition: started unready");

    // The endpoint comes back.
    server.reset().await;
    mount_jwks(
        &server,
        ResponseTemplate::new(200).set_body_json(jwks_body()),
    )
    .await;

    let healed = tokio::time::timeout(Duration::from_secs(5), async {
        while !cache.is_ready().await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;

    assert!(
        healed.is_ok(),
        "background refresher never recovered the empty cache"
    );
}

/// `get_key` runs on every authenticated request. Concurrent misses must collapse into one
/// upstream fetch, not one per request — otherwise keeping the process alive with an empty
/// cache would point a request-rate flood at the failing endpoint.
#[tokio::test]
async fn concurrent_cache_misses_collapse_into_one_fetch() {
    let server = MockServer::start().await;
    // Slow enough that the whole burst arrives while the first fetch is still in flight.
    mount_jwks(
        &server,
        ResponseTemplate::new(200)
            .set_body_json(jwks_body())
            .set_delay(Duration::from_millis(300)),
    )
    .await;

    let cache = JwksCache::with_policy(uri(&server), fast_policy()).await;
    let after_startup = request_count(&server).await;

    let mut tasks = Vec::new();
    for _ in 0..10 {
        let cache: Arc<JwksCache> = cache.clone();
        tasks.push(tokio::spawn(
            async move { cache.get_key("unknown-kid").await },
        ));
    }
    for task in tasks {
        assert!(task.await.unwrap().is_none(), "kid was never in the JWKS");
    }

    assert_eq!(
        request_count(&server).await - after_startup,
        1,
        "10 concurrent misses should share a single upstream fetch"
    );
}

/// After an attempt, further misses inside the cooldown must not re-dial at all — this is what
/// bounds the load when the cache is empty and every request is missing.
#[tokio::test]
async fn misses_inside_the_cooldown_do_not_redial() {
    let server = MockServer::start().await;
    mount_jwks(
        &server,
        ResponseTemplate::new(200).set_body_json(jwks_body()),
    )
    .await;

    let policy = RetryPolicy {
        miss_cooldown: Duration::from_secs(60),
        ..fast_policy()
    };
    let cache = JwksCache::with_policy(uri(&server), policy).await;

    assert!(cache.get_key("unknown-kid").await.is_none());
    let after_first_miss = request_count(&server).await;

    for _ in 0..5 {
        assert!(cache.get_key("unknown-kid").await.is_none());
    }

    assert_eq!(
        request_count(&server).await,
        after_first_miss,
        "misses within the cooldown must not reach the network"
    );
}
