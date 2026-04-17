use runtara_management_sdk::{ListInstancesOptions, ListInstancesOrder};
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::api::dto::executions::ExecutionFilters;
use crate::api::dto::scenarios::{PageScenarioInstanceHistoryDto, ScenarioInstanceDto};
use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::repositories::scenarios::ScenarioRepository;
use crate::api::repositories::trigger_stream::TriggerStreamPublisher;
use crate::runtime_client::RuntimeClient;
use crate::workers::runtara_dto::{
    ExecutionWithMetadata, enrich_pending_input, execution_status_to_runtara, runtara_info_to_dto,
    runtara_info_to_execution_with_metadata, runtara_instance_to_dto_with_info,
};

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

            // Suspended, failed, or cancelled - can be resumed from checkpoint
            "suspended" | "failed" | "cancelled" => {
                client.resume_instance(instance_id).await.map_err(|e| {
                    ServiceError::DatabaseError(format!("Failed to send resume signal: {}", e))
                })?;

                info!(
                    instance_id = %instance_id,
                    previous_status = %status_str,
                    "Sent resume signal to instance"
                );

                Ok(ResumeInstanceResult::Resumed {
                    previous_status: status_str,
                })
            }

            // Not resumable (completed, cancelled, queued)
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

#[cfg(test)]
mod tests {
    use super::*;

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
