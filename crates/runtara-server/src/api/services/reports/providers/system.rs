//! System source provider — runtime metric buckets, system snapshot, and
//! connection rate-limit data. Pulls from [`RuntimeClient`] and the
//! connections facade; never touches Postgres directly. Aggregates are
//! always virtual (no pushdown).

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::api::dto::object_model::{AggregateRequest, Condition};
use crate::api::dto::reports::*;
use crate::api::services::reports::{
    ReportServiceError, aggregate_output_fields, condition_from_value, condition_matches_row,
    f64_value, humanize_label, option_f64_value, validate_block_interactions,
    validate_report_aggregate_specs, validate_report_condition_field_refs,
    validate_report_condition_filter_refs, validate_report_interaction_buttons,
    validate_report_source_filter_mappings, validate_report_table_action_config,
    validate_report_table_display_templates,
};
use crate::runtime_client::{GetTenantMetricsOptions, MetricsGranularity, RuntimeClient};

use super::{
    FetchAggregateOutput, FetchParams, FetchRowsOutput, ReportSourceProvider, dotted_field_known,
};

pub struct SystemProvider {
    runtime_client: Option<Arc<RuntimeClient>>,
    connections: Arc<runtara_connections::ConnectionsFacade>,
}

impl SystemProvider {
    pub fn new(
        runtime_client: Option<Arc<RuntimeClient>>,
        connections: Arc<runtara_connections::ConnectionsFacade>,
    ) -> Self {
        Self {
            runtime_client,
            connections,
        }
    }
}

#[async_trait]
impl ReportSourceProvider for SystemProvider {
    fn kind(&self) -> ReportSourceKind {
        ReportSourceKind::System
    }

    async fn fetch_rows(
        &self,
        params: FetchParams<'_>,
    ) -> Result<FetchRowsOutput, ReportServiceError> {
        let rows = fetch_rows_inner(self, params.tenant_id, params.block, params.condition).await?;
        Ok(FetchRowsOutput {
            rows,
            total_count: None,
        })
    }

    async fn fetch_aggregate(
        &self,
        params: FetchParams<'_>,
        request: AggregateRequest,
    ) -> Result<FetchAggregateOutput, ReportServiceError> {
        let rows = fetch_rows_inner(self, params.tenant_id, params.block, params.condition).await?;
        let result = runtara_report_dsl::virtual_aggregate::aggregate_virtual_rows(
            &params.block.id,
            &rows,
            request.into(),
            |condition, row, block_id| {
                let local = local_condition_from_store(condition);
                condition_matches_row(&local, row, block_id).map_err(|err| err.to_string())
            },
        )
        .map_err(|err| ReportServiceError::Validation(err.0))?;
        Ok(FetchAggregateOutput::from(result))
    }

    fn validate_block(
        &self,
        block: &ReportBlockDefinition,
        filter_ids: &HashSet<String>,
        view_ids: &HashSet<String>,
        filter_defs: &HashMap<String, &ReportFilterDefinition>,
    ) -> Result<(), ReportServiceError> {
        validate_system_block(block, filter_ids, view_ids, filter_defs)
    }

    fn field_is_known(&self, block: &ReportBlockDefinition, field: &str) -> bool {
        let Ok(entity) = system_entity(block) else {
            return false;
        };
        let fields = system_fields(entity);
        system_row_field_known(&fields, field)
    }

    fn field_set(&self, block: &ReportBlockDefinition) -> Option<HashSet<&'static str>> {
        system_entity(block).ok().map(system_fields)
    }

    fn markdown_field_known(&self, block: &ReportBlockDefinition, field_path: &str) -> bool {
        match block.source.mode {
            ReportSourceMode::Filter => dotted_field_known(field_path, &|candidate| {
                self.field_is_known(block, candidate)
            }),
            ReportSourceMode::Aggregate => {
                let output = aggregate_output_fields(block);
                dotted_field_known(field_path, &|candidate| output.contains(candidate))
            }
        }
    }

    fn table_columns(
        &self,
        block: &ReportBlockDefinition,
    ) -> Result<Vec<Value>, ReportServiceError> {
        let entity = system_entity(block)?;
        Ok(system_table_columns(block.table.as_ref(), entity))
    }
}

// ============================================================================
// Row acquisition
// ============================================================================

/// Bridge `runtara_object_store::Condition` (used by the
/// `runtara_report_dsl::virtual_aggregate` engine) back to the local
/// `Condition` flavour that the server's `condition_matches_row` accepts.
/// Field-identical wire shape — cheap clone.
fn local_condition_from_store(condition: &runtara_object_store::Condition) -> Condition {
    Condition {
        op: condition.op.clone(),
        arguments: condition.arguments.clone(),
    }
}

async fn fetch_rows_inner(
    provider: &SystemProvider,
    tenant_id: &str,
    block: &ReportBlockDefinition,
    condition: Option<&Condition>,
) -> Result<Vec<Map<String, Value>>, ReportServiceError> {
    let entity = system_entity(block)?;
    let rows = match entity {
        ReportWorkflowRuntimeEntity::RuntimeExecutionMetricBuckets => {
            runtime_execution_metric_rows(provider, tenant_id, block, condition).await?
        }
        ReportWorkflowRuntimeEntity::RuntimeSystemSnapshot => vec![runtime_system_snapshot_row()],
        ReportWorkflowRuntimeEntity::ConnectionRateLimitStatus => {
            connection_rate_limit_status_rows(provider, tenant_id, block).await?
        }
        ReportWorkflowRuntimeEntity::ConnectionRateLimitEvents => {
            connection_rate_limit_event_rows(provider, tenant_id, block, condition).await?
        }
        ReportWorkflowRuntimeEntity::ConnectionRateLimitTimeline => {
            connection_rate_limit_timeline_rows(provider, tenant_id, block, condition).await?
        }
        ReportWorkflowRuntimeEntity::Instances | ReportWorkflowRuntimeEntity::Actions => {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' system source does not support workflow_runtime entity {:?}",
                block.id, block.source.entity
            )));
        }
    };

    if let Some(condition) = condition {
        rows.into_iter()
            .filter_map(
                |row| match condition_matches_row(condition, &row, &block.id) {
                    Ok(true) => Some(Ok(row)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                },
            )
            .collect()
    } else {
        Ok(rows)
    }
}

async fn runtime_execution_metric_rows(
    provider: &SystemProvider,
    tenant_id: &str,
    block: &ReportBlockDefinition,
    condition: Option<&Condition>,
) -> Result<Vec<Map<String, Value>>, ReportServiceError> {
    let runtime_client = provider.runtime_client.as_ref().ok_or_else(|| {
        ReportServiceError::Validation(
            "System runtime metric blocks require a configured runtime client".to_string(),
        )
    })?;
    let now = Utc::now();
    let (start_time, end_time) = extract_time_bounds(condition, &["bucketTime"]);
    let end_time = end_time.unwrap_or(now);
    let start_time = start_time.unwrap_or(end_time - Duration::days(30));
    let granularity = parse_metrics_granularity(block.source.granularity.as_deref())?;

    let result = runtime_client
        .get_tenant_metrics(
            GetTenantMetricsOptions::new(tenant_id)
                .with_start_time(start_time)
                .with_end_time(end_time)
                .with_granularity(granularity),
        )
        .await
        .map_err(|err| {
            ReportServiceError::Database(format!(
                "Failed to fetch runtime execution metrics: {}",
                err
            ))
        })?;

    let result_tenant_id = result.tenant_id.clone();
    let result_granularity = format!("{:?}", result.granularity).to_lowercase();

    Ok(result
        .buckets
        .into_iter()
        .map(|bucket| {
            Map::from_iter([
                (
                    "tenantId".to_string(),
                    Value::String(result_tenant_id.clone()),
                ),
                (
                    "bucketTime".to_string(),
                    Value::String(bucket.bucket_time.to_rfc3339()),
                ),
                (
                    "granularity".to_string(),
                    Value::String(result_granularity.clone()),
                ),
                (
                    "invocationCount".to_string(),
                    json!(bucket.invocation_count),
                ),
                ("successCount".to_string(), json!(bucket.success_count)),
                ("failureCount".to_string(), json!(bucket.failure_count)),
                ("cancelledCount".to_string(), json!(bucket.cancelled_count)),
                (
                    "avgDurationSeconds".to_string(),
                    option_f64_value(bucket.avg_duration_seconds),
                ),
                (
                    "minDurationSeconds".to_string(),
                    option_f64_value(bucket.min_duration_seconds),
                ),
                (
                    "maxDurationSeconds".to_string(),
                    option_f64_value(bucket.max_duration_seconds),
                ),
                (
                    "avgMemoryBytes".to_string(),
                    bucket
                        .avg_memory_bytes
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                ),
                (
                    "maxMemoryBytes".to_string(),
                    bucket
                        .max_memory_bytes
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                ),
                (
                    "successRatePercent".to_string(),
                    option_f64_value(bucket.success_rate_percent),
                ),
            ])
        })
        .collect())
}

async fn connection_rate_limit_status_rows(
    provider: &SystemProvider,
    tenant_id: &str,
    block: &ReportBlockDefinition,
) -> Result<Vec<Map<String, Value>>, ReportServiceError> {
    let interval = block.source.interval.as_deref().unwrap_or("24h");
    let service = provider.connections.rate_limit_service();
    let statuses = service
        .list_all_rate_limits(tenant_id, Some(interval))
        .await?;
    Ok(statuses.into_iter().map(rate_limit_status_row).collect())
}

async fn connection_rate_limit_event_rows(
    provider: &SystemProvider,
    tenant_id: &str,
    block: &ReportBlockDefinition,
    condition: Option<&Condition>,
) -> Result<Vec<Map<String, Value>>, ReportServiceError> {
    let service = provider.connections.rate_limit_service();
    let connection_id = system_connection_id(block, condition);
    let (from, to) = extract_time_bounds(condition, &["createdAt"]);
    let from = from.or_else(|| Some(Utc::now() - Duration::days(30)));
    let event_type = extract_eq_string_condition(condition, "eventType");
    let limit = block.source.limit.unwrap_or(1000).clamp(1, 1000);

    let mut events = Vec::new();
    if let Some(connection_id) = connection_id {
        let response = service
            .get_rate_limit_history(
                &connection_id,
                tenant_id,
                &runtara_connections::types::RateLimitHistoryQuery {
                    limit,
                    offset: 0,
                    event_type,
                    from,
                    to,
                },
            )
            .await?;
        events.extend(response.data);
    } else {
        let statuses = service.list_all_rate_limits(tenant_id, Some("24h")).await?;
        for status in statuses {
            let response = service
                .get_rate_limit_history(
                    &status.connection_id,
                    tenant_id,
                    &runtara_connections::types::RateLimitHistoryQuery {
                        limit,
                        offset: 0,
                        event_type: event_type.clone(),
                        from,
                        to,
                    },
                )
                .await?;
            events.extend(response.data);
        }
    }

    events.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    events.truncate(limit as usize);
    Ok(events.into_iter().map(rate_limit_event_row).collect())
}

async fn connection_rate_limit_timeline_rows(
    provider: &SystemProvider,
    tenant_id: &str,
    block: &ReportBlockDefinition,
    condition: Option<&Condition>,
) -> Result<Vec<Map<String, Value>>, ReportServiceError> {
    let Some(connection_id) = system_connection_id(block, condition) else {
        return Ok(Vec::new());
    };

    let now = Utc::now();
    let (start_time, end_time) =
        extract_time_bounds(condition, &["bucket", "bucketTime", "createdAt"]);
    let end_time = end_time.unwrap_or(now);
    let start_time = start_time.unwrap_or(end_time - Duration::hours(24));
    let granularity = block
        .source
        .granularity
        .clone()
        .unwrap_or_else(|| infer_rate_limit_timeline_granularity(start_time, end_time));
    let tag = extract_eq_string_condition(condition, "tag");

    let service = provider.connections.rate_limit_service();
    let response = service
        .get_rate_limit_timeline(
            &connection_id,
            tenant_id,
            &runtara_connections::types::RateLimitTimelineQuery {
                start_time: Some(start_time),
                end_time: Some(end_time),
                granularity: granularity.clone(),
                tag,
            },
        )
        .await?;

    Ok(response
        .data
        .buckets
        .into_iter()
        .map(|bucket| {
            Map::from_iter([
                (
                    "connectionId".to_string(),
                    Value::String(connection_id.clone()),
                ),
                (
                    "bucket".to_string(),
                    Value::String(bucket.bucket.to_rfc3339()),
                ),
                (
                    "bucketTime".to_string(),
                    Value::String(bucket.bucket.to_rfc3339()),
                ),
                (
                    "granularity".to_string(),
                    Value::String(granularity.clone()),
                ),
                ("requestCount".to_string(), json!(bucket.request_count)),
                (
                    "rateLimitedCount".to_string(),
                    json!(bucket.rate_limited_count),
                ),
                ("retryCount".to_string(), json!(bucket.retry_count)),
            ])
        })
        .collect())
}

// ============================================================================
// Row builders
// ============================================================================

fn runtime_system_snapshot_row() -> Map<String, Value> {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();

    let total_memory = sys.total_memory();
    let available_memory = sys.available_memory();
    let available_for_workflows = (available_memory as f64 * 0.8) as u64;
    let used_memory = total_memory.saturating_sub(available_memory);
    let memory_used_percent = percent(used_memory as f64, total_memory as f64);

    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string());
    let data_path = PathBuf::from(&data_dir);
    let canonical_path = data_path.canonicalize().unwrap_or(data_path);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let disk_info = disks
        .iter()
        .filter(|disk| canonical_path.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| {
            (
                disk.total_space(),
                disk.available_space(),
                canonical_path.display().to_string(),
            )
        })
        .or_else(|| {
            disks
                .iter()
                .next()
                .map(|disk| (disk.total_space(), disk.available_space(), data_dir.clone()))
        })
        .unwrap_or((0, 0, data_dir));
    let disk_used = disk_info.0.saturating_sub(disk_info.1);
    let disk_used_percent = percent(disk_used as f64, disk_info.0 as f64);

    Map::from_iter([
        (
            "capturedAt".to_string(),
            Value::String(Utc::now().to_rfc3339()),
        ),
        (
            "cpuArchitecture".to_string(),
            Value::String(std::env::consts::ARCH.to_string()),
        ),
        (
            "cpuPhysicalCores".to_string(),
            json!(num_cpus::get_physical()),
        ),
        ("cpuLogicalCores".to_string(), json!(num_cpus::get())),
        ("memoryTotalBytes".to_string(), json!(total_memory)),
        ("memoryAvailableBytes".to_string(), json!(available_memory)),
        (
            "memoryAvailableForWorkflowsBytes".to_string(),
            json!(available_for_workflows),
        ),
        ("memoryUsedBytes".to_string(), json!(used_memory)),
        (
            "memoryUsedPercent".to_string(),
            f64_value(memory_used_percent),
        ),
        ("diskPath".to_string(), Value::String(disk_info.2)),
        ("diskTotalBytes".to_string(), json!(disk_info.0)),
        ("diskAvailableBytes".to_string(), json!(disk_info.1)),
        ("diskUsedBytes".to_string(), json!(disk_used)),
        ("diskUsedPercent".to_string(), f64_value(disk_used_percent)),
    ])
}

fn rate_limit_status_row(
    status: runtara_connections::types::RateLimitStatusDto,
) -> Map<String, Value> {
    let config_value = status
        .config
        .as_ref()
        .and_then(|config| serde_json::to_value(config).ok())
        .unwrap_or(Value::Null);
    let state_value = serde_json::to_value(&status.state).unwrap_or(Value::Null);
    let metrics_value = serde_json::to_value(&status.metrics).unwrap_or(Value::Null);
    let period_value = status
        .period_stats
        .as_ref()
        .and_then(|stats| serde_json::to_value(stats).ok())
        .unwrap_or(Value::Null);

    let mut row = Map::new();
    row.insert(
        "connectionId".to_string(),
        Value::String(status.connection_id),
    );
    row.insert(
        "connectionTitle".to_string(),
        Value::String(status.connection_title),
    );
    row.insert(
        "integrationId".to_string(),
        status
            .integration_id
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    row.insert("config".to_string(), config_value);
    row.insert("state".to_string(), state_value);
    row.insert("metrics".to_string(), metrics_value);
    row.insert("periodStats".to_string(), period_value);

    if let Some(config) = status.config {
        row.insert(
            "configRequestsPerSecond".to_string(),
            json!(config.requests_per_second),
        );
        row.insert("configBurstSize".to_string(), json!(config.burst_size));
        row.insert(
            "configRetryOnLimit".to_string(),
            Value::Bool(config.retry_on_limit),
        );
        row.insert("configMaxRetries".to_string(), json!(config.max_retries));
        row.insert("configMaxWaitMs".to_string(), json!(config.max_wait_ms));
    } else {
        row.insert("configRequestsPerSecond".to_string(), Value::Null);
        row.insert("configBurstSize".to_string(), Value::Null);
        row.insert("configRetryOnLimit".to_string(), Value::Null);
        row.insert("configMaxRetries".to_string(), Value::Null);
        row.insert("configMaxWaitMs".to_string(), Value::Null);
    }

    row.insert(
        "stateAvailable".to_string(),
        Value::Bool(status.state.available),
    );
    row.insert(
        "stateCurrentTokens".to_string(),
        option_f64_value(status.state.current_tokens),
    );
    row.insert(
        "stateLastRefillMs".to_string(),
        status
            .state
            .last_refill_ms
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "stateLearnedLimit".to_string(),
        status
            .state
            .learned_limit
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "stateCallsInWindow".to_string(),
        status
            .state
            .calls_in_window
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "stateTotalCalls".to_string(),
        status
            .state
            .total_calls
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "stateWindowStartMs".to_string(),
        status
            .state
            .window_start_ms
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    row.insert(
        "capacityPercent".to_string(),
        option_f64_value(status.metrics.capacity_percent),
    );
    row.insert(
        "utilizationPercent".to_string(),
        option_f64_value(status.metrics.utilization_percent),
    );
    row.insert(
        "isRateLimited".to_string(),
        Value::Bool(status.metrics.is_rate_limited),
    );
    row.insert(
        "retryAfterMs".to_string(),
        status
            .metrics
            .retry_after_ms
            .map(Value::from)
            .unwrap_or(Value::Null),
    );

    if let Some(period) = status.period_stats {
        row.insert("periodInterval".to_string(), Value::String(period.interval));
        row.insert(
            "periodTotalRequests".to_string(),
            json!(period.total_requests),
        );
        row.insert(
            "periodRateLimitedCount".to_string(),
            json!(period.rate_limited_count),
        );
        row.insert("periodRetryCount".to_string(), json!(period.retry_count));
        row.insert(
            "periodRateLimitedPercent".to_string(),
            f64_value(period.rate_limited_percent),
        );
    } else {
        row.insert("periodInterval".to_string(), Value::Null);
        row.insert("periodTotalRequests".to_string(), Value::Null);
        row.insert("periodRateLimitedCount".to_string(), Value::Null);
        row.insert("periodRetryCount".to_string(), Value::Null);
        row.insert("periodRateLimitedPercent".to_string(), Value::Null);
    }

    row
}

fn rate_limit_event_row(
    event: runtara_connections::types::RateLimitEventDto,
) -> Map<String, Value> {
    let tag = event
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("tag"))
        .cloned()
        .unwrap_or(Value::Null);
    Map::from_iter([
        ("id".to_string(), json!(event.id)),
        (
            "connectionId".to_string(),
            Value::String(event.connection_id),
        ),
        ("eventType".to_string(), Value::String(event.event_type)),
        (
            "createdAt".to_string(),
            Value::String(event.created_at.to_rfc3339()),
        ),
        (
            "metadata".to_string(),
            event.metadata.unwrap_or(Value::Null),
        ),
        ("tag".to_string(), tag),
    ])
}

// ============================================================================
// Entity / field metadata
// ============================================================================

pub(crate) fn system_entity(
    block: &ReportBlockDefinition,
) -> Result<ReportWorkflowRuntimeEntity, ReportServiceError> {
    let entity = block.source.entity.ok_or_else(|| {
        ReportServiceError::Validation(format!(
            "Block '{}' system source must specify entity",
            block.id
        ))
    })?;
    match entity {
        ReportWorkflowRuntimeEntity::RuntimeExecutionMetricBuckets
        | ReportWorkflowRuntimeEntity::RuntimeSystemSnapshot
        | ReportWorkflowRuntimeEntity::ConnectionRateLimitStatus
        | ReportWorkflowRuntimeEntity::ConnectionRateLimitEvents
        | ReportWorkflowRuntimeEntity::ConnectionRateLimitTimeline => Ok(entity),
        ReportWorkflowRuntimeEntity::Instances | ReportWorkflowRuntimeEntity::Actions => {
            Err(ReportServiceError::Validation(format!(
                "Block '{}' system source does not support workflow_runtime entity {:?}",
                block.id, entity
            )))
        }
    }
}

pub(crate) fn system_fields(entity: ReportWorkflowRuntimeEntity) -> HashSet<&'static str> {
    match entity {
        ReportWorkflowRuntimeEntity::RuntimeExecutionMetricBuckets => [
            "tenantId",
            "bucketTime",
            "granularity",
            "invocationCount",
            "successCount",
            "failureCount",
            "cancelledCount",
            "avgDurationSeconds",
            "minDurationSeconds",
            "maxDurationSeconds",
            "avgMemoryBytes",
            "maxMemoryBytes",
            "successRatePercent",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::RuntimeSystemSnapshot => [
            "capturedAt",
            "cpuArchitecture",
            "cpuPhysicalCores",
            "cpuLogicalCores",
            "memoryTotalBytes",
            "memoryAvailableBytes",
            "memoryAvailableForWorkflowsBytes",
            "memoryUsedBytes",
            "memoryUsedPercent",
            "diskPath",
            "diskTotalBytes",
            "diskAvailableBytes",
            "diskUsedBytes",
            "diskUsedPercent",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::ConnectionRateLimitStatus => [
            "connectionId",
            "connectionTitle",
            "integrationId",
            "config",
            "state",
            "metrics",
            "periodStats",
            "configRequestsPerSecond",
            "configBurstSize",
            "configRetryOnLimit",
            "configMaxRetries",
            "configMaxWaitMs",
            "stateAvailable",
            "stateCurrentTokens",
            "stateLastRefillMs",
            "stateLearnedLimit",
            "stateCallsInWindow",
            "stateTotalCalls",
            "stateWindowStartMs",
            "capacityPercent",
            "utilizationPercent",
            "isRateLimited",
            "retryAfterMs",
            "periodInterval",
            "periodTotalRequests",
            "periodRateLimitedCount",
            "periodRetryCount",
            "periodRateLimitedPercent",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::ConnectionRateLimitEvents => [
            "id",
            "connectionId",
            "eventType",
            "createdAt",
            "metadata",
            "tag",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::ConnectionRateLimitTimeline => [
            "connectionId",
            "bucket",
            "bucketTime",
            "granularity",
            "requestCount",
            "rateLimitedCount",
            "retryCount",
        ]
        .into_iter()
        .collect(),
        ReportWorkflowRuntimeEntity::Instances | ReportWorkflowRuntimeEntity::Actions => {
            HashSet::new()
        }
    }
}

pub(crate) fn system_row_field_known(fields: &HashSet<&'static str>, field: &str) -> bool {
    fields.contains(field)
        || field
            .split_once('.')
            .is_some_and(|(root, _)| fields.contains(root))
}

pub(crate) fn system_table_columns(
    table: Option<&ReportTableConfig>,
    entity: ReportWorkflowRuntimeEntity,
) -> Vec<Value> {
    if let Some(table) = table
        && !table.columns.is_empty()
    {
        return table
            .columns
            .iter()
            .map(|column| {
                json!({
                    "key": column.field,
                    "label": column.label.clone().unwrap_or_else(|| humanize_label(&column.field)),
                    "format": column.format,
                })
            })
            .collect();
    }

    let columns: &[(&str, &str, Option<&str>)] = match entity {
        ReportWorkflowRuntimeEntity::RuntimeExecutionMetricBuckets => &[
            ("bucketTime", "Bucket", Some("datetime")),
            ("invocationCount", "Invocations", Some("number")),
            ("successCount", "Successes", Some("number")),
            ("failureCount", "Failures", Some("number")),
            ("cancelledCount", "Cancelled", Some("number")),
            ("successRatePercent", "Success Rate", Some("percent")),
            ("avgDurationSeconds", "Avg Duration", Some("number")),
            ("maxMemoryBytes", "Max Memory", Some("bytes")),
        ],
        ReportWorkflowRuntimeEntity::RuntimeSystemSnapshot => &[
            ("capturedAt", "Captured", Some("datetime")),
            ("cpuArchitecture", "CPU Architecture", None),
            ("cpuPhysicalCores", "Physical Cores", Some("number")),
            ("cpuLogicalCores", "Logical Cores", Some("number")),
            ("memoryUsedPercent", "Memory Used", Some("percent")),
            ("diskUsedPercent", "Disk Used", Some("percent")),
        ],
        ReportWorkflowRuntimeEntity::ConnectionRateLimitStatus => &[
            ("connectionTitle", "Connection", None),
            ("integrationId", "Integration", None),
            ("isRateLimited", "Rate Limited", Some("boolean")),
            ("capacityPercent", "Capacity", Some("percent")),
            ("utilizationPercent", "Utilization", Some("percent")),
            ("periodTotalRequests", "Requests", Some("number")),
            ("periodRateLimitedCount", "Limited", Some("number")),
            ("periodRetryCount", "Retries", Some("number")),
        ],
        ReportWorkflowRuntimeEntity::ConnectionRateLimitEvents => &[
            ("createdAt", "Created", Some("datetime")),
            ("connectionId", "Connection", None),
            ("eventType", "Event", None),
            ("tag", "Tag", None),
        ],
        ReportWorkflowRuntimeEntity::ConnectionRateLimitTimeline => &[
            ("bucketTime", "Bucket", Some("datetime")),
            ("requestCount", "Requests", Some("number")),
            ("rateLimitedCount", "Limited", Some("number")),
            ("retryCount", "Retries", Some("number")),
        ],
        ReportWorkflowRuntimeEntity::Instances | ReportWorkflowRuntimeEntity::Actions => &[],
    };

    columns
        .iter()
        .map(|(key, label, format)| {
            json!({
                "key": key,
                "label": label,
                "format": format,
            })
        })
        .collect()
}

// ============================================================================
// Validation
// ============================================================================

fn validate_system_block(
    block: &ReportBlockDefinition,
    filter_ids: &HashSet<String>,
    view_ids: &HashSet<String>,
    filter_defs: &HashMap<String, &ReportFilterDefinition>,
) -> Result<(), ReportServiceError> {
    let entity = system_entity(block)?;
    if !block.source.schema.trim().is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' system source must not set schema",
            block.id
        )));
    }
    if block.source.workflow_id.is_some() || block.source.instance_id.is_some() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' system source must not set workflowId or instanceId",
            block.id
        )));
    }
    if !block.source.join.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' system source does not support joins",
            block.id
        )));
    }
    if matches!(
        block.block_type,
        ReportBlockType::Actions | ReportBlockType::Card
    ) {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' system source only supports table, chart, metric, and markdown blocks",
            block.id
        )));
    }

    let fields = system_fields(entity);
    let agg_output = aggregate_output_fields(block);
    validate_report_aggregate_specs(&format!("block '{}'", block.id), &block.source.aggregates)?;
    if block.source.mode == ReportSourceMode::Aggregate && block.source.aggregates.is_empty() {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' aggregate source must define at least one aggregate",
            block.id
        )));
    }
    validate_report_condition_filter_refs(
        block.source.condition.as_ref(),
        filter_defs,
        &format!("block '{}'", block.id),
    )?;
    validate_report_condition_field_refs(
        block.source.condition.as_ref(),
        &|field| system_row_field_known(&fields, field),
        &format!("block '{}'", block.id),
    )?;
    validate_report_source_filter_mappings(
        &block.source.filter_mappings,
        filter_ids,
        &|field| system_row_field_known(&fields, field),
        "source.filterMappings",
        &format!("block '{}'", block.id),
    )?;
    let is_table_value_field = |field: &str| -> bool {
        match block.source.mode {
            ReportSourceMode::Filter => system_row_field_known(&fields, field),
            ReportSourceMode::Aggregate => agg_output.contains(field),
        }
    };

    if let Some(table) = &block.table {
        validate_report_table_display_templates(table, &format!("block '{}'", block.id))?;
        for column in &table.columns {
            if column.is_interaction_buttons() {
                validate_report_interaction_buttons(
                    &column.interaction_buttons,
                    filter_ids,
                    view_ids,
                    &is_table_value_field,
                    &format!(
                        "Block '{}' interaction button column '{}'",
                        block.id, column.field
                    ),
                )?;
                continue;
            }
            if column.is_chart() || column.is_value_lookup() || column.is_workflow_button() {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' system table columns cannot use nested sources or workflow buttons",
                    block.id
                )));
            }
            if !is_table_value_field(&column.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown system table field '{}'",
                    block.id, column.field
                )));
            }
            if let Some(display_field) = &column.display_field
                && !is_table_value_field(display_field)
            {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown system table displayField '{}'",
                    block.id, display_field
                )));
            }
        }
        for sort in &table.default_sort {
            if !is_table_value_field(&sort.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown system table sort field '{}'",
                    block.id, sort.field
                )));
            }
        }
        for action in &table.actions {
            validate_report_table_action_config(
                action,
                &format!("block '{}' table action '{}'", block.id, action.id),
            )?;
        }
    }

    for group_field in &block.source.group_by {
        if !system_row_field_known(&fields, group_field) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' references unknown system groupBy field '{}'",
                block.id, group_field
            )));
        }
    }
    for aggregate in &block.source.aggregates {
        if let Some(field) = &aggregate.field
            && !system_row_field_known(&fields, field)
        {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' references unknown system aggregate field '{}'",
                block.id, field
            )));
        }
        for order_by in &aggregate.order_by {
            if !system_row_field_known(&fields, &order_by.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' aggregate '{}' references unknown system orderBy field '{}'",
                    block.id, aggregate.alias, order_by.field
                )));
            }
        }
    }
    for order_by in &block.source.order_by {
        let known = match block.source.mode {
            ReportSourceMode::Filter => system_row_field_known(&fields, &order_by.field),
            ReportSourceMode::Aggregate => agg_output.contains(&order_by.field),
        };
        if !known {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' references unknown system orderBy field '{}'",
                block.id, order_by.field
            )));
        }
    }
    if let Some(chart) = &block.chart {
        if !agg_output.contains(&chart.x) {
            return Err(ReportServiceError::Validation(format!(
                "Block '{}' references unknown system chart x field '{}'",
                block.id, chart.x
            )));
        }
        for series in &chart.series {
            if !agg_output.contains(&series.field) {
                return Err(ReportServiceError::Validation(format!(
                    "Block '{}' references unknown system chart series field '{}'",
                    block.id, series.field
                )));
            }
        }
    }
    if let Some(metric) = &block.metric
        && !agg_output.contains(&metric.value_field)
    {
        return Err(ReportServiceError::Validation(format!(
            "Block '{}' references unknown system metric valueField '{}'",
            block.id, metric.value_field
        )));
    }

    validate_block_interactions(block, filter_ids, view_ids)
}

// ============================================================================
// Helpers (system-only)
// ============================================================================

fn percent(numerator: f64, denominator: f64) -> f64 {
    if denominator <= 0.0 {
        0.0
    } else {
        (numerator / denominator) * 100.0
    }
}

fn parse_metrics_granularity(
    granularity: Option<&str>,
) -> Result<MetricsGranularity, ReportServiceError> {
    match granularity
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("hourly")
        .to_ascii_lowercase()
        .as_str()
    {
        "hour" | "hourly" => Ok(MetricsGranularity::Hourly),
        "day" | "daily" => Ok(MetricsGranularity::Daily),
        other => Err(ReportServiceError::Validation(format!(
            "Unsupported system metrics granularity '{}'. Use hourly or daily.",
            other
        ))),
    }
}

fn infer_rate_limit_timeline_granularity(
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
) -> String {
    let duration = end_time - start_time;
    if duration <= Duration::hours(2) {
        "minute".to_string()
    } else if duration <= Duration::days(7) {
        "hourly".to_string()
    } else {
        "daily".to_string()
    }
}

fn system_connection_id(
    block: &ReportBlockDefinition,
    condition: Option<&Condition>,
) -> Option<String> {
    block
        .source
        .connection_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| extract_eq_string_condition(condition, "connectionId"))
}

fn extract_eq_string_condition(condition: Option<&Condition>, field: &str) -> Option<String> {
    let condition = condition?;
    let op = condition.op.to_ascii_uppercase();
    let args = condition.arguments.as_deref().unwrap_or(&[]);
    if matches!(op.as_str(), "AND" | "OR") {
        return args.iter().find_map(|argument| {
            condition_from_value(argument)
                .as_ref()
                .and_then(|child| extract_eq_string_condition(Some(child), field))
        });
    }
    if op == "EQ" && args.len() == 2 && args.first().and_then(Value::as_str) == Some(field) {
        return args.get(1).and_then(condition_scalar_to_string);
    }
    if op == "IN" && args.len() == 2 && args.first().and_then(Value::as_str) == Some(field) {
        return args
            .get(1)
            .and_then(Value::as_array)
            .and_then(|values| values.iter().find_map(condition_scalar_to_string));
    }
    None
}

fn condition_scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn extract_time_bounds(
    condition: Option<&Condition>,
    fields: &[&str],
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let mut lower: Option<DateTime<Utc>> = None;
    let mut upper: Option<DateTime<Utc>> = None;
    if let Some(condition) = condition {
        collect_time_bounds(condition, fields, &mut lower, &mut upper);
    }
    (lower, upper)
}

fn collect_time_bounds(
    condition: &Condition,
    fields: &[&str],
    lower: &mut Option<DateTime<Utc>>,
    upper: &mut Option<DateTime<Utc>>,
) {
    let op = condition.op.to_ascii_uppercase();
    let args = condition.arguments.as_deref().unwrap_or(&[]);

    for argument in args {
        if let Some(child) = condition_from_value(argument) {
            collect_time_bounds(&child, fields, lower, upper);
        }
    }

    if args.len() != 2 {
        return;
    }
    let Some(field) = args.first().and_then(Value::as_str) else {
        return;
    };
    if !fields.contains(&field) {
        return;
    }
    let Some(bound) = args.get(1).and_then(parse_datetime_value) else {
        return;
    };
    match op.as_str() {
        "GT" | "GTE" => {
            if lower.is_none_or(|current| bound > current) {
                *lower = Some(bound);
            }
        }
        "LT" | "LTE" => {
            if upper.is_none_or(|current| bound < current) {
                *upper = Some(bound);
            }
        }
        _ => {}
    }
}

fn parse_datetime_value(value: &Value) -> Option<DateTime<Utc>> {
    match value {
        Value::String(value) => DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.with_timezone(&Utc)),
        Value::Number(value) => value
            .as_i64()
            .and_then(DateTime::<Utc>::from_timestamp_millis),
        _ => None,
    }
}
