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
        crate::middleware::entitlement::agent_decision(
            &crate::config::entitlements_for(tenant),
            agent,
        )
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
    /// A storage connection whose key is sealed cannot give it to the
    /// presigning guest. seal presigns the URL instead, within what the key's
    /// own policy allows, and the answer is the capability's.
    async fn answer(
        &self,
        tenant: &str,
        agent: &str,
        capability: &str,
        connection: &str,
        allowed: &[String],
        input: &[u8],
    ) -> Option<Result<Vec<u8>, String>> {
        // The checks `resolve` makes, in its order.
        if crate::middleware::entitlement::agent_decision(
            &crate::config::entitlements_for(tenant),
            agent,
        )
        .is_err()
        {
            return Some(Err(error("AGENT_NOT_ENABLED", "Agent is not enabled")));
        }
        self.answer_for_sealed(tenant, capability, connection, allowed, input)
            .await
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

/// The longest a presigned URL lasts, as the storage agents have it.
const MAX_PRESIGN_SECONDS: u64 = 604_800;
/// How long one lasts when the caller does not say.
const DEFAULT_PRESIGN_SECONDS: u64 = 3600;

impl BuiltinTrustedCredentials {
    /// The host's answer for a connection whose secrets are sealed, once the
    /// agent is known to be enabled for the tenant. `None` for a connection
    /// that is not sealed, or that `resolve` would refuse: it reports that.
    pub async fn answer_for_sealed(
        &self,
        tenant: &str,
        capability: &str,
        connection: &str,
        allowed: &[String],
        input: &[u8],
    ) -> Option<Result<Vec<u8>, String>> {
        let conn = self
            .0
            .get_with_parameters_for_types(connection, tenant, allowed)
            .await
            .ok()??;
        let integration = conn.integration_id.as_deref().unwrap_or_default();
        let params = conn.connection_parameters.as_ref()?;
        if !conn.sealed
            || runtara_connections::sealed::request_fields(integration, params).is_empty()
        {
            return None;
        }
        Some(
            self.presign_sealed(tenant, capability, connection, integration, params, input)
                .await,
        )
    }

    /// `storage-generate-presigned-url` for a connection whose key is sealed,
    /// answered as the storage agents answer it.
    async fn presign_sealed(
        &self,
        tenant: &str,
        capability: &str,
        connection: &str,
        integration: &str,
        params: &serde_json::Value,
        input: &[u8],
    ) -> Result<Vec<u8>, String> {
        let unavailable = |message: &str| error("TRUSTED_CREDENTIALS_UNAVAILABLE", message);
        if capability != "storage-generate-presigned-url" {
            return Err(unavailable(
                "The connection's credentials are sealed and cannot be given to this capability",
            ));
        }
        let sealed = self.0.sealed();
        let invalid = || error("INVALID_INPUT", "Bucket and object key are required");
        let input: serde_json::Value = serde_json::from_slice(input).map_err(|_| invalid())?;
        let filled = |name: &str| input[name].as_str().filter(|value| !value.is_empty());
        let bucket = filled("bucket").ok_or_else(invalid)?;
        let key = filled("key").ok_or_else(invalid)?;

        let mut headers = std::collections::HashMap::new();
        let base = self
            .0
            .resolve_connection_auth(connection, tenant, integration, params, &mut headers)
            .await
            .ok()
            .and_then(|resolved| resolved.base_url)
            .ok_or_else(|| unavailable("Missing connection endpoint"))?;
        let mut url = runtara_agent_trusted::object_url(&base, &format!("/{bucket}/{key}"))?;

        let answer = |output: serde_json::Value| {
            serde_json::to_vec(&output)
                .map_err(|_| error("INVALID_OUTPUT", "Invalid capability output"))
        };
        let operation = input["operation"].as_str().unwrap_or_default();
        let method = match operation.to_lowercase().as_str() {
            "download" | "get" | "read" => "GET",
            "upload" | "put" | "write" | "create" => "PUT",
            "delete" => "DELETE",
            other => {
                return answer(json!({
                    "success": false,
                    "error": format!(
                        "Unsupported operation `{other}` (expected download, upload, or delete)"
                    ),
                }));
            }
        };
        let expires = input["expires_in_seconds"]
            .as_u64()
            .unwrap_or(DEFAULT_PRESIGN_SECONDS)
            .min(MAX_PRESIGN_SECONDS);
        // A shared access signature can say the content type a download is
        // served with, and it is then part of what is signed.
        if integration == "azure_blob_storage"
            && let Some(content_type) = filled("content_type")
        {
            url.query_pairs_mut().append_pair("rsct", content_type);
        }

        let presigned = sealed
            .presign(
                tenant,
                connection,
                integration,
                params,
                method,
                url.as_str(),
                expires as u32,
            )
            .await;
        match presigned {
            Ok(url) => answer(json!({
                "success": true,
                "url": url.as_str(),
                "expires_in_seconds": expires,
            })),
            // seal's detail has identifiers and static text only.
            Err(refusal) => answer(json!({
                "success": false,
                "error": format!(
                    "The URL was not presigned: {}: {}",
                    refusal.code.as_str(),
                    refusal.detail
                ),
            })),
        }
    }
}
