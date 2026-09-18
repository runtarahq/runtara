//! Credential preparation for approved built-in trusted execution. The host
//! selects the implementation; the guest cannot supply authoritative metadata.

use runtara_agent_trusted::{TrustedContext, error};
use runtara_component_host::trusted::TrustedCredentials;
use runtara_connections::ConnectionsFacade;
use serde_json::json;
use std::sync::Arc;

pub struct BuiltinTrustedCredentials(pub Arc<ConnectionsFacade>);

#[async_trait::async_trait]
impl TrustedCredentials for BuiltinTrustedCredentials {
    async fn resolve(
        &self,
        tenant: &str,
        agent: &str,
        connection: &str,
        allowed: &[String],
    ) -> Result<TrustedContext, String> {
        crate::middleware::entitlement::agent_decision(crate::config::entitlements(), agent)
            .map_err(|_| error("AGENT_NOT_ENABLED", "Agent is not enabled"))?;
        let conn = self
            .0
            .get_with_parameters_for_types(connection, tenant, allowed)
            .await
            .map_err(|_| {
                error(
                    "TRUSTED_CREDENTIALS_UNAVAILABLE",
                    "Cannot resolve connection",
                )
            })?
            .ok_or_else(|| {
                error(
                    "TRUSTED_CONNECTION_DENIED",
                    "Connection is unavailable or incompatible",
                )
            })?;
        let integration = conn.integration_id.as_deref().unwrap_or_default();
        let params = conn.connection_parameters.as_ref().ok_or_else(|| {
            error(
                "TRUSTED_CREDENTIALS_UNAVAILABLE",
                "Missing connection credentials",
            )
        })?;
        // Reuse native provider configuration extraction. Network-bound OAuth
        // resolution remains host-side; the isolated component cannot do I/O.
        let mut headers = std::collections::HashMap::new();
        let resolved = self
            .0
            .resolve_connection_auth(connection, tenant, integration, params, &mut headers)
            .await
            .map_err(|_| {
                error(
                    "TRUSTED_CREDENTIALS_UNAVAILABLE",
                    "Cannot resolve signing credentials",
                )
            })?;
        let base = resolved.base_url.ok_or_else(|| {
            error(
                "TRUSTED_CREDENTIALS_UNAVAILABLE",
                "Missing connection endpoint",
            )
        })?;
        let credentials = if let Some(aws) = resolved.aws_signing {
            json!({"base_url": base, "access_key_id": aws.access_key_id,
                "secret_access_key": aws.secret_access_key, "region": aws.region,
                "session_token": aws.session_token})
        } else if let Some(azure) = resolved.azure_signing {
            json!({"base_url": base, "account_name": azure.account_name, "account_key": azure.account_key})
        } else {
            return Err(error(
                "TRUSTED_CONNECTION_TYPE",
                "Connection does not support signing",
            ));
        };
        Ok(TrustedContext {
            integration_id: integration.to_owned(),
            credentials,
            now_ms: chrono::Utc::now().timestamp_millis(),
        })
    }
    fn validate_input(
        &self,
        _agent: &str,
        capability: &str,
        input: &[u8],
        context: &TrustedContext,
    ) -> Result<(), String> {
        if capability != "storage-generate-presigned-url" {
            return Ok(());
        }
        let invalid = || error("INVALID_INPUT", "Bucket and object key are required");
        let input: serde_json::Value = serde_json::from_slice(input).map_err(|_| invalid())?;
        let bucket = input["bucket"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)?;
        let key = input["key"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)?;
        let base = context.credentials["base_url"]
            .as_str()
            .ok_or_else(invalid)?;
        runtara_agent_trusted::object_url(base, &format!("/{bucket}/{key}"))?;
        Ok(())
    }
}
