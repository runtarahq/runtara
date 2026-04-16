// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read-only instance status query handler.

use anyhow::Result;
use tracing::{debug, instrument};

use super::mappers::map_status;
use super::state::InstanceHandlerState;
use super::types::{GetInstanceStatusRequest, GetInstanceStatusResponse, InstanceStatus};

/// Handle instance status query.
///
/// Returns the current status of an instance including:
/// - Current status (pending, running, suspended, completed, failed, cancelled)
/// - Last checkpoint ID
/// - Start/finish timestamps
/// - Output data (if completed) or error message (if failed)
#[instrument(skip(state, request), fields(instance_id = %request.instance_id))]
pub async fn handle_get_instance_status(
    state: &InstanceHandlerState,
    request: GetInstanceStatusRequest,
) -> Result<GetInstanceStatusResponse> {
    debug!("Getting instance status");

    let instance = state.persistence.get_instance(&request.instance_id).await?;

    match instance {
        Some(inst) => {
            let status = map_status(&inst.status);

            Ok(GetInstanceStatusResponse {
                instance_id: request.instance_id,
                status: status.into(),
                checkpoint_id: inst.checkpoint_id,
                started_at_ms: inst.started_at.map(|t| t.timestamp_millis()).unwrap_or(0),
                finished_at_ms: inst.finished_at.map(|t| t.timestamp_millis()),
                output: inst.output,
                error: inst.error,
            })
        }
        None => Ok(GetInstanceStatusResponse {
            instance_id: request.instance_id,
            status: InstanceStatus::StatusUnknown.into(),
            checkpoint_id: None,
            started_at_ms: 0,
            finished_at_ms: None,
            output: None,
            error: Some("Instance not found".to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::instance_handlers::mock_persistence::{MockPersistence, make_instance};

    #[tokio::test]
    async fn test_get_status_not_found() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = GetInstanceStatusRequest {
            instance_id: "nonexistent".to_string(),
        };

        let result = handle_get_instance_status(&state, request).await.unwrap();
        // Instance not found returns StatusUnknown
        assert_eq!(result.status, InstanceStatus::StatusUnknown as i32);
    }

    #[tokio::test]
    async fn test_get_status_found() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = GetInstanceStatusRequest {
            instance_id: "inst-1".to_string(),
        };

        let result = handle_get_instance_status(&state, request).await.unwrap();
        assert_eq!(result.status, InstanceStatus::StatusRunning as i32);
    }
}
