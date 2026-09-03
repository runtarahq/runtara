// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Typed, bounded execution-timeout policy.
//!
//! An execution timeout is a safety deadline for one active guest launch. It
//! is deliberately distinct from a durable workflow's total lifetime: a
//! parked approval holds no runner permit and can wait indefinitely, while a
//! running guest must always have a finite deadline.

use std::time::Duration;

use serde_json::Value;
use thiserror::Error;

/// The default maximum active execution time, in seconds.
///
/// This is a product hard limit. An operator may choose a lower maximum in an
/// [`ExecutionTimeoutPolicy`], but cannot configure a larger active deadline.
pub const MAX_EXECUTION_TIMEOUT_SECS: u32 = 3_600;

/// The default active execution time when a workflow definition names none.
pub const DEFAULT_EXECUTION_TIMEOUT_SECS: u32 = 300;

/// A validated positive execution-timeout duration measured in whole seconds.
///
/// This newtype is the boundary that prevents a raw API/database number from
/// becoming a `Duration` through a narrowing cast. Values are always within
/// the product hard maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExecutionTimeoutSeconds(u32);

impl ExecutionTimeoutSeconds {
    /// Construct a timeout from whole seconds, enforcing the product bounds.
    pub fn new(seconds: u32) -> Result<Self, ExecutionTimeoutError> {
        if seconds == 0 {
            return Err(ExecutionTimeoutError::MustBePositive);
        }
        if seconds > MAX_EXECUTION_TIMEOUT_SECS {
            return Err(ExecutionTimeoutError::ExceedsMaximum {
                seconds: u64::from(seconds),
                maximum_seconds: MAX_EXECUTION_TIMEOUT_SECS,
            });
        }
        Ok(Self(seconds))
    }

    /// Return the timeout in seconds.
    pub const fn as_secs(self) -> u32 {
        self.0
    }

    /// Return the timeout as a [`Duration`].
    pub const fn as_duration(self) -> Duration {
        Duration::from_secs(self.0 as u64)
    }
}

impl TryFrom<u64> for ExecutionTimeoutSeconds {
    type Error = ExecutionTimeoutError;

    fn try_from(seconds: u64) -> Result<Self, Self::Error> {
        let seconds =
            u32::try_from(seconds).map_err(|_| ExecutionTimeoutError::ExceedsMaximum {
                seconds,
                maximum_seconds: MAX_EXECUTION_TIMEOUT_SECS,
            })?;
        Self::new(seconds)
    }
}

impl TryFrom<i64> for ExecutionTimeoutSeconds {
    type Error = ExecutionTimeoutError;

    fn try_from(seconds: i64) -> Result<Self, Self::Error> {
        let seconds = u64::try_from(seconds).map_err(|_| ExecutionTimeoutError::MustBePositive)?;
        Self::try_from(seconds)
    }
}

/// A finite policy used by every path that starts or relaunches a guest.
///
/// The server constructs one policy at boot and passes the same value into its
/// runtime client and embedded Environment. Standalone Environment hosts can
/// use [`Default`] or supply a stricter one through the runtime builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionTimeoutPolicy {
    default_timeout: ExecutionTimeoutSeconds,
    maximum_timeout: ExecutionTimeoutSeconds,
}

impl Default for ExecutionTimeoutPolicy {
    fn default() -> Self {
        // Both values are constants validated by this module. Keeping the
        // construction here makes accidental changes fail loudly in tests.
        Self::new(DEFAULT_EXECUTION_TIMEOUT_SECS, MAX_EXECUTION_TIMEOUT_SECS)
            .expect("default execution timeout policy is valid")
    }
}

impl ExecutionTimeoutPolicy {
    /// Create a policy with a default and maximum active duration.
    ///
    /// The maximum may be stricter than the product hard maximum, but cannot
    /// be larger. The default must be positive and no larger than that maximum.
    pub fn new(default_seconds: u32, maximum_seconds: u32) -> Result<Self, ExecutionTimeoutError> {
        let maximum_timeout = ExecutionTimeoutSeconds::new(maximum_seconds)?;
        let default_timeout = ExecutionTimeoutSeconds::new(default_seconds)?;
        if default_timeout > maximum_timeout {
            return Err(ExecutionTimeoutError::DefaultExceedsMaximum {
                default_seconds,
                maximum_seconds,
            });
        }
        Ok(Self {
            default_timeout,
            maximum_timeout,
        })
    }

    /// The timeout used when a workflow definition does not specify one.
    pub const fn default_timeout(self) -> ExecutionTimeoutSeconds {
        self.default_timeout
    }

    /// The largest timeout this deployment permits for an active guest.
    pub const fn maximum_timeout(self) -> ExecutionTimeoutSeconds {
        self.maximum_timeout
    }

    /// Validate a raw whole-second value against this deployment's policy.
    pub fn timeout_from_seconds(
        self,
        seconds: u64,
    ) -> Result<ExecutionTimeoutSeconds, ExecutionTimeoutError> {
        let timeout = ExecutionTimeoutSeconds::try_from(seconds)?;
        self.validate(timeout)
    }

    /// Validate a previously parsed timeout against this deployment's policy.
    pub fn validate(
        self,
        timeout: ExecutionTimeoutSeconds,
    ) -> Result<ExecutionTimeoutSeconds, ExecutionTimeoutError> {
        if timeout > self.maximum_timeout {
            return Err(ExecutionTimeoutError::ExceedsMaximum {
                seconds: u64::from(timeout.as_secs()),
                maximum_seconds: self.maximum_timeout.as_secs(),
            });
        }
        Ok(timeout)
    }

    /// Resolve an optional raw request value, applying the policy default only
    /// when the caller omitted the field.
    pub fn resolve_raw(
        self,
        requested_seconds: Option<u64>,
    ) -> Result<ExecutionTimeoutSeconds, ExecutionTimeoutError> {
        match requested_seconds {
            Some(seconds) => self.timeout_from_seconds(seconds),
            None => Ok(self.default_timeout),
        }
    }

    /// Resolve an already parsed timeout, applying the default only when it
    /// was omitted and still checking a deployment-specific lower maximum.
    pub fn resolve(
        self,
        requested: Option<ExecutionTimeoutSeconds>,
    ) -> Result<ExecutionTimeoutSeconds, ExecutionTimeoutError> {
        match requested {
            Some(timeout) => self.validate(timeout),
            None => Ok(self.default_timeout),
        }
    }

    /// Resolve a persisted database value without casting signed data through
    /// an unsigned integer.
    pub fn resolve_persisted(
        self,
        requested_seconds: Option<i64>,
    ) -> Result<ExecutionTimeoutSeconds, ExecutionTimeoutError> {
        match requested_seconds {
            Some(seconds) => {
                let seconds =
                    u64::try_from(seconds).map_err(|_| ExecutionTimeoutError::MustBePositive)?;
                self.timeout_from_seconds(seconds)
            }
            None => Ok(self.default_timeout),
        }
    }

    /// Parse and validate `executionTimeoutSeconds` in a workflow definition.
    ///
    /// The definition is intentionally strict: only a JSON integer is valid.
    /// String coercion once let old stored values travel through a signed cast,
    /// so an invalid legacy definition now fails before it can create a run.
    pub fn timeout_from_definition(
        self,
        definition: &Value,
    ) -> Result<Option<ExecutionTimeoutSeconds>, ExecutionTimeoutError> {
        let Some(value) = definition.get("executionTimeoutSeconds") else {
            return Ok(None);
        };
        if value.is_null() {
            return Ok(None);
        }
        let seconds = match value.as_i64() {
            Some(seconds) => {
                u64::try_from(seconds).map_err(|_| ExecutionTimeoutError::MustBePositive)?
            }
            None => value
                .as_u64()
                .ok_or(ExecutionTimeoutError::MustBeWholeSeconds)?,
        };
        self.timeout_from_seconds(seconds).map(Some)
    }
}

/// Why an execution timeout is invalid.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecutionTimeoutError {
    /// The timeout was zero or negative.
    #[error("execution timeout must be a positive whole number of seconds")]
    MustBePositive,
    /// The timeout was not represented as a JSON integer.
    #[error("execution timeout must be a whole number of seconds")]
    MustBeWholeSeconds,
    /// The timeout was above the product or deployment maximum.
    #[error(
        "execution timeout of {seconds} seconds exceeds the maximum of {maximum_seconds} seconds"
    )]
    ExceedsMaximum {
        /// Requested timeout value.
        seconds: u64,
        /// Maximum permitted timeout value.
        maximum_seconds: u32,
    },
    /// The configured default is larger than the configured maximum.
    #[error(
        "default execution timeout of {default_seconds} seconds exceeds the maximum of {maximum_seconds} seconds"
    )]
    DefaultExceedsMaximum {
        /// Configured default value.
        default_seconds: u32,
        /// Configured maximum value.
        maximum_seconds: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn policy_applies_its_default_only_when_timeout_is_omitted() {
        let policy = ExecutionTimeoutPolicy::new(120, 300).unwrap();

        assert_eq!(policy.resolve_raw(None).unwrap().as_secs(), 120);
        assert_eq!(policy.resolve_raw(Some(299)).unwrap().as_secs(), 299);
    }

    #[test]
    fn policy_rejects_zero_negative_and_oversized_values_without_casting() {
        let policy = ExecutionTimeoutPolicy::new(120, 300).unwrap();

        assert_eq!(
            policy.resolve_raw(Some(0)).unwrap_err(),
            ExecutionTimeoutError::MustBePositive
        );
        assert_eq!(
            policy.resolve_persisted(Some(-1)).unwrap_err(),
            ExecutionTimeoutError::MustBePositive
        );
        assert_eq!(
            policy.resolve_raw(Some(u64::MAX)).unwrap_err(),
            ExecutionTimeoutError::ExceedsMaximum {
                seconds: u64::MAX,
                maximum_seconds: MAX_EXECUTION_TIMEOUT_SECS,
            }
        );
        assert_eq!(
            policy.resolve_raw(Some(301)).unwrap_err(),
            ExecutionTimeoutError::ExceedsMaximum {
                seconds: 301,
                maximum_seconds: 300,
            }
        );
    }

    #[test]
    fn definition_parser_rejects_strings_fractions_and_values_above_policy() {
        let policy = ExecutionTimeoutPolicy::new(120, 300).unwrap();

        assert_eq!(
            policy
                .timeout_from_definition(&json!({ "executionTimeoutSeconds": -1 }))
                .unwrap_err(),
            ExecutionTimeoutError::MustBePositive
        );
        assert_eq!(
            policy
                .timeout_from_definition(&json!({ "executionTimeoutSeconds": "120" }))
                .unwrap_err(),
            ExecutionTimeoutError::MustBeWholeSeconds
        );
        assert_eq!(
            policy
                .timeout_from_definition(&json!({ "executionTimeoutSeconds": 1.5 }))
                .unwrap_err(),
            ExecutionTimeoutError::MustBeWholeSeconds
        );
        assert_eq!(
            policy
                .timeout_from_definition(&json!({ "executionTimeoutSeconds": 301 }))
                .unwrap_err(),
            ExecutionTimeoutError::ExceedsMaximum {
                seconds: 301,
                maximum_seconds: 300,
            }
        );
    }

    #[test]
    fn definition_parser_treats_null_as_an_omitted_timeout() {
        let policy = ExecutionTimeoutPolicy::default();

        assert_eq!(
            policy
                .timeout_from_definition(&json!({ "executionTimeoutSeconds": null }))
                .unwrap(),
            None
        );
    }

    #[test]
    fn default_cannot_be_larger_than_maximum() {
        assert_eq!(
            ExecutionTimeoutPolicy::new(301, 300).unwrap_err(),
            ExecutionTimeoutError::DefaultExceedsMaximum {
                default_seconds: 301,
                maximum_seconds: 300,
            }
        );
    }
}
