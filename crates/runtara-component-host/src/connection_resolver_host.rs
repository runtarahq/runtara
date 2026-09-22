// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host implementation of the universal connection resolver interface.
//!
//! The workflow supplies only a resolved opaque connection id. This host calls
//! the native connection service with the authoritative tenant id and returns
//! safe JSON metadata. Raw parameters and credentials never enter workflow
//! memory. Results are cached for the lifetime of one workflow run.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use wasmtime::StoreContextMut;
use wasmtime::component::Linker;

use crate::workflow::WorkflowState;

type ResourceCacheKey = (String, Vec<u8>);
type ResourceCache = HashMap<ResourceCacheKey, Vec<u8>>;

/// Fully-qualified component import name of the resolver interface.
pub const CONNECTION_RESOLVER_INTERFACE_NAME: &str =
    runtara_workflow_wit::CONNECTION_RESOLVER_INTERFACE_NAME;

/// Process-wide native service injected by the embedding application.
/// The linker supplies host-owned tenant authority and per-run caches.
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
pub(crate) struct RunConnectionResolver {
    backend: Arc<dyn ConnectionResolverHost>,
    tenant: String,
    descriptions: Mutex<HashMap<String, Vec<u8>>>,
    resources: Mutex<ResourceCache>,
}

impl RunConnectionResolver {
    async fn describe(&self, connection_id: String) -> Result<Vec<u8>, String> {
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

    async fn resolve_resource(
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

pub(crate) fn resolver_for_run(
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

pub(crate) trait ConnectionResolverContext {
    fn resolver(&self) -> Result<Arc<RunConnectionResolver>, String>;
}

impl ConnectionResolverContext for WorkflowState {
    fn resolver(&self) -> Result<Arc<RunConnectionResolver>, String> {
        self.connection_resolver_host().cloned().ok_or_else(|| {
            self.connection_resolver_error()
                .unwrap_or("resolver was not configured")
                .to_owned()
        })
    }
}

impl ConnectionResolverContext for crate::host_state::HostState {
    fn resolver(&self) -> Result<Arc<RunConnectionResolver>, String> {
        if self.restricted {
            return Err("connection resolution is denied in trusted instances".into());
        }
        self.connection_resolver.clone()
    }
}

fn require_host<T: ConnectionResolverContext>(
    state: &T,
) -> wasmtime::Result<Arc<RunConnectionResolver>> {
    state
        .resolver()
        .map_err(|error| wasmtime::format_err!("connection resolution is unavailable: {error}"))
}

/// Bind both supported resolver ABIs to the native run-scoped service.
pub(crate) fn add_connection_resolver_to_linker<T: ConnectionResolverContext + Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    let mut inst = linker.instance(CONNECTION_RESOLVER_INTERFACE_NAME)?;
    inst.func_wrap_concurrent("describe", |accessor, (connection_id,): (String,)| {
        let host = accessor.with(|mut access| require_host(access.get()));
        Box::pin(async move { Ok((host?.describe(connection_id).await,)) })
    })?;
    inst.func_wrap_concurrent(
        "resolve-resource",
        |accessor, (connection_id, request): (String, Vec<u8>)| {
            let host = accessor.with(|mut access| require_host(access.get()));
            Box::pin(async move { Ok((host?.resolve_resource(connection_id, request).await,)) })
        },
    )?;
    // Already-built artifacts keep the synchronous 0.1 contract. Registration
    // depends only on the artifact's ABI version, never a workflow feature flag.
    let mut legacy =
        linker.instance(runtara_workflow_wit::LEGACY_CONNECTION_RESOLVER_INTERFACE_NAME)?;
    legacy.func_wrap_async(
        "describe",
        |store: StoreContextMut<'_, T>, (connection_id,): (String,)| {
            let host = require_host(store.data());
            Box::new(async move { Ok((host?.describe(connection_id).await,)) })
        },
    )?;
    legacy.func_wrap_async(
        "resolve-resource",
        |store: StoreContextMut<'_, T>, (connection_id, request): (String, Vec<u8>)| {
            let host = require_host(store.data());
            Box::new(async move { Ok((host?.resolve_resource(connection_id, request).await,)) })
        },
    )?;
    Ok(())
}
