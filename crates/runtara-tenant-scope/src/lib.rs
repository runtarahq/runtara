// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Whose rows a piece of work may reach (C11).
//!
//! Work runs in one of two scopes. Inside [`scoped`] it is one tenant's;
//! inside [`system`] it is the platform's own, across every tenant: a worker
//! that sweeps all of them, or the lookup that finds which tenant an
//! unauthenticated request is for. Work in neither is nobody's, and a
//! database pool that names the scope on its connections
//! (`runtara_store_postgres::tenant_scope::scoping`) has the database refuse
//! it every tenant's rows.
//!
//! The scope is a task-local: work spawned onto another task
//! (`tokio::spawn`, `spawn_blocking`) leaves it, and is in no scope unless it
//! is given one ([`spawn`], [`inherit`], [`scoped`], [`system`]).
//!
//! This crate holds the scope and nothing else, so that a crate with no
//! database of its own can still keep the scope of the work it spawns.

use std::future::Future;

/// Whose rows the current work may reach.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Scope {
    /// One tenant's.
    Tenant(String),
    /// Every tenant's: the platform's own work.
    System,
}

tokio::task_local! {
    static SCOPE: Scope;
}

/// Runs `work` for `tenant`: connections it takes from a pool
/// that names the scope see only that tenant's rows.
pub async fn scoped<F: Future>(tenant: impl Into<String>, work: F) -> F::Output {
    SCOPE.scope(Scope::Tenant(tenant.into()), work).await
}

/// Runs `work` as the platform's own: connections it takes from a pool
/// that names the scope see every tenant's rows. For work that is no one tenant's
/// (start-up, a worker over all tenants, finding the tenant of a request that
/// carries only an id); anything done for a tenant belongs in [`scoped`],
/// which may be entered from inside this.
pub async fn system<F: Future>(work: F) -> F::Output {
    SCOPE.scope(Scope::System, work).await
}

/// Runs `work` that finds which tenant something belongs to, from an id that
/// is all a caller presented (an API key, a webhook's trigger): in the scope
/// already entered, if there is one, and otherwise across tenants, as the
/// platform. What is then done for the tenant found belongs in [`scoped`].
pub async fn locating<F: Future>(work: F) -> F::Output {
    if SCOPE.try_with(|_| ()).is_ok() {
        work.await
    } else {
        system(work).await
    }
}

/// The tenant of the current scope, if it is one tenant's.
pub fn current() -> Option<String> {
    match SCOPE.try_with(Clone::clone) {
        Ok(Scope::Tenant(tenant)) => Some(tenant),
        Ok(Scope::System) | Err(_) => None,
    }
}

/// Whether the current work is the platform's own ([`system`]).
pub fn is_system() -> bool {
    SCOPE
        .try_with(|scope| *scope == Scope::System)
        .unwrap_or(false)
}

/// `work` in the current scope, wherever it is later run: for work spawned
/// onto another task, which would otherwise leave the scope.
pub fn inherit<F: Future>(work: F) -> impl Future<Output = F::Output> {
    let scope = SCOPE.try_with(Clone::clone).ok();
    async move {
        match scope {
            Some(scope) => SCOPE.scope(scope, work).await,
            None => work.await,
        }
    }
}

/// Spawns `work` in the current scope: `tokio::spawn`, for work that reaches
/// the database and is part of what spawned it.
pub fn spawn<F>(work: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(inherit(work))
}

/// The scope the current work is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Named {
    /// One tenant's.
    Tenant(String),
    /// The platform's own, across every tenant.
    System,
    /// None.
    Nobody,
}

/// The scope the current work is in, for whatever enforces it.
pub fn named() -> Named {
    match SCOPE.try_with(Clone::clone) {
        Ok(Scope::Tenant(tenant)) => Named::Tenant(tenant),
        Ok(Scope::System) => Named::System,
        Err(_) => Named::Nobody,
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[tokio::test]
    async fn spawned_work_keeps_the_scope_only_when_it_inherits_it() {
        let (kept, left) = scoped("acme", async {
            let kept = tokio::spawn(inherit(async { current() }));
            let left = tokio::spawn(async { current() });
            (kept.await.unwrap(), left.await.unwrap())
        })
        .await;
        assert_eq!(kept.as_deref(), Some("acme"));
        assert_eq!(left, None);
        // Outside any scope there is nothing to inherit.
        assert_eq!(
            tokio::spawn(inherit(async { current() })).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn the_platforms_scope_is_no_tenants_and_is_inherited() {
        assert!(!is_system());
        let (inside, tenant, inherited, left) = system(async {
            let inherited = tokio::spawn(inherit(async { is_system() }));
            let left = tokio::spawn(async { is_system() });
            (
                is_system(),
                current(),
                inherited.await.unwrap(),
                left.await.unwrap(),
            )
        })
        .await;
        assert!(inside);
        assert_eq!(tenant, None);
        assert!(inherited);
        assert!(!left, "spawned work is in no scope unless it is given one");
    }

    #[tokio::test]
    async fn the_innermost_scope_is_the_one_in_force() {
        let tenant_inside_system = system(scoped("acme", async { (current(), is_system()) })).await;
        assert_eq!(tenant_inside_system, (Some("acme".to_owned()), false));
        let system_inside_tenant = scoped("acme", system(async { (current(), is_system()) })).await;
        assert_eq!(system_inside_tenant, (None, true));
    }
}
