//! Object Model Repository
//!
//! Manages ObjectStore instances per tenant/connection, providing connection
//! pooling and caching for the Object Model API layer.
//!
//! The store cache is **bounded** (LRU cap) and **self-evicting** (idle TTL), and
//! concurrent first-hits for the same key build a single pool (single-flight via
//! [`Cache::try_get_with`]) instead of racing to build duplicates.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use runtara_object_store::{ObjectStore, ObjectStoreError, PoolConfig, StoreConfig};
use sqlx::PgPool;

/// Default maximum number of distinct object-model stores (pools) kept warm.
/// Bounds file descriptors and customer-side connection slots.
const DEFAULT_STORE_CACHE_CAPACITY: u64 = 256;
/// Default idle eviction: drop a store unused for this long, closing its pool.
const DEFAULT_STORE_CACHE_TTL: Duration = Duration::from_secs(900);

/// Cache key for a built store.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum StoreKey {
    /// Per-tenant store in DB-per-tenant template mode (`{tenant_id}` URL).
    Tenant(String),
    /// Connection-based store, keyed by a hash of the resolved database URL.
    /// Hashing keeps credentials out of the key and makes credential rotation
    /// transparent: a rotated URL hashes to a new key, so the next request builds
    /// a fresh pool and the stale one idles out.
    Url(u64),
}

fn hash_url(url: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    hasher.finish()
}

fn build_cache(capacity: u64, ttl: Duration) -> Cache<StoreKey, Arc<ObjectStore>> {
    Cache::builder()
        .max_capacity(capacity)
        .time_to_idle(ttl)
        .build()
}

// ============================================================================
// ObjectStore Manager
// ============================================================================

/// Manages ObjectStore instances per tenant/connection.
///
/// Built stores are cached so we don't reconnect on every request. In the
/// DB-per-tenant architecture each tenant (or connection) has its own database;
/// the cache is keyed accordingly and bounded so a multi-tenant server doesn't
/// hold an unbounded number of pools open.
pub struct ObjectStoreManager {
    /// Cache of built stores keyed by tenant or resolved-URL hash.
    stores: Cache<StoreKey, Arc<ObjectStore>>,
    /// Shared store for single-database (`from_pool`) mode; always returned by
    /// [`get_store`](Self::get_store) when set, and never evicted.
    default_store: Option<Arc<ObjectStore>>,
    /// Database URL template with `{tenant_id}` placeholder, or a fixed URL.
    database_url: String,
    /// Pool tuning applied to every store this manager builds.
    pool_config: PoolConfig,
}

impl ObjectStoreManager {
    /// Create a new ObjectStoreManager
    ///
    /// The database_url can either be:
    /// - A fixed URL (all tenants share the same database)
    /// - A template with `{tenant_id}` placeholder for DB-per-tenant
    pub fn new(database_url: String) -> Self {
        Self {
            stores: build_cache(DEFAULT_STORE_CACHE_CAPACITY, DEFAULT_STORE_CACHE_TTL),
            default_store: None,
            database_url,
            pool_config: PoolConfig::default(),
        }
    }

    /// Override the connection-pool tuning used for stores this manager builds.
    pub fn with_pool_config(mut self, pool_config: PoolConfig) -> Self {
        self.pool_config = pool_config;
        self
    }

    /// Override the store-cache bounds: the max number of distinct warm pools
    /// (LRU cap) and how long an unused store survives before eviction.
    ///
    /// Rebuilds the (empty) cache, so call right after construction.
    pub fn with_cache_config(mut self, capacity: u64, ttl: Duration) -> Self {
        self.stores = build_cache(capacity, ttl);
        self
    }

    /// Create a manager from an existing pool (single-database mode)
    ///
    /// All tenants will share the same ObjectStore instance backed by this pool.
    pub async fn from_pool(pool: PgPool) -> Result<Self, ObjectStoreError> {
        // Use builder with empty URL since we're using an existing pool
        let config = StoreConfig::builder("")
            .soft_delete(crate::config::object_model_soft_delete())
            .bulk_request_limit(crate::config::object_model_bulk_request_limit())
            .build();
        let store = ObjectStore::from_pool(pool, config).await?;

        Ok(Self {
            stores: build_cache(DEFAULT_STORE_CACHE_CAPACITY, DEFAULT_STORE_CACHE_TTL),
            default_store: Some(Arc::new(store)),
            database_url: String::new(), // Not used in pool mode
            pool_config: PoolConfig::default(),
        })
    }

    /// Get (or build, single-flight) the store for a resolved URL + cache key.
    async fn get_or_build(
        &self,
        key: StoreKey,
        database_url: String,
    ) -> Result<Arc<ObjectStore>, ObjectStoreError> {
        let pool_config = self.pool_config.clone();
        self.stores
            .try_get_with(key, async move {
                let config = StoreConfig::builder(&database_url)
                    .soft_delete(crate::config::object_model_soft_delete())
                    .bulk_request_limit(crate::config::object_model_bulk_request_limit())
                    .pool(pool_config)
                    .build();
                ObjectStore::new(config).await.map(Arc::new)
            })
            .await
            .map_err(|e: Arc<ObjectStoreError>| {
                ObjectStoreError::Connection(format!("failed to initialize object store: {e}"))
            })
    }

    /// Get or create an ObjectStore for the given tenant
    ///
    /// In single-database mode (created via from_pool), always returns the shared store.
    /// In multi-database mode, creates a new store per tenant if not cached.
    pub async fn get_store(&self, tenant_id: &str) -> Result<Arc<ObjectStore>, ObjectStoreError> {
        // Single-database mode: always return the shared store.
        if let Some(store) = &self.default_store {
            return Ok(Arc::clone(store));
        }

        let database_url = self.database_url.replace("{tenant_id}", tenant_id);
        self.get_or_build(StoreKey::Tenant(tenant_id.to_string()), database_url)
            .await
    }

    /// Remove a tenant's cached store (e.g., on tenant deletion)
    #[allow(dead_code)]
    pub async fn remove_store(&self, tenant_id: &str) {
        self.stores
            .invalidate(&StoreKey::Tenant(tenant_id.to_string()))
            .await;
    }

    /// Get or create an ObjectStore for a specific database URL
    ///
    /// This is the core method for connection-based access (a `connection_id`
    /// resolving to a customer/external database). Caches by a hash of the URL.
    pub async fn get_store_by_url(
        &self,
        database_url: &str,
    ) -> Result<Arc<ObjectStore>, ObjectStoreError> {
        self.get_or_build(
            StoreKey::Url(hash_url(database_url)),
            database_url.to_string(),
        )
        .await
    }

    /// Get the default store URL (for fallback when no connection specified)
    pub fn default_database_url(&self) -> Option<&str> {
        if self.database_url.is_empty() {
            None
        } else {
            Some(&self.database_url)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_url_is_stable_and_distinct() {
        let a = "postgres://u:p@host:5432/db_a";
        let b = "postgres://u:p@host:5432/db_b";
        assert_eq!(hash_url(a), hash_url(a), "same URL must hash identically");
        assert_ne!(hash_url(a), hash_url(b), "different URLs should differ");
    }

    #[test]
    fn store_keys_are_distinct_across_variants() {
        // A tenant id and a URL hash must never collide as the same key.
        let tenant = StoreKey::Tenant("42".to_string());
        let url = StoreKey::Url(hash_url("postgres://localhost/42"));
        assert_ne!(tenant, url);
    }
}
