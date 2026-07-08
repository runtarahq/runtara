//! Default rate limit configurations per connection type (integration_id)
//!
//! These defaults are applied when creating a connection that has no explicit
//! rate_limit_config. They are also exposed via the connection types API
//! so the frontend can show the defaults in the UI.
//!
//! Defaults are keyed on the *target provider/API*, not the credential-flow
//! `integration_id`: two credential flows that hit the same API (e.g. Shopify
//! `access_token` vs `client_credentials`, or HubSpot `private_app` vs
//! `access_token`) share one default.
//!
//! Every `integration_id` that has a registered `HttpConnectionExtractor`
//! (i.e. its egress flows through the internal proxy where `check_rate_limit`
//! runs) MUST be either given a default here or listed in [`RATE_LIMIT_OPT_OUT`].
//! The `every_http_extractor_has_a_default_or_explicit_opt_out` test enforces
//! this, so a newly-registered integration can never silently ship unlimited.

use crate::types::RateLimitConfigDto;

/// `integration_id`s that intentionally have **no** default rate limit.
///
/// Listed explicitly (rather than falling through to `None`) so the coverage
/// test can distinguish "deliberately unlimited" from "forgot to decide".
pub const RATE_LIMIT_OPT_OUT: &[&str] = &[
    // Generic HTTP connections: the target API and its limits are unknown, so a
    // default would surprise-throttle legitimate high-throughput integrations.
    // Honesty about the unprotected state is handled in the UI, not a covert
    // backstop (see SYN-495).
    "http_api_key",
    "http_bearer",
    "http_oauth2_client_credentials",
    "http_oauth2_authorization_code",
    // Arbitrary user-provided MCP server — target limits unknowable.
    "mcp",
    // OAuth client-credentials token-mint endpoint only; the throttled Graph/API
    // calls ride other connections, so a limit here would throttle auth, not the
    // workload.
    "microsoft_entra_client_credentials",
];

/// Get the default rate limit configuration for a given integration_id.
///
/// Returns `None` for connection types where rate limiting doesn't apply
/// (e.g., database connections), where egress does not traverse the proxy
/// (e.g., native SFTP/postgres, presigned S3/Azure), or where the target API's
/// limits are unknown (see [`RATE_LIMIT_OPT_OUT`]).
pub fn get_default_rate_limit_config(integration_id: &str) -> Option<RateLimitConfigDto> {
    match integration_id {
        // Shopify Admin API: bucket-based throttling (~50 cost points/sec for GraphQL).
        // 2 req/s with burst of 4 is conservative and avoids 429s. Both credential
        // flows hit the same Admin API.
        "shopify_access_token" | "shopify_client_credentials" => Some(RateLimitConfigDto {
            requests_per_second: 2,
            burst_size: 4,
            retry_on_limit: true,
            max_retries: 3,
            max_wait_ms: 60000,
        }),
        // OpenAI API: tiered rate limits vary by model and account tier.
        // 5 req/s with burst of 10 is a safe starting point for most tiers.
        "openai_api_key" => Some(RateLimitConfigDto {
            requests_per_second: 5,
            burst_size: 10,
            retry_on_limit: true,
            max_retries: 3,
            max_wait_ms: 60000,
        }),
        // AWS Bedrock: per-model invocation throttling.
        // 2 req/s with burst of 5 is conservative for most models.
        "aws_credentials" => Some(RateLimitConfigDto {
            requests_per_second: 2,
            burst_size: 5,
            retry_on_limit: true,
            max_retries: 3,
            max_wait_ms: 60000,
        }),
        // Stripe API: ~100 req/s in live mode (25 in test). 20 req/s with burst 40
        // is well under the live cap and avoids tripping the limiter on bursts.
        "stripe_api_key" => Some(RateLimitConfigDto {
            requests_per_second: 20,
            burst_size: 40,
            retry_on_limit: true,
            max_retries: 3,
            max_wait_ms: 60000,
        }),
        // HubSpot: ~100 requests / 10s for most private-app & OAuth tiers (=10/s),
        // plus daily caps. 8 req/s with burst 15 stays under the rolling window.
        // Both credential flows hit the same CRM API.
        "hubspot_access_token" | "hubspot_private_app" => Some(RateLimitConfigDto {
            requests_per_second: 8,
            burst_size: 15,
            retry_on_limit: true,
            max_retries: 3,
            max_wait_ms: 60000,
        }),
        // Mailgun: per-plan sending/API limits vary; 5 req/s with burst 10 is a
        // conservative floor that suits transactional sending without 429s.
        "mailgun" => Some(RateLimitConfigDto {
            requests_per_second: 5,
            burst_size: 10,
            retry_on_limit: true,
            max_retries: 3,
            max_wait_ms: 60000,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shopify_defaults() {
        let config = get_default_rate_limit_config("shopify_access_token").unwrap();
        assert_eq!(config.requests_per_second, 2);
        assert_eq!(config.burst_size, 4);
        assert!(config.retry_on_limit);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.max_wait_ms, 60000);
    }

    #[test]
    fn test_openai_defaults() {
        let config = get_default_rate_limit_config("openai_api_key").unwrap();
        assert_eq!(config.requests_per_second, 5);
        assert_eq!(config.burst_size, 10);
    }

    #[test]
    fn test_aws_defaults() {
        let config = get_default_rate_limit_config("aws_credentials").unwrap();
        assert_eq!(config.requests_per_second, 2);
        assert_eq!(config.burst_size, 5);
    }

    #[test]
    fn test_stripe_defaults() {
        let config = get_default_rate_limit_config("stripe_api_key").unwrap();
        assert_eq!(config.requests_per_second, 20);
        assert_eq!(config.burst_size, 40);
    }

    #[test]
    fn test_hubspot_defaults() {
        let config = get_default_rate_limit_config("hubspot_access_token").unwrap();
        assert_eq!(config.requests_per_second, 8);
        assert_eq!(config.burst_size, 15);
    }

    #[test]
    fn test_mailgun_defaults() {
        let config = get_default_rate_limit_config("mailgun").unwrap();
        assert_eq!(config.requests_per_second, 5);
        assert_eq!(config.burst_size, 10);
    }

    /// Credential flows that hit the same target API must share a default.
    #[test]
    fn test_same_api_credential_flows_share_a_default() {
        assert_eq!(
            get_default_rate_limit_config("shopify_access_token"),
            get_default_rate_limit_config("shopify_client_credentials"),
        );
        assert_eq!(
            get_default_rate_limit_config("hubspot_access_token"),
            get_default_rate_limit_config("hubspot_private_app"),
        );
    }

    #[test]
    fn test_no_defaults_for_non_proxy_types() {
        // Native socket / presigned egress never traverses the proxy, so a
        // rate_limit_config could not be enforced even if set.
        assert!(get_default_rate_limit_config("sftp").is_none());
        assert!(get_default_rate_limit_config("postgres").is_none());
    }

    #[test]
    fn test_opt_out_types_return_none() {
        for id in RATE_LIMIT_OPT_OUT {
            assert!(
                get_default_rate_limit_config(id).is_none(),
                "opt-out id '{id}' must not also have a default",
            );
        }
    }

    #[test]
    fn test_unknown_integration_returns_none() {
        assert!(get_default_rate_limit_config("nonexistent").is_none());
    }

    /// Every default must be enforceable: a positive refill rate, and a burst
    /// capacity at least as large as the refill rate. (`requests_per_second == 0`
    /// is treated as "no limit" by the limiter, so it would be a silent bypass.)
    #[test]
    fn test_all_defaults_are_enforceable() {
        for id in runtara_agents::extractors::get_http_extractor_ids() {
            if let Some(cfg) = get_default_rate_limit_config(id) {
                assert!(
                    cfg.requests_per_second >= 1,
                    "{id}: requests_per_second must be >= 1 (0 silently disables enforcement)",
                );
                assert!(
                    cfg.burst_size >= cfg.requests_per_second,
                    "{id}: burst_size ({}) must be >= requests_per_second ({})",
                    cfg.burst_size,
                    cfg.requests_per_second,
                );
            }
        }
    }

    /// Opt-out entries must be real, currently-registered HTTP extractor ids —
    /// guards against typos and stale entries drifting out of sync.
    #[test]
    fn test_opt_out_entries_are_registered_extractors() {
        let registered = runtara_agents::extractors::get_http_extractor_ids();
        for id in RATE_LIMIT_OPT_OUT {
            assert!(
                registered.contains(id),
                "RATE_LIMIT_OPT_OUT entry '{id}' is not a registered HTTP extractor id \
                 (typo, or the extractor was removed) — remove it from the allowlist",
            );
        }
    }

    /// The core guarantee of SYN-493: every connection type whose egress flows
    /// through the proxy (where `check_rate_limit` runs) must have an explicit
    /// disposition — either a default or a deliberate opt-out. A new integration
    /// can never silently land with unlimited egress.
    #[test]
    fn every_http_extractor_has_a_default_or_explicit_opt_out() {
        for id in runtara_agents::extractors::get_http_extractor_ids() {
            let has_default = get_default_rate_limit_config(id).is_some();
            let opted_out = RATE_LIMIT_OPT_OUT.contains(&id);
            assert!(
                has_default ^ opted_out,
                "integration_id '{id}': must have EITHER a rate-limit default OR be on \
                 RATE_LIMIT_OPT_OUT (exactly one, never both/neither). A new HTTP integration \
                 was registered without deciding its rate-limit default — add one in \
                 get_default_rate_limit_config or, if the target's limits are genuinely \
                 unknowable, add it to RATE_LIMIT_OPT_OUT with a rationale.",
            );
        }
    }
}
