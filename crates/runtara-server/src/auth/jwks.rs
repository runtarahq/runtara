use jsonwebtoken::DecodingKey;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
// tokio's Instant, not std's, so the startup budget and the miss cooldown both respect a
// paused clock — otherwise testing them means waiting out the real production intervals.
use tokio::time::Instant;

/// Timing knobs for fetching and refreshing the JWKS.
///
/// Grouped into one struct so tests can shrink every interval at once instead of waiting out
/// production values; [`RetryPolicy::default`] is what the server runs with.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// First backoff step for the startup fetch; doubles up to `max_backoff`.
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    /// How long the startup fetch may keep retrying before the server continues without keys.
    pub startup_budget: Duration,
    /// Background refresh interval once the cache holds keys.
    pub refresh_interval: Duration,
    /// Background refresh interval while the cache is still empty — far shorter, because in
    /// that state every authenticated request is failing.
    pub empty_retry_interval: Duration,
    /// Minimum gap between refreshes triggered from the request path, so a cache miss storm
    /// cannot fan out one upstream fetch per request.
    pub miss_cooldown: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(8),
            startup_budget: Duration::from_secs(30),
            refresh_interval: Duration::from_secs(3600),
            empty_retry_interval: Duration::from_secs(15),
            miss_cooldown: Duration::from_secs(10),
        }
    }
}

/// JWKS cache that stores decoding keys indexed by `kid`.
/// Supports background refresh and on-demand refresh on cache miss.
pub struct JwksCache {
    keys: RwLock<HashMap<String, DecodingKey>>,
    jwks_uri: String,
    client: reqwest::Client,
    policy: RetryPolicy,
    /// Single-flight gate: only one refresh is in flight at a time, so concurrent cache
    /// misses coalesce into one upstream fetch instead of one fetch each.
    refreshing: Mutex<()>,
    /// When the last refresh attempt finished, used to rate-limit request-path refreshes.
    last_attempt: RwLock<Option<Instant>>,
}

/// A single JWK key from the JWKS endpoint
#[derive(serde::Deserialize)]
struct JwkKey {
    kid: Option<String>,
    kty: String,
    #[serde(rename = "use")]
    key_use: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

#[derive(serde::Deserialize)]
struct JwksResponse {
    keys: Vec<JwkKey>,
}

impl JwksCache {
    /// Create a JWKS cache, arm its background refresher, and populate it.
    ///
    /// Never panics, and never gives up. A failed startup fetch used to be terminal: one
    /// unlucky request during boot left the process alive but unable to validate any token
    /// for the rest of its life, because the refresher was armed on the line *after* the
    /// fetch and so was never spawned. Now the refresher is armed first, the initial fetch
    /// retries with bounded backoff, and if the endpoint is still unreachable when that
    /// budget runs out the server starts anyway and the refresher keeps trying until it
    /// succeeds. Readiness is reported separately by [`JwksCache::is_ready`].
    pub async fn new(jwks_uri: String) -> Arc<Self> {
        Self::with_policy(jwks_uri, RetryPolicy::default()).await
    }

    /// [`JwksCache::new`] with explicit timings — for tests that cannot wait out the
    /// production backoff and refresh intervals.
    pub async fn with_policy(jwks_uri: String, policy: RetryPolicy) -> Arc<Self> {
        let cache = Arc::new(Self {
            keys: RwLock::new(HashMap::new()),
            jwks_uri,
            client: reqwest::Client::new(),
            policy,
            refreshing: Mutex::new(()),
            last_attempt: RwLock::new(None),
        });

        // Armed BEFORE the first fetch: this is the path that recovers a failed startup, so
        // it must exist even when that fetch never succeeds.
        Self::spawn_refresh_task(cache.clone());
        cache.populate_with_retry().await;
        cache
    }

    /// Whether the cache holds any signing key. False means every token this process is asked
    /// to validate will be rejected, whatever the token.
    pub async fn is_ready(&self) -> bool {
        !self.keys.read().await.is_empty()
    }

    /// Fetch the JWKS, retrying transient failures with exponential backoff until the
    /// startup budget is spent. Returns either way — the caller starts serving regardless.
    async fn populate_with_retry(&self) {
        let deadline = Instant::now() + self.policy.startup_budget;
        let mut backoff = self.policy.initial_backoff;

        for attempt in 1.. {
            let Err(e) = self.refresh().await else {
                return;
            };

            if Instant::now() >= deadline {
                tracing::error!(
                    jwks_uri = %self.jwks_uri,
                    attempts = attempt,
                    error = %e,
                    "JWKS unavailable at startup — starting with an EMPTY key cache; every \
                     token validation will fail until a background refresh succeeds"
                );
                return;
            }

            tracing::warn!(
                attempt,
                error = %e,
                backoff_ms = backoff.as_millis() as u64,
                "JWKS fetch failed at startup; retrying"
            );
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(self.policy.max_backoff);
        }
    }

    /// Fetch JWKS from the endpoint and update the cache.
    async fn refresh(&self) -> Result<(), String> {
        let response = self
            .client
            .get(&self.jwks_uri)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("JWKS fetch failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "JWKS endpoint returned status {}",
                response.status()
            ));
        }

        let jwks: JwksResponse = response
            .json()
            .await
            .map_err(|e| format!("JWKS parse failed: {e}"))?;

        let mut new_keys = HashMap::new();

        for key in jwks.keys {
            if key.kty != "RSA" {
                continue;
            }
            if let Some(ref u) = key.key_use
                && u != "sig"
            {
                continue;
            }

            let Some(ref kid) = key.kid else {
                continue;
            };
            let Some(ref n) = key.n else { continue };
            let Some(ref e) = key.e else { continue };

            match DecodingKey::from_rsa_components(n, e) {
                Ok(decoding_key) => {
                    new_keys.insert(kid.clone(), decoding_key);
                }
                Err(err) => {
                    tracing::warn!(kid = %kid, error = %err, "Skipping invalid RSA key from JWKS");
                }
            }
        }

        tracing::info!(key_count = new_keys.len(), "JWKS cache refreshed");

        let mut cache = self.keys.write().await;
        *cache = new_keys;

        Ok(())
    }

    /// `refresh`, stamping the attempt time whatever the outcome, so the request-path
    /// cooldown measures from the last attempt rather than the last success.
    async fn refresh_and_record(&self) -> Result<(), String> {
        let result = self.refresh().await;
        *self.last_attempt.write().await = Some(Instant::now());
        result
    }

    async fn cooldown_elapsed(&self) -> bool {
        match *self.last_attempt.read().await {
            None => true,
            Some(at) => at.elapsed() >= self.policy.miss_cooldown,
        }
    }

    /// Get a decoding key by `kid`, refreshing once if it is not cached (key rotation).
    ///
    /// This runs on EVERY authenticated request, so the miss path is both coalesced and
    /// rate-limited. An empty or stale cache would otherwise turn each in-flight request into
    /// its own upstream fetch — aimed at the endpoint that is, by definition, already
    /// failing. Concurrent misses wait on one shared fetch and all see its result; once an
    /// attempt has just been made, further misses return immediately rather than queueing.
    pub async fn get_key(&self, kid: &str) -> Option<DecodingKey> {
        // First check the cache
        {
            let keys = self.keys.read().await;
            if let Some(key) = keys.get(kid) {
                return Some(key.clone());
            }
        }

        tracing::info!(kid = %kid, "JWKS cache miss, refreshing");
        self.refresh_if_due().await;

        // Check again after refresh
        let keys = self.keys.read().await;
        keys.get(kid).cloned()
    }

    /// Refresh unless one just happened. At most one fetch is in flight; callers that arrive
    /// during it wait for its result instead of starting their own.
    async fn refresh_if_due(&self) {
        // Checked before taking the gate so that, while the endpoint is down, misses bail out
        // here instead of queueing behind a fetch that is going to time out anyway.
        if !self.cooldown_elapsed().await {
            return;
        }

        let _gate = self.refreshing.lock().await;

        // The task that held the gate may have refreshed while we waited for it.
        if !self.cooldown_elapsed().await {
            return;
        }

        if let Err(e) = self.refresh_and_record().await {
            tracing::error!(error = %e, "JWKS refresh on cache miss failed");
        }
    }

    /// Spawn the background refresher. Armed by [`JwksCache::with_policy`] before the first
    /// fetch, so it is running even when that fetch fails.
    ///
    /// The interval adapts: while the cache is empty it retries on `empty_retry_interval` and
    /// re-logs the condition every time, because an empty cache means the process is
    /// rejecting every token and a single line at boot is exactly what gets missed. Once keys
    /// are loaded it settles to `refresh_interval`.
    fn spawn_refresh_task(cache: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                let ready = cache.is_ready().await;
                let wait = if ready {
                    cache.policy.refresh_interval
                } else {
                    cache.policy.empty_retry_interval
                };
                tokio::time::sleep(wait).await;

                if !cache.is_ready().await {
                    tracing::error!(
                        jwks_uri = %cache.jwks_uri,
                        "JWKS cache is still EMPTY — all token validation is failing; retrying"
                    );
                }

                // Shares the request path's gate so a background refresh and a cache-miss
                // refresh cannot both be in flight against the same endpoint.
                let _gate = cache.refreshing.lock().await;
                if let Err(e) = cache.refresh_and_record().await {
                    tracing::error!(error = %e, "Background JWKS refresh failed");
                }
            }
        });
    }
}
