//! Native connection boundary shared by workflow and interactive component runs.
use std::sync::Arc;

use runtara_component_host::ConnectionResolverHost;
use runtara_connections::{ConnectionsError, ConnectionsFacade};

pub struct NativeConnectionResolver(pub Arc<ConnectionsFacade>);

// Errors crossing into a guest never contain provider bodies, parameters, or
// database diagnostics. Detailed failures stay behind the native boundary; a
// missing connection is named by the id the guest itself asked for.
fn guest_error(connection_id: &str, error: ConnectionsError) -> String {
    let (code, message) = match error {
        ConnectionsError::NotFound(_) => {
            let message = format!("Connection '{connection_id}' not found");
            return serde_json::json!({"code": "CONNECTION_NOT_FOUND", "message": message})
                .to_string();
        }
        ConnectionsError::Validation(_) => (
            "INVALID_RESOURCE_REQUEST",
            "Invalid connection resource request",
        ),
        ConnectionsError::AuthResolution(_) => (
            "RESOURCE_DISCOVERY_FAILED",
            "Connection resource discovery failed",
        ),
        _ => ("CONNECTION_RESOLUTION_FAILED", "Cannot resolve connection"),
    };
    serde_json::json!({"code": code, "message": message}).to_string()
}

#[async_trait::async_trait]
impl ConnectionResolverHost for NativeConnectionResolver {
    async fn describe(&self, tenant: &str, connection_id: String) -> Result<Vec<u8>, String> {
        let mut descriptor = self
            .0
            .describe_connection(&connection_id, tenant)
            .await
            .map_err(|error| guest_error(&connection_id, error))?
            .ok_or_else(|| {
                guest_error(&connection_id, ConnectionsError::NotFound(String::new()))
            })?;
        if descriptor.integration_id == "postgres" {
            let config = runtara_object_store::StoreConfig::builder("")
                .soft_delete(crate::config::object_model_soft_delete())
                .bulk_request_limit(crate::config::object_model_bulk_request_limit())
                .build();
            descriptor.metadata = serde_json::json!({
                "object_model": runtara_object_store::config::ObjectModelLayout::from(&config)
            });
        }
        serde_json::to_vec(&descriptor).map_err(|_| "Cannot encode connection metadata".into())
    }

    async fn resolve_resource(
        &self,
        tenant: &str,
        connection_id: String,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let request = serde_json::from_slice(&request).map_err(|_| {
            guest_error(&connection_id, ConnectionsError::Validation(String::new()))
        })?;
        let page = self
            .0
            .resolve_connection_resource(&connection_id, tenant, &request)
            .await
            .map_err(|error| guest_error(&connection_id, error))?;
        serde_json::to_vec(&page).map_err(|_| "Cannot encode connection resource".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_errors_do_not_include_internal_diagnostics() {
        for error in [
            ConnectionsError::Validation("synthetic-private-detail".into()),
            ConnectionsError::NotFound("synthetic-private-detail".into()),
        ] {
            let message = guest_error("conn-1", error);
            assert!(!message.contains("synthetic-private-detail"));
            assert!(
                serde_json::from_str::<serde_json::Value>(&message).unwrap()["code"].is_string()
            );
        }
    }

    #[test]
    fn a_missing_connection_is_named_by_the_requested_id() {
        let message = guest_error("conn-1", ConnectionsError::NotFound(String::new()));
        let body: serde_json::Value = serde_json::from_str(&message).unwrap();
        assert_eq!(body["code"], "CONNECTION_NOT_FOUND");
        assert_eq!(body["message"], "Connection 'conn-1' not found");
    }
}
