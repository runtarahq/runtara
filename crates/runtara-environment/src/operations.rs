//! Tenant-scoped Operations reads. Queue cardinality is requests, never runs.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder};

use crate::instance_repository::{InstanceRepository, ListInstancesOptions};

/// A typed JSON value sort; missing/null values always come last.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSort {
    /// One top-level field.
    pub field: String,
    /// Descending instead of ascending order.
    pub descending: bool,
}

/// Validate projection and sort fields before binding them into queries.
pub fn validate_fields(fields: &[String], sort: Option<&StateSort>) -> Result<(), String> {
    if fields.len() > 32 {
        return Err("Select at most 32 state fields".into());
    }
    for field in fields.iter().chain(sort.map(|s| &s.field)) {
        if field.is_empty() || field.len() > 256 || field.chars().any(char::is_control) {
            return Err("State fields must contain 1–256 bytes and no control characters".into());
        }
    }
    Ok(())
}

pub(crate) fn push_state_order(query: &mut QueryBuilder<'_, Postgres>, sort: &StateSort) {
    // JSONB compares numbers numerically (not as text), preserves scalar types,
    // and stores date-times in UTC. Field names are always bound parameters.
    query
        .push(" ORDER BY (SELECT NULLIF(s.state -> ")
        .push_bind(sort.field.clone())
        .push(", 'null'::jsonb) FROM instance_state s WHERE s.instance_id=i.instance_id) ")
        .push(if sort.descending { "DESC" } else { "ASC" })
        .push(" NULLS LAST, i.instance_id COLLATE \"C\" ASC");
}

/// A queue row, including only the explicitly selected state fields.
#[derive(Debug, sqlx::FromRow)]
pub struct RequestRow {
    /// Owning run.
    pub instance_id: String,
    /// Immutable business label.
    pub run_label: Option<String>,
    /// Versioned workflow image name.
    pub image_name: String,
    /// Immutable registered request identity.
    pub request_id: String,
    /// Queue key.
    pub action_key: String,
    /// First registration time.
    pub requested_at: DateTime<Utc>,
    /// Request deadline.
    pub deadline: Option<DateTime<Utc>>,
    /// Original request spec, kept as text to preserve its JSON representation.
    pub spec: String,
    /// Projected state, absent unless fields were requested.
    pub state: Option<Value>,
}

/// Request page and its matching total from the same snapshot.
#[derive(Debug)]
pub struct RequestPage {
    /// Requested page.
    pub rows: Vec<RequestRow>,
    /// Number of matching actionable requests.
    pub total: i64,
}

/// Discovered queue with outstanding requests, including removed action keys.
#[derive(Debug, sqlx::FromRow)]
pub struct ActiveQueue {
    /// Workflow owning the requests.
    pub workflow_id: String,
    /// Registered action key.
    pub action_key: String,
    /// Actionable request count.
    pub count: i64,
}

const REQUEST_FROM: &str = " FROM instance_input_requests r JOIN instances i ON i.instance_id=r.instance_id JOIN instance_images ii ON ii.instance_id=i.instance_id JOIN images img ON img.image_id=ii.image_id";

fn push_actionable(query: &mut QueryBuilder<'_, Postgres>, at: DateTime<Utc>) {
    // Match managed-input discovery, including the latest owning invocation.
    query.push(" AND r.tenant_id=i.tenant_id AND r.state='open' AND (r.deadline IS NULL OR r.deadline>")
        .push_bind(at)
        .push(") AND i.status NOT IN ('completed','failed','cancelled') AND (r.invocation_path='' OR (SELECT a.state FROM invocation_attempts a WHERE a.instance_id=r.instance_id AND a.invocation_path=r.invocation_path ORDER BY a.generation DESC LIMIT 1)='active')");
}

fn push_projection(query: &mut QueryBuilder<'_, Postgres>, fields: &[String]) {
    if fields.is_empty() {
        query.push("NULL::jsonb AS state");
    } else {
        query.push("COALESCE((SELECT jsonb_object_agg(v.key,v.value) FROM instance_state s CROSS JOIN LATERAL jsonb_each(s.state) v WHERE s.instance_id=i.instance_id AND v.key=ANY(")
            .push_bind(fields.to_vec()).push(")), '{}'::jsonb) AS state");
    }
}

impl InstanceRepository {
    /// Query one workflow and action key, applying filters before pagination.
    pub async fn operation_requests(
        &self,
        tenant: &str,
        workflow: &str,
        action_key: &str,
        listing: &ListInstancesOptions,
        fields: &[String],
    ) -> crate::error::Result<RequestPage> {
        let mut options = listing.clone();
        options.tenant_id = Some(tenant.into());
        options.image_name_prefix = Some(format!("{workflow}:"));
        let mut tx = self.pool().begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let at: DateTime<Utc> = sqlx::query_scalar("SELECT transaction_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let filters = |q: &mut QueryBuilder<'_, Postgres>| {
            crate::db::push_instance_filters(q, &options);
            q.push(" AND r.action_key=")
                .push_bind(action_key.to_owned());
            push_actionable(q, at);
        };
        let mut count = QueryBuilder::new("SELECT COUNT(*)");
        count.push(REQUEST_FROM);
        filters(&mut count);
        let total = count.build_query_scalar().fetch_one(&mut *tx).await?;
        let mut page = QueryBuilder::new(
            "SELECT i.instance_id, i.run_label, img.name AS image_name, r.request_id, r.action_key, r.created_at AS requested_at, r.deadline, r.spec, ",
        );
        push_projection(&mut page, fields);
        page.push(REQUEST_FROM);
        filters(&mut page);
        if let Some(sort) = &options.state_sort {
            push_state_order(&mut page, sort);
            page.push(", r.request_id COLLATE \"C\" ASC");
        } else {
            page.push(" ORDER BY r.created_at ASC, r.instance_id COLLATE \"C\" ASC, r.request_id COLLATE \"C\" ASC");
        }
        page.push(" LIMIT ")
            .push_bind(options.limit.clamp(1, 100))
            .push(" OFFSET ")
            .push_bind(options.offset.max(0));
        let rows = page.build_query_as().fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(RequestPage { rows, total })
    }

    /// Tenant-wide discovery includes action keys removed from the current graph.
    pub async fn active_operation_queues(
        &self,
        tenant: &str,
    ) -> crate::error::Result<Vec<ActiveQueue>> {
        let mut query = QueryBuilder::new(
            "SELECT split_part(img.name,':',1) AS workflow_id, r.action_key, count(*) AS count",
        );
        query
            .push(REQUEST_FROM)
            .push(" WHERE i.tenant_id=")
            .push_bind(tenant.to_owned())
            .push(" AND r.action_key IS NOT NULL");
        push_actionable(&mut query, Utc::now());
        query.push(
            " GROUP BY split_part(img.name,':',1), r.action_key ORDER BY workflow_id, r.action_key",
        );
        Ok(query.build_query_as().fetch_all(self.pool()).await?)
    }

    /// One batched projection for a page; no per-run fetches or full-state transport.
    pub async fn operation_state_projection(
        &self,
        tenant: &str,
        ids: &[String],
        fields: &[String],
    ) -> crate::error::Result<Vec<(String, Option<Value>)>> {
        let mut query = QueryBuilder::new("SELECT i.instance_id, ");
        push_projection(&mut query, fields);
        query
            .push(" FROM instances i WHERE i.tenant_id=")
            .push_bind(tenant.to_owned())
            .push(" AND i.instance_id=ANY(")
            .push_bind(ids.to_vec())
            .push(")");
        Ok(query.build_query_as().fetch_all(self.pool()).await?)
    }
}

/// Latest failed step and host failure reason for a page of terminal runs.
#[derive(Debug, sqlx::FromRow)]
pub struct RunFailure {
    /// Owning run.
    pub instance_id: String,
    /// Host-level reason, including failures with no step event.
    pub error: Option<String>,
    /// JSON error from the latest failed step, returned as text losslessly.
    pub detail: Option<String>,
}
impl InstanceRepository {
    /// A single tenant-scoped query enriches all failed rows in a page.
    pub async fn operation_failures(
        &self,
        tenant: &str,
        ids: &[String],
    ) -> crate::error::Result<Vec<RunFailure>> {
        let vocabulary = crate::step_vocabulary::workflow_steps();
        Ok(sqlx::query_as("SELECT i.instance_id,i.error,e.detail FROM instances i LEFT JOIN LATERAL (SELECT (convert_from(payload,'UTF8')::json -> $3)::text AS detail FROM instance_events WHERE instance_id=i.instance_id AND subtype=$4 AND (convert_from(payload,'UTF8')::json -> $3)::text IS NOT NULL AND (convert_from(payload,'UTF8')::json -> $3)::text <> 'null' ORDER BY id DESC LIMIT 1) e ON TRUE WHERE i.tenant_id=$1 AND i.instance_id=ANY($2) AND i.status='failed'")
            .bind(tenant).bind(ids).bind(vocabulary.error_key()).bind(vocabulary.end_subtype()).fetch_all(self.pool()).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projections_bound_cost_and_treat_field_names_as_data() {
        assert!(validate_fields(&vec!["amount".into(); 33], None).is_err());
        assert!(validate_fields(&["bad\0field".into()], None).is_err());
        let sort = StateSort {
            field: "amount'); SELECT 1; --".into(),
            descending: false,
        };
        assert!(validate_fields(&[], Some(&sort)).is_ok());
        let mut query = QueryBuilder::new("SELECT i.instance_id FROM instances i");
        push_state_order(&mut query, &sort);
        assert!(query.sql().contains("s.state -> $1"));
        assert!(!query.sql().contains(&sort.field));
        assert!(query.sql().contains("NULLS LAST"));
    }
}
