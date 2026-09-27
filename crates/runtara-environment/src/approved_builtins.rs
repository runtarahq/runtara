// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Approved history of host-executed built-in artifacts (decision D2).
//!
//! The control executor runs a workflow's control call only while the
//! workflow's `runtara:builtin-artifacts/…` pin, and every composed control
//! copy it audited, is approved. The server approves the control bytes of its
//! component bundles at boot; revocation (setting `revoked_at`) is manual and
//! takes effect at the next boot, when [`ApprovedBuiltins::load`] runs before
//! any wake or recovery. Rows are never deleted.
//!
//! A revoked pin stays in the history the executor loads against: a parked
//! run pinned to it still loads after the next boot, and its control call
//! fails with `denied` instead.

use std::collections::BTreeSet;

use sqlx::PgPool;

use crate::error::{Error, Result};

/// The approved history, loaded once at boot: the non-revoked pins, and
/// the pins revoked since.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApprovedBuiltins {
    pins: BTreeSet<String>,
    revoked: BTreeSet<String>,
}

impl ApprovedBuiltins {
    /// Approve `pins` (idempotent). A revoked pin stays revoked: approval
    /// never undoes a revocation.
    pub async fn approve(pool: &PgPool, pins: &[String]) -> Result<()> {
        for pin in pins {
            let (agent, wasm) = runtara_dsl::agent_meta::parse_builtin_artifact_import(pin)
                .ok_or_else(|| Error::Other(format!("not a built-in artifact pin: {pin}")))?;
            let metadata = pin
                .strip_suffix("@0.1.0")
                .and_then(|rest| rest.rsplit_once("-h"))
                .map(|(_, metadata)| metadata)
                .ok_or_else(|| Error::Other(format!("not a built-in artifact pin: {pin}")))?;
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

    /// Load the history: approved pins, split by revocation.
    pub async fn load(pool: &PgPool) -> Result<Self> {
        let rows: Vec<(String, bool)> = sqlx::query_as(
            "SELECT pin, revoked_at IS NOT NULL FROM approved_builtin_artifacts ORDER BY pin",
        )
        .fetch_all(pool)
        .await?;
        let mut history = Self::default();
        for (pin, revoked) in rows {
            if revoked {
                history.revoked.insert(pin);
            } else {
                history.pins.insert(pin);
            }
        }
        Ok(history)
    }

    /// The approved pins.
    pub fn pins(&self) -> impl Iterator<Item = &str> {
        self.pins.iter().map(String::as_str)
    }

    /// Whether `pin` is approved.
    pub fn contains(&self, pin: &str) -> bool {
        self.pins.contains(pin)
    }

    /// Whether `pin` was approved and then revoked.
    pub fn is_revoked(&self, pin: &str) -> bool {
        self.revoked.contains(pin)
    }

    /// Boot step, before the environment wakes or recovers any run: approve
    /// the installed control bytes (`approve`), load the history and install
    /// it on `executor` (approved pins, and revoked ones, which still load).
    /// A pin revoked earlier stays revoked.
    pub async fn install(
        pool: &PgPool,
        executor: &runtara_component_host::control_executor::ControlExecutor,
        approve: &[String],
    ) -> Result<Self> {
        Self::approve(pool, approve).await?;
        let approved = Self::load(pool).await?;
        executor.set_approved_pins(approved.pins.iter().cloned());
        executor.set_revoked_pins(approved.revoked.iter().cloned());
        if !approved.contains(executor.pin()) {
            tracing::warn!(
                pin = executor.pin(),
                "the installed control agent is revoked; every control call is denied"
            );
        }
        Ok(approved)
    }
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod db_tests {
    use super::*;

    fn pin(seed: &str) -> String {
        runtara_component_host::control_executor::control_pin(seed.as_bytes(), b"meta")
    }

    #[tokio::test]
    async fn approvals_are_idempotent_revocations_stick_and_rows_are_never_deleted() {
        let pool = crate::test_support::pool().await;
        let seed = uuid::Uuid::new_v4().to_string();
        let (a, b) = (pin(&format!("{seed}a")), pin(&format!("{seed}b")));
        ApprovedBuiltins::approve(&pool, &[a.clone(), b.clone()])
            .await
            .unwrap();
        ApprovedBuiltins::approve(&pool, std::slice::from_ref(&a))
            .await
            .unwrap();
        let approved = ApprovedBuiltins::load(&pool).await.unwrap();
        assert!(approved.contains(&a) && approved.contains(&b));

        assert!(ApprovedBuiltins::revoke(&pool, &a, "test").await.unwrap());
        assert!(!ApprovedBuiltins::revoke(&pool, &a, "again").await.unwrap());
        // A later boot re-approving the same bytes does not undo it.
        ApprovedBuiltins::approve(&pool, std::slice::from_ref(&a))
            .await
            .unwrap();
        let approved = ApprovedBuiltins::load(&pool).await.unwrap();
        assert!(!approved.contains(&a) && approved.contains(&b));
        assert!(approved.is_revoked(&a) && !approved.is_revoked(&b));

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
        assert!(
            ApprovedBuiltins::approve(&pool, &["runtara:trusted-artifacts/x".into()])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn install_approves_the_installed_bytes_and_loads_the_history() {
        let pool = crate::test_support::pool().await;
        let engine = runtara_component_host::build_engine(&runtara_component_host::EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        let wasm = wat::parse_str(
            "(component (instance $e) (export \"runtara:control/execution@0.1.0\" (instance $e)))",
        )
        .unwrap();
        // A fresh sidecar makes the pin unique in the shared test database.
        let meta = uuid::Uuid::new_v4().to_string();
        let executor = runtara_component_host::control_executor::ControlExecutor::new(
            engine,
            &wasm,
            meta.as_bytes(),
        )
        .unwrap();
        let other = pin(&uuid::Uuid::new_v4().to_string());
        let approved = ApprovedBuiltins::install(
            &pool,
            &executor,
            &[executor.pin().to_owned(), other.clone()],
        )
        .await
        .unwrap();
        assert!(approved.contains(executor.pin()) && approved.contains(&other));
        assert!(executor.approved_pins().contains(executor.pin()));

        // Revoked: the next boot installs a history without it.
        ApprovedBuiltins::revoke(&pool, executor.pin(), "test")
            .await
            .unwrap();
        ApprovedBuiltins::install(&pool, &executor, &[executor.pin().to_owned()])
            .await
            .unwrap();
        assert!(!executor.approved_pins().contains(executor.pin()));
        assert!(executor.approved_pins().contains(&other));
    }
}
