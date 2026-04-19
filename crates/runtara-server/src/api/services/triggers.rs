/// Triggers service - business logic and validation for invocation triggers
use std::sync::Arc;

use crate::api::dto::triggers::*;
use crate::api::repositories::triggers::TriggerRepository;

/// Service for invocation trigger business logic
pub struct TriggerService {
    repository: Arc<TriggerRepository>,
}

impl TriggerService {
    /// Create a new TriggerService
    pub fn new(repository: Arc<TriggerRepository>) -> Self {
        Self { repository }
    }

    /// Create a new invocation trigger
    pub async fn create_trigger(
        &self,
        request: CreateInvocationTriggerRequest,
        tenant_id: Option<&str>,
    ) -> Result<InvocationTrigger, ServiceError> {
        // Validation: workflow_id should not be empty
        if request.workflow_id.trim().is_empty() {
            return Err(ServiceError::ValidationError(
                "Workflow ID cannot be empty".to_string(),
            ));
        }

        // Validation: configuration should be valid for trigger type
        if let Err(e) = self.validate_configuration(&request.trigger_type, &request.configuration) {
            return Err(ServiceError::ValidationError(e));
        }

        // Delegate to repository
        self.repository
            .create(&request, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))
    }

    /// List all invocation triggers
    pub async fn list_triggers(
        &self,
        tenant_id: Option<&str>,
    ) -> Result<Vec<InvocationTrigger>, ServiceError> {
        self.repository
            .list(tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))
    }

    /// Get a trigger by ID
    pub async fn get_trigger(
        &self,
        id: &str,
        tenant_id: Option<&str>,
    ) -> Result<Option<InvocationTrigger>, ServiceError> {
        self.repository
            .get_by_id(id, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))
    }

    /// Update an invocation trigger
    pub async fn update_trigger(
        &self,
        id: &str,
        request: UpdateInvocationTriggerRequest,
        tenant_id: Option<&str>,
    ) -> Result<Option<InvocationTrigger>, ServiceError> {
        // Validation: workflow_id should not be empty
        if request.workflow_id.trim().is_empty() {
            return Err(ServiceError::ValidationError(
                "Workflow ID cannot be empty".to_string(),
            ));
        }

        // Validation: configuration should be valid for trigger type
        if let Err(e) = self.validate_configuration(&request.trigger_type, &request.configuration) {
            return Err(ServiceError::ValidationError(e));
        }

        self.repository
            .update(id, &request, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))
    }

    /// Delete an invocation trigger
    pub async fn delete_trigger(
        &self,
        id: &str,
        tenant_id: Option<&str>,
    ) -> Result<bool, ServiceError> {
        self.repository
            .delete(id, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))
    }

    /// Validate trigger configuration based on trigger type
    fn validate_configuration(
        &self,
        trigger_type: &TriggerType,
        configuration: &Option<serde_json::Value>,
    ) -> Result<(), String> {
        match trigger_type {
            TriggerType::Cron => {
                // CRON triggers should have an expression
                if let Some(config) = configuration {
                    if config.get("expression").is_none() {
                        return Err(
                            "CRON trigger requires 'expression' in configuration".to_string()
                        );
                    }
                } else {
                    return Err("CRON trigger requires configuration with 'expression'".to_string());
                }
            }
            TriggerType::Http => {
                // HTTP triggers might have path configuration
                // Validation can be added here if needed
            }
            TriggerType::Email => {
                // Email triggers might have sender/subject filters
                // Validation can be added here if needed
            }
            TriggerType::Application => {
                // Application triggers require connection information
                // Validation can be added here if needed
            }
            TriggerType::Channel => {
                // Channel triggers require a connection_id in configuration.
                if let Some(config) = configuration {
                    if config
                        .get("connection_id")
                        .and_then(|v| v.as_str())
                        .is_none()
                        || config["connection_id"].as_str().unwrap_or("").is_empty()
                    {
                        return Err(
                            "Channel trigger requires 'connection_id' in configuration".to_string()
                        );
                    }
                } else {
                    return Err(
                        "Channel trigger requires configuration with 'connection_id'".to_string(),
                    );
                }
            }
        }
        Ok(())
    }
}

/// Service-layer errors
#[derive(Debug)]
#[allow(dead_code)]
pub enum ServiceError {
    ValidationError(String),
    NotFound(String),
    DatabaseError(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            ServiceError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ServiceError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
        }
    }
}

impl std::error::Error for ServiceError {}
