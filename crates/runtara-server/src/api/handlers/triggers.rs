/// Triggers HTTP handlers
/// Thin layer that delegates to TriggerService
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::api::dto::common::ApiResponse;
use crate::api::dto::triggers::*;
use crate::api::repositories::triggers::TriggerRepository;
use crate::api::services::triggers::{ServiceError, TriggerService};
use crate::api::services::webhook_manager::{WebhookManager, extract_connection_id};

/// Best-effort webhook registration after a Channel trigger is created/activated.
/// Stores the webhook secret in the trigger's configuration for request validation.
async fn maybe_register_webhook(pool: &PgPool, trigger: &InvocationTrigger, tenant_id: &str) {
    if trigger.trigger_type == TriggerType::Channel
        && trigger.active
        && let Some(conn_id) = extract_connection_id(&trigger.configuration)
    {
        let manager = WebhookManager::new(pool.clone());
        match manager.register(conn_id, tenant_id).await {
            Ok(registration) => {
                // Store webhook secret and platform in the trigger's configuration.
                let mut config = trigger
                    .configuration
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({}));
                if let Some(obj) = config.as_object_mut() {
                    obj.insert(
                        "webhook_secret".to_string(),
                        serde_json::Value::String(registration.webhook_secret),
                    );
                    obj.insert(
                        "platform".to_string(),
                        serde_json::Value::String(registration.platform),
                    );
                }
                let repo = TriggerRepository::new(pool.clone());
                if let Err(e) = repo.update_configuration(&trigger.id, &config).await {
                    tracing::warn!(error = %e, "Failed to store webhook secret in trigger");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, connection_id = %conn_id, "Failed to register webhook");
            }
        }
    }
}

/// Best-effort webhook unregistration after a Channel trigger is deactivated/deleted.
async fn maybe_unregister_webhook(pool: &PgPool, trigger: &InvocationTrigger, tenant_id: &str) {
    if trigger.trigger_type == TriggerType::Channel
        && trigger.active
        && let Some(conn_id) = extract_connection_id(&trigger.configuration)
    {
        let manager = WebhookManager::new(pool.clone());
        if let Err(e) = manager.unregister(conn_id, tenant_id).await {
            tracing::warn!(error = %e, connection_id = %conn_id, "Failed to unregister webhook");
        }
    }
}

/// Create a new invocation trigger
#[utoipa::path(
    post,
    path = "/api/runtime/triggers",
    request_body = CreateInvocationTriggerRequest,
    responses(
        (status = 201, description = "Trigger created successfully", body = ApiResponse<InvocationTrigger>),
        (status = 400, description = "Validation error"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    tag = "Invocation Triggers"
)]
pub async fn create_invocation_trigger(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Json(request): Json<CreateInvocationTriggerRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let repository = Arc::new(TriggerRepository::new(pool.clone()));
    let service = TriggerService::new(repository);

    match service.create_trigger(request, Some(&tenant_id)).await {
        Ok(trigger) => {
            maybe_register_webhook(&pool, &trigger, &tenant_id).await;

            // Re-read the trigger to get updated config (webhook_secret, platform).
            let trigger = service
                .get_trigger(&trigger.id, Some(&tenant_id))
                .await
                .ok()
                .flatten()
                .unwrap_or(trigger);
            let trigger_response = InvocationTriggerResponse::from_trigger(trigger, &tenant_id);

            let response =
                ApiResponse::success_with_message("Trigger created successfully", trigger_response);
            Ok((
                StatusCode::CREATED,
                Json(serde_json::to_value(response).unwrap()),
            ))
        }
        Err(ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "data": Value::Null
            });
            Err((StatusCode::BAD_REQUEST, Json(error_response)))
        }
        Err(e) => {
            eprintln!("Failed to create trigger: {:?}", e);
            let error_response = json!({
                "success": false,
                "message": format!("Failed to create trigger: {}", e),
                "data": Value::Null
            });
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

/// List all invocation triggers
#[utoipa::path(
    get,
    path = "/api/runtime/triggers",
    responses(
        (status = 200, description = "List of invocation triggers", body = ApiResponse<Vec<InvocationTrigger>>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    tag = "Invocation Triggers"
)]
pub async fn list_invocation_triggers(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let repository = Arc::new(TriggerRepository::new(pool));
    let service = TriggerService::new(repository);

    match service.list_triggers(Some(&tenant_id)).await {
        Ok(triggers) => {
            let triggers: Vec<_> = triggers
                .into_iter()
                .map(|t| InvocationTriggerResponse::from_trigger(t, &tenant_id))
                .collect();
            let response = ApiResponse::success(triggers);
            Ok((
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            ))
        }
        Err(e) => {
            eprintln!("Failed to list triggers: {:?}", e);
            let error_response = json!({
                "success": false,
                "message": format!("Failed to list triggers: {}", e),
                "data": Value::Null
            });
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

/// Get a single invocation trigger by ID
#[utoipa::path(
    get,
    path = "/api/runtime/triggers/{id}",
    params(
        ("id" = String, Path, description = "Invocation trigger ID")
    ),
    responses(
        (status = 200, description = "Invocation trigger found", body = ApiResponse<InvocationTrigger>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Invocation trigger not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "Invocation Triggers"
)]
pub async fn get_invocation_trigger(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let repository = Arc::new(TriggerRepository::new(pool));
    let service = TriggerService::new(repository);

    match service.get_trigger(&id, Some(&tenant_id)).await {
        Ok(Some(trigger)) => {
            let trigger_response = InvocationTriggerResponse::from_trigger(trigger, &tenant_id);
            let response = ApiResponse::success(trigger_response);
            Ok((
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            ))
        }
        Ok(None) => {
            let error_response = json!({
                "success": false,
                "message": "Trigger not found",
                "data": Value::Null
            });
            Err((StatusCode::NOT_FOUND, Json(error_response)))
        }
        Err(e) => {
            eprintln!("Failed to get trigger: {:?}", e);
            let error_response = json!({
                "success": false,
                "message": format!("Failed to get trigger: {}", e),
                "data": Value::Null
            });
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

/// Update an invocation trigger by ID
#[utoipa::path(
    put,
    path = "/api/runtime/triggers/{id}",
    params(
        ("id" = String, Path, description = "Invocation trigger ID")
    ),
    request_body = UpdateInvocationTriggerRequest,
    responses(
        (status = 200, description = "Trigger updated successfully", body = ApiResponse<InvocationTrigger>),
        (status = 400, description = "Validation error"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Invocation trigger not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "Invocation Triggers"
)]
pub async fn update_invocation_trigger(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(id): Path<String>,
    Json(request): Json<UpdateInvocationTriggerRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let repository = Arc::new(TriggerRepository::new(pool.clone()));
    let service = TriggerService::new(repository);

    // Load previous state for webhook lifecycle.
    let old_trigger = service
        .get_trigger(&id, Some(&tenant_id))
        .await
        .ok()
        .flatten();

    match service.update_trigger(&id, request, Some(&tenant_id)).await {
        Ok(Some(trigger)) => {
            // Handle webhook lifecycle on state transitions.
            let was_active_channel = old_trigger
                .as_ref()
                .map(|t| t.trigger_type == TriggerType::Channel && t.active)
                .unwrap_or(false);
            let is_active_channel = trigger.trigger_type == TriggerType::Channel && trigger.active;

            if !was_active_channel && is_active_channel {
                maybe_register_webhook(&pool, &trigger, &tenant_id).await;
            } else if was_active_channel
                && !is_active_channel
                && let Some(ref old) = old_trigger
            {
                maybe_unregister_webhook(&pool, old, &tenant_id).await;
            }

            // Re-read to get updated config.
            let trigger = service
                .get_trigger(&trigger.id, Some(&tenant_id))
                .await
                .ok()
                .flatten()
                .unwrap_or(trigger);
            let trigger_response = InvocationTriggerResponse::from_trigger(trigger, &tenant_id);

            let response =
                ApiResponse::success_with_message("Trigger updated successfully", trigger_response);
            Ok((
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            ))
        }
        Ok(None) => {
            let error_response = json!({
                "success": false,
                "message": "Trigger not found",
                "data": Value::Null
            });
            Err((StatusCode::NOT_FOUND, Json(error_response)))
        }
        Err(ServiceError::ValidationError(msg)) => {
            let error_response = json!({
                "success": false,
                "message": msg,
                "data": Value::Null
            });
            Err((StatusCode::BAD_REQUEST, Json(error_response)))
        }
        Err(e) => {
            eprintln!("Failed to update trigger: {:?}", e);
            let error_response = json!({
                "success": false,
                "message": format!("Failed to update trigger: {}", e),
                "data": Value::Null
            });
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

/// Delete an invocation trigger by ID
#[utoipa::path(
    delete,
    path = "/api/runtime/triggers/{id}",
    params(
        ("id" = String, Path, description = "Invocation trigger ID")
    ),
    responses(
        (status = 200, description = "Trigger deleted successfully"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Invocation trigger not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "Invocation Triggers"
)]
pub async fn delete_invocation_trigger(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let repository = Arc::new(TriggerRepository::new(pool.clone()));
    let service = TriggerService::new(repository);

    // Load trigger before deleting for webhook cleanup.
    let old_trigger = service
        .get_trigger(&id, Some(&tenant_id))
        .await
        .ok()
        .flatten();

    match service.delete_trigger(&id, Some(&tenant_id)).await {
        Ok(true) => {
            if let Some(ref old) = old_trigger {
                maybe_unregister_webhook(&pool, old, &tenant_id).await;
            }

            let response = json!({
                "success": true,
                "message": "Trigger deleted successfully",
                "data": Value::Null
            });
            Ok((StatusCode::OK, Json(response)))
        }
        Ok(false) => {
            let error_response = json!({
                "success": false,
                "message": "Trigger not found",
                "data": Value::Null
            });
            Err((StatusCode::NOT_FOUND, Json(error_response)))
        }
        Err(e) => {
            eprintln!("Failed to delete trigger: {:?}", e);
            let error_response = json!({
                "success": false,
                "message": format!("Failed to delete trigger: {}", e),
                "data": Value::Null
            });
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}
