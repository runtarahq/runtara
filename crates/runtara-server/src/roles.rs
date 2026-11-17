// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The server's two roles, and which background work is whose.
//!
//! The API takes requests; the engine owns durable execution and everything
//! done with a connection's credentials (see [`crate::engine`]). One process
//! runs both today. Each piece of background work is started for the role it
//! belongs to, so that a process that runs one role starts that role's work
//! and none of the other's.

use std::future::Future;
use std::sync::Mutex;

use crate::shutdown::ShutdownCoordinator;

/// One of the server's two roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Takes requests: handlers, MCP, and what keeps them answering.
    Api,
    /// Durable execution, and the use of connections' credentials.
    Engine,
}

/// The roles a process runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Roles {
    api: bool,
    engine: bool,
}

impl Roles {
    /// Both roles, in one process.
    pub const fn all() -> Self {
        Self {
            api: true,
            engine: true,
        }
    }

    /// One role alone.
    pub const fn only(role: Role) -> Self {
        Self {
            api: matches!(role, Role::Api),
            engine: matches!(role, Role::Engine),
        }
    }

    /// Whether this process runs `role`.
    pub const fn runs(self, role: Role) -> bool {
        match role {
            Role::Api => self.api,
            Role::Engine => self.engine,
        }
    }
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
