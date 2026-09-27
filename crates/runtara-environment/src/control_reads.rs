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
    /// The run that started this one through `control:start`.
    pub parent_instance_id: Option<String>,
    /// When `control:start` admitted this run, for a child.
    pub admitted_at: Option<DateTime<Utc>>,
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
    Option<String>,
    Option<DateTime<Utc>>,
);

fn columns(output_cap: i64, error_cap: i64) -> String {
    format!(
        "i.instance_id, i.tenant_id, img.name, i.run_label, i.status::TEXT, \
         i.termination_reason::TEXT, {EXPLICITLY_PAUSED_SQL}, i.created_at, i.started_at, \
         i.finished_at, \
         CASE WHEN octet_length(i.output) <= {output_cap} THEN i.output END, \
         octet_length(i.output)::BIGINT, \
         CASE WHEN octet_length(i.error) <= {error_cap} THEN i.error END, \
         octet_length(i.error)::BIGINT, i.parent_instance_id, i.admitted_at"
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
        parent_instance_id: row.14,
        admitted_at: row.15,
    })
}

const FROM: &str = " FROM instances i \
    LEFT JOIN instance_images ii ON i.instance_id = ii.instance_id \
    LEFT JOIN images img ON ii.image_id = img.image_id";

/// A child `control:start` admitted that has no instance row yet, as the
/// server's admission records know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedChild {
    /// The child's instance id.
    pub instance_id: String,
    /// Its run label, if any.
    pub run_label: Option<String>,
    /// When it was admitted.
    pub admitted_at: DateTime<Utc>,
}

/// One child of a parent, launched or still in admission.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlChild {
    /// The child has an instance row.
    Launched(Box<ControlInstance>),
    /// Admitted and not launched yet (`queued`).
    Admitted(AdmittedChild),
}

impl ControlChild {
    /// The child's instance id.
    pub fn instance_id(&self) -> &str {
        match self {
            Self::Launched(row) => &row.instance_id,
            Self::Admitted(child) => &child.instance_id,
        }
    }
}

/// How [`InstanceRepository::control_children`] orders a parent's children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildOrder {
    /// By admission time, oldest first.
    AdmittedAsc,
    /// By admission time, newest first.
    AdmittedDesc,
    /// By finish time, unfinished last, then admission time.
    FinishedAsc,
    /// By finish time descending, unfinished last, then admission time.
    FinishedDesc,
}

#[derive(sqlx::FromRow)]
struct ChildRow {
    total: i64,
    launched: Option<bool>,
    instance_id: Option<String>,
    tenant_id: Option<String>,
    image_name: Option<String>,
    run_label: Option<String>,
    status: Option<String>,
    termination_reason: Option<String>,
    explicitly_paused: Option<bool>,
    created_at: Option<DateTime<Utc>>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    parent_instance_id: Option<String>,
    admitted_at: Option<DateTime<Utc>>,
}

/// Longest lineage [`InstanceRepository::control_lineage`] walks.
const MAX_LINEAGE_WALK: i32 = 64;

impl InstanceRepository {
    /// The lineage of `instance_id` in `tenant`, the run itself first, then
    /// its parent, grandparent and so on, as far as rows of the tenant exist
    /// (at most 64). Each entry is `(instance_id, parent_instance_id)`, so a
    /// last entry with a parent names an ancestor whose row is gone. Empty
    /// when the run is not a run of the tenant.
    pub async fn control_lineage(
        &self,
        tenant: &str,
        instance_id: &str,
    ) -> Result<Vec<(String, Option<String>)>> {
        Ok(sqlx::query_as(
            r#"
            WITH RECURSIVE chain(instance_id, parent_instance_id, depth) AS (
                SELECT instance_id, parent_instance_id, 1
                FROM instances
                WHERE instance_id = $1 AND tenant_id = $2
                UNION ALL
                SELECT i.instance_id, i.parent_instance_id, chain.depth + 1
                FROM instances AS i
                JOIN chain ON i.instance_id = chain.parent_instance_id
                WHERE i.tenant_id = $2 AND chain.depth < $3
            )
            SELECT instance_id, parent_instance_id FROM chain ORDER BY depth
            "#,
        )
        .bind(instance_id)
        .bind(tenant)
        .bind(MAX_LINEAGE_WALK)
        .fetch_all(self.pool())
        .await?)
    }

    /// One page of the children of `parent` in `tenant`: the launched ones
    /// matching `options` (tenant and parent are set here) merged with the
    /// `admitted` ones that have no instance row yet, in one statement, plus
    /// the merged total. A child both admitted and launched counts once, as
    /// launched. The caller filters `admitted` by everything but launch;
    /// without `include_launched` only `admitted` children are listed.
    pub async fn control_children(
        &self,
        tenant: &str,
        parent: &str,
        options: &ListInstancesOptions,
        include_launched: bool,
        admitted: &[AdmittedChild],
        order: ChildOrder,
    ) -> Result<(Vec<ControlChild>, i64)> {
        let options = ListInstancesOptions {
            tenant_id: Some(tenant.to_owned()),
            parent_instance_id: Some(parent.to_owned()),
            ..options.clone()
        };
        let order_sql = match order {
            ChildOrder::AdmittedAsc => "admitted_at ASC, instance_id COLLATE \"C\" ASC",
            ChildOrder::AdmittedDesc => "admitted_at DESC, instance_id COLLATE \"C\" DESC",
            ChildOrder::FinishedAsc => {
                "finished_at ASC NULLS LAST, admitted_at ASC, instance_id COLLATE \"C\" ASC"
            }
            ChildOrder::FinishedDesc => {
                "finished_at DESC NULLS LAST, admitted_at DESC, instance_id COLLATE \"C\" DESC"
            }
        };
        let mut query = sqlx::QueryBuilder::new(format!(
            "WITH children AS (SELECT TRUE AS launched, i.instance_id, i.tenant_id, \
             img.name AS image_name, i.run_label, i.status::TEXT AS status, \
             i.termination_reason::TEXT AS termination_reason, \
             {EXPLICITLY_PAUSED_SQL} AS explicitly_paused, i.created_at, i.started_at, \
             i.finished_at, i.parent_instance_id, i.admitted_at{FROM}"
        ));
        crate::db::push_instance_filters(&mut query, &options);
        if !include_launched {
            query.push(" AND FALSE");
        }
        query
            .push(
                " UNION ALL SELECT FALSE, f.instance_id, NULL, NULL, f.run_label, NULL, NULL, \
                 FALSE, f.admitted_at, NULL, NULL, NULL, f.admitted_at \
                 FROM unnest(",
            )
            .push_bind(
                admitted
                    .iter()
                    .map(|child| child.instance_id.clone())
                    .collect::<Vec<_>>(),
            )
            .push("::TEXT[], ")
            .push_bind(
                admitted
                    .iter()
                    .map(|child| child.run_label.clone())
                    .collect::<Vec<_>>(),
            )
            .push("::TEXT[], ")
            .push_bind(
                admitted
                    .iter()
                    .map(|child| child.admitted_at)
                    .collect::<Vec<_>>(),
            )
            .push(
                "::TIMESTAMPTZ[]) AS f(instance_id, run_label, admitted_at) \
                 WHERE NOT EXISTS (SELECT 1 FROM instances AS x WHERE x.instance_id = f.instance_id)), \
                 page AS (SELECT * FROM children ORDER BY ",
            )
            .push(order_sql)
            .push(" LIMIT ")
            .push_bind(options.limit)
            .push(" OFFSET ")
            .push_bind(options.offset)
            .push(
                ") SELECT (SELECT count(*) FROM children)::BIGINT AS total, page.* \
                 FROM (SELECT 1) AS one LEFT JOIN page ON TRUE ORDER BY ",
            )
            .push(order_sql);
        let rows: Vec<ChildRow> = query.build_query_as().fetch_all(self.pool()).await?;
        let total = rows.first().map_or(0, |row| row.total);
        let mut children = Vec::with_capacity(rows.len());
        for row in rows {
            let (Some(launched), Some(instance_id)) = (row.launched, row.instance_id) else {
                continue;
            };
            if !launched {
                if let Some(child) = admitted
                    .iter()
                    .find(|child| child.instance_id == instance_id)
                {
                    children.push(ControlChild::Admitted(child.clone()));
                }
                continue;
            }
            children.push(ControlChild::Launched(Box::new(ControlInstance {
                instance_id,
                tenant_id: row.tenant_id.unwrap_or_default(),
                image_name: row.image_name,
                run_label: row.run_label,
                status: runtara_store_postgres::encoding::status_from_str(
                    row.status.as_deref().unwrap_or_default(),
                )?,
                termination_reason: row.termination_reason,
                explicitly_paused: row.explicitly_paused.unwrap_or(false),
                created_at: row.created_at.unwrap_or_default(),
                started_at: row.started_at,
                finished_at: row.finished_at,
                output: None,
                output_bytes: None,
                error: None,
                error_bytes: None,
                parent_instance_id: row.parent_instance_id,
                admitted_at: row.admitted_at,
            })));
        }
        Ok((children, total))
    }

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

#[cfg(all(test, feature = "db-integration-tests"))]
mod parent_tests {
    use super::*;
    use crate::launch_queue::{
        EnqueueRequest, InitialLaunchOutcome, InitialLaunchRequest, LaunchKind, LaunchQueueError,
        LaunchRepository,
    };
    use runtara_core::persistence::ParentLink;
    use std::time::Duration;

    fn link(parent: &str, admitted_at: DateTime<Utc>) -> ParentLink {
        ParentLink {
            parent_instance_id: parent.into(),
            parent_close_policy: "cancel".into(),
            admitted_at,
        }
    }

    fn initial(
        tenant: &str,
        image: &str,
        id: &str,
        parent: Option<ParentLink>,
    ) -> InitialLaunchRequest {
        InitialLaunchRequest {
            run_label: Some(format!("label-{id}")),
            parent,
            launch: EnqueueRequest::immediate(
                uuid::Uuid::new_v4().to_string(),
                id,
                tenant,
                image,
                LaunchKind::Start,
                Duration::from_secs(600),
            ),
            input: None,
            env: None,
            timeout_seconds: None,
        }
    }

    async fn image(pool: &sqlx::PgPool, tenant: &str) -> String {
        let image = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ($1, $2, $3, '/unused')",
        )
        .bind(&image)
        .bind(tenant)
        .bind(format!("wf-{tenant}:1@fixture"))
        .execute(pool)
        .await
        .unwrap();
        image
    }

    fn ms(offset: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(1_800_000_000_000 + offset).unwrap()
    }

    #[tokio::test]
    async fn children_carry_their_parent_and_merge_with_admitted_ones() {
        let pool = crate::test_support::pool().await;
        let repo = LaunchRepository::new(pool.clone());
        let instances = InstanceRepository::new(pool.clone());
        let tenant = format!("parent-{}", uuid::Uuid::new_v4());
        let image = image(&pool, &tenant).await;
        let id = |name: &str| format!("{tenant}-{name}");

        // parent <- child-a, child-b; child-a <- grandchild
        for (name, parent, at) in [
            ("parent", None, 0),
            ("child-a", Some("parent"), 10),
            ("child-b", Some("parent"), 30),
            ("grandchild", Some("child-a"), 40),
        ] {
            let outcome = repo
                .claim_initial(initial(
                    &tenant,
                    &image,
                    &id(name),
                    parent.map(|parent| link(&id(parent), ms(at))),
                ))
                .await
                .unwrap();
            assert!(
                matches!(outcome, InitialLaunchOutcome::Enqueued(_)),
                "{name}"
            );
        }
        let child = instances
            .control_instance(&tenant, &id("child-a"), 0, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(child.parent_instance_id, Some(id("parent")));
        assert_eq!(child.admitted_at, Some(ms(10)));

        // A parent of another tenant, or none at all, is refused and writes
        // nothing.
        let foreign = format!("foreign-{}", uuid::Uuid::new_v4());
        let foreign_image = image_for(&pool, &foreign).await;
        repo.claim_initial(initial(
            &foreign,
            &foreign_image,
            &id("foreign-parent"),
            None,
        ))
        .await
        .unwrap();
        for parent in [id("foreign-parent"), id("missing")] {
            let refused = repo
                .claim_initial(initial(
                    &tenant,
                    &image,
                    &id("orphan"),
                    Some(link(&parent, ms(1))),
                ))
                .await;
            assert!(
                matches!(refused, Err(LaunchQueueError::InvalidParent(_))),
                "{parent}: {refused:?}"
            );
            assert!(
                instances
                    .control_instance(&tenant, &id("orphan"), 0, 0)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        // A replay of a claimed child is the existing launch, not a refusal.
        assert!(matches!(
            repo.claim_initial(initial(
                &tenant,
                &image,
                &id("child-a"),
                Some(link(&id("parent"), ms(10)))
            ))
            .await
            .unwrap(),
            InitialLaunchOutcome::ExistingLaunch(_)
        ));

        // Lineage: the run itself first, then its ancestors.
        let lineage = instances
            .control_lineage(&tenant, &id("grandchild"))
            .await
            .unwrap();
        assert_eq!(
            lineage,
            vec![
                (id("grandchild"), Some(id("child-a"))),
                (id("child-a"), Some(id("parent"))),
                (id("parent"), None),
            ]
        );
        assert!(
            instances
                .control_lineage("other-tenant", &id("grandchild"))
                .await
                .unwrap()
                .is_empty()
        );

        // The public filter lists direct children only.
        let options = ListInstancesOptions {
            tenant_id: Some(tenant.clone()),
            parent_instance_id: Some(id("parent")),
            limit: 10,
            ..Default::default()
        };
        let page = instances.list(&options).await.unwrap();
        assert_eq!(page.total_count, 2);
        assert!(
            page.instances
                .iter()
                .all(|row| row.parent_instance_id == Some(id("parent")))
        );

        // Merged children: two launched, two admitted (one of which already
        // launched, so it counts once), paged by admission time.
        let admitted = vec![
            AdmittedChild {
                instance_id: id("child-a"),
                run_label: None,
                admitted_at: ms(10),
            },
            AdmittedChild {
                instance_id: id("queued-1"),
                run_label: Some("q1".into()),
                admitted_at: ms(20),
            },
            AdmittedChild {
                instance_id: id("queued-2"),
                run_label: None,
                admitted_at: ms(50),
            },
        ];
        let page = |limit: i64, offset: i64, order: ChildOrder| {
            let instances = &instances;
            let admitted = &admitted;
            let tenant = &tenant;
            let parent = id("parent");
            async move {
                instances
                    .control_children(
                        tenant,
                        &parent,
                        &ListInstancesOptions {
                            limit,
                            offset,
                            ..Default::default()
                        },
                        true,
                        admitted,
                        order,
                    )
                    .await
                    .unwrap()
            }
        };
        let (all, total) = page(10, 0, ChildOrder::AdmittedAsc).await;
        assert_eq!(total, 4);
        let ids: Vec<_> = all.iter().map(ControlChild::instance_id).collect();
        assert_eq!(
            ids,
            [id("child-a"), id("queued-1"), id("child-b"), id("queued-2")]
        );
        assert!(
            matches!(all[0], ControlChild::Launched(_)),
            "a launched child wins"
        );
        assert!(matches!(all[1], ControlChild::Admitted(_)));
        let (first, total) = page(2, 0, ChildOrder::AdmittedDesc).await;
        assert_eq!(total, 4);
        let ids: Vec<_> = first.iter().map(ControlChild::instance_id).collect();
        assert_eq!(ids, [id("queued-2"), id("child-b")]);
        let (last, total) = page(2, 2, ChildOrder::AdmittedDesc).await;
        assert_eq!(total, 4);
        let ids: Vec<_> = last.iter().map(ControlChild::instance_id).collect();
        assert_eq!(ids, [id("queued-1"), id("child-a")]);
        let (beyond, total) = page(2, 10, ChildOrder::AdmittedAsc).await;
        assert!(beyond.is_empty());
        assert_eq!(total, 4, "the total survives an empty page");
        // Without launched children only the admitted ones remain.
        let (only_admitted, total) = instances
            .control_children(
                &tenant,
                &id("parent"),
                &ListInstancesOptions {
                    limit: 10,
                    ..Default::default()
                },
                false,
                &admitted,
                ChildOrder::AdmittedAsc,
            )
            .await
            .unwrap();
        assert_eq!(total, 2);
        assert!(
            only_admitted
                .iter()
                .all(|child| matches!(child, ControlChild::Admitted(_)))
        );
        // Another tenant's parent id sees none of them.
        let (none, total) = instances
            .control_children(
                "other-tenant",
                &id("parent"),
                &ListInstancesOptions {
                    limit: 10,
                    ..Default::default()
                },
                true,
                &[],
                ChildOrder::AdmittedAsc,
            )
            .await
            .unwrap();
        assert!(none.is_empty());
        assert_eq!(total, 0);

        // Open inputs of the parent's children.
        sqlx::query(
            "INSERT INTO instance_input_requests \
             (request_id, instance_id, tenant_id, signal_id, invocation_path, spec, state) \
             VALUES ($1, $2, $3, 'sig', 'root', '{}'::jsonb, 'open')",
        )
        .bind("a".repeat(64))
        .bind(id("child-b"))
        .bind(&tenant)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            instances
                .input_candidate_ids_for_parent(&tenant, &id("parent"))
                .await
                .unwrap(),
            vec![id("child-b")]
        );
        assert!(
            instances
                .input_candidate_ids_for_parent(&tenant, &id("child-a"))
                .await
                .unwrap()
                .is_empty()
        );
    }

    async fn image_for(pool: &sqlx::PgPool, tenant: &str) -> String {
        image(pool, tenant).await
    }
}
