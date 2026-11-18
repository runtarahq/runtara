//! Generic webhook signature verification for trigger-linked connections.
//!
//! When a trigger has a `connection_id` in its configuration, incoming webhook
//! requests are verified against the connection's credentials. Each integration
//! type defines its own verification method.

use axum::http::HeaderMap;
use runtara_connections::{ConnectionWithParameters, ConnectionsFacade, SignatureCheck};
use serde_json::Value;
use tracing::debug;

/// Verify a webhook request against the trigger's linked connection.
///
/// Returns Ok(()) if verification passes or if no verification is needed.
/// Returns Err with a message if verification fails.
pub async fn verify_webhook(
    facade: &ConnectionsFacade,
    trigger_config: &Option<Value>,
    tenant_id: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), String> {
    let connection_id = match trigger_config
        .as_ref()
        .and_then(|c| c.get("connection_id"))
        .and_then(|v| v.as_str())
    {
        Some(id) => id,
        None => return Ok(()),
    };

    let conn = match facade.get_with_parameters(connection_id, tenant_id).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            debug!(connection_id = %connection_id, "Webhook verification skipped: connection not found");
            return Ok(());
        }
        Err(e) => {
            debug!(error = %e, "Webhook verification skipped: DB error");
            return Ok(());
        }
    };

    let integration_id = conn.integration_id.as_deref().unwrap_or("");
    if conn.connection_parameters.is_none() {
        return Ok(());
    }

    match integration_id {
        "mailgun" => verify_mailgun(facade, &conn, tenant_id, headers, body).await,
        _ => Ok(()),
    }
}

/// The connection's field that holds Mailgun's webhook signing key.
const MAILGUN_FIELD: &str = "webhook_signing_key";

/// What a Mailgun webhook signs, and the signature it carries, from its
/// body: Mailgun signs HMAC-SHA256(signing_key, timestamp + token), and puts
/// all three in the body, at the top level or under `signature`.
fn mailgun_signed(body: &[u8]) -> Result<(String, String), String> {
    let body_value: Value = serde_json::from_slice(body).unwrap_or_default();
    let sig_obj = body_value.get("signature").unwrap_or(&body_value);

    let timestamp_str = match sig_obj.get("timestamp") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err("Missing timestamp in webhook body".to_string()),
    };
    let token = sig_obj
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or("Missing token in webhook body")?;
    let signature = sig_obj
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or("Missing signature in webhook body")?;
    Ok((format!("{timestamp_str}{token}"), signature.to_owned()))
}

/// Verify Mailgun webhook signature.
///
/// A connection with no signing key accepts the webhook unchecked; a sealed
/// key is checked by seal.
async fn verify_mailgun(
    facade: &ConnectionsFacade,
    conn: &ConnectionWithParameters,
    tenant_id: &str,
    _headers: &HeaderMap,
    body: &[u8],
) -> Result<(), String> {
    let has_key = conn
        .connection_parameters
        .as_ref()
        .and_then(|params| params.get(MAILGUN_FIELD))
        .is_some_and(|key| !key.is_null() && key.as_str() != Some(""));
    if !has_key {
        return Ok(());
    }

    let (message, signature) = mailgun_signed(body)?;
    match facade
        .check_webhook_signature(
            conn,
            tenant_id,
            MAILGUN_FIELD,
            message.as_bytes(),
            &signature,
        )
        .await
        .map_err(|e| format!("Mailgun webhook signature could not be checked: {e}"))?
    {
        SignatureCheck::Valid | SignatureCheck::NoSecret => Ok(()),
        SignatureCheck::Invalid => Err("Mailgun webhook signature mismatch".to_string()),
    }
}

/// Checks a trigger's webhook before a run is accepted for it.
#[async_trait::async_trait]
pub trait WebhookCheck: Send + Sync {
    /// `Ok` if the webhook is the provider's, or needs no check. `Err` says
    /// why it is refused.
    async fn check(
        &self,
        trigger: &crate::api::dto::triggers::InvocationTrigger,
        tenant_id: &str,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), String>;
}

/// The check the engine makes: against the trigger's connection, read as it
/// is now.
pub struct ByConnection(pub std::sync::Arc<ConnectionsFacade>);

#[async_trait::async_trait]
impl WebhookCheck for ByConnection {
    async fn check(
        &self,
        trigger: &crate::api::dto::triggers::InvocationTrigger,
        tenant_id: &str,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), String> {
        verify_webhook(&self.0, &trigger.configuration, tenant_id, headers, body).await
    }
}

/// What is kept with a trigger for checking its webhook
/// (`invocation_trigger.webhook_verification`): the connection it is bound
/// to, that connection's kind, and its signing secret as it is stored,
/// sealed. A process that receives webhooks checks them from this alone.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Material {
    /// The connection the trigger was bound to when this was written.
    pub connection_id: String,
    /// The connection's kind, which says how its webhooks are signed.
    pub integration: String,
    /// The signing secret's field and its sealed value, if the kind signs
    /// its webhooks and the connection holds a secret.
    pub secret: Option<(String, Value)>,
}

impl Material {
    /// What is kept for a trigger bound to `connection`.
    pub fn of(connection: &ConnectionWithParameters) -> Self {
        let integration = connection.integration_id.clone().unwrap_or_default();
        let secret = match integration.as_str() {
            "mailgun" => connection
                .connection_parameters
                .as_ref()
                .and_then(|parameters| parameters.get(MAILGUN_FIELD))
                .filter(|value| runtara_connections::sealed::is_sealed(value))
                .map(|value| (MAILGUN_FIELD.to_owned(), value.clone())),
            _ => None,
        };
        Self {
            connection_id: connection.id.clone(),
            integration,
            secret,
        }
    }
}

/// The check a process makes that holds no connection: from what is kept
/// with the trigger. A trigger bound to a connection with nothing kept for
/// it, or kept for another connection, is refused: it is not let through
/// unchecked for want of what to check it with.
pub struct ByTrigger {
    pub pool: sqlx::PgPool,
    pub sealed: std::sync::Arc<runtara_connections::sealed::SealedCredentials>,
}

#[async_trait::async_trait]
impl WebhookCheck for ByTrigger {
    async fn check(
        &self,
        trigger: &crate::api::dto::triggers::InvocationTrigger,
        tenant_id: &str,
        _headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), String> {
        let Some(connection_id) = trigger
            .configuration
            .as_ref()
            .and_then(|c| c.get("connection_id"))
            .and_then(|v| v.as_str())
        else {
            return Ok(());
        };
        let kept = crate::api::repositories::triggers::TriggerRepository::new(self.pool.clone())
            .webhook_verification(&trigger.id)
            .await
            .map_err(|error| format!("what checks this webhook could not be read: {error}"))?;
        let material = kept
            .and_then(|kept| serde_json::from_value::<Material>(kept).ok())
            .filter(|material| material.connection_id == connection_id)
            .ok_or("this trigger's webhooks cannot be checked yet")?;
        let (sealed, connection) = (&self.sealed, material.connection_id.as_str());
        check_with(
            &material,
            |field, parameters, message, signature| async move {
                sealed
                    .verify(
                        tenant_id,
                        connection,
                        &parameters,
                        &field,
                        message,
                        signature,
                    )
                    .await
                    .map_err(|error| format!("{}: {}", error.code.as_str(), error.detail))
            },
            body,
        )
        .await
    }
}

/// Checks `body` against `material`, asking `verify` (seal) whether the
/// signature is the secret's. A kind that signs nothing, or a connection
/// with no secret, needs no check.
async fn check_with<F, Fut>(material: &Material, verify: F, body: &[u8]) -> Result<(), String>
where
    F: FnOnce(String, Value, Vec<u8>, Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<bool, String>>,
{
    let Some((field, sealed)) = &material.secret else {
        return Ok(());
    };
    match material.integration.as_str() {
        "mailgun" => {
            let (message, signature) = mailgun_signed(body)?;
            let Ok(signature) = hex::decode(signature) else {
                return Err("Mailgun webhook signature mismatch".to_string());
            };
            let parameters = serde_json::json!({ field.as_str(): sealed });
            match verify(field.clone(), parameters, message.into_bytes(), signature).await {
                Ok(true) => Ok(()),
                Ok(false) => Err("Mailgun webhook signature mismatch".to_string()),
                Err(error) => Err(format!(
                    "Mailgun webhook signature could not be checked: {error}"
                )),
            }
        }
        _ => Ok(()),
    }
}

/// Writes, for each of `tenant_id`'s triggers bound to a connection, what
/// checking its webhook needs, where what is kept is not what the connection
/// holds now. The engine runs it when a trigger is saved and every little
/// while, so that a connection whose secret is entered again is followed.
/// Answers how many it wrote.
pub async fn keep_material_current(
    pool: &sqlx::PgPool,
    facade: &ConnectionsFacade,
    tenant_id: &str,
) -> Result<usize, String> {
    let triggers = crate::api::repositories::triggers::TriggerRepository::new(pool.clone());
    let bound = triggers
        .bound_to_connections(tenant_id)
        .await
        .map_err(|error| error.to_string())?;
    let mut written = 0;
    for (trigger_id, connection_id, kept) in bound {
        let connection = match facade.get_with_parameters(&connection_id, tenant_id).await {
            Ok(Some(connection)) => connection,
            // A trigger whose connection is gone keeps nothing to check with.
            Ok(None) => continue,
            Err(error) => return Err(error.to_string()),
        };
        let material =
            serde_json::to_value(Material::of(&connection)).map_err(|error| error.to_string())?;
        if kept.as_ref() == Some(&material) {
            continue;
        }
        if triggers
            .set_webhook_verification(&trigger_id, &connection_id, &material)
            .await
            .map_err(|error| error.to_string())?
        {
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod material_tests {
    use super::*;

    fn mailgun(secret: Option<&str>) -> Material {
        Material {
            connection_id: "c-1".into(),
            integration: "mailgun".into(),
            secret: secret.map(|sealed| (MAILGUN_FIELD.to_owned(), serde_json::json!(sealed))),
        }
    }

    const SIGNED: &[u8] =
        br#"{"signature":{"timestamp":"1700000000","token":"tok","signature":"abcd"}}"#;

    async fn checked(
        material: &Material,
        body: &[u8],
        answer: Result<bool, String>,
    ) -> Result<(), String> {
        check_with(
            material,
            |field, parameters, message, signature| async move {
                assert_eq!(field, MAILGUN_FIELD);
                assert_eq!(parameters[MAILGUN_FIELD], "sealed");
                assert_eq!(message, b"1700000000tok");
                assert_eq!(signature, [0xab, 0xcd]);
                answer
            },
            body,
        )
        .await
    }

    #[tokio::test]
    async fn a_signed_webhook_is_what_seal_says_it_is() {
        let material = mailgun(Some("sealed"));
        assert!(checked(&material, SIGNED, Ok(true)).await.is_ok());
        assert_eq!(
            checked(&material, SIGNED, Ok(false)).await.unwrap_err(),
            "Mailgun webhook signature mismatch"
        );
        // A check that could not be made refuses the webhook.
        assert!(
            checked(&material, SIGNED, Err("seal is away".into()))
                .await
                .unwrap_err()
                .contains("could not be checked")
        );
    }

    #[tokio::test]
    async fn a_webhook_that_carries_no_signature_is_refused_without_asking() {
        let never = |_, _, _, _| async { panic!("seal is not asked") };
        let material = mailgun(Some("sealed"));
        assert!(check_with(&material, never, b"{}").await.is_err());
        let not_hex = br#"{"timestamp":"1","token":"t","signature":"zz"}"#;
        let never = |_, _, _, _| async { panic!("seal is not asked") };
        assert!(check_with(&material, never, not_hex).await.is_err());
    }

    #[tokio::test]
    async fn a_connection_with_nothing_to_check_against_needs_no_check() {
        let never = |_, _, _, _| async { panic!("seal is not asked") };
        assert!(check_with(&mailgun(None), never, b"anything").await.is_ok());
        let other = Material {
            connection_id: "c-1".into(),
            integration: "slack".into(),
            secret: None,
        };
        let never = |_, _, _, _| async { panic!("seal is not asked") };
        assert!(check_with(&other, never, b"anything").await.is_ok());
    }

    #[test]
    fn what_is_kept_is_read_back_as_it_was_written() {
        let material = mailgun(Some("sealed"));
        let written = serde_json::to_value(&material).unwrap();
        assert_eq!(
            serde_json::from_value::<Material>(written).unwrap(),
            material
        );
    }
}
