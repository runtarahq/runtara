// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The run state service of every run, over core persistence.

use std::sync::Arc;

use runtara_component_host::run_state_host::{
    RunStateAuthority, RunStateError, RunStateErrorCode, RunStateHost,
};
use runtara_core::error::CoreError;
use runtara_core::persistence::Persistence;
use runtara_core::persistence::run_state::StatePatch;
use serde_json::{Map, Value};

/// [`RunStateHost`] backed by [`Persistence::run_state`]. Without it every
/// call fails with `unavailable`.
pub(super) struct PersistenceRunState(pub(super) Arc<dyn Persistence>);

fn state_error(error: CoreError) -> RunStateError {
    match error {
        CoreError::InstanceNotFound { .. } | CoreError::InvalidInstanceState { .. } => {
            RunStateError::new(RunStateErrorCode::NotRunning, error.to_string())
        }
        CoreError::ValidationError { ref field, .. } if field == "state" => {
            RunStateError::new(RunStateErrorCode::TooLarge, error.to_string())
        }
        CoreError::ValidationError { .. } => {
            RunStateError::new(RunStateErrorCode::Invalid, error.to_string())
        }
        other => RunStateError::new(RunStateErrorCode::Unavailable, other.to_string()),
    }
}

fn unavailable() -> RunStateError {
    RunStateError::new(
        RunStateErrorCode::Unavailable,
        "this persistence backend keeps no run state",
    )
}

#[async_trait::async_trait]
impl RunStateHost for PersistenceRunState {
    async fn set(
        &self,
        authority: &RunStateAuthority,
        operation_id: &str,
        patch: Map<String, Value>,
    ) -> Result<(), RunStateError> {
        let store = self.0.run_state().ok_or_else(unavailable)?;
        store
            .apply_state(
                &authority.tenant,
                &authority.instance,
                operation_id,
                &StatePatch::from_object(&patch),
            )
            .await
            .map(|_| ())
            .map_err(state_error)
    }

    async fn get(
        &self,
        authority: &RunStateAuthority,
    ) -> Result<Map<String, Value>, RunStateError> {
        let store = self.0.run_state().ok_or_else(unavailable)?;
        Ok(store
            .get_state(&authority.tenant, &authority.instance)
            .await
            .map_err(state_error)?
            .map(|record| record.state)
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::domain::InstanceStatus;
    use runtara_core::persistence::memory::InMemoryPersistence;
    use serde_json::json;

    async fn running(persistence: &InMemoryPersistence) -> RunStateAuthority {
        persistence
            .register_instance("run", "tenant")
            .await
            .unwrap();
        persistence
            .update_instance_status("run", InstanceStatus::Running, None)
            .await
            .unwrap();
        RunStateAuthority {
            tenant: "tenant".into(),
            instance: "run".into(),
        }
    }

    #[tokio::test]
    async fn writes_once_per_operation_and_reads_back() {
        let persistence = Arc::new(InMemoryPersistence::new());
        let authority = running(&persistence).await;
        let host = PersistenceRunState(persistence.clone());
        assert_eq!(host.get(&authority).await.unwrap(), Map::new());

        let op = "ab".repeat(32);
        let patch = |v: Value| v.as_object().unwrap().clone();
        host.set(&authority, &op, patch(json!({"stage": "approval"})))
            .await
            .unwrap();
        host.set(&authority, &op, patch(json!({"stage": "replayed"})))
            .await
            .unwrap();
        assert_eq!(
            Value::Object(host.get(&authority).await.unwrap()),
            json!({"stage": "approval"})
        );
    }

    #[tokio::test]
    async fn maps_core_errors_to_state_codes() {
        let persistence = Arc::new(InMemoryPersistence::new());
        let authority = running(&persistence).await;
        let host = PersistenceRunState(persistence.clone());
        let big = json!({"big": "x".repeat(70 * 1024)});
        let error = host
            .set(
                &authority,
                &"cd".repeat(32),
                big.as_object().unwrap().clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, RunStateErrorCode::TooLarge);

        let error = host
            .set(&authority, "not-a-hash", Map::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, RunStateErrorCode::Invalid);

        let foreign = RunStateAuthority {
            tenant: "other".into(),
            instance: "run".into(),
        };
        let error = host.get(&foreign).await.unwrap_err();
        assert_eq!(error.code, RunStateErrorCode::NotRunning);
    }
}
