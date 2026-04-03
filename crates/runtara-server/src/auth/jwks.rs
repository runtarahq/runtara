use jsonwebtoken::DecodingKey;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// JWKS cache that stores decoding keys indexed by `kid`.
/// Supports background refresh and on-demand refresh on cache miss.
pub struct JwksCache {
    keys: RwLock<HashMap<String, DecodingKey>>,
    jwks_uri: String,
    client: reqwest::Client,
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
    /// Create a new JWKS cache and perform the initial fetch.
    /// Panics if the JWKS endpoint is unreachable on startup (fail-fast).
    pub async fn new(jwks_uri: String) -> Arc<Self> {
        let client = reqwest::Client::new();
        let cache = Arc::new(Self {
            keys: RwLock::new(HashMap::new()),
            jwks_uri,
            client,
        });

        // Initial fetch — panic if unreachable (fail-fast on startup)
        cache
            .refresh()
            .await
            .expect("Failed to fetch JWKS on startup — check OAUTH2_JWKS_URI");

        cache
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

    /// Get a decoding key by `kid`. If not found, triggers a single refresh attempt.
    pub async fn get_key(&self, kid: &str) -> Option<DecodingKey> {
        // First check the cache
        {
            let keys = self.keys.read().await;
            if let Some(key) = keys.get(kid) {
                return Some(key.clone());
            }
        }

        // Cache miss — try refreshing once (handles key rotation)
        tracing::info!(kid = %kid, "JWKS cache miss, refreshing");
        if let Err(e) = self.refresh().await {
            tracing::error!(error = %e, "JWKS refresh on cache miss failed");
            return None;
        }

        // Check again after refresh
        let keys = self.keys.read().await;
        keys.get(kid).cloned()
    }

    /// Spawn a background task that refreshes the JWKS cache periodically.
    pub fn spawn_refresh_task(cache: Arc<Self>, interval_secs: u64) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            // Skip the first tick (already fetched on startup)
            interval.tick().await;

            loop {
                interval.tick().await;
                if let Err(e) = cache.refresh().await {
                    tracing::error!(error = %e, "Background JWKS refresh failed");
                }
            }
        });
    }
}
