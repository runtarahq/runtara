//! Whether a tenant may start work now (C11 suspension, SYN-916): with
//! `RUNTARA_AUTH_TENANT_STATUS=required`, a new run of any kind (an API
//! execute, a webhook, a cron tick, a channel message, a child run) is
//! admitted only while the tenant's status record says `ready`. Authenticated
//! requests are refused earlier, in the auth middleware.
//!
//! A status is cached for a few seconds, since admission runs per run. An
//! unreadable status fails closed.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use redis::aio::ConnectionManager;

use crate::entitlement_error::EntitlementDenial;

static MANAGER: OnceLock<ConnectionManager> = OnceLock::new();
static CACHE: OnceLock<DashMap<String, (Instant, bool)>> = OnceLock::new();

/// How long a read status is trusted.
const FRESH_FOR: Duration = Duration::from_secs(5);

/// Read statuses from `manager`'s Valkey. Set once, at boot.
pub fn install(manager: ConnectionManager) {
    let _ = MANAGER.set(manager);
}

/// Admit new work for `tenant`, or say why not.
pub async fn admit(tenant: &str) -> Result<(), EntitlementDenial> {
    if !crate::config::tenant_status_required() {
        return Ok(());
    }
    let cache = CACHE.get_or_init(DashMap::new);
    if let Some(entry) = cache.get(tenant)
        && entry.0.elapsed() < FRESH_FOR
    {
        return if entry.1 {
            Ok(())
        } else {
            Err(EntitlementDenial::TenantNotReady)
        };
    }
    let Some(manager) = MANAGER.get() else {
        tracing::error!(tenant_id = %tenant, "tenant status required but Valkey is not configured");
        return Err(EntitlementDenial::TenantStatusUnavailable);
    };
    match crate::valkey::auth::tenant_is_ready(manager, tenant).await {
        Ok(ready) => {
            cache.insert(tenant.to_owned(), (Instant::now(), ready));
            if ready {
                Ok(())
            } else {
                Err(EntitlementDenial::TenantNotReady)
            }
        }
        Err(error) => {
            tracing::error!(tenant_id = %tenant, detail = %error, "tenant status lookup failed");
            Err(EntitlementDenial::TenantStatusUnavailable)
        }
    }
}
