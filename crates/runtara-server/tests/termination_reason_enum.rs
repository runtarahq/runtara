// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `TerminationReason` against the migrated `termination_reason` enum.
//!
//! The server decodes the label Environment wrote into the instance row. A
//! label the enum holds but the server cannot decode reads as `None` and hides
//! why a run ended; a label the SQL writes but the enum lacks fails the write
//! itself with 22P02, which is how `start_gate_failed` runs used to end as
//! launch-queue timeouts instead.
//!
//! Gated by `db-integration-tests` and fails closed without a database. It
//! migrates a database of its own, derived from `TEST_RUNTARA_DATABASE_URL`,
//! because the combined core and Environment migrations must not share a
//! `_sqlx_migrations` table with the core-only runtime suite.

use std::collections::BTreeSet;

use runtara_server::runtime_types::TerminationReason;
use sqlx::PgPool;

/// Every variant. The match fails to compile when a variant is added, so it
/// has to join this list too.
fn every_variant() -> Vec<TerminationReason> {
    use TerminationReason::*;
    let all = vec![
        Completed,
        ApplicationError,
        Crashed,
        Timeout,
        HeartbeatTimeout,
        Cancelled,
        Aborted,
        Paused,
        Sleeping,
        Orphaned,
        WaitingSignal,
        WaitingInstances,
        ShutdownRequested,
        EnvironmentRestart,
        LaunchQueueTimeout,
        StartGateFailed,
    ];
    for reason in &all {
        match reason {
            Completed | ApplicationError | Crashed | Timeout | HeartbeatTimeout | Cancelled
            | Aborted | Paused | Sleeping | Orphaned | WaitingSignal | WaitingInstances
            | ShutdownRequested | EnvironmentRestart | LaunchQueueTimeout | StartGateFailed => {}
        }
    }
    all
}

/// A database beside the runtime test database, created on first use and
/// migrated with the combined core and Environment migrations.
async fn migrated_pool() -> PgPool {
    use sqlx::{ConnectOptions, Executor};
    let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_DATABASE_URL"))
        .expect("db-integration-tests requires TEST_RUNTARA_DATABASE_URL");
    let base: sqlx::postgres::PgConnectOptions =
        url.parse().expect("TEST_RUNTARA_DATABASE_URL must parse");
    let name = format!(
        "{}_termination_reasons",
        base.get_database().unwrap_or("runtara_test")
    );
    // A duplicate-database error means an earlier run created it already.
    let mut admin = base
        .clone()
        .database("postgres")
        .connect()
        .await
        .expect("the test database server must accept connections");
    let _ = admin
        .execute(format!("CREATE DATABASE \"{name}\"").as_str())
        .await;
    let pool = PgPool::connect_with(base.database(&name))
        .await
        .expect("the derived test database must accept connections");
    runtara_environment::migrations::run(&pool)
        .await
        .expect("core and Environment migrations must succeed");
    pool
}

/// Labels the core and Environment sources write as SQL literals, such as
/// `termination_reason = 'start_gate_failed'`.
fn labels_written_by_sql() -> BTreeSet<String> {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the server crate sits under crates/");
    let pattern = regex::Regex::new(
        r"termination_reason\s*=\s*'([a-z_]+)'|'([a-z_]+)'\s*::\s*termination_reason",
    )
    .expect("pattern compiles");
    let mut labels = BTreeSet::new();
    let mut pending = vec![
        crates.join("runtara-environment/src"),
        crates.join("runtara-store-postgres/src"),
    ];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{dir:?}: {e}")) {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).expect("source is UTF-8");
                for captures in pattern.captures_iter(&source) {
                    let label = captures.get(1).or_else(|| captures.get(2)).unwrap();
                    labels.insert(label.as_str().to_string());
                }
            }
        }
    }
    assert!(
        labels.contains("start_gate_failed"),
        "the scan must find the literals it exists to check: {labels:?}"
    );
    labels
}

#[tokio::test]
async fn termination_reason_matches_the_migrated_enum() {
    let pool = migrated_pool().await;
    let enum_labels: BTreeSet<String> =
        sqlx::query_scalar("SELECT unnest(enum_range(NULL::termination_reason))::text")
            .fetch_all(&pool)
            .await
            .expect("the enum must be readable")
            .into_iter()
            .collect();

    let mut decoded = BTreeSet::new();
    for label in &enum_labels {
        let reason = TerminationReason::from_str(label)
            .unwrap_or_else(|| panic!("the server must decode the enum label `{label}`"));
        assert_eq!(reason.as_str(), label);
        decoded.insert(reason.as_str());
    }
    for reason in every_variant() {
        assert!(
            enum_labels.contains(reason.as_str()),
            "`{}` has no label in the termination_reason enum",
            reason.as_str()
        );
    }
    assert_eq!(decoded.len(), enum_labels.len());

    for label in labels_written_by_sql() {
        assert!(
            enum_labels.contains(&label),
            "SQL writes termination_reason `{label}`, which the enum does not hold"
        );
    }
    pool.close().await;
}
