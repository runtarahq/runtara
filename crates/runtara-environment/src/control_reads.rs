// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Narrow, capped instance reads for the control service.
//!
//! Control reads a run of the caller's tenant without loading payloads it
//! would not return: the output and error are selected only when they fit
//! their inline cap, and their sizes always. The same predicate decides
//! "explicitly paused" in Rust and in SQL, so control, the public API and
//! the pause/resume paths cannot disagree about it.

use chrono::{DateTime, Utc};
use runtara_core::domain::InstanceStatus;

use crate::error::Result;
use crate::instance_repository::{InstanceRepository, ListInstancesOptions};

/// SQL predicate over an `instances` row aliased `i`: suspended by an explicit
/// pause, not parked on a timer, signal or shutdown. Must match
/// [`is_explicitly_paused`].
pub const EXPLICITLY_PAUSED_SQL: &str =
    "(i.status = 'suspended' AND i.termination_reason IS NULL AND i.wake_reason IS NULL)";

/// Whether a run is explicitly paused: suspended with neither a termination
/// reason (sleep, signal wait, shutdown) nor a pending wake. Must match
/// [`EXPLICITLY_PAUSED_SQL`].
pub fn is_explicitly_paused(
    status: InstanceStatus,
    termination_reason: Option<&str>,
    wake_reason: Option<&str>,
) -> bool {
    status == InstanceStatus::Suspended && termination_reason.is_none() && wake_reason.is_none()
}

/// One run as control reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlInstance {
    /// Instance id.
    pub instance_id: String,
    /// Owning tenant.
    pub tenant_id: String,
    /// Image name (`<workflow>:<version>@…`).
    pub image_name: Option<String>,
    /// Run label.
    pub run_label: Option<String>,
    /// Lifecycle status.
    pub status: InstanceStatus,
    /// `termination_reason` as stored.
    pub termination_reason: Option<String>,
    /// Suspended by an explicit pause ([`is_explicitly_paused`]).
    pub explicitly_paused: bool,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// First start.
    pub started_at: Option<DateTime<Utc>>,
    /// Terminal time.
    pub finished_at: Option<DateTime<Utc>>,
    /// Output bytes, only when at most the requested cap.
    pub output: Option<Vec<u8>>,
    /// Size of the stored output.
    pub output_bytes: Option<u64>,
    /// Error text, only when at most the requested cap.
    pub error: Option<String>,
    /// Size of the stored error, in bytes.
    pub error_bytes: Option<u64>,
}

type Row = (
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    bool,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<Vec<u8>>,
    Option<i64>,
    Option<String>,
    Option<i64>,
);

fn columns(output_cap: i64, error_cap: i64) -> String {
    format!(
        "i.instance_id, i.tenant_id, img.name, i.run_label, i.status::TEXT, \
         i.termination_reason::TEXT, {EXPLICITLY_PAUSED_SQL}, i.created_at, i.started_at, \
         i.finished_at, \
         CASE WHEN octet_length(i.output) <= {output_cap} THEN i.output END, \
         octet_length(i.output)::BIGINT, \
         CASE WHEN octet_length(i.error) <= {error_cap} THEN i.error END, \
         octet_length(i.error)::BIGINT"
    )
}

fn decode(row: Row) -> Result<ControlInstance> {
    let bytes = |size: Option<i64>| size.map(|size| size.max(0) as u64);
    Ok(ControlInstance {
        instance_id: row.0,
        tenant_id: row.1,
        image_name: row.2,
        run_label: row.3,
        status: runtara_store_postgres::encoding::status_from_str(&row.4)?,
        termination_reason: row.5,
        explicitly_paused: row.6,
        created_at: row.7,
        started_at: row.8,
        finished_at: row.9,
        output: row.10,
        output_bytes: bytes(row.11),
        error: row.12,
        error_bytes: bytes(row.13),
    })
}

const FROM: &str = " FROM instances i \
    LEFT JOIN instance_images ii ON i.instance_id = ii.instance_id \
    LEFT JOIN images img ON ii.image_id = img.image_id";

impl InstanceRepository {
    /// Read one run of `tenant`, inlining its output and error only when at
    /// most `output_cap` / `error_cap` bytes. A missing or foreign run is
    /// `None`, indistinguishably.
    pub async fn control_instance(
        &self,
        tenant: &str,
        instance_id: &str,
        output_cap: usize,
        error_cap: usize,
    ) -> Result<Option<ControlInstance>> {
        let sql = format!(
            "SELECT {}{FROM} WHERE i.instance_id = $1 AND i.tenant_id = $2",
            columns(output_cap as i64, error_cap as i64)
        );
        let row: Option<Row> = sqlx::query_as(&sql)
            .bind(instance_id)
            .bind(tenant)
            .fetch_optional(self.pool())
            .await?;
        row.map(decode).transpose()
    }

    /// A page of runs matching `options` (which must carry the tenant), with
    /// no payloads, plus the unpaged total.
    pub async fn control_instances(
        &self,
        options: &ListInstancesOptions,
    ) -> Result<(Vec<ControlInstance>, i64)> {
        let mut query = sqlx::QueryBuilder::new(format!("SELECT {}{FROM}", columns(-1, -1)));
        crate::db::push_instance_filters(&mut query, options);
        query.push(match options.order_by.as_deref() {
            Some("created_at_asc") => " ORDER BY i.created_at ASC, i.instance_id ASC",
            Some("finished_at_desc") => {
                " ORDER BY i.finished_at DESC NULLS LAST, i.instance_id DESC"
            }
            Some("finished_at_asc") => " ORDER BY i.finished_at ASC NULLS LAST, i.instance_id ASC",
            _ => " ORDER BY i.created_at DESC, i.instance_id DESC",
        });
        query
            .push(" LIMIT ")
            .push_bind(options.limit)
            .push(" OFFSET ")
            .push_bind(options.offset);
        let rows: Vec<Row> = query.build_query_as().fetch_all(self.pool()).await?;
        let total = crate::db::count_instances(self.pool(), options).await?;
        Ok((rows.into_iter().map(decode).collect::<Result<_>>()?, total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_bare_suspension_is_an_explicit_pause() {
        assert!(is_explicitly_paused(InstanceStatus::Suspended, None, None));
        for (status, termination, wake) in [
            (InstanceStatus::Suspended, Some("sleeping"), None),
            (InstanceStatus::Suspended, Some("waiting_signal"), None),
            (InstanceStatus::Suspended, None, Some("manual_resume")),
            (InstanceStatus::Running, None, None),
            (InstanceStatus::Completed, None, None),
        ] {
            assert!(!is_explicitly_paused(status, termination, wake));
        }
        // The SQL twin names the same three columns.
        for column in [
            "status = 'suspended'",
            "termination_reason IS NULL",
            "wake_reason IS NULL",
        ] {
            assert!(EXPLICITLY_PAUSED_SQL.contains(column), "{column}");
        }
    }
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod db_tests {
    use super::*;
    use runtara_core::persistence::{CompleteInstanceParams, Persistence};
    use runtara_store_postgres::PostgresPersistence;

    #[tokio::test]
    async fn the_capped_read_and_the_explicit_pause_predicate_agree_with_the_rows() {
        let pool = crate::test_support::pool().await;
        let persistence = PostgresPersistence::new(pool.clone());
        let repository = InstanceRepository::new(pool.clone());
        let tenant = format!("control-{}", uuid::Uuid::new_v4());
        let id = |name: &str| format!("{tenant}-{name}");
        // (name, status, termination_reason, wake_reason)
        let parked = [
            ("paused", None, None),
            ("sleeping", Some("sleeping"), None),
            ("waiting", Some("waiting_signal"), None),
            ("resuming", None, Some("manual_resume")),
        ];
        for (name, termination, wake) in parked {
            persistence
                .register_instance(&id(name), &tenant)
                .await
                .unwrap();
            sqlx::query(
                "UPDATE instances SET status = 'suspended', \
                 termination_reason = $2::termination_reason, wake_reason = $3 \
                 WHERE instance_id = $1",
            )
            .bind(id(name))
            .bind(termination)
            .bind(wake)
            .execute(&pool)
            .await
            .unwrap();
        }
        persistence
            .register_instance(&id("done"), &tenant)
            .await
            .unwrap();
        let output = vec![b'1'; 2048];
        persistence
            .complete_instance(
                CompleteInstanceParams::new(&id("done"), InstanceStatus::Completed)
                    .with_output(&output),
            )
            .await
            .unwrap();
        persistence
            .register_instance(&id("failed"), &tenant)
            .await
            .unwrap();
        persistence
            .complete_instance(
                CompleteInstanceParams::new(&id("failed"), InstanceStatus::Failed)
                    .with_error("boom"),
            )
            .await
            .unwrap();

        for (name, termination, wake) in parked {
            let row = repository
                .control_instance(&tenant, &id(name), 1024, 1024)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(row.status, InstanceStatus::Suspended);
            assert_eq!(
                row.explicitly_paused,
                is_explicitly_paused(row.status, termination, wake),
                "{name}: SQL and Rust disagree"
            );
            assert_eq!(row.explicitly_paused, name == "paused", "{name}");
        }

        // Over the cap: the size is read, the bytes are not.
        let done = repository
            .control_instance(&tenant, &id("done"), 1024, 1024)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((done.output, done.output_bytes), (None, Some(2048)));
        let done = repository
            .control_instance(&tenant, &id("done"), 4096, 1024)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(done.output.as_deref(), Some(output.as_slice()));
        let failed = repository
            .control_instance(&tenant, &id("failed"), 0, 1024)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (failed.error.as_deref(), failed.error_bytes),
            (Some("boom"), Some(4))
        );

        // Another tenant's run is indistinguishable from a missing one.
        assert!(
            repository
                .control_instance("other-tenant", &id("done"), 1024, 1024)
                .await
                .unwrap()
                .is_none()
        );

        let options = ListInstancesOptions {
            tenant_id: Some(tenant.clone()),
            statuses: Some(vec!["suspended".into()]),
            order_by: Some("created_at_asc".into()),
            limit: 3,
            ..Default::default()
        };
        let (page, total) = repository.control_instances(&options).await.unwrap();
        assert_eq!((page.len(), total), (3, 4));
        assert!(page.iter().all(|row| row.output.is_none()));
    }
}
