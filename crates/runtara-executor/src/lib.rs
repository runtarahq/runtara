// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The executor: a service that runs the workflows a runtara server places
//! with it, each as a WebAssembly guest in a sandboxed process of its own
//! (`runtara-run`), on a host that holds none of the server's secrets.
//!
//! ```text
//! runtara-server ◀──mutual TLS──▶ runtara-executor ──socket pair──▶ runtara-run
//!  places runs                     one supervisor per run            one process per run:
//!  serves the run channel          holds the launch's credential     the guest, sandboxed,
//!                                  relays the guest's API calls      no network, no files
//! ```
//!
//! The server calls the placement API here ([`protocol`]); each run's
//! workflow API calls go back to the server's run channel. Both directions
//! are mutual TLS between two certificates of one deployment CA ([`pki`]),
//! each side accepting only the other's.
//!
//! What this isolates: a guest that escapes Wasmtime lands in a process that
//! can open no file, make no socket and signal nothing, on a host with no
//! database connection and no key. What it does not: the kernel is shared, so
//! this is defence in depth for a self-hosted deployment, not the boundary a
//! virtual machine is.

pub mod api;
pub mod artifacts;
pub mod pki;
pub mod protocol;
pub mod runs;
pub mod tls;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

/// How an executor is set up.
#[derive(Clone, Debug)]
pub struct Config {
    /// Where the server connects (`RUNTARA_EXECUTOR_LISTEN`).
    pub listen: SocketAddr,
    /// `ca.pem`, `tls.pem` and `tls.key` (`RUNTARA_EXECUTOR_TLS_DIR`).
    pub tls_dir: PathBuf,
    /// Compiled workflows (`RUNTARA_EXECUTOR_STATE_DIR`).
    pub state_dir: PathBuf,
    /// Runs at once (`RUNTARA_EXECUTOR_MAX_RUNS`).
    pub max_runs: usize,
    /// The `runtara-run` to start (`RUNTARA_EXECUTOR_RUN_BIN`; by default the
    /// one beside this executable).
    pub run_binary: PathBuf,
    /// Run without Landlock on a kernel that has none
    /// (`RUNTARA_EXECUTOR_NO_LANDLOCK=true`); seccomp still applies.
    pub no_landlock: bool,
    /// Run with no sandbox at all (`RUNTARA_EXECUTOR_UNSANDBOXED=true`): for
    /// development on a host that has none. Never for a deployment.
    pub unsandboxed: bool,
}

impl Config {
    /// From the environment.
    pub fn from_env() -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let required = |name: &str| var(name).ok_or_else(|| format!("{name} is required"));
        let flag = |name: &str| var(name).is_some_and(|value| value == "true");
        let run_binary = match var("RUNTARA_EXECUTOR_RUN_BIN") {
            Some(path) => PathBuf::from(path),
            None => std::env::current_exe()
                .map_err(|error| error.to_string())?
                .with_file_name("runtara-run"),
        };
        Ok(Self {
            listen: var("RUNTARA_EXECUTOR_LISTEN")
                .unwrap_or_else(|| "0.0.0.0:7443".to_owned())
                .parse()
                .map_err(|_| "RUNTARA_EXECUTOR_LISTEN is not an address".to_owned())?,
            tls_dir: required("RUNTARA_EXECUTOR_TLS_DIR")?.into(),
            state_dir: required("RUNTARA_EXECUTOR_STATE_DIR")?.into(),
            max_runs: match var("RUNTARA_EXECUTOR_MAX_RUNS") {
                Some(runs) => runs
                    .parse()
                    .ok()
                    .filter(|runs| (1..=1024).contains(runs))
                    .ok_or("RUNTARA_EXECUTOR_MAX_RUNS must be 1 to 1024")?,
                None => 16,
            },
            run_binary,
            no_landlock: flag("RUNTARA_EXECUTOR_NO_LANDLOCK"),
            unsandboxed: flag("RUNTARA_EXECUTOR_UNSANDBOXED"),
        })
    }

    fn runner(&self) -> Result<artifacts::Runner, String> {
        let mut flags = Vec::new();
        if self.unsandboxed {
            flags.push("--unsandboxed".to_owned());
        }
        if self.no_landlock {
            flags.push("--no-landlock".to_owned());
        }
        artifacts::Runner::new(self.run_binary.clone(), flags)
    }
}

/// A bound executor, about to serve.
pub struct Executor {
    listener: tokio::net::TcpListener,
    tls: rustls::ServerConfig,
    api: api::Api,
}

impl Executor {
    /// Checks the sandbox holds on this host, loads the certificates and
    /// binds the listener.
    pub async fn bind(config: &Config) -> Result<Self, String> {
        let runner = config.runner()?;
        if config.unsandboxed {
            tracing::warn!("RUNS ARE NOT SANDBOXED (RUNTARA_EXECUTOR_UNSANDBOXED)");
        } else {
            selftest(&runner).await?;
        }
        let identity = Arc::new(tls::Identity::load(&config.tls_dir)?);
        let tls = identity.server_config()?;
        let artifacts = Arc::new(artifacts::Artifacts::new(
            &config.state_dir,
            runner.clone(),
        )?);
        let runs = runs::Runs::new(artifacts.clone(), identity, runner, config.max_runs);
        Ok(Self {
            listener: tls::bind(config.listen).await?,
            tls,
            api: api::Api { artifacts, runs },
        })
    }

    /// Where it listens.
    pub fn address(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("a bound listener has an address")
    }

    /// Its runs.
    pub fn runs(&self) -> Arc<runs::Runs> {
        self.api.runs.clone()
    }

    /// Serves the placement API to the server, until dropped.
    pub async fn serve(self) {
        tls::serve(
            self.listener,
            self.tls,
            protocol::SERVER_NAME,
            api::router(self.api),
        )
        .await
    }
}

/// Has the runner sandbox itself and try what it must refuse: an executor
/// does not start on a host where the sandbox does not hold.
async fn selftest(runner: &artifacts::Runner) -> Result<(), String> {
    let output = tokio::process::Command::new(&runner.binary)
        .arg("selftest")
        .args(&runner.flags)
        .env_clear()
        .output()
        .await
        .map_err(|error| format!("{}: {error}", runner.binary.display()))?;
    if output.status.success() {
        tracing::info!(
            report = %String::from_utf8_lossy(&output.stdout).trim(),
            "the sandbox holds"
        );
        return Ok(());
    }
    Err(format!(
        "the sandbox does not hold on this host: {} {}",
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}
