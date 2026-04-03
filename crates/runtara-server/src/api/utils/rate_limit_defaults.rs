//! Default rate limit configurations per connection type (integration_id)
//!
//! These defaults are applied when creating a connection that has no explicit
//! rate_limit_config. They are also exposed via the connection types API
//! so the frontend can show the defaults in the UI.

use crate::api::dto::rate_limits::RateLimitConfigDto;

/// Get the default rate limit configuration for a given integration_id.
///
/// Returns `None` for connection types where rate limiting doesn't apply
/// (e.g., database connections) or where the target API's limits are unknown
/// (e.g., generic HTTP connections).
pub fn get_default_rate_limit_config(integration_id: &str) -> Option<RateLimitConfigDto> {
    match integration_id {
        // Shopify Admin API: bucket-based throttling (~50 cost points/sec for GraphQL).
        // 2 req/s with burst of 4 is conservative and avoids 429s.
        "shopify_access_token" => Some(RateLimitConfigDto {
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
    fn test_no_defaults_for_generic_types() {
        assert!(get_default_rate_limit_config("sftp").is_none());
        assert!(get_default_rate_limit_config("http_bearer").is_none());
        assert!(get_default_rate_limit_config("postgres").is_none());
    }

    #[test]
    fn test_unknown_integration_returns_none() {
        assert!(get_default_rate_limit_config("nonexistent").is_none());
    }
}
