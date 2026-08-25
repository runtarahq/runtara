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
        empty_retry_interval: Duration::from_secs(300),
        refresh_interval: Duration::from_secs(300),
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

/// A reachable endpoint must leave the cache populated before `with_policy` returns — the
/// happy path still warms the cache before the server serves its first request.
#[tokio::test]
async fn successful_startup_populates_before_returning() {
    let server = MockServer::start().await;
    mount_jwks(
        &server,
        ResponseTemplate::new(200).set_body_json(jwks_body()),
    )
    .await;

    let cache = JwksCache::with_policy(uri(&server), fast_policy()).await;

    assert!(cache.is_ready().await, "cache should be warm on return");
    assert_eq!(
        request_count(&server).await,
        1,
        "one round trip, not a loop"
    );
}

/// Startup must not BLOCK on an unreachable endpoint.
///
/// Found on the dev canary: retrying inline held the listening port unbound for ~32s, so
/// `/ready` could not report 503 during exactly the window it exists for — a monitor saw
/// connection-refused, indistinguishable from a dead process. One attempt, then hand off.
#[tokio::test]
async fn startup_does_not_block_on_an_unreachable_endpoint() {
    let server = MockServer::start().await;
    // Every request hangs well past any sane startup budget.
    mount_jwks(
        &server,
        ResponseTemplate::new(200).set_delay(Duration::from_secs(30)),
    )
    .await;

    let started = std::time::Instant::now();
    let cache = tokio::time::timeout(
        Duration::from_secs(20),
        JwksCache::with_policy(uri(&server), fast_policy()),
    )
    .await
    .expect("startup must not block on the JWKS endpoint");

    assert!(!cache.is_ready().await, "cache is empty, so report unready");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "startup took {:?} — it is retrying inline again",
        started.elapsed()
    );
}

/// A healthy start must not trigger a second fetch moments later.
///
/// Also found on the canary: arming the refresher *before* the first fetch meant it always
/// picked its empty-cache interval, so every successful boot paid a redundant round trip.
#[tokio::test]
async fn healthy_startup_does_not_trigger_a_redundant_refresh() {
    let server = MockServer::start().await;
    mount_jwks(
        &server,
        ResponseTemplate::new(200).set_body_json(jwks_body()),
    )
    .await;

    let policy = RetryPolicy {
        // Short enough that a refresher using the EMPTY interval would fire during the wait.
        empty_retry_interval: Duration::from_millis(100),
        refresh_interval: Duration::from_secs(300),
        ..fast_policy()
    };
    let cache = JwksCache::with_policy(uri(&server), policy).await;
    assert!(cache.is_ready().await);

    tokio::time::sleep(Duration::from_millis(600)).await;

    assert_eq!(
        request_count(&server).await,
        1,
        "a warm cache should sit on the hourly interval, not the empty-cache one"
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
    assert_eq!(
        request_count(&server).await,
        1,
        "startup makes exactly one attempt; retrying is the refresher's job"
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

    // The startup fetch stamps the attempt clock, so wait out the miss cooldown first —
    // otherwise this measures the cooldown (covered by its own test) rather than coalescing.
    tokio::time::sleep(Duration::from_millis(120)).await;
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
