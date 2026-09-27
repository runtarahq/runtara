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
//!
//! Trusted built-ins (S3, Azure) share the history (trusted pins, option B):
//! the server approves the installed `runtara:trusted-artifacts/…` pins at
//! boot too, and [`ApprovedBuiltins::install_trusted`] hands the approved,
//! non-revoked ones to the trusted executor. A parked run pinned to an
//! earlier approved version keeps calling that agent on wake or resume, on
//! the installed bytes; a start never uses the history. Trusted pins never
//! count as control pins, nor towards compilation readiness.

use std::collections::BTreeSet;

use sqlx::PgPool;

use crate::error::{Error, Result};

/// The approved history, loaded once at boot: the non-revoked pins, and
/// the pins revoked since, host-executed built-ins (control) and trusted
/// built-ins apart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApprovedBuiltins {
    pins: BTreeSet<String>,
    revoked: BTreeSet<String>,
    trusted: BTreeSet<String>,
    trusted_revoked: BTreeSet<String>,
}

/// `(agent id, component sha256, metadata sha256)` of a built-in or trusted
/// artifact pin.
fn parse_pin(pin: &str) -> Option<(&str, &str, &str)> {
    use runtara_dsl::agent_meta::{
        parse_builtin_artifact_import, trusted_artifact_import_agent_id,
        trusted_artifact_import_wasm_sha256,
    };
    let (agent, wasm) = parse_builtin_artifact_import(pin).or_else(|| {
        Some((
            trusted_artifact_import_agent_id(pin)?,
            trusted_artifact_import_wasm_sha256(pin)?,
        ))
    })?;
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
    /// Approve `pins` (idempotent): `runtara:builtin-artifacts/…` (control)
    /// or `runtara:trusted-artifacts/…` pins. A revoked pin stays revoked:
    /// approval never undoes a revocation.
    pub async fn approve(pool: &PgPool, pins: &[String]) -> Result<()> {
        for pin in pins {
            let (agent, wasm, metadata) = parse_pin(pin)
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
            let set = match (is_trusted_pin(&pin), revoked) {
                (false, false) => &mut history.pins,
                (false, true) => &mut history.revoked,
                (true, false) => &mut history.trusted,
                (true, true) => &mut history.trusted_revoked,
            };
            set.insert(pin);
        }
        Ok(history)
    }

    /// The approved host-executed built-in (control) pins. Never trusted
    /// pins: those do not make an artifact ready to compile or start.
    pub fn pins(&self) -> impl Iterator<Item = &str> {
        self.pins.iter().map(String::as_str)
    }

    /// The approved, non-revoked trusted pins, current and earlier.
    pub fn trusted_pins(&self) -> impl Iterator<Item = &str> {
        self.trusted.iter().map(String::as_str)
    }

    /// Whether trusted `pin` was approved and then revoked.
    pub fn is_trusted_revoked(&self, pin: &str) -> bool {
        self.trusted_revoked.contains(pin)
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

    /// Trusted pins share the history (append-only, revoke-once) but are
    /// kept apart: they are never control pins, so they never make an
    /// artifact ready to compile or start; only the executor's wake/resume
    /// check reads them.
    #[tokio::test]
    async fn trusted_pins_share_the_history_but_never_count_as_control_pins() {
        let pool = crate::test_support::pool().await;
        let seed = uuid::Uuid::new_v4().simple().to_string();
        // Fresh 64-hex digests keep the pins unique in a shared database.
        let hex = || {
            format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            )
        };
        let (old_wasm, current_wasm, meta) = (hex(), hex(), hex());
        let trusted = |wasm: &str| {
            runtara_dsl::agent_meta::trusted_artifact_import("s3-storage", wasm, &meta)
        };
        let (old, current) = (trusted(&old_wasm), trusted(&current_wasm));
        let control = pin(&format!("{seed}control"));
        ApprovedBuiltins::approve(&pool, &[old.clone(), current.clone(), control.clone()])
            .await
            .unwrap();
        let approved = ApprovedBuiltins::load(&pool).await.unwrap();
        let trusted_pins: BTreeSet<&str> = approved.trusted_pins().collect();
        assert!(trusted_pins.contains(old.as_str()) && trusted_pins.contains(current.as_str()));
        assert!(!trusted_pins.contains(control.as_str()));
        assert!(approved.contains(&control));
        assert!(
            approved
                .pins()
                .all(|pin| !pin.starts_with("runtara:trusted-artifacts/")),
            "trusted pins never count towards readiness or control"
        );

        assert!(ApprovedBuiltins::revoke(&pool, &old, "test").await.unwrap());
        ApprovedBuiltins::approve(&pool, std::slice::from_ref(&old))
            .await
            .unwrap();
        let approved = ApprovedBuiltins::load(&pool).await.unwrap();
        assert!(!approved.trusted_pins().any(|pin| pin == old));
        assert!(approved.is_trusted_revoked(&old));
        assert!(approved.trusted_pins().any(|pin| pin == current));
        assert!(
            sqlx::query("DELETE FROM approved_builtin_artifacts WHERE pin = $1")
                .bind(&current)
                .execute(&pool)
                .await
                .is_err(),
            "trusted rows are never deleted either"
        );
        // A pin whose prefix is neither kind is refused by the table itself.
        let forged = current.replace("runtara:trusted-artifacts/", "runtara:forged-artifacts/");
        assert!(
            sqlx::query(
                "INSERT INTO approved_builtin_artifacts (pin, agent_id, wasm_sha256, metadata_sha256) \
                 VALUES ($1, 's3-storage', $2, $3)",
            )
            .bind(&forged)
            .bind(&current_wasm)
            .bind(&meta)
            .execute(&pool)
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
