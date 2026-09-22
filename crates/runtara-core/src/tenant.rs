// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tenant identity supplied by the embedding host.

use std::{fmt, str::FromStr};

use crate::error::CoreError;

/// An explicit, nonempty tenant identity.
///
/// Construction validates representation, not authorization or existence. The
/// embedding host authenticates the caller and supplies the authorized tenant.
/// Core never resolves a tenant from environment configuration or an instance ID.
/// Values are case-sensitive and retained verbatim; normalization could silently
/// select a different tenant. There is deliberately no default identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TenantId(String);

impl TenantId {
    /// Validate an identity without changing its spelling.
    ///
    /// Empty identities, surrounding whitespace, and control characters are
    /// rejected. Other characters remain opaque to core.
    pub fn new(value: impl Into<String>) -> Result<Self, CoreError> {
        let value = value.into();
        if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
            return Err(CoreError::ValidationError {
                field: "tenant_id".into(),
                message: "tenant identity must be nonempty, without surrounding whitespace or control characters".into(),
            });
        }
        Ok(Self(value))
    }

    /// Borrow the exact identity for storage predicates and host integration.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for TenantId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for TenantId {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for TenantId {
    type Error = CoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for TenantId {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CoreErrorClass;

    #[test]
    fn invalid_identity_is_a_typed_validation_error() {
        for value in [
            "",
            " ",
            " tenant",
            "tenant ",
            "\t",
            "a\nb",
            "a\0b",
            "\u{2003}tenant",
        ] {
            let error = TenantId::new(value).unwrap_err();
            assert_eq!(error.classify(), CoreErrorClass::Invalid);
            assert!(
                matches!(error, CoreError::ValidationError { field, .. } if field == "tenant_id")
            );
        }
    }

    #[test]
    fn identity_is_opaque_and_never_normalized() {
        for value in [
            "Tenant-A",
            "org:123",
            "tenant with space",
            "租户",
            "*",
            "default",
        ] {
            let tenant: TenantId = value.parse().unwrap();
            assert_eq!(tenant.as_str(), value);
            assert_eq!(tenant.to_string(), value);
            assert_eq!(TenantId::try_from(value.to_owned()).unwrap(), tenant);
            assert_eq!(TenantId::try_from(value).unwrap(), tenant);
        }
        assert_ne!(TenantId::new("A").unwrap(), TenantId::new("a").unwrap());
    }
}
