// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The server's side of an executor (`runtara-executor`): runtara's
//! provider-neutral runner ([`ProviderRunner`]) with an executor as its
//! provider.
//!
//! With `RUNTARA_RUNNER=executor` no workflow runs in the server's process.
//! Each launch is placed with the executor, which fetches the launch's
//! workflow from the run channel's listener if it does not hold it; its
//! guest reaches the workflow API over the run channel this process serves,
//! each call carrying the credential minted for its launch; and the run ends
//! on the outcome it posts. The executor says when a run's process ended
//! without one, and a run whose executor cannot be reached is handed to
//! recovery.
//!
//! Both connections are mutual TLS between the server's and the executor's
//! certificates (`runtara-executor init-pki`): this process accepts on its
//! run channel only the executor's, and places runs only with it.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::Router;
use axum::extract::Path;
use axum::response::IntoResponse;
use axum::routing::get;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use runtara_component_host::api_launches::ApiLaunches;
use runtara_environment::runner::{
    LaunchOptions, LaunchWorkflows, Provider, ProviderEnd, ProviderRunner, RunChannel, Runner,
    RunnerFactory,
};
use runtara_executor::protocol::{
    ARTIFACTS_PATH, EXECUTOR_NAME, Placed, Refusal, RunState, SERVER_NAME, valid_digest,
    valid_tenant,
};
use runtara_executor::tls::{self, Identity};
use runtara_workflow_api::Binding;
use serde_json::{Value, json};
use sha2::Digest;

/// A guest's memory when its launch names none: what a local run gets
/// (`runtara-component-host`'s `DEFAULT_GUEST_MEMORY_MAX_BYTES`).
const DEFAULT_MEMORY_BYTES: u64 = 1024 * 1024 * 1024;
/// An executor unreachable this long has lost its runs: runtara recovers
/// them.
const LOST_AFTER: Duration = Duration::from_secs(30);
/// How long one call to the executor may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(40);

/// How the server reaches its executor, and the executor the server.
#[derive(Clone, Debug)]
pub struct Config {
    /// `https://host:port` of the executor (`RUNTARA_EXECUTOR_URL`).
    pub executor_url: String,
    /// `ca.pem`, `tls.pem` and `tls.key`: the server's certificate
    /// (`RUNTARA_EXECUTOR_TLS_DIR`).
    pub tls_dir: PathBuf,
    /// Where the run channel listens (`RUNTARA_RUN_CHANNEL_LISTEN`).
    pub run_channel_listen: SocketAddr,
    /// `https://host:port` at which the executor reaches it
    /// (`RUNTARA_RUN_CHANNEL_ENDPOINT`).
    pub run_channel_endpoint: String,
    /// Runs being placed at once (`RUNTARA_EXECUTOR_RUNNER_LIMIT`). What runs
    /// at once is the executor's capacity and the tenants' limits.
    pub runner_limit: usize,
}

impl Config {
    /// From the environment: `None` unless `RUNTARA_RUNNER=executor`.
    pub fn from_env() -> Result<Option<Self>, String> {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        match var("RUNTARA_RUNNER").as_deref() {
            None | Some("embedded") => return Ok(None),
            Some("executor") => {}
            Some(other) => {
                return Err(format!(
                    "RUNTARA_RUNNER is `{other}`; it is `embedded` or `executor`"
                ));
            }
        }
        let required = |name: &str| {
            var(name).ok_or_else(|| format!("{name} is required with RUNTARA_RUNNER=executor"))
        };
        let https = |name: &str| {
            let url = required(name)?.trim_end_matches('/').to_owned();
            if !url.starts_with("https://") {
                return Err(format!("{name} must be https://host:port"));
            }
            Ok(url)
        };
        Ok(Some(Self {
            executor_url: https("RUNTARA_EXECUTOR_URL")?,
            tls_dir: required("RUNTARA_EXECUTOR_TLS_DIR")?.into(),
            run_channel_listen: var("RUNTARA_RUN_CHANNEL_LISTEN")
                .unwrap_or_else(|| "0.0.0.0:7444".to_owned())
                .parse()
                .map_err(|_| "RUNTARA_RUN_CHANNEL_LISTEN is not an address".to_owned())?,
            run_channel_endpoint: https("RUNTARA_RUN_CHANNEL_ENDPOINT")?,
            runner_limit: match var("RUNTARA_EXECUTOR_RUNNER_LIMIT") {
                Some(limit) => limit
                    .parse()
                    .ok()
                    .filter(|limit| (1..=1024).contains(limit))
                    .ok_or("RUNTARA_EXECUTOR_RUNNER_LIMIT must be 1 to 1024")?,
                None => 16,
            },
        }))
    }
}

/// A tenant as the executor's path segment: itself when it is one, else a
/// digest of it.
fn tenant_segment(tenant: &str) -> String {
    if valid_tenant(tenant) {
        return tenant.to_owned();
    }
    let digest = sha2::Sha256::digest(tenant.as_bytes());
    format!("t-{}", hex(&digest[..16]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// An executor as a capacity provider.
pub struct ExecutorProvider {
    client: tls::Client,
    url: String,
    /// When each run's executor first failed to answer.
    unreachable: Mutex<HashMap<String, Instant>>,
}

impl ExecutorProvider {
    /// The executor at `url`, reached as `identity`.
    pub fn new(identity: &Identity, url: String) -> Result<Self, String> {
        let config = identity.client_config(&identity.ca_pem, None)?;
        Ok(Self {
            client: tls::client(config, EXECUTOR_NAME)?,
            url,
            unreachable: Mutex::default(),
        })
    }

    /// One call; its status and body, or why it could not be made.
    async fn call(
        &self,
        method: http::Method,
        path: &str,
        body: Bytes,
        timeout: Duration,
    ) -> Result<(http::StatusCode, Bytes), String> {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.url))
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Full::new(body))
            .map_err(|error| error.to_string())?;
        let exchange = async {
            let response = self
                .client
                .request(request)
                .await
                .map_err(|error| format!("the executor could not be reached: {error}"))?;
            let status = response.status();
            let body = response
                .into_body()
                .collect()
                .await
                .map_err(|error| format!("the executor's answer broke off: {error}"))?
                .to_bytes();
            Ok((status, body))
        };
        tokio::time::timeout(timeout, exchange)
            .await
            .map_err(|_| "the executor did not answer in time".to_owned())?
    }

    fn refusal(status: http::StatusCode, body: &[u8]) -> String {
        match serde_json::from_slice::<Refusal>(body) {
            Ok(refusal) => format!(
                "the executor refused ({}): {}",
                refusal.error, refusal.message
            ),
            Err(_) => format!("the executor answered {status}"),
        }
    }

    fn reached(&self, run: &str) {
        self.unreachable
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run);
    }
}

#[async_trait]
impl Provider for ExecutorProvider {
    fn name(&self) -> &'static str {
        "executor"
    }

    fn spec(&self, options: &LaunchOptions, _component_bytes: u64) -> Value {
        // The guest's limits. It learns who it is over the workflow API, so
        // it is given no environment.
        json!({
            "wasm_memory_bytes": options.memory_limit_bytes.unwrap_or(DEFAULT_MEMORY_BYTES),
            "lifetime_seconds": options.timeout.as_secs().clamp(1, 86_400),
            "env": {},
        })
    }

    /// Nothing is sent: the executor fetches a workflow it does not hold
    /// when a run of it is placed ([`workflow_router`]).
    async fn stage(&self, _tenant: &str, bytes: &[u8]) -> Result<String, String> {
        Ok(hex(&sha2::Sha256::digest(bytes)))
    }

    async fn place(
        &self,
        spec: Value,
        artifact: &str,
        binding: &Binding,
    ) -> Result<String, String> {
        let place = json!({
            "tenant": tenant_segment(&binding.tenant_id),
            "artifact": artifact,
            "spec": spec,
        });
        let (status, body) = self
            .call(
                http::Method::POST,
                "/v1/runs",
                Bytes::from(place.to_string()),
                CALL_TIMEOUT,
            )
            .await?;
        if !status.is_success() {
            return Err(Self::refusal(status, &body));
        }
        serde_json::from_slice::<Placed>(&body)
            .map(|placed| placed.run)
            .map_err(|_| "the executor's answer is not a placed run".to_owned())
    }

    async fn stop(&self, run: &str) {
        let _ = self
            .call(
                http::Method::DELETE,
                &format!("/v1/runs/{run}"),
                Bytes::new(),
                CALL_TIMEOUT,
            )
            .await;
    }

    async fn ended(&self, run: &str, timeout: Duration) -> Option<ProviderEnd> {
        let path = format!("/v1/runs/{run}?wait_ms={}", timeout.as_millis());
        match self
            .call(http::Method::GET, &path, Bytes::new(), CALL_TIMEOUT)
            .await
        {
            Ok((status, body)) if status.is_success() => {
                self.reached(run);
                match serde_json::from_slice::<RunState>(&body) {
                    Ok(RunState::Running) => None,
                    Ok(RunState::Exited { code }) => Some(ProviderEnd::Exited(code)),
                    Ok(RunState::Failed { reason }) => Some(ProviderEnd::Failed(reason)),
                    Err(_) => Some(ProviderEnd::Failed(
                        "the executor's answer is not a run's state".to_owned(),
                    )),
                }
            }
            // An executor that started again holds none of its earlier runs.
            Ok((http::StatusCode::NOT_FOUND, _)) => {
                self.reached(run);
                Some(ProviderEnd::Lost(
                    "its executor no longer holds it".to_owned(),
                ))
            }
            unanswered => {
                let since = *self
                    .unreachable
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .entry(run.to_owned())
                    .or_insert_with(Instant::now);
                if since.elapsed() >= LOST_AFTER {
                    self.reached(run);
                    let why = match unanswered {
                        Ok((status, _)) => format!("its executor answers {status}"),
                        Err(error) => error,
                    };
                    return Some(ProviderEnd::Lost(format!("lost with its executor: {why}")));
                }
                // Not a tight loop against an executor that is away.
                tokio::time::sleep(timeout).await;
                None
            }
        }
    }
}

/// What an executor fetches a launch's workflow from, on the run channel's
/// listener: `GET /executor/v1/artifacts/{sha256}` with the launch's
/// credential answers with that launch's workflow component, and with nothing
/// for any other digest. It is outside `/v1`, which is all an executor relays
/// from a guest, and it is answered before the launch's start is confirmed:
/// a workflow being fetched is not a guest acting.
pub fn workflow_router(launches: Arc<ApiLaunches>, workflows: Arc<LaunchWorkflows>) -> Router {
    let fetch = move |Path(digest): Path<String>, headers: http::HeaderMap| {
        let (launches, workflows) = (launches.clone(), workflows.clone());
        async move {
            let Some(binding) = launches.credential_launch(&headers) else {
                return http::StatusCode::UNAUTHORIZED.into_response();
            };
            if !valid_digest(&digest) {
                return http::StatusCode::NOT_FOUND.into_response();
            }
            match workflows.get(&binding.launch_id, &digest).await {
                Ok(Some(component)) => component.into_response(),
                Ok(None) => http::StatusCode::NOT_FOUND.into_response(),
                Err(error) => {
                    tracing::error!(%error, launch = %binding.launch_id, "reading a launch's workflow failed");
                    http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
    };
    Router::new().route(&format!("{ARTIFACTS_PATH}/{{digest}}"), get(fetch))
}

/// Serves the run channel and connects to the executor; the factory of the
/// runner over them. Called once, at boot, on the server's runtime.
pub async fn prepare(config: Config) -> Result<RunnerFactory, String> {
    let identity = Identity::load(&config.tls_dir)?;
    let launches = Arc::new(ApiLaunches::default());
    let workflows = Arc::new(LaunchWorkflows::default());
    let listener = tls::bind(config.run_channel_listen).await?;
    tracing::info!(
        listen = %config.run_channel_listen,
        endpoint = %config.run_channel_endpoint,
        executor = %config.executor_url,
        "workflows run on an executor; run channel listening"
    );
    tokio::spawn(tls::serve(
        listener,
        identity.server_config()?,
        EXECUTOR_NAME,
        launches
            .credentialed_router()
            .merge(workflow_router(launches.clone(), workflows.clone())),
    ));
    let channel = RunChannel {
        endpoint: config.run_channel_endpoint,
        ca_pem: identity.ca_pem.clone(),
        pin: None,
        server_name: Some(SERVER_NAME.to_owned()),
    };
    let provider = Arc::new(ExecutorProvider::new(&identity, config.executor_url)?);
    let limit = config.runner_limit;
    Ok(Arc::new(move |parts| {
        Ok(Arc::new(
            ProviderRunner::new(provider.clone(), parts, launches.clone(), limit)
                .with_run_channel(channel.clone())
                .with_launch_workflows(workflows.clone()),
        ) as Arc<dyn Runner>)
    }))
}

/// [`prepare`] from the environment: `None` unless `RUNTARA_RUNNER=executor`.
pub async fn from_env() -> Result<Option<RunnerFactory>, String> {
    match Config::from_env()? {
        Some(config) => prepare(config).await.map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tenant_that_is_not_a_path_segment_is_named_by_its_digest() {
        assert_eq!(tenant_segment("org_1"), "org_1");
        let odd = tenant_segment("a/b");
        assert!(valid_tenant(&odd), "{odd}");
        assert_ne!(odd, tenant_segment("a.b"));
    }
}
