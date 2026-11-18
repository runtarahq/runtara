// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The server's roles, and which background work is whose.
//!
//! The API takes requests; the engine owns durable execution and everything
//! done with a connection's credentials (see [`crate::engine`]); intake
//! receives what anyone may send (a trigger's webhooks) and holds no run
//! state. One process runs all three by default. Each piece of background work is started for the role it
//! belongs to, so that a process that runs one role starts that role's work
//! and none of the other's.

use std::future::Future;
use std::sync::Mutex;

use crate::shutdown::ShutdownCoordinator;

/// One of the server's roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Takes requests: handlers, MCP, and what keeps them answering.
    Api,
    /// Durable execution, and the use of connections' credentials.
    Engine,
    /// What anyone may send: a trigger's webhooks, checked and queued. A
    /// process that runs it alone holds no runtime database and may only ask
    /// seal whether a signature is good (docs/splitting-the-engine.md).
    Intake,
}

/// The roles a process runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Roles {
    api: bool,
    engine: bool,
    intake: bool,
}

impl Roles {
    /// Every role, in one process.
    pub const fn all() -> Self {
        Self {
            api: true,
            engine: true,
            intake: true,
        }
    }

    /// One role alone.
    pub const fn only(role: Role) -> Self {
        Self {
            api: matches!(role, Role::Api),
            engine: matches!(role, Role::Engine),
            intake: matches!(role, Role::Intake),
        }
    }

    /// Whether this process runs `role`.
    pub const fn runs(self, role: Role) -> bool {
        match role {
            Role::Api => self.api,
            Role::Engine => self.engine,
            Role::Intake => self.intake,
        }
    }
}

impl Roles {
    /// The roles `RUNTARA_ROLE` names: `all` (the default), `api`, `engine`
    /// or `intake`.
    pub fn from_env() -> Result<Self, String> {
        Self::named(std::env::var("RUNTARA_ROLE").ok().as_deref())
    }

    fn named(role: Option<&str>) -> Result<Self, String> {
        match role.map(str::trim).filter(|role| !role.is_empty()) {
            None | Some("all") => Ok(Self::all()),
            Some("api") => Ok(Self::only(Role::Api)),
            Some("engine") => Ok(Self::only(Role::Engine)),
            Some("intake") => Ok(Self::only(Role::Intake)),
            Some(other) => Err(format!(
                "RUNTARA_ROLE is `{other}`; it is `all`, `api`, `engine` or `intake`"
            )),
        }
    }
}

fn set(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Where the engine listens for the API role, and its certificate.
pub struct EngineListen {
    pub address: std::net::SocketAddr,
    pub tls_dir: std::path::PathBuf,
}

/// `RUNTARA_ENGINE_LISTEN`, if the engine is to listen for an API role in
/// another process, with `RUNTARA_ENGINE_TLS_DIR`: the server's directory
/// from `runtara-executor init-pki`, which is `RUNTARA_EXECUTOR_TLS_DIR`'s
/// when that is set and this is not.
pub fn engine_listen() -> Result<Option<EngineListen>, String> {
    let Some(address) = set("RUNTARA_ENGINE_LISTEN") else {
        return Ok(None);
    };
    let address = address
        .parse()
        .map_err(|_| format!("RUNTARA_ENGINE_LISTEN is `{address}`, which is not an address"))?;
    let tls_dir = set("RUNTARA_ENGINE_TLS_DIR")
        .or_else(|| set("RUNTARA_EXECUTOR_TLS_DIR"))
        .ok_or(
            "RUNTARA_ENGINE_LISTEN needs RUNTARA_ENGINE_TLS_DIR: the server's directory from \
             `runtara-executor init-pki`",
        )?;
    Ok(Some(EngineListen {
        address,
        tls_dir: tls_dir.into(),
    }))
}

/// `RUNTARA_ENGINE_HEALTH_LISTEN`: where an engine that binds no public
/// listener says it is up, to whatever checks its health. It answers
/// `/health` and nothing else, with no TLS: it is for the engine's own host
/// or container.
pub fn engine_health_listen() -> Result<Option<std::net::SocketAddr>, String> {
    let Some(address) = set("RUNTARA_ENGINE_HEALTH_LISTEN") else {
        return Ok(None);
    };
    address.parse().map(Some).map_err(|_| {
        format!("RUNTARA_ENGINE_HEALTH_LISTEN is `{address}`, which is not an address")
    })
}

/// Where the API role reaches the engine, and its certificate.
pub struct EngineAt {
    pub url: String,
    pub tls_dir: std::path::PathBuf,
}

/// `RUNTARA_ENGINE_URL` and `RUNTARA_ENGINE_TLS_DIR` (the API's directory
/// from `runtara-executor init-pki --api-dir`), which the API role needs.
pub fn engine_at() -> Result<EngineAt, String> {
    let url = set("RUNTARA_ENGINE_URL").ok_or(
        "RUNTARA_ROLE=api needs RUNTARA_ENGINE_URL: where the engine listens, as https://host:port",
    )?;
    let tls_dir = set("RUNTARA_ENGINE_TLS_DIR").ok_or(
        "RUNTARA_ROLE=api needs RUNTARA_ENGINE_TLS_DIR: the API's directory from \
         `runtara-executor init-pki --api-dir`",
    )?;
    Ok(EngineAt {
        url,
        tls_dir: tls_dir.into(),
    })
}

/// What the API role is not to be given: the way to the runtime database, a
/// key that opens or wraps connections' secrets, the way to seal, an
/// executor's side of the run channel, and the store of compiled workflows.
/// It needs none of them, and a process that takes requests from outside is
/// not where they are kept.
const NOT_FOR_THE_API: &[&str] = &[
    "RUNTARA_DATABASE_URL",
    "RUNTARA_CONNECTIONS_ENCRYPTION_KEY",
    "RUNTARA_CONNECTIONS_RETIRED_KEYS",
    "RUNTARA_SEAL_WRAPPING_KEY",
    "RUNTARA_SEAL_SOCKET",
    "RUNTARA_SEAL_ADDRESS",
    "RUNTARA_SEAL_TLS_DIR",
    "RUNTARA_EXECUTOR_TLS_DIR",
    "RUNTARA_ARTIFACT_STORE_URL",
    "RUNTARA_ENGINE_LISTEN",
    "RUNTARA_ENGINE_HEALTH_LISTEN",
    "RUNTARA_INTAKE_LISTEN",
    "RUNTARA_INTAKE_HEALTH_LISTEN",
];

/// What the intake role is not to be given: the runtime database, the keys
/// that open connections' parameters or seal's keys, an executor's
/// certificate, the artifact store, or the engine's own settings. It
/// receives what anyone may send, and holds nothing a run is made of.
const NOT_FOR_INTAKE: &[&str] = &[
    "RUNTARA_DATABASE_URL",
    "RUNTARA_CONNECTIONS_ENCRYPTION_KEY",
    "RUNTARA_CONNECTIONS_RETIRED_KEYS",
    "RUNTARA_SEAL_WRAPPING_KEY",
    "RUNTARA_EXECUTOR_TLS_DIR",
    "RUNTARA_ARTIFACT_STORE_URL",
    "RUNTARA_ENGINE_LISTEN",
    "RUNTARA_ENGINE_HEALTH_LISTEN",
    "RUNTARA_COMPILER_WASM",
];

/// Which of what the intake role is not to be given this process was given.
pub fn given_to_the_intake_role() -> Vec<&'static str> {
    NOT_FOR_INTAKE
        .iter()
        .copied()
        .filter(|name| set(name).is_some())
        .collect()
}

/// What an intake that is a process of its own needs: where it listens for
/// the API (`RUNTARA_INTAKE_LISTEN`), its certificate
/// (`RUNTARA_INTAKE_TLS_DIR`, from `runtara-executor init-pki --intake-dir`),
/// and where the engine listens (`RUNTARA_ENGINE_URL`), which it asks to run
/// a workflow and wait.
pub struct IntakeAt {
    pub listen: std::net::SocketAddr,
    pub tls_dir: std::path::PathBuf,
    pub engine_url: String,
    /// `RUNTARA_INTAKE_HEALTH_LISTEN`: where it says it is up, if anywhere.
    pub health: Option<std::net::SocketAddr>,
}

pub fn intake_at() -> Result<IntakeAt, String> {
    let address = |name: &str| -> Result<Option<std::net::SocketAddr>, String> {
        set(name)
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| format!("{name} is `{value}`, which is not an address"))
            })
            .transpose()
    };
    Ok(IntakeAt {
        listen: address("RUNTARA_INTAKE_LISTEN")?.ok_or(
            "RUNTARA_ROLE=intake needs RUNTARA_INTAKE_LISTEN: the address it listens on for \
             the API, as host:port",
        )?,
        tls_dir: set("RUNTARA_INTAKE_TLS_DIR")
            .ok_or(
                "RUNTARA_ROLE=intake needs RUNTARA_INTAKE_TLS_DIR: intake's directory from \
                 `runtara-executor init-pki --intake-dir`",
            )?
            .into(),
        engine_url: set("RUNTARA_ENGINE_URL").ok_or(
            "RUNTARA_ROLE=intake needs RUNTARA_ENGINE_URL: where the engine listens, as \
             https://host:port",
        )?,
        health: address("RUNTARA_INTAKE_HEALTH_LISTEN")?,
    })
}

/// `RUNTARA_INTAKE_URL`, for the API: where intake listens, if what anyone
/// may send goes to a process of its own and not to the engine.
pub fn intake_url() -> Option<String> {
    set("RUNTARA_INTAKE_URL")
}

/// Which of what the API role is not to be given this process was given.
pub fn given_to_the_api_role() -> Vec<&'static str> {
    NOT_FOR_THE_API
        .iter()
        .copied()
        .filter(|name| set(name).is_some())
        .collect()
}

/// Starts the server's background work, each piece for the role it belongs
/// to. Work for a role this process does not run is not started.
pub struct Workers {
    roles: Roles,
    started: Mutex<Vec<(Role, &'static str)>>,
}

impl Workers {
    pub fn new(roles: Roles) -> Self {
        Self {
            roles,
            started: Mutex::new(Vec::new()),
        }
    }

    fn starts(&self, role: Role, name: &'static str) -> bool {
        if !self.roles.runs(role) {
            return false;
        }
        self.started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((role, name));
        true
    }

    /// Starts work that stops itself on the shutdown flag, and is waited for
    /// when the server stops.
    pub fn intake<F>(&self, role: Role, name: &'static str, shutdown: &ShutdownCoordinator, work: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.starts(role, name) {
            shutdown.spawn_intake(work);
        }
    }

    /// Starts work that runs until the process ends, in the scope it is
    /// started from.
    pub fn detached<F>(&self, role: Role, name: &'static str, work: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.starts(role, name) {
            runtara_store_postgres::tenant_scope::spawn(work);
        }
    }

    /// Says in the log what was started, for each role.
    pub fn log_started(&self) {
        let names = |role: Role| {
            let mut names: Vec<&str> = self
                .started()
                .into_iter()
                .filter(|(started, _)| *started == role)
                .map(|(_, name)| name)
                .collect();
            names.sort_unstable();
            names.dedup();
            names.join(", ")
        };
        tracing::info!(
            engine = %names(Role::Engine),
            api = %names(Role::Api),
            intake = %names(Role::Intake),
            "Background work started"
        );
    }

    /// What was started, by role and name. A piece started once per tenant
    /// is named once per tenant.
    pub fn started(&self) -> Vec<(Role, &'static str)> {
        self.started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
mod tests {

    /// The evaluation stack's overlay (`docker-compose.roles.yml`) carries
    /// what the API's database role and Valkey user are granted, because it
    /// is one file that runs on its own. They are the two files under
    /// `scripts/roles`, as they are.
    #[test]
    fn the_compose_overlay_grants_what_the_role_files_grant() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
        let overlay = read("docker-compose.roles.yml");
        // A config's content, as compose hands it over: its block, without
        // the indentation, and with `$$` for `$`.
        let content_of = |name: &str| -> String {
            let start = format!("  {name}:\n    content: |\n");
            let from = overlay
                .find(&start)
                .unwrap_or_else(|| panic!("no config {name}"))
                + start.len();
            overlay[from..]
                .lines()
                .take_while(|line| line.is_empty() || line.starts_with("      "))
                .map(|line| line.strip_prefix("      ").unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n")
                .replace("$$", "$")
        };

        assert_eq!(
            content_of("api-database-role").trim_end(),
            read("scripts/roles/api-database-role.sql").trim_end(),
            "docker-compose.roles.yml and scripts/roles/api-database-role.sql differ"
        );

        let rules = read("scripts/roles/api-valkey-user.acl")
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect::<Vec<_>>()
            .join(" ");
        let users = content_of("valkey-users");
        let api = users
            .lines()
            .find(|line| line.starts_with("user runtara-api "))
            .expect("the API's Valkey user");
        assert_eq!(
            api,
            format!("user runtara-api on >runtara_api {rules}"),
            "docker-compose.roles.yml and scripts/roles/api-valkey-user.acl differ"
        );
    }
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_process_runs_both_roles_or_one() {
        assert!(Roles::all().runs(Role::Api) && Roles::all().runs(Role::Engine));
        assert!(Roles::only(Role::Api).runs(Role::Api));
        assert!(!Roles::only(Role::Api).runs(Role::Engine));
        assert!(Roles::only(Role::Engine).runs(Role::Engine));
        assert!(!Roles::only(Role::Engine).runs(Role::Api));
    }

    #[test]
    fn a_role_is_named_or_is_both() {
        assert_eq!(Roles::named(None), Ok(Roles::all()));
        assert_eq!(Roles::named(Some("")), Ok(Roles::all()));
        assert_eq!(Roles::named(Some("all")), Ok(Roles::all()));
        assert_eq!(Roles::named(Some(" api ")), Ok(Roles::only(Role::Api)));
        assert_eq!(Roles::named(Some("engine")), Ok(Roles::only(Role::Engine)));
        assert!(Roles::named(Some("both")).is_err());
    }

    #[tokio::test]
    async fn work_is_started_only_for_a_role_the_process_runs() {
        let ran = Arc::new(AtomicUsize::new(0));
        let count = |ran: &Arc<AtomicUsize>| {
            let ran = ran.clone();
            async move {
                ran.fetch_add(1, Ordering::SeqCst);
            }
        };
        let shutdown = ShutdownCoordinator::new(crate::config::ShutdownGrace {
            executions: std::time::Duration::from_secs(1),
            intake: std::time::Duration::from_secs(1),
        });

        let api = Workers::new(Roles::only(Role::Api));
        api.detached(Role::Api, "its own", count(&ran));
        api.detached(Role::Engine, "the engine's", count(&ran));
        api.intake(Role::Engine, "the engine's, joined", &shutdown, count(&ran));
        assert_eq!(api.started(), vec![(Role::Api, "its own")]);

        let both = Workers::new(Roles::all());
        both.detached(Role::Engine, "the engine's", count(&ran));
        both.intake(Role::Api, "the API's, joined", &shutdown, count(&ran));
        assert_eq!(
            both.started(),
            vec![
                (Role::Engine, "the engine's"),
                (Role::Api, "the API's, joined")
            ]
        );

        for _ in 0..200 {
            if ran.load(Ordering::SeqCst) == 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            ran.load(Ordering::SeqCst),
            3,
            "the three that were started ran"
        );
    }
}
