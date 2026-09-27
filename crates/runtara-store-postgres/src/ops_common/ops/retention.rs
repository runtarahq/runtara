// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Retention / cleanup operations.
//!
//! Covers `get_terminal_instances_older_than` (with the parent pin),
//! `prune_pinned_terminal` (the pin inverted) and `delete_instances_batch`.
//!
//! `delete_instances_batch` still delegates to an inherent
//! [`crate::dialect::PostgresDialect::exec_delete_instances_batch`]
//! rather than expanding inline. That indirection existed to keep a second
//! backend's placeholder fan-out out of the macro; with one backend it is
//! simply an extra hop, and folding it back in belongs with the wider dialect
//! cleanup.

/// One retention page without its cursor, order and limit: terminal rows
/// past `$1`, each flagged pinned when its parent (same tenant) exists and is
/// not terminal with `finished_at < $1`.
pub(crate) const RETENTION_PAGE_SQL: &str = r#"
    SELECT i.instance_id, i.finished_at,
           (p.instance_id IS NOT NULL AND NOT (
                p.status IN ('completed', 'failed', 'cancelled')
                AND p.finished_at IS NOT NULL
                AND p.finished_at < $1
           )) AS pinned
    FROM instances AS i
    LEFT JOIN instances AS p
        ON i.parent_instance_id IS NOT NULL
       AND p.instance_id = i.parent_instance_id
       AND p.tenant_id = i.tenant_id
    WHERE i.status IN ('completed', 'failed', 'cancelled')
      AND i.finished_at IS NOT NULL
      AND i.finished_at < $1"#;

/// One prune page without its cursor, order and limit: terminal children
/// past `$1` by their own finish whose parent (same tenant) exists and is not
/// terminal. The slice 8 pin inverted and narrowed to live parents: a child of
/// a recently finished parent is left to retention, which deletes it soon.
pub(crate) const PRUNE_PAGE_SQL: &str = r#"
    SELECT i.instance_id, i.finished_at
    FROM instances AS i
    JOIN instances AS p
        ON p.instance_id = i.parent_instance_id
       AND p.tenant_id = i.tenant_id
    WHERE i.parent_instance_id IS NOT NULL
      AND i.status IN ('completed', 'failed', 'cancelled')
      AND i.finished_at IS NOT NULL
      AND i.finished_at < $1
      AND p.status NOT IN ('completed', 'failed', 'cancelled')"#;

/// What a prune removes, per table, for the locked children in `$1`. Each
/// statement returns the instance ids it touched, so the page counts only
/// children that still had something to prune.
pub(crate) const PRUNE_STATEMENTS: [&str; 8] = [
    "DELETE FROM checkpoints WHERE instance_id = ANY($1) RETURNING instance_id",
    "DELETE FROM pending_signals WHERE instance_id = ANY($1) RETURNING instance_id",
    "DELETE FROM pending_checkpoint_signals WHERE instance_id = ANY($1) RETURNING instance_id",
    // Accepted requests stay: their receipts answer a replayed send-signal.
    "DELETE FROM instance_input_requests \
     WHERE instance_id = ANY($1) AND state = 'closed' RETURNING instance_id",
    "DELETE FROM instance_input_parks WHERE instance_id = ANY($1) RETURNING instance_id",
    "DELETE FROM invocation_attempts WHERE instance_id = ANY($1) RETURNING instance_id",
    "DELETE FROM invocation_root_leases WHERE instance_id = ANY($1) RETURNING instance_id",
    // Not `OF status`, so neither terminal trigger fires.
    "UPDATE instances SET input = NULL, stderr = NULL \
     WHERE instance_id = ANY($1) AND (input IS NOT NULL OR stderr IS NOT NULL) \
     RETURNING instance_id",
];

macro_rules! impl_retention_ops {
    ($Backend:ty, $Pool:ty, $Dialect:ty) => {
        impl $Backend {
            /// One page of a retention pass: terminal instances (completed /
            /// failed / cancelled) with `finished_at < older_than`, strictly
            /// after the cursor, in `(finished_at, instance_id)` order, each
            /// flagged pinned or eligible. SELECT-only.
            ///
            /// A child is pinned while its parent (of the same tenant) exists
            /// and is not terminal with `finished_at < older_than`: the child
            /// stays until its parent is terminal, aged from the later of the
            /// two finishes. One level: only the direct parent is consulted,
            /// and a missing parent pins nothing. The cursor lets a pass read
            /// every pinned row once instead of once per page.
            pub(crate) async fn op_get_terminal_instances_older_than(
                pool: &$Pool,
                older_than: ::chrono::DateTime<::chrono::Utc>,
                after: ::core::option::Option<&::runtara_core::persistence::RetentionCursor>,
                limit: i64,
            ) -> ::core::result::Result<
                ::runtara_core::persistence::RetentionPage,
                ::runtara_core::error::CoreError,
            > {
                // Two statements rather than `$3 IS NULL OR ...`, which would
                // keep the planner off the index range scan.
                let page_sql = crate::ops_common::ops::retention::RETENTION_PAGE_SQL;
                let order = "ORDER BY i.finished_at ASC, i.instance_id COLLATE \"C\" ASC LIMIT $2";
                let sql = match after {
                    None => format!("{page_sql} {order}"),
                    Some(_) => format!(
                        "{page_sql} AND (i.finished_at, i.instance_id COLLATE \"C\") \
                         > ($3, $4::TEXT COLLATE \"C\") {order}"
                    ),
                };
                let mut query = ::sqlx::query_as::<
                    _,
                    (
                        ::std::string::String,
                        ::chrono::DateTime<::chrono::Utc>,
                        bool,
                    ),
                >(&sql)
                .bind(older_than)
                .bind(limit);
                if let Some(cursor) = after {
                    query = query
                        .bind(cursor.finished_at)
                        .bind(cursor.instance_id.as_str());
                }
                let rows = query.fetch_all(pool).await.db()?;
                let mut page = ::runtara_core::persistence::RetentionPage::default();
                if limit > 0 && rows.len() as i64 == limit {
                    page.next = rows.last().map(|(id, finished_at, _)| {
                        ::runtara_core::persistence::RetentionCursor {
                            finished_at: *finished_at,
                            instance_id: id.clone(),
                        }
                    });
                }
                for (id, _, pinned) in rows {
                    if pinned {
                        page.pinned += 1;
                    } else {
                        page.eligible.push(id);
                    }
                }
                Ok(page)
            }

            /// One page of a prune pass over pinned terminal children (see
            /// `Persistence::prune_pinned_terminal`): the page is read without
            /// locks, then pruned in one transaction that first locks the
            /// children still terminal (`FOR NO KEY UPDATE`, in id order), so a
            /// concurrent status change cannot interleave and foreign-key
            /// inserts (events) are not blocked.
            pub(crate) async fn op_prune_pinned_terminal(
                pool: &$Pool,
                older_than: ::chrono::DateTime<::chrono::Utc>,
                after: ::core::option::Option<&::runtara_core::persistence::RetentionCursor>,
                limit: i64,
            ) -> ::core::result::Result<
                ::runtara_core::persistence::PrunePage,
                ::runtara_core::error::CoreError,
            > {
                let page_sql = crate::ops_common::ops::retention::PRUNE_PAGE_SQL;
                let order = "ORDER BY i.finished_at ASC, i.instance_id COLLATE \"C\" ASC LIMIT $2";
                let sql = match after {
                    None => format!("{page_sql} {order}"),
                    Some(_) => format!(
                        "{page_sql} AND (i.finished_at, i.instance_id COLLATE \"C\") \
                         > ($3, $4::TEXT COLLATE \"C\") {order}"
                    ),
                };
                let mut query = ::sqlx::query_as::<
                    _,
                    (::std::string::String, ::chrono::DateTime<::chrono::Utc>),
                >(&sql)
                .bind(older_than)
                .bind(limit);
                if let Some(cursor) = after {
                    query = query
                        .bind(cursor.finished_at)
                        .bind(cursor.instance_id.as_str());
                }
                let rows = query.fetch_all(pool).await.db()?;
                let mut page = ::runtara_core::persistence::PrunePage::default();
                if limit > 0 && rows.len() as i64 == limit {
                    page.next = rows.last().map(|(id, finished_at)| {
                        ::runtara_core::persistence::RetentionCursor {
                            finished_at: *finished_at,
                            instance_id: id.clone(),
                        }
                    });
                }
                if rows.is_empty() {
                    return Ok(page);
                }
                let ids: ::std::vec::Vec<::std::string::String> =
                    rows.into_iter().map(|(id, _)| id).collect();
                let mut tx = pool.begin().await.db()?;
                let locked: ::std::vec::Vec<::std::string::String> = ::sqlx::query_scalar(
                    "SELECT instance_id FROM instances \
                     WHERE instance_id = ANY($1) \
                       AND status IN ('completed', 'failed', 'cancelled') \
                     ORDER BY instance_id COLLATE \"C\" \
                     FOR NO KEY UPDATE",
                )
                .bind(&ids)
                .fetch_all(&mut *tx)
                .await
                .db()?;
                let mut touched = ::std::collections::HashSet::new();
                if !locked.is_empty() {
                    for statement in crate::ops_common::ops::retention::PRUNE_STATEMENTS {
                        let hit: ::std::vec::Vec<::std::string::String> =
                            ::sqlx::query_scalar(statement)
                                .bind(&locked)
                                .fetch_all(&mut *tx)
                                .await
                                .db()?;
                        touched.extend(hit);
                    }
                }
                tx.commit().await.db()?;
                page.pruned = touched.len() as u64;
                Ok(page)
            }

            /// DELETE a batch of instances by ID. Returns the number of
            /// rows removed. Delegates to the dialect's inherent
            /// `exec_delete_instances_batch`, which binds `&[String]` as
            /// `TEXT[]` for `= ANY($1)`.
            pub(crate) async fn op_delete_instances_batch(
                pool: &$Pool,
                instance_ids: &[::std::string::String],
            ) -> ::core::result::Result<u64, ::runtara_core::error::CoreError> {
                <$Dialect>::exec_delete_instances_batch(pool, instance_ids).await
            }

            /// DELETE the vocabulary's paired events older than
            /// `older_than`, up to `limit` rows. Returns the number removed.
            ///
            /// Only the vocabulary's own start and end subtypes: lifecycle
            /// events (`completed`, `failed`, `suspended`) are the run's
            /// history and are removed only when the instance itself is, via
            /// ON DELETE CASCADE. Paired payloads are the bulk of the table
            /// and are read while a run is recent, so they get their own,
            /// shorter window.
            ///
            /// The two subtypes are spliced, like every other vocabulary
            /// name this crate puts into SQL. They are validated identifiers,
            /// so it is safe, and one rule for the whole crate beats two.
            /// Binding them instead measures the same: over a table where
            /// these subtypes are the great majority of rows, a generic plan
            /// for `IN ($1, $2)` picks the same LIMIT-over-primary-key-index
            /// scan the literal form does, differing only in the row estimate.
            /// Consistency is the reason here, not the plan.
            ///
            /// Bounded by `limit` and driven in a loop by the caller so a
            /// large backlog never becomes one long-running DELETE.
            pub(crate) async fn op_delete_paired_events_older_than(
                pool: &$Pool,
                vocabulary: &::runtara_core::persistence::EventVocabulary,
                older_than: ::chrono::DateTime<::chrono::Utc>,
                limit: i64,
            ) -> ::core::result::Result<u64, ::runtara_core::error::CoreError> {
                use crate::dialect::Dialect;
                let p1 = <$Dialect>::placeholder(1);
                let p2 = <$Dialect>::placeholder(2);
                let vocabulary = crate::vocabulary::SqlVocabulary::new(vocabulary)?;
                let start_subtype = vocabulary.start_subtype();
                let end_subtype = vocabulary.end_subtype();
                let sql = format!(
                    "DELETE FROM instance_events \
                     WHERE id IN ( \
                         SELECT id FROM instance_events \
                         WHERE subtype IN ('{start_subtype}', '{end_subtype}') \
                           AND created_at < {p1} \
                         ORDER BY id \
                         LIMIT {p2} \
                     )"
                );
                let result = ::sqlx::query(&sql)
                    .bind(older_than)
                    .bind(limit)
                    .execute(pool)
                    .await
                    .map_err(|e| ::runtara_core::error::CoreError::PersistenceError {
                        operation: "delete_paired_events_older_than".into(),
                        details: e.to_string(),
                    })?;
                Ok(result.rows_affected())
            }
        }
    };
}

pub(crate) use impl_retention_ops;
