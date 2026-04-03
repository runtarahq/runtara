use chrono::{DateTime, Utc};
use runtara_management_sdk::{
    EventSortOrder, InstanceInfo, InstanceStatus as RuntaraInstanceStatus, ListEventsOptions,
    ListInstancesOptions, ListInstancesOrder,
};
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::api::dto::executions::ExecutionFilters;
use crate::api::dto::scenarios::{
    InstanceInputs, PageScenarioInstanceHistoryDto, ScenarioInstanceDto,
};
use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::repositories::scenarios::ScenarioRepository;
use crate::api::repositories::trigger_stream::TriggerStreamPublisher;
use crate::runtime_client::RuntimeClient;
use crate::types::ExecutionStatus;

/// Extended execution data with metadata from scenario
///
/// Used when fetching a single execution with full details.
#[derive(Debug)]
pub struct ExecutionWithMetadata {
    pub instance: ScenarioInstanceDto,
    pub scenario_name: Option<String>,
    pub scenario_description: Option<String>,
    pub worker_id: Option<String>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub retry_count: Option<i32>,
    pub max_retries: Option<i32>,
    pub additional_metadata: Option<Value>,
    pub error_message: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Service for scenario execution operations
///
/// All execution data is stored in runtara-environment. This service proxies
/// queries to Runtara via RuntimeClient.
pub struct ExecutionService {
    scenario_repo: Arc<ScenarioRepository>,
    trigger_stream: Option<Arc<TriggerStreamPublisher>>,
    /// Runtime client for proxying queries to Runtara Environment
    runtime_client: Option<Arc<RuntimeClient>>,
}

impl ExecutionService {
    /// Create a new service with runtime client for proxying to Runtara
    ///
    /// This is the primary constructor. All execution queries go through
    /// runtara-environment.
    pub fn new(scenario_repo: Arc<ScenarioRepository>, runtime_client: Arc<RuntimeClient>) -> Self {
        Self {
            scenario_repo,
            trigger_stream: None,
            runtime_client: Some(runtime_client),
        }
    }

    /// Create a service with trigger stream for queuing executions
    pub fn with_trigger_stream(
        scenario_repo: Arc<ScenarioRepository>,
        trigger_stream: Arc<TriggerStreamPublisher>,
    ) -> Self {
        Self {
            scenario_repo,
            trigger_stream: Some(trigger_stream),
            runtime_client: None,
        }
    }

    /// Create a fully-configured service with both trigger stream and runtime client
    #[allow(dead_code)]
    pub fn with_all(
        scenario_repo: Arc<ScenarioRepository>,
        trigger_stream: Arc<TriggerStreamPublisher>,
        runtime_client: Arc<RuntimeClient>,
    ) -> Self {
        Self {
            scenario_repo,
            trigger_stream: Some(trigger_stream),
            runtime_client: Some(runtime_client),
        }
    }

    /// Queue a scenario execution
    ///
    /// This validates the scenario exists, validates inputs against the input schema,
    /// then publishes a trigger event to the Valkey stream for background processing.
    /// The trigger worker will handle compilation and execution.
    pub async fn queue_execution(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: Option<i32>,
        inputs: Value,
        debug: bool,
    ) -> Result<QueuedExecutionResult, ServiceError> {
        // 1. Resolve version (explicit or current/latest)
        let version = match version {
            Some(v) => v,
            None => self
                .scenario_repo
                .get_current_or_latest_version(tenant_id, scenario_id)
                .await
                .map_err(|e| {
                    ServiceError::DatabaseError(format!("Failed to get current version: {}", e))
                })?
                .ok_or_else(|| {
                    ServiceError::NotFound(format!("Scenario '{}' not found", scenario_id))
                })?,
        };

        if version == 0 {
            return Err(ServiceError::NotFound(format!(
                "Scenario '{}' has no versions",
                scenario_id
            )));
        }

        // 2. Get scenario with input schema for validation
        let scenario = self
            .scenario_repo
            .get_by_id(tenant_id, scenario_id, Some(version))
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to get scenario: {}", e)))?
            .ok_or_else(|| {
                ServiceError::NotFound(if version > 0 {
                    format!("Scenario '{}' version {} not found", scenario_id, version)
                } else {
                    format!("Scenario '{}' not found", scenario_id)
                })
            })?;

        // 3. Validate inputs against input schema (if schema is not empty)
        //    The schema describes the user data shape, so validate inputs.data
        //    (not the full {data, variables} wrapper).
        if !is_empty_schema(&scenario.input_schema) {
            let data_to_validate = inputs.get("data").cloned().unwrap_or(serde_json::json!({}));
            validate_inputs(&data_to_validate, &scenario.input_schema).map_err(|e| {
                ServiceError::ValidationError(format!("Input validation failed: {}", e))
            })?;
        }

        // 4. Get track_events (already have it from scenario)
        let track_events = scenario.track_events;

        // 5. Get trigger stream (required for execution)
        let trigger_stream = self.trigger_stream.as_ref().ok_or_else(|| {
            ServiceError::DatabaseError(
                "Valkey trigger stream not configured. Cannot queue execution.".to_string(),
            )
        })?;

        // 6. Generate instance ID
        let instance_id = Uuid::new_v4();

        // 7. Publish to trigger stream (Valkey)
        let event = TriggerEvent::http_api(
            instance_id.to_string(),
            tenant_id.to_string(),
            scenario_id.to_string(),
            Some(version),
            inputs,
            track_events,
            None, // correlation_id
            debug,
        );

        // Publish to stream
        trigger_stream
            .publish(tenant_id, &event)
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to publish to trigger stream: {}", e))
            })?;

        info!(
            instance_id = %instance_id,
            scenario_id = %scenario_id,
            version = version,
            "Published execution to trigger stream"
        );

        Ok(QueuedExecutionResult {
            instance_id,
            status: "queued".to_string(),
        })
    }

    /// Get execution results by instance ID
    ///
    /// Proxies to Runtara Environment via RuntimeClient. The instance status and output
    /// are fetched directly from runtara-environment.
    pub async fn get_execution_results(
        &self,
        instance_id: &str,
        _tenant_id: &str,
    ) -> Result<ScenarioInstanceDto, ServiceError> {
        // Proxy to Runtara - this is the source of truth for executions
        let client = self.runtime_client.as_ref().ok_or_else(|| {
            ServiceError::DatabaseError("Runtime client not configured. Cannot get execution without runtara-environment connection.".to_string())
        })?;

        // Get instance info from Runtara
        let info = client.get_instance_info(instance_id).await.map_err(|e| {
            if e.to_string().contains("not found") {
                ServiceError::NotFound(format!("Instance '{}' not found", instance_id))
            } else {
                ServiceError::DatabaseError(format!("Failed to get instance from Runtara: {}", e))
            }
        })?;

        Ok(runtara_info_to_dto(info))
    }

    /// Get execution by scenario and instance with extended metadata
    ///
    /// Fetches from Runtara Environment and enriches with scenario metadata from the local database.
    /// Requires runtime_client to be configured.
    pub async fn get_execution_with_metadata(
        &self,
        scenario_id: &str,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<ExecutionWithMetadata, ServiceError> {
        // Parse UUID to validate format
        let _instance_uuid = Uuid::parse_str(instance_id).map_err(|_| {
            ServiceError::ValidationError(
                "Invalid instance ID format. Instance ID must be a valid UUID".to_string(),
            )
        })?;

        // Require runtime_client - runtara-environment is the source of truth
        let client = self.runtime_client.as_ref().ok_or_else(|| {
            ServiceError::DatabaseError("Runtime client not configured. Cannot get execution without runtara-environment connection.".to_string())
        })?;

        self.get_execution_with_metadata_from_runtara(client, scenario_id, instance_id, tenant_id)
            .await
    }

    /// Get execution with metadata from Runtara Environment
    ///
    /// Fetches instance info from Runtara and enriches with scenario metadata.
    async fn get_execution_with_metadata_from_runtara(
        &self,
        client: &RuntimeClient,
        scenario_id: &str,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<ExecutionWithMetadata, ServiceError> {
        // Get instance info from Runtara
        let info = client.get_instance_info(instance_id).await.map_err(|e| {
            let error_str = e.to_string();
            warn!(
                instance_id = %instance_id,
                scenario_id = %scenario_id,
                error = %error_str,
                "Failed to get instance info from Runtara"
            );
            if error_str.contains("not found") || error_str.contains("InstanceNotFound") {
                ServiceError::NotFound(format!(
                    "Instance '{}' not found for scenario '{}'",
                    instance_id, scenario_id
                ))
            } else {
                ServiceError::DatabaseError(format!("Failed to get instance from Runtara: {}", e))
            }
        })?;

        // Verify the instance belongs to the expected scenario by checking image_name
        // Image names follow the pattern: {scenario_id}:{version}
        let expected_prefix = format!("{}:", scenario_id);
        debug!(
            instance_id = %instance_id,
            image_name = %info.image_name,
            image_id = %info.image_id,
            expected_prefix = %expected_prefix,
            "Checking instance scenario match"
        );
        if !info.image_name.starts_with(&expected_prefix) {
            warn!(
                instance_id = %instance_id,
                image_name = %info.image_name,
                expected_prefix = %expected_prefix,
                "Instance image_name does not match expected scenario prefix"
            );
            return Err(ServiceError::NotFound(format!(
                "Instance '{}' not found for scenario '{}'",
                instance_id, scenario_id
            )));
        }

        // Get scenario metadata for enrichment
        let scenario = self
            .scenario_repo
            .get_by_id(tenant_id, scenario_id, None)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to get scenario: {}", e)))?;

        let (scenario_name, scenario_description) = match scenario {
            Some(s) => (Some(s.name), Some(s.description)),
            None => (None, None),
        };

        // Convert InstanceInfo to ExecutionWithMetadata
        let mut result =
            runtara_info_to_execution_with_metadata(info, scenario_name, scenario_description);

        // Enrich with has_pending_input flag
        enrich_pending_input(std::slice::from_mut(&mut result.instance), client).await;

        Ok(result)
    }

    /// List executions for a scenario with pagination
    ///
    /// Proxies to Runtara Environment using image_name_prefix filter
    /// (images are named "{scenario_id}:{version}").
    /// Requires runtime_client to be configured.
    pub async fn list_executions(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        page: Option<i32>,
        size: Option<i32>,
    ) -> Result<PageScenarioInstanceHistoryDto, ServiceError> {
        // Normalize pagination parameters
        let page = page.unwrap_or(0).max(0);
        let size = size.unwrap_or(10).clamp(1, 100);

        // Require runtime_client - runtara-environment is the source of truth
        let client = self.runtime_client.as_ref().ok_or_else(|| {
            ServiceError::DatabaseError("Runtime client not configured. Cannot list executions without runtara-environment connection.".to_string())
        })?;

        self.list_executions_from_runtara(client, tenant_id, scenario_id, page, size)
            .await
    }

    /// List executions from Runtara Environment using image_name_prefix filter
    async fn list_executions_from_runtara(
        &self,
        client: &RuntimeClient,
        tenant_id: &str,
        scenario_id: &str,
        page: i32,
        size: i32,
    ) -> Result<PageScenarioInstanceHistoryDto, ServiceError> {
        // Image names follow pattern: {scenario_id}:{version}
        // Use prefix "{scenario_id}:" to match all versions
        let image_name_prefix = format!("{}:", scenario_id);

        let options = ListInstancesOptions::new()
            .with_tenant_id(tenant_id)
            .with_image_name_prefix(&image_name_prefix)
            .with_limit(size as u32)
            .with_offset((page * size) as u32);

        debug!(
            tenant_id = %tenant_id,
            scenario_id = %scenario_id,
            image_name_prefix = %image_name_prefix,
            page = page,
            size = size,
            "Listing executions from Runtara"
        );

        let result = client
            .list_instances_with_options(options)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to query Runtara: {}", e)))?;

        // We already know the scenario_id, so fetch the scenario name directly
        let scenario_name = match self
            .scenario_repo
            .get_scenario_names_bulk(tenant_id, &[scenario_id.to_string()])
            .await
        {
            Ok(names) => names
                .get(scenario_id)
                .map(|(name, _)| name.clone())
                .filter(|n| !n.is_empty()),
            Err(e) => {
                warn!(
                    tenant_id = %tenant_id,
                    scenario_id = %scenario_id,
                    error = %e,
                    "Failed to fetch scenario name"
                );
                None
            }
        };

        // Collect unique image IDs to look up version info
        let image_ids: Vec<String> = result
            .instances
            .iter()
            .map(|inst| inst.image_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        // Look up version info by registered_image_id
        let version_info: std::collections::HashMap<String, i32> = if !image_ids.is_empty() {
            match self
                .scenario_repo
                .get_scenario_info_by_image_ids(tenant_id, &image_ids)
                .await
            {
                Ok(info) => info.into_iter().map(|(k, (_, ver, _))| (k, ver)).collect(),
                Err(e) => {
                    warn!(
                        tenant_id = %tenant_id,
                        scenario_id = %scenario_id,
                        error = %e,
                        "Failed to fetch version info for executions list"
                    );
                    std::collections::HashMap::new()
                }
            }
        } else {
            std::collections::HashMap::new()
        };

        // Note: If image_id is not in our DB, we just show version=0
        // No fallback to runtara get_image() - it would be slow and shouldn't happen
        // in normal operation since we always store registered_image_id on compilation

        // Convert Runtara instances to ScenarioInstanceDto
        let mut instances: Vec<ScenarioInstanceDto> = result
            .instances
            .into_iter()
            .map(|inst| {
                let version = version_info.get(&inst.image_id).copied().unwrap_or(0);
                runtara_instance_to_dto_with_info(
                    inst,
                    scenario_id.to_string(),
                    version,
                    scenario_name.clone(),
                )
            })
            .collect();

        // Enrich running instances with has_pending_input flag
        enrich_pending_input(&mut instances, client).await;

        // Use total_count from Runtara (server-side filtering gives accurate count)
        let total_elements = result.total_count as i64;
        let total_pages = if total_elements == 0 {
            0
        } else {
            ((total_elements as f64) / (size as f64)).ceil() as i32
        };
        let number_of_elements = instances.len() as i32;

        Ok(PageScenarioInstanceHistoryDto {
            content: instances,
            total_pages,
            total_elements,
            size,
            number: page,
            first: page == 0,
            last: page >= total_pages.max(1) - 1,
            number_of_elements,
        })
    }

    /// List all executions across all scenarios with filtering, sorting, and pagination
    ///
    /// Proxies to Runtara Environment via RuntimeClient. All execution data is stored in
    /// runtara-environment, not in the local database.
    pub async fn list_all_executions(
        &self,
        tenant_id: &str,
        page: Option<i32>,
        size: Option<i32>,
        filters: ExecutionFilters,
    ) -> Result<PageScenarioInstanceHistoryDto, ServiceError> {
        // Normalize pagination parameters
        let page = page.unwrap_or(0).max(0);
        let size = size.unwrap_or(20).clamp(1, 100);

        // Proxy to Runtara - this is the source of truth for executions
        let client = self.runtime_client.as_ref().ok_or_else(|| {
            ServiceError::DatabaseError("Runtime client not configured. Cannot list executions without runtara-environment connection.".to_string())
        })?;

        self.list_all_executions_from_runtara(client, tenant_id, page, size, &filters)
            .await
    }

    /// List all executions from Runtara Environment
    async fn list_all_executions_from_runtara(
        &self,
        client: &RuntimeClient,
        tenant_id: &str,
        page: i32,
        size: i32,
        filters: &ExecutionFilters,
    ) -> Result<PageScenarioInstanceHistoryDto, ServiceError> {
        // Build Runtara query options
        let mut options = ListInstancesOptions::new()
            .with_tenant_id(tenant_id)
            .with_limit(size as u32)
            .with_offset((page * size) as u32);

        // Apply scenario_id filter if provided (images are named "{scenario_id}:{version}")
        if let Some(ref scenario_id) = filters.scenario_id {
            let image_name_prefix = format!("{}:", scenario_id);
            options = options.with_image_name_prefix(&image_name_prefix);
        }

        // Apply status filter if provided
        if let Some(ref statuses) = filters.statuses {
            // Convert local status strings to Runtara status
            // Only apply first status for now (Runtara SDK may not support multiple)
            if let Some(first_status) = statuses.first()
                && let Some(runtara_status) = execution_status_to_runtara(first_status)
            {
                options = options.with_status(runtara_status);
            }
        }

        // Apply date filters
        if let Some(created_from) = filters.created_from {
            options = options.with_created_after(created_from);
        }
        if let Some(created_to) = filters.created_to {
            options = options.with_created_before(created_to);
        }
        if let Some(completed_from) = filters.completed_from {
            options = options.with_finished_after(completed_from);
        }
        if let Some(completed_to) = filters.completed_to {
            options = options.with_finished_before(completed_to);
        }

        // Apply sorting
        let order = match (filters.sort_by.as_str(), filters.sort_order.as_str()) {
            ("created_at", "ASC") => ListInstancesOrder::CreatedAtAsc,
            ("created_at", "DESC") => ListInstancesOrder::CreatedAtDesc,
            ("completed_at", "ASC") => ListInstancesOrder::FinishedAtAsc,
            ("completed_at", "DESC") => ListInstancesOrder::FinishedAtDesc,
            // For status/scenario_id sorting, Runtara doesn't support these natively,
            // so fall back to finished_at (completed_at) which is the default
            (_, "ASC") => ListInstancesOrder::FinishedAtAsc,
            _ => ListInstancesOrder::FinishedAtDesc,
        };
        options = options.with_order_by(order);

        debug!(
            tenant_id = %tenant_id,
            page = page,
            size = size,
            scenario_id_filter = ?filters.scenario_id,
            status_filter = ?filters.statuses,
            created_from = ?filters.created_from,
            created_to = ?filters.created_to,
            completed_from = ?filters.completed_from,
            completed_to = ?filters.completed_to,
            "Listing all executions from Runtara"
        );

        let result = client
            .list_instances_with_options(options)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to query Runtara: {}", e)))?;

        // Collect unique image IDs (UUIDs from Runtara) to look up scenario info
        let image_ids: Vec<String> = result
            .instances
            .iter()
            .map(|inst| inst.image_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        // Look up scenario info by registered_image_id from our database
        let scenario_info: std::collections::HashMap<String, (String, i32, String)> =
            if !image_ids.is_empty() {
                match self
                    .scenario_repo
                    .get_scenario_info_by_image_ids(tenant_id, &image_ids)
                    .await
                {
                    Ok(info) => info,
                    Err(e) => {
                        warn!(
                            tenant_id = %tenant_id,
                            error = %e,
                            "Failed to fetch scenario info for executions list"
                        );
                        std::collections::HashMap::new()
                    }
                }
            } else {
                std::collections::HashMap::new()
            };

        // Note: If image_id is not in our DB, we just show empty scenario info
        // No fallback to runtara get_image() - it would be slow and shouldn't happen
        // in normal operation since we always store registered_image_id on compilation

        // Fetch scenario names for all scenarios that don't have names yet
        let scenario_ids_needing_names: Vec<String> = scenario_info
            .values()
            .filter(|(_, _, name)| name.is_empty())
            .map(|(sid, _, _)| sid.clone())
            .filter(|sid| !sid.is_empty())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let scenario_names: std::collections::HashMap<String, String> =
            if !scenario_ids_needing_names.is_empty() {
                match self
                    .scenario_repo
                    .get_scenario_names_bulk(tenant_id, &scenario_ids_needing_names)
                    .await
                {
                    Ok(names) => names
                        .into_iter()
                        .filter(|(_, (name, _))| !name.is_empty())
                        .map(|(sid, (name, _))| (sid, name))
                        .collect(),
                    Err(e) => {
                        warn!(
                            tenant_id = %tenant_id,
                            error = %e,
                            "Failed to fetch scenario names"
                        );
                        std::collections::HashMap::new()
                    }
                }
            } else {
                std::collections::HashMap::new()
            };

        // Convert Runtara instances to ScenarioInstanceDto with scenario info
        let mut instances: Vec<ScenarioInstanceDto> = result
            .instances
            .into_iter()
            .map(|inst| {
                // Look up scenario info by image_id (registered_image_id in our DB or from Runtara)
                let (scenario_id, version, scenario_name) = scenario_info
                    .get(&inst.image_id)
                    .map(|(sid, ver, name)| {
                        // If name is empty, try to get it from the bulk lookup
                        let final_name = if name.is_empty() {
                            scenario_names.get(sid).cloned()
                        } else {
                            Some(name.clone())
                        };
                        (sid.clone(), *ver, final_name)
                    })
                    .unwrap_or_else(|| (String::new(), 0, None));

                runtara_instance_to_dto_with_info(inst, scenario_id, version, scenario_name)
            })
            .collect();

        // Enrich running instances with has_pending_input flag
        enrich_pending_input(&mut instances, client).await;

        // Use total_count from Runtara
        let total_elements = result.total_count as i64;
        let total_pages = if total_elements == 0 {
            0
        } else {
            ((total_elements as f64) / (size as f64)).ceil() as i32
        };
        let number_of_elements = instances.len() as i32;

        Ok(PageScenarioInstanceHistoryDto {
            content: instances,
            total_pages,
            total_elements,
            size,
            number: page,
            first: page == 0,
            last: page >= total_pages.max(1) - 1,
            number_of_elements,
        })
    }

    /// Stop a running instance
    ///
    /// Queries runtara-environment for instance status and sends cancel signal.
    /// Runtara-environment is the source of truth for all executions.
    #[allow(dead_code)]
    pub async fn stop_instance(
        &self,
        instance_id: &str,
        _tenant_id: &str,
        _running_executions: &dashmap::DashMap<Uuid, crate::types::CancellationHandle>,
        runtime_client: Option<&Arc<RuntimeClient>>,
    ) -> Result<StopInstanceResult, ServiceError> {
        // Validate UUID format
        let _ = Uuid::parse_str(instance_id).map_err(|_| {
            ServiceError::ValidationError(
                "Invalid instance ID. Instance ID must be a valid UUID".to_string(),
            )
        })?;

        // Runtime client is required - runtara-environment is the only way to stop instances
        let client = runtime_client.ok_or_else(|| {
            ServiceError::DatabaseError("Runtime client not configured".to_string())
        })?;

        // Get instance status from runtara-environment
        let runtara_status = client
            .get_instance_status(instance_id)
            .await
            .map_err(|e| ServiceError::NotFound(format!("Instance not found: {}", e)))?;

        let status_str = format!("{:?}", runtara_status).to_lowercase();

        // Check if already in terminal state
        if matches!(
            runtara_status,
            crate::runtime_client::InstanceStatus::Completed
                | crate::runtime_client::InstanceStatus::Failed
                | crate::runtime_client::InstanceStatus::Cancelled
        ) {
            return Ok(StopInstanceResult::AlreadyStopped { status: status_str });
        }

        // Send cancel signal to runtara-environment
        client.cancel_instance(instance_id).await.map_err(|e| {
            ServiceError::DatabaseError(format!("Failed to cancel instance: {}", e))
        })?;

        // If the instance is suspended (paused at breakpoint or by user), the process
        // is not running and won't consume the cancel signal. Resume it so the relaunched
        // process picks up the pending cancel signal and terminates immediately.
        if matches!(
            runtara_status,
            crate::runtime_client::InstanceStatus::Suspended
        ) && let Err(e) = client.resume_instance(instance_id).await
        {
            warn!(
                instance_id = %instance_id,
                error = %e,
                "Failed to resume suspended instance for cancellation"
            );
        }

        info!(
            instance_id = %instance_id,
            previous_status = %status_str,
            "Cancelled instance via runtara-environment"
        );

        Ok(StopInstanceResult::Stopped {
            previous_status: status_str,
            cancellation_flag_set: true,
        })
    }

    /// Pause a running workflow instance
    ///
    /// Sends a pause signal to the instance via RuntimeClient.
    /// The instance will checkpoint its state and suspend execution.
    pub async fn pause_instance(
        &self,
        instance_id: &str,
        _tenant_id: &str,
        runtime_client: Option<&Arc<RuntimeClient>>,
    ) -> Result<PauseInstanceResult, ServiceError> {
        // Validate UUID format
        let _ = Uuid::parse_str(instance_id).map_err(|_| {
            ServiceError::ValidationError(
                "Invalid instance ID. Instance ID must be a valid UUID".to_string(),
            )
        })?;

        // Runtime client is required - runtara-environment is the only way to pause instances
        let client = runtime_client.ok_or_else(|| {
            ServiceError::DatabaseError("Runtime client not configured".to_string())
        })?;

        // Get instance status from runtara-environment
        let runtara_status = client
            .get_instance_status(instance_id)
            .await
            .map_err(|e| ServiceError::NotFound(format!("Instance not found: {}", e)))?;

        let status_str = format!("{:?}", runtara_status).to_lowercase();

        // Check status and determine action
        match status_str.as_str() {
            // Already paused/suspended
            "suspended" => Ok(PauseInstanceResult::AlreadyPaused),

            // Running - can be paused
            "running" => {
                client.pause_instance(instance_id).await.map_err(|e| {
                    ServiceError::DatabaseError(format!("Failed to send pause signal: {}", e))
                })?;

                info!(
                    instance_id = %instance_id,
                    "Sent pause signal to instance"
                );

                Ok(PauseInstanceResult::Paused {
                    previous_status: status_str,
                })
            }

            // Not pausable (terminal states or not yet running)
            _ => Ok(PauseInstanceResult::NotPausable { status: status_str }),
        }
    }

    /// Resume a paused/suspended workflow instance
    ///
    /// Sends a resume signal to the instance via RuntimeClient.
    /// The instance will resume execution from its last checkpoint.
    pub async fn resume_instance(
        &self,
        instance_id: &str,
        _tenant_id: &str,
        runtime_client: Option<&Arc<RuntimeClient>>,
    ) -> Result<ResumeInstanceResult, ServiceError> {
        // Validate UUID format
        let _ = Uuid::parse_str(instance_id).map_err(|_| {
            ServiceError::ValidationError(
                "Invalid instance ID. Instance ID must be a valid UUID".to_string(),
            )
        })?;

        // Runtime client is required - runtara-environment is the only way to resume instances
        let client = runtime_client.ok_or_else(|| {
            ServiceError::DatabaseError("Runtime client not configured".to_string())
        })?;

        // Get instance status from runtara-environment
        let runtara_status = client
            .get_instance_status(instance_id)
            .await
            .map_err(|e| ServiceError::NotFound(format!("Instance not found: {}", e)))?;

        let status_str = format!("{:?}", runtara_status).to_lowercase();

        // Check status and determine action
        match status_str.as_str() {
            // Already running
            "running" => Ok(ResumeInstanceResult::AlreadyRunning),

            // Suspended/paused - can be resumed
            "suspended" => {
                client.resume_instance(instance_id).await.map_err(|e| {
                    ServiceError::DatabaseError(format!("Failed to send resume signal: {}", e))
                })?;

                info!(
                    instance_id = %instance_id,
                    "Sent resume signal to instance"
                );

                Ok(ResumeInstanceResult::Resumed {
                    previous_status: status_str,
                })
            }

            // Not resumable (terminal states, queued, or running)
            _ => Ok(ResumeInstanceResult::NotResumable { status: status_str }),
        }
    }
}

// ============================================================================
// DTOs
// ============================================================================

/// Result of queuing an execution
#[derive(Debug)]
pub struct QueuedExecutionResult {
    pub instance_id: Uuid,
    pub status: String,
}

/// Result of stop instance operation
#[derive(Debug)]
#[allow(dead_code)]
pub enum StopInstanceResult {
    /// Instance was already in a terminal state
    AlreadyStopped { status: String },
    /// Instance was stopped successfully
    Stopped {
        previous_status: String,
        cancellation_flag_set: bool,
    },
}

/// Result of pause instance operation
#[derive(Debug)]
pub enum PauseInstanceResult {
    /// Instance was paused successfully
    Paused { previous_status: String },
    /// Instance was already paused/suspended
    AlreadyPaused,
    /// Instance is not in a pausable state
    NotPausable { status: String },
}

/// Result of resume instance operation
#[derive(Debug)]
pub enum ResumeInstanceResult {
    /// Instance was resumed successfully
    Resumed { previous_status: String },
    /// Instance was already running
    AlreadyRunning,
    /// Instance is not in a resumable state
    NotResumable { status: String },
}

// ============================================================================
// Helper re-exports
// ============================================================================

use super::input_validation::{is_empty_schema, validate_inputs};

// ============================================================================
// Errors
// ============================================================================

/// Service-level errors for execution operations
#[derive(Debug)]
#[allow(dead_code)]
pub enum ServiceError {
    ValidationError(String),
    NotFound(String),
    DatabaseError(String),
}

/// Convert Runtara instance status to string
#[allow(dead_code)]
fn runtara_status_to_string(status: RuntaraInstanceStatus) -> String {
    match status {
        RuntaraInstanceStatus::Unknown => "unknown".to_string(),
        RuntaraInstanceStatus::Pending => "queued".to_string(),
        RuntaraInstanceStatus::Running => "running".to_string(),
        RuntaraInstanceStatus::Suspended => "suspended".to_string(),
        RuntaraInstanceStatus::Completed => "completed".to_string(),
        RuntaraInstanceStatus::Failed => "failed".to_string(),
        RuntaraInstanceStatus::Cancelled => "cancelled".to_string(),
    }
}

/// Convert Runtara instance status to ExecutionStatus enum
fn runtara_status_to_execution_status(status: RuntaraInstanceStatus) -> ExecutionStatus {
    match status {
        RuntaraInstanceStatus::Unknown => ExecutionStatus::Queued,
        RuntaraInstanceStatus::Pending => ExecutionStatus::Queued,
        RuntaraInstanceStatus::Running => ExecutionStatus::Running,
        RuntaraInstanceStatus::Suspended => ExecutionStatus::Suspended,
        RuntaraInstanceStatus::Completed => ExecutionStatus::Completed,
        RuntaraInstanceStatus::Failed => ExecutionStatus::Failed,
        RuntaraInstanceStatus::Cancelled => ExecutionStatus::Cancelled,
    }
}

/// Convert local execution status string to Runtara status enum
fn execution_status_to_runtara(status: &str) -> Option<RuntaraInstanceStatus> {
    match status {
        "queued" => Some(RuntaraInstanceStatus::Pending),
        "compiling" => Some(RuntaraInstanceStatus::Pending), // No direct equivalent
        "running" => Some(RuntaraInstanceStatus::Running),
        "suspended" => Some(RuntaraInstanceStatus::Suspended),
        "completed" => Some(RuntaraInstanceStatus::Completed),
        "failed" | "timeout" => Some(RuntaraInstanceStatus::Failed),
        "cancelled" => Some(RuntaraInstanceStatus::Cancelled),
        _ => None,
    }
}

/// Enrich running instances with `has_pending_input` by checking for unresolved
/// `external_input_requested` events. Only queries events for running instances.
async fn enrich_pending_input(instances: &mut [ScenarioInstanceDto], client: &RuntimeClient) {
    for instance in instances.iter_mut() {
        if instance.status != ExecutionStatus::Running {
            continue;
        }

        // Check for external_input_requested events
        let options = ListEventsOptions::new()
            .with_limit(1)
            .with_event_type("custom")
            .with_subtype("external_input_requested")
            .with_sort_order(EventSortOrder::Desc);

        match client.list_events(&instance.id, Some(options)).await {
            Ok(result) if !result.events.is_empty() => {
                // Found at least one external_input_requested event.
                // Check if the most recent one has been resolved by looking for
                // a corresponding step_debug_end event (covers both
                // AiAgentToolCall and standalone WaitForSignal).
                let end_options = ListEventsOptions::new()
                    .with_limit(1)
                    .with_event_type("custom")
                    .with_subtype("step_debug_end")
                    .with_sort_order(EventSortOrder::Desc);

                let has_completion = match client.list_events(&instance.id, Some(end_options)).await
                {
                    Ok(end_result) => {
                        // If the latest end event is newer than the latest request,
                        // the most recent request has been resolved
                        if let (Some(req), Some(end)) =
                            (result.events.first(), end_result.events.first())
                        {
                            end.created_at >= req.created_at
                        } else {
                            false
                        }
                    }
                    Err(_) => false,
                };

                instance.has_pending_input = !has_completion;
            }
            _ => {}
        }
    }
}

/// Convert Runtara InstanceSummary to ScenarioInstanceDto with scenario info from database lookup
fn runtara_instance_to_dto_with_info(
    inst: runtara_management_sdk::InstanceSummary,
    scenario_id: String,
    version: i32,
    scenario_name: Option<String>,
) -> ScenarioInstanceDto {
    // Convert to execution status
    let status = runtara_status_to_execution_status(inst.status);

    // Calculate execution duration if available
    let execution_duration_seconds = inst.started_at.and_then(|start| {
        inst.finished_at
            .map(|end| (end - start).num_milliseconds() as f64 / 1000.0)
    });

    ScenarioInstanceDto {
        id: inst.instance_id.clone(),
        created: inst.created_at.to_rfc3339(),
        updated: inst
            .finished_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| inst.created_at.to_rfc3339()),
        status,
        termination_type: None, // Not available from Runtara summary
        scenario_id,
        scenario_name,
        inputs: InstanceInputs {
            data: serde_json::Value::Null,
            variables: serde_json::Value::Null,
        },
        outputs: None, // Not available in summary
        tags: vec![],
        used_version: version,
        steps: vec![],
        execution_duration_seconds,
        max_memory_mb: None,
        queue_duration_seconds: None,
        processing_overhead_seconds: None,
        has_pending_input: false,
    }
}

/// Convert Runtara InstanceSummary to ScenarioInstanceDto (legacy - parses image_id as scenario:version)
#[allow(dead_code)]
fn runtara_instance_to_dto(
    inst: runtara_management_sdk::InstanceSummary,
    scenario_name: Option<String>,
) -> ScenarioInstanceDto {
    // Extract scenario_id and version from image_id (format: scenario_id:version)
    // Note: This assumes image_id follows the name format, but Runtara returns a UUID
    let (scenario_id, version) = parse_image_id(&inst.image_id);
    runtara_instance_to_dto_with_info(inst, scenario_id, version, scenario_name)
}

/// Convert Runtara InstanceInfo (detailed) to ScenarioInstanceDto
///
/// InstanceInfo fields (from SDK):
/// - instance_id, image_id, image_name, tenant_id: String
/// - status: InstanceStatus
/// - checkpoint_id: Option<String>
/// - created_at: DateTime<Utc>
/// - started_at, finished_at, heartbeat_at: Option<DateTime<Utc>>
/// - input, output: Option<Value>
/// - error: Option<String>
/// - retry_count, max_retries: u32
fn runtara_info_to_dto(info: InstanceInfo) -> ScenarioInstanceDto {
    // Convert to execution status
    let status = runtara_status_to_execution_status(info.status);

    // Calculate execution duration if available
    let execution_duration_seconds = info.started_at.and_then(|start| {
        info.finished_at
            .map(|end| (end - start).num_milliseconds() as f64 / 1000.0)
    });

    // Use created_at (now available in SDK)
    let created = info.created_at.to_rfc3339();

    let updated = info
        .finished_at
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| created.clone());

    // Extract scenario_id and version from image_name (format: scenario_id:version)
    let (scenario_id, version) = parse_image_id(&info.image_name);

    // Extract data and variables from input to avoid double-wrapping
    let (data, variables) = extract_input_fields(info.input.as_ref());

    ScenarioInstanceDto {
        id: info.instance_id.clone(),
        created,
        updated,
        status,
        termination_type: None,
        scenario_id,
        scenario_name: None,
        inputs: InstanceInputs { data, variables },
        outputs: info.output,
        tags: vec![],
        used_version: version,
        steps: vec![],
        execution_duration_seconds,
        max_memory_mb: None,
        queue_duration_seconds: None,
        processing_overhead_seconds: None,
        has_pending_input: false,
    }
}

/// Convert Runtara InstanceInfo to ExecutionWithMetadata
///
/// Used by get_execution_with_metadata when proxying to Runtara.
fn runtara_info_to_execution_with_metadata(
    info: InstanceInfo,
    scenario_name: Option<String>,
    scenario_description: Option<String>,
) -> ExecutionWithMetadata {
    // Convert to ScenarioInstanceDto first
    let status = runtara_status_to_execution_status(info.status);

    let execution_duration_seconds = info.started_at.and_then(|start| {
        info.finished_at
            .map(|end| (end - start).num_milliseconds() as f64 / 1000.0)
    });

    let created = info.created_at.to_rfc3339();
    let updated = info
        .finished_at
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| created.clone());

    let (scenario_id, version) = parse_image_id(&info.image_name);

    // Extract data and variables from input to avoid double-wrapping
    let (data, variables) = extract_input_fields(info.input.as_ref());

    let instance = ScenarioInstanceDto {
        id: info.instance_id.clone(),
        created,
        updated,
        status,
        termination_type: None,
        scenario_id,
        scenario_name: scenario_name.clone(),
        inputs: InstanceInputs { data, variables },
        outputs: info.output,
        tags: vec![],
        used_version: version,
        steps: vec![],
        execution_duration_seconds,
        max_memory_mb: None,
        queue_duration_seconds: None,
        processing_overhead_seconds: None,
        has_pending_input: false,
    };

    ExecutionWithMetadata {
        instance,
        scenario_name,
        scenario_description,
        worker_id: None, // Not tracked by Runtara at smo-runtime level
        heartbeat_at: info.heartbeat_at,
        retry_count: Some(info.retry_count as i32),
        max_retries: Some(info.max_retries as i32),
        additional_metadata: None,
        error_message: info.error,
        started_at: info.started_at,
        completed_at: info.finished_at,
    }
}

/// Extract data and variables from Runtara input
///
/// Runtara stores inputs in the format: {"data": {...}, "variables": {...}}
/// This function extracts those fields to avoid double-wrapping when
/// constructing InstanceInputs.
fn extract_input_fields(input: Option<&Value>) -> (Value, Value) {
    if let Some(obj) = input.and_then(|v| v.as_object()) {
        (
            obj.get("data").cloned().unwrap_or(Value::Null),
            obj.get("variables").cloned().unwrap_or(Value::Null),
        )
    } else {
        // Fallback: treat entire input as data
        (input.cloned().unwrap_or(Value::Null), Value::Null)
    }
}

/// Parse image_id (format: "scenario_id:version") into (scenario_id, version)
fn parse_image_id(image_id: &str) -> (String, i32) {
    if let Some(pos) = image_id.rfind(':') {
        let scenario_id = image_id[..pos].to_string();
        let version = image_id[pos + 1..].parse::<i32>().unwrap_or(0);
        (scenario_id, version)
    } else {
        (image_id.to_string(), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // =========================================================================
    // parse_image_id tests
    // =========================================================================

    #[test]
    fn test_parse_image_id_standard_format() {
        let (scenario_id, version) = parse_image_id("my-scenario:5");
        assert_eq!(scenario_id, "my-scenario");
        assert_eq!(version, 5);
    }

    #[test]
    fn test_parse_image_id_uuid_format() {
        let (scenario_id, version) = parse_image_id("550e8400-e29b-41d4-a716-446655440000:42");
        assert_eq!(scenario_id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(version, 42);
    }

    #[test]
    fn test_parse_image_id_no_version() {
        let (scenario_id, version) = parse_image_id("scenario-without-version");
        assert_eq!(scenario_id, "scenario-without-version");
        assert_eq!(version, 0);
    }

    #[test]
    fn test_parse_image_id_invalid_version() {
        let (scenario_id, version) = parse_image_id("my-scenario:invalid");
        assert_eq!(scenario_id, "my-scenario");
        assert_eq!(version, 0);
    }

    #[test]
    fn test_parse_image_id_multiple_colons() {
        // Uses rfind so it should parse from the last colon
        let (scenario_id, version) = parse_image_id("org:tenant:scenario:10");
        assert_eq!(scenario_id, "org:tenant:scenario");
        assert_eq!(version, 10);
    }

    #[test]
    fn test_parse_image_id_empty_string() {
        let (scenario_id, version) = parse_image_id("");
        assert_eq!(scenario_id, "");
        assert_eq!(version, 0);
    }

    // =========================================================================
    // extract_input_fields tests
    // =========================================================================

    #[test]
    fn test_extract_input_fields_standard_format() {
        let input = json!({
            "data": {"user": "john"},
            "variables": {"env": "prod"}
        });

        let (data, variables) = extract_input_fields(Some(&input));
        assert_eq!(data, json!({"user": "john"}));
        assert_eq!(variables, json!({"env": "prod"}));
    }

    #[test]
    fn test_extract_input_fields_missing_variables() {
        let input = json!({"data": {"user": "john"}});

        let (data, variables) = extract_input_fields(Some(&input));
        assert_eq!(data, json!({"user": "john"}));
        assert_eq!(variables, Value::Null);
    }

    #[test]
    fn test_extract_input_fields_none() {
        let (data, variables) = extract_input_fields(None);
        assert_eq!(data, Value::Null);
        assert_eq!(variables, Value::Null);
    }

    #[test]
    fn test_extract_input_fields_non_object() {
        let input = json!("just a string");
        let (data, variables) = extract_input_fields(Some(&input));
        // Fallback: treat entire input as data
        assert_eq!(data, json!("just a string"));
        assert_eq!(variables, Value::Null);
    }

    // =========================================================================
    // execution_status_to_runtara tests
    // =========================================================================

    #[test]
    fn test_execution_status_to_runtara_queued() {
        assert_eq!(
            execution_status_to_runtara("queued"),
            Some(RuntaraInstanceStatus::Pending)
        );
    }

    #[test]
    fn test_execution_status_to_runtara_running() {
        assert_eq!(
            execution_status_to_runtara("running"),
            Some(RuntaraInstanceStatus::Running)
        );
    }

    #[test]
    fn test_execution_status_to_runtara_completed() {
        assert_eq!(
            execution_status_to_runtara("completed"),
            Some(RuntaraInstanceStatus::Completed)
        );
    }

    #[test]
    fn test_execution_status_to_runtara_failed() {
        assert_eq!(
            execution_status_to_runtara("failed"),
            Some(RuntaraInstanceStatus::Failed)
        );
    }

    #[test]
    fn test_execution_status_to_runtara_cancelled() {
        assert_eq!(
            execution_status_to_runtara("cancelled"),
            Some(RuntaraInstanceStatus::Cancelled)
        );
    }

    #[test]
    fn test_execution_status_to_runtara_unknown() {
        assert_eq!(execution_status_to_runtara("invalid_status"), None);
    }

    // =========================================================================
    // runtara_status_to_execution_status tests
    // =========================================================================

    #[test]
    fn test_runtara_status_to_execution_status_pending() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Pending),
            ExecutionStatus::Queued
        );
    }

    #[test]
    fn test_runtara_status_to_execution_status_running() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Running),
            ExecutionStatus::Running
        );
    }

    #[test]
    fn test_runtara_status_to_execution_status_completed() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Completed),
            ExecutionStatus::Completed
        );
    }

    #[test]
    fn test_runtara_status_to_execution_status_failed() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Failed),
            ExecutionStatus::Failed
        );
    }

    #[test]
    fn test_runtara_status_to_execution_status_cancelled() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Cancelled),
            ExecutionStatus::Cancelled
        );
    }

    #[test]
    fn test_runtara_status_to_execution_status_suspended() {
        assert_eq!(
            runtara_status_to_execution_status(RuntaraInstanceStatus::Suspended),
            ExecutionStatus::Suspended
        );
    }

    // =========================================================================
    // ServiceError tests
    // =========================================================================

    #[test]
    fn test_service_error_debug_format() {
        let error = ServiceError::ValidationError("test validation".to_string());
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("ValidationError"));
        assert!(debug_str.contains("test validation"));
    }

    #[test]
    fn test_service_error_not_found_debug() {
        let error = ServiceError::NotFound("Instance '123' not found".to_string());
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("NotFound"));
    }

    #[test]
    fn test_service_error_database_debug() {
        let error = ServiceError::DatabaseError("Connection failed".to_string());
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("DatabaseError"));
    }

    // =========================================================================
    // StopInstanceResult tests
    // =========================================================================

    #[test]
    fn test_stop_instance_result_already_stopped() {
        let result = StopInstanceResult::AlreadyStopped {
            status: "completed".to_string(),
        };
        if let StopInstanceResult::AlreadyStopped { status } = result {
            assert_eq!(status, "completed");
        } else {
            panic!("Expected AlreadyStopped variant");
        }
    }

    #[test]
    fn test_stop_instance_result_stopped() {
        let result = StopInstanceResult::Stopped {
            previous_status: "running".to_string(),
            cancellation_flag_set: true,
        };
        if let StopInstanceResult::Stopped {
            previous_status,
            cancellation_flag_set,
        } = result
        {
            assert_eq!(previous_status, "running");
            assert!(cancellation_flag_set);
        } else {
            panic!("Expected Stopped variant");
        }
    }

    // =========================================================================
    // PauseInstanceResult tests
    // =========================================================================

    #[test]
    fn test_pause_instance_result_paused() {
        let result = PauseInstanceResult::Paused {
            previous_status: "running".to_string(),
        };
        if let PauseInstanceResult::Paused { previous_status } = result {
            assert_eq!(previous_status, "running");
        } else {
            panic!("Expected Paused variant");
        }
    }

    #[test]
    fn test_pause_instance_result_already_paused() {
        let result = PauseInstanceResult::AlreadyPaused;
        assert!(matches!(result, PauseInstanceResult::AlreadyPaused));
    }

    #[test]
    fn test_pause_instance_result_not_pausable() {
        let result = PauseInstanceResult::NotPausable {
            status: "completed".to_string(),
        };
        if let PauseInstanceResult::NotPausable { status } = result {
            assert_eq!(status, "completed");
        } else {
            panic!("Expected NotPausable variant");
        }
    }

    // =========================================================================
    // ResumeInstanceResult tests
    // =========================================================================

    #[test]
    fn test_resume_instance_result_resumed() {
        let result = ResumeInstanceResult::Resumed {
            previous_status: "suspended".to_string(),
        };
        if let ResumeInstanceResult::Resumed { previous_status } = result {
            assert_eq!(previous_status, "suspended");
        } else {
            panic!("Expected Resumed variant");
        }
    }

    #[test]
    fn test_resume_instance_result_already_running() {
        let result = ResumeInstanceResult::AlreadyRunning;
        assert!(matches!(result, ResumeInstanceResult::AlreadyRunning));
    }

    #[test]
    fn test_resume_instance_result_not_resumable() {
        let result = ResumeInstanceResult::NotResumable {
            status: "failed".to_string(),
        };
        if let ResumeInstanceResult::NotResumable { status } = result {
            assert_eq!(status, "failed");
        } else {
            panic!("Expected NotResumable variant");
        }
    }

    // =========================================================================
    // QueuedExecutionResult tests
    // =========================================================================

    #[test]
    fn test_queued_execution_result_fields() {
        let instance_id = Uuid::new_v4();
        let result = QueuedExecutionResult {
            instance_id,
            status: "queued".to_string(),
        };
        assert_eq!(result.instance_id, instance_id);
        assert_eq!(result.status, "queued");
    }
}
