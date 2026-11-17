// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! How a server comes to know this executor: the executor tells it.
//!
//! At start, and again at the interval the server answers with, the executor
//! says where it is and how many runs it holds
//! ([`crate::protocol::Registration`]), on the server's run channel listener
//! and with its own certificate. A server places runs with the executors it
//! has heard from lately, so no server is configured with its executors, and
//! one that started again learns them from their next call. Stopping, the
//! executor says so, and the server recovers the runs it held at once.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};

use crate::protocol::{EXECUTORS_PATH, Registered, Registration, SERVER_NAME};
use crate::runs::Runs;
use crate::tls::{Client, Identity};

/// How long one call to the server may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// How soon a call the server did not take is made again.
const RETRY: Duration = Duration::from_secs(2);
/// The shortest and longest interval a server may ask for.
const SOONEST: Duration = Duration::from_millis(100);
const LATEST: Duration = Duration::from_secs(60);

/// Keeps a server told that this executor is here.
pub struct Registrar {
    client: Client,
    /// `{server}/executor/v1/executors/{boot}`.
    url: String,
    boot: String,
    address: Option<String>,
    port: u16,
    max_runs: usize,
    runs: Arc<Runs>,
}

impl Registrar {
    /// For the executor listening on `port`, reached at `address` if that is
    /// not where its calls come from, telling the server at `server`
    /// (`https://host:port` of its run channel).
    pub fn new(
        identity: &Identity,
        server: &str,
        address: Option<String>,
        port: u16,
        max_runs: usize,
        runs: Arc<Runs>,
    ) -> Result<Self, String> {
        let server = server.trim_end_matches('/');
        if !server.starts_with("https://") {
            return Err("the server's address must be https://host:port".to_owned());
        }
        // New each start: the server tells a restarted executor, which holds
        // none of its earlier runs, from the one that was there.
        let boot = uuid::Uuid::new_v4().simple().to_string();
        let config = identity.client_config(&identity.ca_pem, None)?;
        Ok(Self {
            client: crate::tls::client(config, SERVER_NAME)?,
            url: format!("{server}{EXECUTORS_PATH}/{boot}"),
            boot,
            address,
            port,
            max_runs,
            runs,
        })
    }

    /// This executor's id for as long as it runs.
    pub fn boot(&self) -> &str {
        &self.boot
    }

    async fn call(&self, method: http::Method, body: Bytes) -> Result<Bytes, String> {
        let request = http::Request::builder()
            .method(method)
            .uri(&self.url)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Full::new(body))
            .map_err(|error| error.to_string())?;
        let exchange = async {
            let response = self
                .client
                .request(request)
                .await
                .map_err(|error| format!("the server could not be reached: {error}"))?;
            let status = response.status();
            let body = Limited::new(response.into_body(), 64 * 1024)
                .collect()
                .await
                .map_err(|_| "the server's answer broke off".to_owned())?
                .to_bytes();
            if status.is_success() {
                Ok(body)
            } else {
                Err(format!("the server answered {status}"))
            }
        };
        tokio::time::timeout(CALL_TIMEOUT, exchange)
            .await
            .map_err(|_| "the server did not answer in time".to_owned())?
    }

    /// Tells the server this executor is here, as it is now.
    pub async fn here(&self) -> Result<Registered, String> {
        let registration = Registration {
            address: self.address.clone(),
            port: self.port,
            max_runs: self.max_runs,
            running: self.runs.running(),
        };
        let body = serde_json::to_vec(&registration).map_err(|error| error.to_string())?;
        let answer = self.call(http::Method::PUT, Bytes::from(body)).await?;
        serde_json::from_slice(&answer).map_err(|_| "the server's answer is not one".to_owned())
    }

    /// Tells the server this executor is stopping. A server that is not told
    /// finds out when the executor stops calling.
    pub async fn leave(&self) {
        if let Err(error) = self.call(http::Method::DELETE, Bytes::new()).await {
            tracing::warn!(%error, "the server was not told this executor is stopping");
        }
    }

    /// Tells the server now and at the interval it answers with, for as long
    /// as this is polled. A server that is away is told when it is back; the
    /// runs held meanwhile are not affected.
    pub async fn keep(self: Arc<Self>) {
        let mut told = false;
        loop {
            let wait = match self.here().await {
                Ok(registered) => {
                    if !told {
                        tracing::info!(boot = %self.boot, "registered with the server");
                    }
                    told = true;
                    Duration::from_millis(registered.heartbeat_ms).clamp(SOONEST, LATEST)
                }
                Err(error) => {
                    if told {
                        tracing::warn!(%error, "the server stopped taking this executor's calls");
                    } else {
                        tracing::debug!(%error, "not registered with the server yet");
                    }
                    told = false;
                    RETRY
                }
            };
            tokio::time::sleep(wait).await;
        }
    }
}
