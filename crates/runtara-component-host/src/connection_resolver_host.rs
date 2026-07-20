// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host implementation of the universal connection resolver interface.
//!
//! The workflow supplies only a resolved opaque connection id. This host calls
//! the trusted internal connection service with the run's tenant id and returns
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

fn refresh_requested(request: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(request)
        .ok()
        .and_then(|value| value.get("refresh").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Fully-qualified component import name of the resolver interface.
pub const CONNECTION_RESOLVER_INTERFACE_NAME: &str =
    runtara_workflow_wit::CONNECTION_RESOLVER_INTERFACE_NAME;

/// Native resolver surface used by the component linker.
#[async_trait::async_trait]
pub trait ConnectionResolverHost: Send + Sync {
    async fn describe(&self, connection_id: String) -> Result<Vec<u8>, String>;
    async fn resolve_resource(
        &self,
        connection_id: String,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, String>;
}

/// Per-run HTTP resolver. Its caches intentionally do not outlive a run, so a
/// later execution observes connection edits while repeated steps in one run
/// avoid redundant metadata and provider discovery calls.
pub(crate) struct HttpConnectionResolverHost {
    client: reqwest::Client,
    base_url: String,
    tenant_id: String,
    descriptions: Mutex<HashMap<String, Vec<u8>>>,
    resources: Mutex<ResourceCache>,
}

impl HttpConnectionResolverHost {
    fn from_env(env: &HashMap<String, String>) -> Result<Self, String> {
        let base_url = env
            .get("CONNECTION_SERVICE_URL")
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .ok_or_else(|| "CONNECTION_SERVICE_URL is not configured for this run".to_string())?;
        let tenant_id = env
            .get("RUNTARA_TENANT_ID")
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .ok_or_else(|| "RUNTARA_TENANT_ID is not configured for this run".to_string())?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| format!("build connection resolver HTTP client: {error}"))?;

        Ok(Self {
            client,
            base_url,
            tenant_id,
            descriptions: Mutex::new(HashMap::new()),
            resources: Mutex::new(HashMap::new()),
        })
    }

    fn endpoint(&self, connection_id: &str, operation: &str) -> Result<reqwest::Url, String> {
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|error| format!("invalid CONNECTION_SERVICE_URL: {error}"))?;
        let mut segments = url.path_segments_mut().map_err(|_| {
            "CONNECTION_SERVICE_URL cannot be used as a hierarchical URL".to_string()
        })?;
        segments.pop_if_empty();
        segments.push(&self.tenant_id);
        segments.push(connection_id);
        segments.push(operation);
        drop(segments);
        Ok(url)
    }

    async fn response_bytes(
        response: reqwest::Response,
        operation: &str,
    ) -> Result<Vec<u8>, String> {
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("read connection {operation} response: {error}"))?;
        if status.is_success() {
            Ok(bytes.to_vec())
        } else {
            let detail = String::from_utf8_lossy(&bytes);
            Err(format!(
                "connection {operation} failed with HTTP {status}: {detail}"
            ))
        }
    }
}

#[async_trait::async_trait]
impl ConnectionResolverHost for HttpConnectionResolverHost {
    async fn describe(&self, connection_id: String) -> Result<Vec<u8>, String> {
        if let Some(cached) = self.descriptions.lock().await.get(&connection_id).cloned() {
            return Ok(cached);
        }

        let response = self
            .client
            .get(self.endpoint(&connection_id, "metadata")?)
            .send()
            .await
            .map_err(|error| format!("resolve connection metadata: {error}"))?;
        let bytes = Self::response_bytes(response, "metadata").await?;
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
        let refresh = refresh_requested(&request);
        let cache_key = (connection_id.clone(), request.clone());
        if !refresh && let Some(cached) = self.resources.lock().await.get(&cache_key).cloned() {
            return Ok(cached);
        }

        let response = self
            .client
            .post(self.endpoint(&connection_id, "resources")?)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request)
            .send()
            .await
            .map_err(|error| format!("resolve connection resource: {error}"))?;
        let bytes = Self::response_bytes(response, "resource resolution").await?;
        if !refresh {
            self.resources.lock().await.insert(cache_key, bytes.clone());
        }
        Ok(bytes)
    }
}

pub(crate) fn resolver_from_env(
    env: &HashMap<String, String>,
) -> Result<Arc<dyn ConnectionResolverHost>, String> {
    HttpConnectionResolverHost::from_env(env)
        .map(|host| Arc::new(host) as Arc<dyn ConnectionResolverHost>)
}

fn require_host(
    store: &mut StoreContextMut<'_, WorkflowState>,
) -> wasmtime::Result<Arc<dyn ConnectionResolverHost>> {
    store.data().connection_resolver_host().cloned().ok_or_else(|| {
        wasmtime::format_err!(
            "workflow imports {CONNECTION_RESOLVER_INTERFACE_NAME} but connection resolution is \
             unavailable: {}",
            store
                .data()
                .connection_resolver_error()
                .unwrap_or("resolver was not configured")
        )
    })
}

/// Bind the universal connection resolver to the run-scoped HTTP host.
pub fn add_connection_resolver_to_linker(linker: &mut Linker<WorkflowState>) -> anyhow::Result<()> {
    let mut inst = linker.instance(CONNECTION_RESOLVER_INTERFACE_NAME)?;
    inst.func_wrap_async(
        "describe",
        |mut store: StoreContextMut<'_, WorkflowState>, (connection_id,): (String,)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.describe(connection_id).await,)) })
        },
    )?;
    inst.func_wrap_async(
        "resolve-resource",
        |mut store: StoreContextMut<'_, WorkflowState>,
         (connection_id, request): (String, Vec<u8>)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.resolve_resource(connection_id, request).await,)) })
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_refresh_flag_bypasses_run_cache() {
        assert!(refresh_requested(
            br#"{"resource":"llm.models","refresh":true}"#
        ));
        assert!(!refresh_requested(br#"{"resource":"llm.models"}"#));
        assert!(!refresh_requested(b"not-json"));
    }
}
