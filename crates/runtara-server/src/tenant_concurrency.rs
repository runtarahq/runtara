// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Per-tenant limits on concurrent requests (M7.16): in a process serving
//! several tenants, one tenant's slow or numerous requests must not hold
//! every connection and database slot it has. With
//! `RUNTARA_TENANT_MAX_CONCURRENT_REQUESTS`, each tenant may have that many
//! authenticated requests in flight here; one more is refused at once with
//! `429`, `TENANT_BUSY` and `Retry-After`, while other tenants are served.
//! Long polls and streams hold their slot while they last, so the limit is
//! meant generous. Unset, nothing is limited.
//!
//! The limit is this process's: a tenant served by several processes may
//! have it in each.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

struct Limits {
    per_tenant: usize,
    tenants: Mutex<HashMap<String, Arc<Semaphore>>>,
}

fn limits() -> Option<&'static Limits> {
    static LIMITS: OnceLock<Option<Limits>> = OnceLock::new();
    LIMITS
        .get_or_init(|| {
            let per_tenant = std::env::var("RUNTARA_TENANT_MAX_CONCURRENT_REQUESTS")
                .ok()?
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|limit| *limit > 0)?;
            Some(Limits {
                per_tenant,
                tenants: Mutex::default(),
            })
        })
        .as_ref()
}

/// A slot for one of `tenant`'s requests, held while it runs: `Ok(None)`
/// when nothing is limited, `Err` when the tenant has none free.
pub fn enter(tenant: &str) -> Result<Option<OwnedSemaphorePermit>, Box<Response>> {
    let Some(limits) = limits() else {
        return Ok(None);
    };
    enter_with(limits, tenant)
}

fn enter_with(
    limits: &Limits,
    tenant: &str,
) -> Result<Option<OwnedSemaphorePermit>, Box<Response>> {
    let semaphore = limits
        .tenants
        .lock()
        .expect("tenant concurrency poisoned")
        .entry(tenant.to_owned())
        .or_insert_with(|| Arc::new(Semaphore::new(limits.per_tenant)))
        .clone();
    semaphore
        .try_acquire_owned()
        .map(Some)
        .map_err(|_| Box::new(busy()))
}

fn busy() -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({
            "success": false,
            "error": "TENANT_BUSY",
            "message": "this tenant has as many requests in flight as it may; retry shortly",
        })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tenant_at_its_limit_is_refused_while_another_is_served() {
        let limits = Limits {
            per_tenant: 2,
            tenants: Mutex::default(),
        };
        let first = enter_with(&limits, "acme").unwrap();
        let second = enter_with(&limits, "acme").unwrap();
        let refused = enter_with(&limits, "acme").unwrap_err();
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.headers()[header::RETRY_AFTER], "1");
        assert!(
            enter_with(&limits, "beta").is_ok(),
            "another tenant is served"
        );
        drop(first);
        assert!(enter_with(&limits, "acme").is_ok(), "a slot came free");
        drop(second);
    }
}
