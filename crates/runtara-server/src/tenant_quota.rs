//! Per-tenant rate limits from the tenant's entitlements: `apiRequestsPerMinute`
//! for authenticated API requests, `compilationsPerHour` for requested
//! compilations, and `workflowCallsPerMinute` for the calls runs make that
//! start external work, which wait for a token rather than being refused.
//!
//! Each is a token bucket in Valkey, `{tenant}:quota:{name}`, shared by every
//! process serving the tenant: it holds the limit's worth of tokens and refills
//! at the limit's rate, so a tenant may burst to its limit and then continues
//! at that rate. A request without a token is refused with `429` and
//! `Retry-After`.
//!
//! Without Valkey, or when it errors, the request goes through: a Valkey outage
//! must not refuse every tenant, which is how connection rate limits behave
//! too. Each such pass is logged as a warning.

use std::sync::OnceLock;

use axum::Json;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use redis::aio::ConnectionManager;
use serde_json::json;

use crate::entitlements::EntitlementLimits;

static MANAGER: OnceLock<ConnectionManager> = OnceLock::new();

/// Keep the buckets in `manager`'s Valkey. Set once, at boot.
pub fn install(manager: ConnectionManager) {
    let _ = MANAGER.set(manager);
}

/// What a tenant is limited in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quota {
    /// Authenticated API requests (`apiRequestsPerMinute`).
    Api,
    /// Requested compilations (`compilationsPerHour`).
    Compile,
    /// Runs' calls that start external work (`workflowCallsPerMinute`).
    WorkflowCalls,
}

impl Quota {
    /// The name in the bucket's key and in a refusal.
    pub const fn name(self) -> &'static str {
        match self {
            Quota::Api => "api",
            Quota::Compile => "compile",
            Quota::WorkflowCalls => "workflow_calls",
        }
    }

    /// The tenant's limit as tokens per window in milliseconds, if it has one.
    fn limit(self, limits: &EntitlementLimits) -> Option<(u32, u64)> {
        let (tokens, window_ms) = match self {
            Quota::Api => (limits.api_requests_per_minute?, 60_000),
            Quota::Compile => (limits.compilations_per_hour?, 3_600_000),
            Quota::WorkflowCalls => (limits.workflow_calls_per_minute?, 60_000),
        };
        Some((tokens, window_ms))
    }
}

fn key(tenant: &str, quota: Quota) -> String {
    format!("{tenant}:quota:{}", quota.name())
}

/// Takes one token, or answers how many milliseconds until one is there.
/// The bucket expires once it would be full again. Only commands a tenant's
/// Valkey user may run (no `PEXPIRE`), since its ACL applies inside scripts.
const TAKE: &str = r#"
local capacity = tonumber(ARGV[1])
local window_ms = tonumber(ARGV[2])
local now_ms = tonumber(ARGV[3])
-- In token-milliseconds, so the arithmetic is exact: a token is window_ms of
-- them, and capacity of them come back every millisecond.
local full = capacity * window_ms
local level = tonumber(redis.call('HGET', KEYS[1], 'level') or full)
local at = tonumber(redis.call('HGET', KEYS[1], 'at') or now_ms)
level = math.min(full, level + math.max(0, now_ms - at) * capacity)
if level < window_ms then
  return -math.ceil((window_ms - level) / capacity)
end
level = level - window_ms
redis.call('HSET', KEYS[1], 'level', string.format('%d', level), 'at', string.format('%d', now_ms))
redis.call('EXPIRE', KEYS[1], math.ceil((full - level) / capacity / 1000))
return 1
"#;

/// Takes one of `tenant`'s `quota` tokens. `Err` carries how long until the
/// next one, in milliseconds.
pub async fn take(tenant: &str, quota: Quota) -> Result<(), u64> {
    let entitlements = crate::config::entitlements_for(tenant);
    let Some((capacity, window_ms)) = quota.limit(&entitlements.limits) else {
        return Ok(());
    };
    if capacity == 0 {
        return Err(window_ms);
    }
    let Some(manager) = MANAGER.get() else {
        tracing::warn!(tenant_id = %tenant, quota = quota.name(), "no Valkey for tenant rate limits; allowing the request");
        return Ok(());
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let taken = take_from(
        &mut manager.clone(),
        &key(tenant, quota),
        (capacity, window_ms),
        now_ms,
    )
    .await;
    match taken {
        Ok(answer) if answer > 0 => Ok(()),
        Ok(answer) => Err(answer.unsigned_abs()),
        Err(error) => {
            tracing::warn!(tenant_id = %tenant, quota = quota.name(), detail = %error, "tenant rate limit unavailable; allowing the request");
            Ok(())
        }
    }
}

/// Runs [`TAKE`] on the bucket at `key`: positive when a token was taken,
/// else minus the milliseconds until one is there.
async fn take_from(
    valkey: &mut ConnectionManager,
    key: &str,
    (capacity, window_ms): (u32, u64),
    now_ms: u64,
) -> redis::RedisResult<i64> {
    redis::Script::new(TAKE)
        .key(key)
        .arg(capacity)
        .arg(window_ms)
        .arg(now_ms)
        .invoke_async(valkey)
        .await
}

/// Refuses a compilation request over the tenant's `compilationsPerHour`.
/// Runs after authentication, which names the tenant.
pub async fn limit_compilations(request: Request, next: Next) -> Response {
    let tenant = request
        .extensions()
        .get::<crate::auth::AuthContext>()
        .map(|context| context.org_id.clone());
    if let Some(tenant) = tenant
        && let Err(retry_after_ms) = take(&tenant, Quota::Compile).await
    {
        return refusal(Quota::Compile, retry_after_ms);
    }
    next.run(request).await
}

/// Paces runs' calls that start external work by their tenant's
/// `workflowCallsPerMinute`: such a call waits for a token instead of being
/// refused, so a run over the limit slows down rather than fails.
pub struct WorkflowCallLimiter;

/// The longest one wait for a token lasts before it is tried again.
const MAX_WAIT_MS: u64 = 5_000;

#[async_trait::async_trait]
impl runtara_workflow_api::server::CallLimiter for WorkflowCallLimiter {
    async fn admit(&self, binding: &runtara_workflow_api::Binding) {
        while let Err(retry_after_ms) = take(&binding.tenant_id, Quota::WorkflowCalls).await {
            tokio::time::sleep(std::time::Duration::from_millis(
                retry_after_ms.clamp(1, MAX_WAIT_MS),
            ))
            .await;
        }
    }
}

/// The refusal of a request over `quota`, retryable after `retry_after_ms`.
pub fn refusal(quota: Quota, retry_after_ms: u64) -> Response {
    let seconds = retry_after_ms.div_ceil(1000).max(1);
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({
            "error": "TENANT_RATE_LIMITED",
            "quota": quota.name(),
            "retryAfterSeconds": seconds,
            "message": format!("this tenant's {} rate limit is reached", quota.name()),
        })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_quota_reads_its_own_limit_over_its_own_window() {
        let limits = EntitlementLimits {
            api_requests_per_minute: Some(5),
            compilations_per_hour: Some(2),
            ..Default::default()
        };
        assert_eq!(Quota::Api.limit(&limits), Some((5, 60_000)));
        assert_eq!(Quota::Compile.limit(&limits), Some((2, 3_600_000)));
        assert_eq!(Quota::WorkflowCalls.limit(&limits), None);
        assert_eq!(Quota::Api.limit(&EntitlementLimits::default()), None);
        assert_eq!(key("t1", Quota::Compile), "t1:quota:compile");
    }

    /// Against a Valkey at `RUNTARA_TEST_VALKEY_URL`; skipped without one.
    #[tokio::test]
    async fn a_bucket_bursts_to_its_limit_then_refills_at_its_rate() {
        let Ok(url) = std::env::var("RUNTARA_TEST_VALKEY_URL") else {
            eprintln!("skipped: RUNTARA_TEST_VALKEY_URL is not set");
            return;
        };
        let client = redis::Client::open(url).unwrap();
        let mut valkey = ConnectionManager::new(client).await.unwrap();
        let key = format!("quota-test-{}:quota:api", uuid::Uuid::new_v4().simple());
        let limit = (2, 60_000);
        let now = 1_000_000;
        assert_eq!(take_from(&mut valkey, &key, limit, now).await.unwrap(), 1);
        assert_eq!(take_from(&mut valkey, &key, limit, now).await.unwrap(), 1);
        // One token comes back every 30 s.
        assert_eq!(
            take_from(&mut valkey, &key, limit, now).await.unwrap(),
            -30_000
        );
        assert_eq!(
            take_from(&mut valkey, &key, limit, now + 20_000)
                .await
                .unwrap(),
            -10_000
        );
        assert_eq!(
            take_from(&mut valkey, &key, limit, now + 30_000)
                .await
                .unwrap(),
            1
        );
        let ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut valkey)
            .await
            .unwrap();
        assert!((1..=60).contains(&ttl), "expires once full again: {ttl}");
        let _: () = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut valkey)
            .await
            .unwrap();
    }

    #[test]
    fn a_refusal_says_when_to_retry_in_whole_seconds() {
        let response = refusal(Quota::Api, 1_200);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "2");
        assert_eq!(refusal(Quota::Api, 10).headers()[header::RETRY_AFTER], "1");
    }
}
