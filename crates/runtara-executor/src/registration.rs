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
        // New each start: the server tells a restarted executor, which holds
        // none of its earlier runs, from the one that was there.
        let boot = uuid::Uuid::new_v4().simple().to_string();
        Self::with_boot(identity, server, boot, address, port, max_runs, runs)
    }

    /// As [`Self::new`], with the id this executor has for as long as it
    /// runs: one for every server it tells.
    pub fn with_boot(
        identity: &Identity,
        server: &str,
        boot: String,
        address: Option<String>,
        port: u16,
        max_runs: usize,
        runs: Arc<Runs>,
    ) -> Result<Self, String> {
        let server = server.trim_end_matches('/');
        if !server.starts_with("https://") {
            return Err("the server's address must be https://host:port".to_owned());
        }
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

/// How often the servers' addresses are looked up again.
const RESOLVE_EVERY: Duration = Duration::from_secs(10);

/// Keeps every server told that this executor is here: each engine of a
/// deployment that has several (docs/multiple-engines-plan.md, E5).
///
/// `RUNTARA_EXECUTOR_SERVER_URL` lists one or more `https://host:port`,
/// comma-separated; a host may resolve to several engines (a headless
/// service). Each address is told on its own, with one boot id for all, and
/// the addresses are looked up again every [`RESOLVE_EVERY`]: an engine that
/// joins is told within that, and one that left is no longer called.
pub struct Registrars {
    identity: Arc<Identity>,
    servers: Vec<String>,
    boot: String,
    address: Option<String>,
    port: u16,
    max_runs: usize,
    runs: Arc<Runs>,
    told: std::sync::Mutex<std::collections::HashMap<String, Told>>,
    /// The addresses each server was last found at.
    found: std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
}

/// One server being told, and the task that tells it.
struct Told {
    registrar: Arc<Registrar>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Told {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The servers named by `urls`: comma-separated `https://host:port`.
pub fn servers(urls: &str) -> Result<Vec<String>, String> {
    let servers: Vec<String> = urls
        .split(',')
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
        .collect();
    if servers.is_empty() {
        return Err("no server's address is named".to_owned());
    }
    for server in &servers {
        host_and_port(server)?;
    }
    Ok(servers)
}

/// The host and port of `https://host:port`.
fn host_and_port(server: &str) -> Result<(String, u16), String> {
    let rest = server
        .strip_prefix("https://")
        .ok_or_else(|| format!("{server}: the server's address must be https://host:port"))?;
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| format!("{server}: the server's address must name its port"))?;
    let port = port
        .parse()
        .map_err(|_| format!("{server}: the port is not a number"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err(format!("{server}: the server's address must name a host"));
    }
    Ok((host.to_owned(), port))
}

/// `https://` and an address, as a URL.
fn url_of(address: std::net::SocketAddr) -> String {
    match address {
        std::net::SocketAddr::V4(v4) => format!("https://{v4}"),
        std::net::SocketAddr::V6(v6) => format!("https://[{}]:{}", v6.ip(), v6.port()),
    }
}

impl Registrars {
    /// For the executor listening on `port`, telling each of `servers`.
    pub fn new(
        identity: Arc<Identity>,
        servers: Vec<String>,
        address: Option<String>,
        port: u16,
        max_runs: usize,
        runs: Arc<Runs>,
    ) -> Self {
        Self {
            identity,
            servers,
            boot: uuid::Uuid::new_v4().simple().to_string(),
            address,
            port,
            max_runs,
            runs,
            told: std::sync::Mutex::new(std::collections::HashMap::new()),
            found: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// This executor's id for as long as it runs.
    pub fn boot(&self) -> &str {
        &self.boot
    }

    /// The addresses of the servers now, each as `https://address:port`. A
    /// host that cannot be looked up now keeps the addresses it had.
    async fn addresses(&self) -> Vec<String> {
        let mut addresses = Vec::new();
        for server in &self.servers {
            let Ok((host, port)) = host_and_port(server) else {
                continue;
            };
            if host.parse::<std::net::IpAddr>().is_ok() {
                addresses.push(server.clone());
                continue;
            }
            let found = match tokio::net::lookup_host((host.as_str(), port)).await {
                Ok(found) => {
                    let found: Vec<String> = found.map(url_of).collect();
                    self.found
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(server.clone(), found.clone());
                    found
                }
                Err(error) => {
                    tracing::debug!(%server, %error, "a server's host could not be looked up");
                    self.found
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .get(server)
                        .cloned()
                        .unwrap_or_default()
                }
            };
            addresses.extend(found);
        }
        addresses.sort();
        addresses.dedup();
        addresses
    }

    /// Tells every server, now and for as long as this is polled.
    pub async fn keep(self: Arc<Self>) {
        // Dropped, this stops telling every server; each is still told when
        // this executor leaves.
        struct StopTelling<'a>(&'a Registrars);
        impl Drop for StopTelling<'_> {
            fn drop(&mut self) {
                for told in self.0.told().values() {
                    told.task.abort();
                }
            }
        }
        let _stop = StopTelling(&self);
        loop {
            let now = self.addresses().await;
            {
                let mut told = self.told();
                told.retain(|address, _| {
                    let stays = now.contains(address);
                    if !stays {
                        tracing::info!(%address, "no longer telling a server that is gone");
                    }
                    stays
                });
                for address in now {
                    if told.contains_key(&address) {
                        continue;
                    }
                    match Registrar::with_boot(
                        &self.identity,
                        &address,
                        self.boot.clone(),
                        self.address.clone(),
                        self.port,
                        self.max_runs,
                        self.runs.clone(),
                    ) {
                        Ok(registrar) => {
                            tracing::info!(%address, "telling a server this executor is here");
                            let registrar = Arc::new(registrar);
                            let task = tokio::spawn(registrar.clone().keep());
                            told.insert(address, Told { registrar, task });
                        }
                        Err(error) => tracing::warn!(%address, %error, "cannot tell a server"),
                    }
                }
            }
            tokio::time::sleep(RESOLVE_EVERY).await;
        }
    }

    fn told(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Told>> {
        self.told
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The servers being told now.
    pub fn telling(&self) -> Vec<String> {
        let mut telling: Vec<String> = self.told().keys().cloned().collect();
        telling.sort();
        telling
    }

    /// Tells every server this executor is stopping.
    pub async fn leave(&self) {
        let registrars: Vec<Arc<Registrar>> = self
            .told()
            .values()
            .map(|told| told.registrar.clone())
            .collect();
        for registrar in registrars {
            registrar.leave().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_server_or_several_are_named() {
        assert_eq!(
            servers("https://engine:7444/").unwrap(),
            ["https://engine:7444"]
        );
        assert_eq!(
            servers("https://a:7444, https://b:7444").unwrap(),
            ["https://a:7444", "https://b:7444"]
        );
        assert!(servers("").is_err());
        assert!(servers("http://a:7444").is_err());
        assert!(servers("https://a").is_err());
    }

    #[test]
    fn an_address_is_a_url_again() {
        assert_eq!(
            url_of("10.0.0.2:7444".parse().unwrap()),
            "https://10.0.0.2:7444"
        );
        assert_eq!(url_of("[::1]:7444".parse().unwrap()), "https://[::1]:7444");
        assert_eq!(
            host_and_port("https://[::1]:7444").unwrap(),
            ("::1".to_owned(), 7444)
        );
    }
}
