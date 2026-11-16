// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The server's side of an executor (`runtara-executor`): runtara's
//! provider-neutral runner ([`ProviderRunner`]) with an executor as its
//! provider.
//!
//! With `RUNTARA_RUNNER=executor` no workflow runs in the server's process.
//! Each launch is placed with one of the executors that told this server
//! they are here ([`registry`]): the one with the most room, and the next if
//! that one is full or cannot be reached. The executor fetches the launch's
//! workflow from the run channel's listener if it does not hold it; the
//! guest reaches the workflow API over the run channel this process serves,
//! each call carrying the credential minted for its launch; and the run ends
//! on the outcome it posts. The executor says when a run's process ended
//! without one, and a run whose executor is gone or cannot be reached is
//! handed to recovery.
//!
//! Both connections are mutual TLS between the server's and the executor's
//! certificates (`runtara-executor init-pki`): this process accepts on its
//! run channel only the executor's, and places runs only with it.

pub mod registry;

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

use crate::registry::{Executor, Registry};
use runtara_workflow_api::Binding;
use serde_json::{Value, json};
use sha2::Digest;

/// A guest's memory when its launch names none: what a local run gets
/// (`runtara-component-host`'s `DEFAULT_GUEST_MEMORY_MAX_BYTES`).
const DEFAULT_MEMORY_BYTES: u64 = 1024 * 1024 * 1024;
/// An executor unreachable this long has lost its runs: runtara recovers
/// them.
const LOST_AFTER: Duration = Duration::from_secs(30);
/// How long one call to an executor about a run may take: a status call is
/// held open for up to 30 seconds.
const CALL_TIMEOUT: Duration = Duration::from_secs(40);
/// How long placing a run may take: an executor answers at once, or waits
/// up to 5 seconds for a slot.
const PLACE_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an executor that was full is offered no run.
const FULL_FOR: Duration = Duration::from_secs(2);
/// How long an executor that could not be reached is offered no run.
const UNREACHABLE_FOR: Duration = Duration::from_secs(5);

/// How executors reach the server. Which executors there are is not
/// configured: they say so ([`registry`]).
#[derive(Clone, Debug)]
pub struct Config {
    /// `ca.pem`, `tls.pem` and `tls.key`: the server's certificate
    /// (`RUNTARA_EXECUTOR_TLS_DIR`).
    pub tls_dir: PathBuf,
    /// Where the run channel listens (`RUNTARA_RUN_CHANNEL_LISTEN`).
    pub run_channel_listen: SocketAddr,
    /// `https://host:port` at which executors reach it
    /// (`RUNTARA_RUN_CHANNEL_ENDPOINT`).
    pub run_channel_endpoint: String,
    /// Runs being placed at once (`RUNTARA_EXECUTOR_RUNNER_LIMIT`). What runs
    /// at once is the executors' capacity and the tenants' limits.
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

/// Why a call to an executor was not answered.
struct Unanswered {
    why: String,
    /// The connection was never made, so the call was never sent.
    never_sent: bool,
}

/// One call to `executor`; its status and body.
async fn call(
    executor: &Executor,
    method: http::Method,
    path: &str,
    body: Bytes,
    timeout: Duration,
) -> Result<(http::StatusCode, Bytes), Unanswered> {
    let sent = |why: String| Unanswered {
        why,
        never_sent: false,
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("{}{path}", executor.address))
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Full::new(body))
        .map_err(|error| sent(error.to_string()))?;
    let exchange = async {
        let response = executor
            .client
            .request(request)
            .await
            .map_err(|error| Unanswered {
                why: format!("the executor could not be reached: {error}"),
                never_sent: error.is_connect(),
            })?;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|error| sent(format!("the executor's answer broke off: {error}")))?
            .to_bytes();
        Ok((status, body))
    };
    tokio::time::timeout(timeout, exchange)
        .await
        .map_err(|_| sent("the executor did not answer in time".to_owned()))?
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

/// The registered executors as a capacity provider. A run it placed is named
/// `<boot>:<run>`: the executor that holds it, and that executor's id for it.
pub struct ExecutorProvider {
    executors: Arc<Registry>,
    /// When each run's executor first failed to answer.
    unreachable: Mutex<HashMap<String, Instant>>,
}

impl ExecutorProvider {
    /// Places runs with the executors in `executors`.
    pub fn new(executors: Arc<Registry>) -> Self {
        Self {
            executors,
            unreachable: Mutex::default(),
        }
    }

    fn reached(&self, run: &str) {
        self.unreachable
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run);
    }

    /// The executor that holds `run`, while it is known, and its id for it.
    fn holder<'run>(&self, run: &'run str) -> Option<(Executor, &'run str)> {
        let (boot, id) = run.split_once(':')?;
        Some((self.executors.get(boot)?, id))
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

    /// Places the run with the executor that has the most room, and with
    /// the next if that one is full or was never reached. An executor that
    /// may have taken the run is not followed by another: a run must not
    /// start twice.
    async fn place(
        &self,
        spec: Value,
        artifact: &str,
        binding: &Binding,
    ) -> Result<String, String> {
        let candidates = self.executors.candidates();
        let mut refused = "no executor is here to place a run with".to_owned();
        for (index, executor) in candidates.iter().enumerate() {
            let mut place = json!({
                "tenant": tenant_segment(&binding.tenant_id),
                "artifact": artifact,
                "spec": spec,
            });
            // With another to try, a full executor says so at once.
            if index + 1 < candidates.len() {
                place["slot_wait_ms"] = json!(0);
            }
            let answer = call(
                executor,
                http::Method::POST,
                "/v1/runs",
                Bytes::from(place.to_string()),
                PLACE_TIMEOUT,
            )
            .await;
            match answer {
                Ok((status, body)) if status.is_success() => {
                    let placed = serde_json::from_slice::<Placed>(&body)
                        .map_err(|_| "the executor's answer is not a placed run".to_owned())?;
                    self.executors.placed(&executor.boot);
                    return Ok(format!("{}:{}", executor.boot, placed.run));
                }
                Ok((http::StatusCode::SERVICE_UNAVAILABLE, body)) => {
                    self.executors.pass_over(&executor.boot, FULL_FOR);
                    refused = refusal(http::StatusCode::SERVICE_UNAVAILABLE, &body);
                }
                Ok((status, body)) => return Err(refusal(status, &body)),
                Err(unanswered) if unanswered.never_sent => {
                    self.executors.pass_over(&executor.boot, UNREACHABLE_FOR);
                    refused = unanswered.why;
                }
                Err(unanswered) => {
                    self.executors.pass_over(&executor.boot, UNREACHABLE_FOR);
                    return Err(unanswered.why);
                }
            }
        }
        Err(refused)
    }

    async fn stop(&self, run: &str) {
        if let Some((executor, id)) = self.holder(run) {
            let _ = call(
                &executor,
                http::Method::DELETE,
                &format!("/v1/runs/{id}"),
                Bytes::new(),
                CALL_TIMEOUT,
            )
            .await;
        }
    }

    async fn ended(&self, run: &str, timeout: Duration) -> Option<ProviderEnd> {
        // An executor that left, or started again, holds none of its runs.
        let Some((executor, id)) = self.holder(run) else {
            self.reached(run);
            return Some(ProviderEnd::Lost("its executor is gone".to_owned()));
        };
        let path = format!("/v1/runs/{id}?wait_ms={}", timeout.as_millis());
        match call(
            &executor,
            http::Method::GET,
            &path,
            Bytes::new(),
            CALL_TIMEOUT,
        )
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
                        Err(unanswered) => unanswered.why,
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

/// What [`prepare`] made: the runner's factory, and the executors that have
/// said they are here.
pub struct Prepared {
    pub factory: RunnerFactory,
    pub executors: Arc<Registry>,
}

/// Serves the run channel, where executors also say they are here and fetch
/// workflows; the factory of the runner over the executors that do. Called
/// once, at boot, on the server's runtime.
pub async fn prepare(config: Config) -> Result<Prepared, String> {
    let identity = Identity::load(&config.tls_dir)?;
    let launches = Arc::new(ApiLaunches::default());
    let workflows = Arc::new(LaunchWorkflows::default());
    let listener = tls::bind(config.run_channel_listen).await?;
    tracing::info!(
        listen = %config.run_channel_listen,
        endpoint = %config.run_channel_endpoint,
        "workflows run on the executors that register here; run channel listening"
    );
    let server_config = identity.server_config()?;
    let channel = RunChannel {
        endpoint: config.run_channel_endpoint,
        ca_pem: identity.ca_pem.clone(),
        pin: None,
        server_name: Some(SERVER_NAME.to_owned()),
    };
    let executors = Arc::new(Registry::new(identity));
    tokio::spawn(tls::serve(
        listener,
        server_config,
        EXECUTOR_NAME,
        launches
            .credentialed_router()
            .merge(workflow_router(launches.clone(), workflows.clone()))
            .merge(registry::router(executors.clone())),
    ));
    let provider = Arc::new(ExecutorProvider::new(executors.clone()));
    let limit = config.runner_limit;
    let factory: RunnerFactory = Arc::new(move |parts| {
        Ok(Arc::new(
            ProviderRunner::new(provider.clone(), parts, launches.clone(), limit)
                .with_run_channel(channel.clone())
                .with_launch_workflows(workflows.clone()),
        ) as Arc<dyn Runner>)
    });
    Ok(Prepared { factory, executors })
}

/// [`prepare`] from the environment: `None` unless `RUNTARA_RUNNER=executor`.
pub async fn from_env() -> Result<Option<RunnerFactory>, String> {
    match Config::from_env()? {
        Some(config) => prepare(config).await.map(|prepared| Some(prepared.factory)),
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
