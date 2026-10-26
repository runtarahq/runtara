// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Native connection resolver behind the workflow API connection routes.
//!
//! The workflow supplies only a resolved opaque connection id. This host calls
//! the native connection service with the authoritative tenant id and returns
//! safe JSON metadata. Raw parameters and credentials never enter workflow
//! memory. Results are cached for the lifetime of one workflow run.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

type ResourceCacheKey = (String, Vec<u8>);
type ResourceCache = HashMap<ResourceCacheKey, Vec<u8>>;

/// Process-wide native service injected by the embedding application.
/// The workflow API supplies host-owned tenant authority and per-run caches.
/// Results must contain safe metadata/resources, never raw credentials.
#[async_trait::async_trait]
pub trait ConnectionResolverHost: Send + Sync {
    async fn describe(&self, tenant: &str, connection_id: String) -> Result<Vec<u8>, String>;
    async fn resolve_resource(
        &self,
        tenant: &str,
        connection_id: String,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, String>;
}

/// Cache and tenant authority belong to one invocation, never to guest env vars.
pub struct RunConnectionResolver {
    backend: Arc<dyn ConnectionResolverHost>,
    tenant: String,
    descriptions: Mutex<HashMap<String, Vec<u8>>>,
    resources: Mutex<ResourceCache>,
}

impl RunConnectionResolver {
    /// The safe descriptor of `connection_id`, cached for the run.
    pub async fn describe(&self, connection_id: String) -> Result<Vec<u8>, String> {
        if let Some(cached) = self.descriptions.lock().await.get(&connection_id).cloned() {
            return Ok(cached);
        }
        let bytes = tokio::time::timeout(
            Duration::from_secs(30),
            self.backend.describe(&self.tenant, connection_id.clone()),
        )
        .await
        .map_err(|_| "connection metadata resolution timed out".to_string())??;
        self.descriptions
            .lock()
            .await
            .insert(connection_id, bytes.clone());
        Ok(bytes)
    }

    /// Resolve a resource request against `connection_id`, cached for the run.
    pub async fn resolve_resource(
        &self,
        connection_id: String,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let key = (connection_id.clone(), request.clone());
        if let Some(cached) = self.resources.lock().await.get(&key).cloned() {
            return Ok(cached);
        }
        let bytes = tokio::time::timeout(
            Duration::from_secs(30),
            self.backend
                .resolve_resource(&self.tenant, connection_id, request),
        )
        .await
        .map_err(|_| "connection resource resolution timed out".to_string())??;
        self.resources.lock().await.insert(key, bytes.clone());
        Ok(bytes)
    }
}

/// Connection resolution for a run of `tenant`, both host-supplied.
pub fn resolver_for_run(
    backend: Option<&Arc<dyn ConnectionResolverHost>>,
    tenant: Option<&str>,
) -> Result<Arc<RunConnectionResolver>, String> {
    let backend = backend.ok_or("native connection resolver is not configured")?;
    let tenant = tenant
        .filter(|tenant| !tenant.trim().is_empty())
        .ok_or("authoritative tenant is not configured")?;
    Ok(Arc::new(RunConnectionResolver {
        backend: Arc::clone(backend),
        tenant: tenant.to_owned(),
        descriptions: Mutex::new(HashMap::new()),
        resources: Mutex::new(HashMap::new()),
    }))
}
