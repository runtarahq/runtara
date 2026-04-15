//! Object Model Repository
//!
//! Manages ObjectStore instances per tenant, providing connection pooling
//! and caching for the Object Model API layer.

use runtara_object_store::{ObjectStore, StoreConfig};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// ObjectStore Manager
// ============================================================================

/// Manages ObjectStore instances per tenant
///
/// In the DB-per-tenant architecture, each tenant has their own database.
/// This manager caches ObjectStore instances to avoid reconnecting on every request.
pub struct ObjectStoreManager {
    /// Cached ObjectStore instances by tenant_id
    stores: RwLock<HashMap<String, Arc<ObjectStore>>>,
    /// Database URL template with {tenant_id} placeholder, or a fixed URL
    database_url: String,
}

impl ObjectStoreManager {
    /// Create a new ObjectStoreManager
    ///
    /// The database_url can either be:
    /// - A fixed URL (all tenants share the same database)
    /// - A template with `{tenant_id}` placeholder for DB-per-tenant
    pub fn new(database_url: String) -> Self {
        Self {
            stores: RwLock::new(HashMap::new()),
            database_url,
        }
    }

    /// Create a manager from an existing pool (single-database mode)
    ///
    /// All tenants will share the same ObjectStore instance backed by this pool.
    pub async fn from_pool(pool: PgPool) -> Result<Self, runtara_object_store::ObjectStoreError> {
        // Use builder with empty URL since we're using an existing pool
        let config = StoreConfig::builder("")
            .soft_delete(crate::config::object_model_soft_delete())
            .build();
        let store = ObjectStore::from_pool(pool, config).await?;

        let manager = Self {
            stores: RwLock::new(HashMap::new()),
            database_url: String::new(), // Not used in pool mode
        };

        // Store as the "default" tenant
        manager
            .stores
            .write()
            .await
            .insert("__default__".to_string(), Arc::new(store));

        Ok(manager)
    }

    /// Get or create an ObjectStore for the given tenant
    ///
    /// In single-database mode (created via from_pool), always returns the shared store.
    /// In multi-database mode, creates a new store per tenant if not cached.
    pub async fn get_store(
        &self,
        tenant_id: &str,
    ) -> Result<Arc<ObjectStore>, runtara_object_store::ObjectStoreError> {
        // Check if we're in single-database mode
        {
            let stores = self.stores.read().await;
            if let Some(store) = stores.get("__default__") {
                return Ok(Arc::clone(store));
            }
            if let Some(store) = stores.get(tenant_id) {
                return Ok(Arc::clone(store));
            }
        }

        // Create new store for this tenant
        let database_url = self.database_url.replace("{tenant_id}", tenant_id);
        let config = StoreConfig::builder(&database_url)
            .soft_delete(crate::config::object_model_soft_delete())
            .build();
        let store = ObjectStore::new(config).await?;
        let store = Arc::new(store);

        // Cache and return
        self.stores
            .write()
            .await
            .insert(tenant_id.to_string(), Arc::clone(&store));

        Ok(store)
    }

    /// Remove a tenant's cached store (e.g., on tenant deletion)
    #[allow(dead_code)]
    pub async fn remove_store(&self, tenant_id: &str) {
        self.stores.write().await.remove(tenant_id);
    }

    /// Get or create an ObjectStore for a specific database URL
    ///
    /// This is the core method for connection-based access.
    /// Caches by URL and creates ObjectStore if not cached.
    /// Used when a connection_id is provided to use a specific database.
    pub async fn get_store_by_url(
        &self,
        database_url: &str,
    ) -> Result<Arc<ObjectStore>, runtara_object_store::ObjectStoreError> {
        // Use URL as cache key (prefixed to avoid collision with tenant IDs)
        let cache_key = format!("url:{}", database_url);

        // Check cache first
        {
            let stores = self.stores.read().await;
            if let Some(store) = stores.get(&cache_key) {
                return Ok(Arc::clone(store));
            }
        }

        // Create new store for this database URL
        let config = StoreConfig::builder(database_url)
            .soft_delete(crate::config::object_model_soft_delete())
            .build();
        let store = ObjectStore::new(config).await?;
        let store = Arc::new(store);

        // Cache and return
        self.stores
            .write()
            .await
            .insert(cache_key, Arc::clone(&store));

        Ok(store)
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
