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
use std::time::{Duration, Instant};

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
    /// Connection-based store of one tenant, keyed by a hash of the resolved
    /// database URL. Hashing keeps credentials out of the key and makes
    /// credential rotation transparent: a rotated URL hashes to a new key, so
    /// the next request builds a fresh pool and the stale one idles out.
    Url { tenant: String, url: u64 },
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

/// How long a failed pool build is remembered so repeated requests to a
/// dead/unreachable database fast-fail instead of re-attempting a (potentially
/// slow, cross-cloud) connect on every call. After this window one request is
/// allowed to retry, so recovery is automatic. Kept short on purpose.
const DEFAULT_STORE_FAILURE_TTL: Duration = Duration::from_secs(5);
/// Bound on the number of remembered build failures.
const STORE_FAILURE_CACHE_CAPACITY: u64 = 1024;

fn build_failure_cache() -> Cache<StoreKey, String> {
    Cache::builder()
        .max_capacity(STORE_FAILURE_CACHE_CAPACITY)
        .time_to_live(DEFAULT_STORE_FAILURE_TTL)
        .build()
}

/// Aggregate connection-pool statistics across the object-model stores
/// (the default store plus every cached per-connection store).
#[derive(Debug, Default, Clone, Copy)]
pub struct ObjectModelPoolStats {
    /// Number of distinct store pools (default + cached).
    pub pools: u64,
    /// Total connections currently established across all pools.
    pub connections: u64,
    /// Idle (pooled, ready-to-hand-out) connections.
    pub idle: u64,
    /// In-use connections (`connections - idle`).
    pub active: u64,
    /// Approximate aggregate capacity (`pools * max_connections`).
    pub capacity: u64,
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
    /// Short-lived negative cache of failed builds (keyed like `stores`) so a
    /// dead database fast-fails instead of cold-connecting on every request.
    failures: Cache<StoreKey, String>,
    /// The most connection pools one tenant may hold open at once.
    tenant_pool_max: usize,
    /// Pools being built, by tenant: counted with the held ones, under this
    /// lock, so concurrent first requests cannot overshoot the limit.
    building: std::sync::Mutex<std::collections::HashMap<String, usize>>,
}

/// Why a store build failed: the tenant was at its pool limit, or the
/// database could not be reached.
#[derive(Debug)]
enum BuildError {
    Capacity(String),
    Connect(String),
}

/// A tenant's slot for a pool being built, released when the build ends.
struct BuildSlot<'a> {
    building: &'a std::sync::Mutex<std::collections::HashMap<String, usize>>,
    tenant: String,
}

impl Drop for BuildSlot<'_> {
    fn drop(&mut self) {
        let mut building = self.building.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = building.get_mut(&self.tenant) {
            *count -= 1;
            if *count == 0 {
                building.remove(&self.tenant);
            }
        }
    }
}

/// Why a store is refused when the server has no default database.
pub const NO_DEFAULT_DATABASE: &str = "this server has no default object model database \
     (OBJECT_MODEL_DATABASE_URL is not set): add a Postgres connection and use its connectionId";

/// The default limit on pools one tenant's connections may hold open.
const DEFAULT_TENANT_POOL_MAX: usize = 16;

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
            failures: build_failure_cache(),
            tenant_pool_max: DEFAULT_TENANT_POOL_MAX,
            building: Default::default(),
        }
    }

    /// A manager with no default database: only connection-based stores.
    pub fn without_default_database() -> Self {
        Self::new(String::new())
    }

    /// Cap the pools one tenant's connections may hold open at once.
    pub fn with_tenant_pool_max(mut self, tenant_pool_max: usize) -> Self {
        self.tenant_pool_max = tenant_pool_max.max(1);
        self
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
            failures: build_failure_cache(),
            tenant_pool_max: DEFAULT_TENANT_POOL_MAX,
            building: Default::default(),
        })
    }

    /// Get (or build, single-flight) the store for a resolved URL + cache key.
    async fn get_or_build(
        &self,
        key: StoreKey,
        database_url: String,
    ) -> Result<Arc<ObjectStore>, ObjectStoreError> {
        // Fast-fail if a recent build for this store failed (negative cache), so a
        // dead/unreachable database doesn't trigger a cold connect on every request.
        // The entry expires after DEFAULT_STORE_FAILURE_TTL, so recovery is automatic.
        if let Some(cached_error) = self.failures.get(&key).await {
            tracing::debug!(key = ?key, "object-model store in failure cooldown; fast-failing");
            return Err(ObjectStoreError::Connection(cached_error));
        }
        let pool_config = self.pool_config.clone();
        // try_get_with is single-flight: this closure runs only on a cache miss —
        // exactly when we pay a (potentially cross-cloud) cold connect. Only the
        // build holds a tenant pool slot: concurrent callers for the same key
        // wait for it and share its result.
        let log_key = key.clone();
        let result = self
            .stores
            .try_get_with(key.clone(), async {
                let _slot = match &key {
                    StoreKey::Url { tenant, .. } => {
                        Some(self.reserve_pool(tenant).map_err(BuildError::Capacity)?)
                    }
                    StoreKey::Tenant(_) => None,
                };
                let start = Instant::now();
                let config = StoreConfig::builder(&database_url)
                    .soft_delete(crate::config::object_model_soft_delete())
                    .bulk_request_limit(crate::config::object_model_bulk_request_limit())
                    .pool(pool_config)
                    .build();
                let result = ObjectStore::connect(config).await.map(Arc::new);
                match &result {
                    Ok(_) => tracing::info!(
                        key = ?log_key,
                        elapsed_ms = start.elapsed().as_millis() as u64,
                        "built object-model store pool (cold connect)"
                    ),
                    Err(_) => tracing::warn!(
                        key = ?log_key,
                        "failed to build object-model store pool"
                    ),
                }
                result.map_err(|error| BuildError::Connect(error.to_string()))
            })
            .await;

        match result {
            Ok(store) => Ok(store),
            // At the tenant's limit: not a failure of this database, so it is
            // not remembered; the next request tries again.
            Err(error) => match error.as_ref() {
                BuildError::Capacity(message) => Err(ObjectStoreError::Connection(message.clone())),
                BuildError::Connect(error) => {
                    let message = format!("failed to initialize object store: {error}");
                    // Remember the failure briefly so repeated requests fast-fail
                    // instead of re-attempting a cold connect to a dead database.
                    self.failures.insert(key, message.clone()).await;
                    Err(ObjectStoreError::Connection(message))
                }
            },
        }
    }

    /// Reserve one of `tenant`'s pool slots for a build, or refuse (with why)
    /// when the pools it holds and is building reach the limit.
    fn reserve_pool(&self, tenant: &str) -> Result<BuildSlot<'_>, String> {
        let mut building = self.building.lock().unwrap_or_else(|e| e.into_inner());
        let held = self
            .stores
            .iter()
            .filter(
                |(held, _)| matches!(held.as_ref(), StoreKey::Url { tenant: t, .. } if t == tenant),
            )
            .count();
        let pending = building.get(tenant).copied().unwrap_or(0);
        if held + pending >= self.tenant_pool_max {
            return Err(format!(
                "this tenant already holds {} database connection pools, the limit \
                 (OBJECT_MODEL_TENANT_POOL_MAX); unused ones close after they idle",
                held + pending
            ));
        }
        *building.entry(tenant.to_string()).or_default() += 1;
        Ok(BuildSlot {
            building: &self.building,
            tenant: tenant.to_string(),
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
        if self.database_url.is_empty() {
            return Err(ObjectStoreError::Validation(
                NO_DEFAULT_DATABASE.to_string(),
            ));
        }

        let database_url = self.database_url.replace("{tenant_id}", tenant_id);
        let store = self
            .get_or_build(StoreKey::Tenant(tenant_id.to_string()), database_url)
            .await?;
        store.initialize_object_model().await?;
        Ok(store)
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
        tenant_id: &str,
        database_url: &str,
    ) -> Result<Arc<ObjectStore>, ObjectStoreError> {
        let store = self.get_database_by_url(tenant_id, database_url).await?;
        store.initialize_object_model().await?;
        Ok(store)
    }

    /// Share the native pool without Object Model initialization or DDL.
    pub async fn get_database_by_url(
        &self,
        tenant_id: &str,
        database_url: &str,
    ) -> Result<Arc<ObjectStore>, ObjectStoreError> {
        self.get_or_build(
            StoreKey::Url {
                tenant: tenant_id.to_string(),
                url: hash_url(database_url),
            },
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

    /// Snapshot aggregate connection-pool stats across the default + cached
    /// stores. Cheap; intended for a periodic monitor.
    pub fn pool_stats(&self) -> ObjectModelPoolStats {
        let mut stats = ObjectModelPoolStats::default();
        if let Some(store) = &self.default_store {
            let pool = store.pool();
            stats.pools += 1;
            stats.connections += pool.size() as u64;
            stats.idle += pool.num_idle() as u64;
        }
        for (_key, store) in self.stores.iter() {
            let pool = store.pool();
            stats.pools += 1;
            stats.connections += pool.size() as u64;
            stats.idle += pool.num_idle() as u64;
        }
        stats.active = stats.connections.saturating_sub(stats.idle);
        stats.capacity = stats.pools * self.pool_config.max_connections as u64;
        stats
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
        let url = StoreKey::Url {
            tenant: "42".to_string(),
            url: hash_url("postgres://localhost/42"),
        };
        assert_ne!(tenant, url);
    }

    #[tokio::test]
    async fn negative_cache_fast_fails_a_known_bad_store() {
        let manager = ObjectStoreManager::new(String::new());
        let url = "postgres://user:pass@127.0.0.1:1/missing";
        let key = StoreKey::Url {
            tenant: "t".to_string(),
            url: hash_url(url),
        };
        // Simulate a recent failed build for this store.
        manager
            .failures
            .insert(key.clone(), "connection refused".to_string())
            .await;
        manager.failures.run_pending_tasks().await;
        // The request fast-fails from the negative cache — no build attempt, so it
        // does not depend on global config or a reachable database.
        match manager.get_store_by_url("t", url).await {
            Ok(_) => panic!("expected a fast-fail from the negative cache"),
            Err(e) => assert!(
                e.to_string().contains("connection refused"),
                "should return the negative-cached failure, got: {e}"
            ),
        }
    }

    #[tokio::test]
    async fn without_a_default_database_the_default_store_says_what_to_do() {
        let manager = ObjectStoreManager::without_default_database();
        let error = manager
            .get_store("t")
            .await
            .err()
            .expect("no default store");
        assert!(
            error.to_string().contains("add a Postgres connection"),
            "{error}"
        );
    }

    /// A database that accepts connections and never answers, so a build
    /// stays in flight until the pool's acquire timeout; answers its URL for
    /// `db`.
    async fn silent_database() -> (tokio::net::TcpListener, impl Fn(&str) -> String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, move |db: &str| {
            format!("postgres://u:p@127.0.0.1:{port}/{db}?sslmode=disable")
        })
    }

    fn quick_failing_manager() -> ObjectStoreManager {
        ObjectStoreManager::without_default_database()
            .with_tenant_pool_max(1)
            .with_pool_config(PoolConfig {
                acquire_timeout: Duration::from_millis(500),
                ..PoolConfig::default()
            })
    }

    /// SYN-787: concurrent first requests for one pool share its build and
    /// its single slot, so none of them is refused for capacity.
    #[tokio::test]
    async fn concurrent_requests_for_one_pool_share_one_slot() {
        let (listener, url) = silent_database().await;
        let held = tokio::spawn(async move {
            let mut sockets = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                sockets.push(socket);
            }
        });
        let manager = quick_failing_manager();
        let url = url("one");
        let (first, second, third) = tokio::join!(
            manager.get_database_by_url("t", &url),
            manager.get_database_by_url("t", &url),
            manager.get_database_by_url("t", &url),
        );
        for result in [first, second, third] {
            let error = result.err().expect("the silent database never answers");
            assert!(
                !error.to_string().contains("OBJECT_MODEL_TENANT_POOL_MAX"),
                "{error}"
            );
        }
        held.abort();
    }

    /// Distinct pools still respect the tenant's limit while they build, and
    /// a capacity refusal is not remembered as a failed database.
    #[tokio::test]
    async fn distinct_pools_respect_the_limit_and_a_refusal_is_not_remembered() {
        let (listener, url) = silent_database().await;
        let held = tokio::spawn(async move {
            let mut sockets = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                sockets.push(socket);
            }
        });
        let manager = quick_failing_manager();
        let (a, b) = (url("a"), url("b"));
        let first = manager.get_database_by_url("t", &a);
        let second = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            manager.get_database_by_url("t", &b).await
        };
        let (first, second) = tokio::join!(first, second);
        assert!(
            !first
                .err()
                .unwrap()
                .to_string()
                .contains("OBJECT_MODEL_TENANT_POOL_MAX")
        );
        let refused = second.err().expect("the second pool is over the limit");
        assert!(
            refused.to_string().contains("OBJECT_MODEL_TENANT_POOL_MAX"),
            "{refused}"
        );
        // The slot is free again: `b` is built (and fails to connect), rather
        // than fast-failing with the remembered refusal.
        let retried = manager.get_database_by_url("t", &b).await.err().unwrap();
        assert!(
            !retried.to_string().contains("OBJECT_MODEL_TENANT_POOL_MAX"),
            "{retried}"
        );
        held.abort();
    }

    #[test]
    fn concurrent_builds_cannot_overshoot_the_tenant_pool_limit() {
        let manager = ObjectStoreManager::without_default_database().with_tenant_pool_max(2);
        let first = manager.reserve_pool("t").expect("first slot");
        let _second = manager.reserve_pool("t").expect("second slot");
        // Two builds in flight hold the limit before either pool exists.
        let refused = manager
            .reserve_pool("t")
            .err()
            .expect("a third slot is refused");
        assert!(
            refused.to_string().contains("OBJECT_MODEL_TENANT_POOL_MAX"),
            "{refused}"
        );
        assert!(
            manager.reserve_pool("other").is_ok(),
            "the limit is per tenant"
        );
        drop(first);
        assert!(
            manager.reserve_pool("t").is_ok(),
            "a finished build frees its slot"
        );
    }
}
