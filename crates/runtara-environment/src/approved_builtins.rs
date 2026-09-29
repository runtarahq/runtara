// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Approved history of trusted built-in artifacts (trusted pins, option B).
//!
//! The server approves the installed `runtara:trusted-artifacts/…` pins of
//! its component bundle at boot, and [`ApprovedBuiltins::install_trusted`]
//! hands the approved, non-revoked ones to the trusted executor. A parked run
//! pinned to an earlier approved version keeps calling that agent on wake or
//! resume, on the installed bytes; a start never uses the history.
//! Revocation (setting `revoked_at`) is manual and takes effect at the next
//! boot, when [`ApprovedBuiltins::load`] runs before any wake or recovery.
//! Rows are never deleted.
//!
//! The table also holds `runtara:builtin-artifacts/…` rows the control agent
//! was approved under before it became an ordinary composed agent. Nothing
//! reads them any more; [`ApprovedBuiltins::load`] skips them.

use std::collections::BTreeSet;

use sqlx::PgPool;

use crate::error::{Error, Result};

/// The approved trusted history, loaded once at boot: the non-revoked pins,
/// and the pins revoked since.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApprovedBuiltins {
    trusted: BTreeSet<String>,
    trusted_revoked: BTreeSet<String>,
}

/// `(agent id, component sha256, metadata sha256)` of a trusted artifact pin.
fn parse_pin(pin: &str) -> Option<(&str, &str, &str)> {
    use runtara_dsl::agent_meta::{
        trusted_artifact_import_agent_id, trusted_artifact_import_wasm_sha256,
    };
    let agent = trusted_artifact_import_agent_id(pin)?;
    let wasm = trusted_artifact_import_wasm_sha256(pin)?;
    let metadata = pin
        .strip_suffix("@0.1.0")
        .and_then(|rest| rest.rsplit_once("-h"))
        .map(|(_, metadata)| metadata)?;
    Some((agent, wasm, metadata))
}

fn is_trusted_pin(pin: &str) -> bool {
    runtara_dsl::agent_meta::trusted_artifact_import_agent_id(pin).is_some()
}

impl ApprovedBuiltins {
    /// Approve trusted `pins` (idempotent). A revoked pin stays revoked:
    /// approval never undoes a revocation.
    pub async fn approve(pool: &PgPool, pins: &[String]) -> Result<()> {
        for pin in pins {
            let (agent, wasm, metadata) = parse_pin(pin)
                .ok_or_else(|| Error::Other(format!("not a trusted artifact pin: {pin}")))?;
            sqlx::query(
                "INSERT INTO approved_builtin_artifacts (pin, agent_id, wasm_sha256, metadata_sha256) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (pin) DO NOTHING",
            )
            .bind(pin)
            .bind(agent)
            .bind(wasm)
            .bind(metadata)
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    /// Revoke `pin` from the next boot on. Returns whether a live approval
    /// was revoked.
    pub async fn revoke(pool: &PgPool, pin: &str, reason: &str) -> Result<bool> {
        let revoked = sqlx::query(
            "UPDATE approved_builtin_artifacts SET revoked_at = now(), revoked_reason = $2 \
             WHERE pin = $1 AND revoked_at IS NULL",
        )
        .bind(pin)
        .bind(reason)
        .execute(pool)
        .await?
        .rows_affected();
        Ok(revoked == 1)
    }

    /// Load the trusted history, split by revocation. Legacy control rows
    /// are skipped.
    pub async fn load(pool: &PgPool) -> Result<Self> {
        let rows: Vec<(String, bool)> = sqlx::query_as(
            "SELECT pin, revoked_at IS NOT NULL FROM approved_builtin_artifacts ORDER BY pin",
        )
        .fetch_all(pool)
        .await?;
        let mut history = Self::default();
        for (pin, revoked) in rows.into_iter().filter(|(pin, _)| is_trusted_pin(pin)) {
            if revoked {
                history.trusted_revoked.insert(pin);
            } else {
                history.trusted.insert(pin);
            }
        }
        Ok(history)
    }

    /// The approved, non-revoked trusted pins, current and earlier.
    pub fn trusted_pins(&self) -> impl Iterator<Item = &str> {
        self.trusted.iter().map(String::as_str)
    }

    /// Whether trusted `pin` was approved and then revoked.
    pub fn is_trusted_revoked(&self, pin: &str) -> bool {
        self.trusted_revoked.contains(pin)
    }

    /// Boot step, before the environment wakes or recovers any run: approve
    /// the installed trusted built-ins' pins, load the history and install
    /// its approved, non-revoked trusted pins on `executor`. Those are what a
    /// wake or resume may still run under (the installed bytes run); a start
    /// needs the installed pin itself. A pin revoked earlier stays revoked,
    /// and a revoked installed pin is only logged: the installed version
    /// keeps working for runs pinned to it.
    pub async fn install_trusted(
        pool: &PgPool,
        executor: &runtara_component_host::trusted::TrustedExecutor,
    ) -> Result<Self> {
        let installed: Vec<String> = executor.artifact_pins().map(str::to_owned).collect();
        Self::approve(pool, &installed).await?;
        let approved = Self::load(pool).await?;
        executor.set_approved_history(approved.trusted.iter().cloned());
        executor.set_revoked_pins(approved.trusted_revoked.iter().cloned());
        for pin in installed
            .iter()
            .filter(|pin| approved.is_trusted_revoked(pin))
        {
            tracing::warn!(
                pin = pin.as_str(),
                "an installed trusted built-in is revoked; every workflow call to it is denied"
            );
        }
        Ok(approved)
    }
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod db_tests {
    use super::*;

    /// A fresh 64-hex digest keeps a pin unique in a shared database.
    fn hex() -> String {
        format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        )
    }

    fn trusted(wasm: &str, meta: &str) -> String {
        runtara_dsl::agent_meta::trusted_artifact_import("s3-storage", wasm, meta)
    }

    #[tokio::test]
    async fn approvals_are_idempotent_revocations_stick_and_rows_are_never_deleted() {
        let pool = crate::test_support::pool().await;
        let meta = hex();
        let (a, b) = (trusted(&hex(), &meta), trusted(&hex(), &meta));
        ApprovedBuiltins::approve(&pool, &[a.clone(), b.clone()])
            .await
            .unwrap();
        ApprovedBuiltins::approve(&pool, std::slice::from_ref(&a))
            .await
            .unwrap();
        let approved = ApprovedBuiltins::load(&pool).await.unwrap();
        let pins: BTreeSet<&str> = approved.trusted_pins().collect();
        assert!(pins.contains(a.as_str()) && pins.contains(b.as_str()));

        assert!(ApprovedBuiltins::revoke(&pool, &a, "test").await.unwrap());
        assert!(!ApprovedBuiltins::revoke(&pool, &a, "again").await.unwrap());
        // A later boot re-approving the same bytes does not undo it.
        ApprovedBuiltins::approve(&pool, std::slice::from_ref(&a))
            .await
            .unwrap();
        let approved = ApprovedBuiltins::load(&pool).await.unwrap();
        assert!(!approved.trusted_pins().any(|pin| pin == a));
        assert!(approved.trusted_pins().any(|pin| pin == b));
        assert!(approved.is_trusted_revoked(&a) && !approved.is_trusted_revoked(&b));

        for statement in [
            "DELETE FROM approved_builtin_artifacts WHERE pin = $1",
            "UPDATE approved_builtin_artifacts SET revoked_at = NULL WHERE pin = $1",
            "UPDATE approved_builtin_artifacts SET wasm_sha256 = repeat('0', 64) WHERE pin = $1",
        ] {
            assert!(
                sqlx::query(statement)
                    .bind(&a)
                    .execute(&pool)
                    .await
                    .is_err(),
                "{statement}"
            );
        }
        // Only trusted pins are approved now.
        let legacy = runtara_dsl::agent_meta::builtin_artifact_import("control", &hex(), &meta);
        assert!(ApprovedBuiltins::approve(&pool, &[legacy]).await.is_err());
        assert!(
            ApprovedBuiltins::approve(&pool, &["runtara:trusted-artifacts/x".into()])
                .await
                .is_err()
        );
    }

    /// Rows the control agent was approved under before it became an
    /// ordinary composed agent stay in the table and are skipped on load.
    #[tokio::test]
    async fn legacy_control_rows_are_skipped() {
        let pool = crate::test_support::pool().await;
        let (wasm, meta) = (hex(), hex());
        let legacy = runtara_dsl::agent_meta::builtin_artifact_import("control", &wasm, &meta);
        sqlx::query(
            "INSERT INTO approved_builtin_artifacts (pin, agent_id, wasm_sha256, metadata_sha256) \
             VALUES ($1, 'control', $2, $3)",
        )
        .bind(&legacy)
        .bind(&wasm)
        .bind(&meta)
        .execute(&pool)
        .await
        .unwrap();
        let current = trusted(&hex(), &meta);
        ApprovedBuiltins::approve(&pool, std::slice::from_ref(&current))
            .await
            .unwrap();
        let approved = ApprovedBuiltins::load(&pool).await.unwrap();
        assert!(approved.trusted_pins().any(|pin| pin == current));
        assert!(!approved.trusted_pins().any(|pin| pin == legacy));
        assert!(!approved.is_trusted_revoked(&legacy));

        // A pin whose prefix is neither kind is refused by the table itself.
        let forged = current.replace("runtara:trusted-artifacts/", "runtara:forged-artifacts/");
        assert!(
            sqlx::query(
                "INSERT INTO approved_builtin_artifacts (pin, agent_id, wasm_sha256, metadata_sha256) \
                 VALUES ($1, 's3-storage', $2, $3)",
            )
            .bind(&forged)
            .bind(hex())
            .bind(&meta)
            .execute(&pool)
            .await
            .is_err()
        );
    }
}
