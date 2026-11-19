//! Webhook checks for trigger-linked connections.
//!
//! When a trigger has a `connection_id` in its configuration, each webhook it
//! receives is checked against the connection's secret, the way the
//! connection's type declares its webhooks are signed
//! (`runtara_connections::webhooks`). There is no code here of any platform's.

use axum::http::HeaderMap;
use runtara_connections::webhooks::{self, Presented, WebhookCheck as Declared};
use runtara_connections::{ConnectionWithParameters, ConnectionsFacade, SignatureCheck};
use serde_json::Value;
use tracing::debug;

/// The time a webhook is checked at, in seconds since the epoch.
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A webhook as it arrived: its headers, its body, and, for a form, the
/// fields read from it.
#[derive(Clone, Copy)]
pub struct Received<'a> {
    pub headers: &'a HeaderMap,
    pub body: &'a [u8],
    pub fields: Option<&'a Value>,
}

/// What `received` presents to `declared`.
fn presented(declared: &Declared, received: Received<'_>) -> Result<Presented, String> {
    let mut request = webhooks::Request::new(
        received
            .headers
            .iter()
            .filter_map(|(name, value)| Some((name.as_str(), value.to_str().ok()?))),
        received.body,
    );
    if let Some(fields) = received.fields {
        request = request.with_fields(fields.clone());
    }
    declared.presented(&request, now())
}

/// Verify a webhook request against the trigger's linked connection, read as
/// it is now. Answers the headers the run is not to be handed.
///
/// `Ok` if the check passes, or if no check is needed: no connection, one
/// that cannot be found, a type that declares no check, or a connection
/// that holds no secret. `Err` says why the webhook is refused.
pub async fn verify_webhook(
    facade: &ConnectionsFacade,
    trigger_config: &Option<Value>,
    tenant_id: &str,
    received: Received<'_>,
) -> Result<Vec<String>, String> {
    let connection_id = match trigger_config
        .as_ref()
        .and_then(|c| c.get("connection_id"))
        .and_then(|v| v.as_str())
    {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };

    let conn = match facade.get_with_parameters(connection_id, tenant_id).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            debug!(connection_id = %connection_id, "Webhook verification skipped: connection not found");
            return Ok(Vec::new());
        }
        Err(e) => {
            debug!(error = %e, "Webhook verification skipped: DB error");
            return Ok(Vec::new());
        }
    };
    let Some(declared) = webhooks::declared(conn.integration_id.as_deref().unwrap_or("")) else {
        return Ok(Vec::new());
    };
    let withheld = declared.withheld_headers();
    if !holds_secret(&conn, &declared) {
        return Ok(withheld);
    }
    let presented = presented(&declared, received)?;
    match facade
        .check_webhook(
            &conn,
            tenant_id,
            &declared.secret_field,
            &presented.message,
            presented.mac,
        )
        .await
        .map_err(|e| format!("the webhook could not be checked: {e}"))?
    {
        SignatureCheck::Valid | SignatureCheck::NoSecret => Ok(withheld),
        SignatureCheck::Invalid => Err("the webhook's signature does not match".to_string()),
    }
}

/// Whether the connection holds the secret its webhooks are checked with. A
/// connection with none accepts its webhooks unchecked.
fn holds_secret(connection: &ConnectionWithParameters, declared: &Declared) -> bool {
    connection
        .connection_parameters
        .as_ref()
        .and_then(|params| params.get(&declared.secret_field))
        .is_some_and(|key| !key.is_null() && key.as_str() != Some(""))
}

/// Checks a trigger's webhook before a run is accepted for it.
#[async_trait::async_trait]
pub trait WebhookCheck: Send + Sync {
    /// `Ok` if the webhook is the provider's, or needs no check, with the
    /// headers its run is not to be handed (one a secret came in). `Err`
    /// says why it is refused.
    async fn check(
        &self,
        trigger: &crate::api::dto::triggers::InvocationTrigger,
        tenant_id: &str,
        received: Received<'_>,
    ) -> Result<Vec<String>, String>;
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
        received: Received<'_>,
    ) -> Result<Vec<String>, String> {
        verify_webhook(&self.0, &trigger.configuration, tenant_id, received).await
    }
}

/// What is kept with a trigger for checking its webhook
/// (`invocation_trigger.webhook_verification`): the connection it is bound
/// to, that connection's kind, how its kind's webhooks are checked, and the
/// secret as it is stored, sealed. A process that receives webhooks checks
/// them from this alone.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Material {
    /// The connection the trigger was bound to when this was written.
    pub connection_id: String,
    /// The connection's kind.
    pub integration: String,
    /// How the kind's webhooks are checked, if they are. Kept before
    /// checks were declared, it is absent, and the kind's is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Declared>,
    /// The secret's field and its sealed value, if the kind's webhooks are
    /// checked and the connection holds a secret.
    pub secret: Option<(String, Value)>,
}

impl Material {
    /// What is kept for a trigger bound to `connection`.
    pub fn of(connection: &ConnectionWithParameters) -> Self {
        let integration = connection.integration_id.clone().unwrap_or_default();
        let check = webhooks::declared(&integration);
        let secret = check.as_ref().and_then(|check| {
            connection
                .connection_parameters
                .as_ref()
                .and_then(|parameters| parameters.get(&check.secret_field))
                .filter(|value| runtara_connections::sealed::is_sealed(value))
                .map(|value| (check.secret_field.clone(), value.clone()))
        });
        Self {
            connection_id: connection.id.clone(),
            integration,
            check,
            secret,
        }
    }

    /// How webhooks are checked: as kept, or, kept before checks were
    /// declared, as the kind declares.
    fn declared(&self) -> Option<Declared> {
        self.check
            .clone()
            .or_else(|| webhooks::declared(&self.integration))
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
        received: Received<'_>,
    ) -> Result<Vec<String>, String> {
        let Some(connection_id) = trigger
            .configuration
            .as_ref()
            .and_then(|c| c.get("connection_id"))
            .and_then(|v| v.as_str())
        else {
            return Ok(Vec::new());
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
            received,
        )
        .await
    }
}

/// Checks a webhook against `material`, asking `verify` (seal) whether
/// what it presents is the secret's. A kind whose webhooks are not checked,
/// or a connection with no secret, needs no check. Answers the headers the
/// run is not to be handed.
async fn check_with<F, Fut>(
    material: &Material,
    verify: F,
    received: Received<'_>,
) -> Result<Vec<String>, String>
where
    F: FnOnce(String, Value, Vec<u8>, Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<bool, String>>,
{
    let Some(declared) = material.declared() else {
        return Ok(Vec::new());
    };
    let withheld = declared.withheld_headers();
    let Some((field, sealed)) = &material.secret else {
        return Ok(withheld);
    };
    let presented = presented(&declared, received)?;
    let parameters = serde_json::json!({ field.as_str(): sealed });
    match verify(field.clone(), parameters, presented.message, presented.mac).await {
        Ok(true) => Ok(withheld),
        Ok(false) => Err("the webhook's signature does not match".to_string()),
        Err(error) => Err(format!("the webhook could not be checked: {error}")),
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

    const MAILGUN_FIELD: &str = "webhook_signing_key";

    fn received<'a>(headers: &'a HeaderMap, body: &'a [u8]) -> Received<'a> {
        Received {
            headers,
            body,
            fields: None,
        }
    }

    fn mailgun(secret: Option<&str>) -> Material {
        Material {
            connection_id: "c-1".into(),
            integration: "mailgun".into(),
            check: webhooks::declared("mailgun"),
            secret: secret.map(|sealed| (MAILGUN_FIELD.to_owned(), serde_json::json!(sealed))),
        }
    }

    const SIGNED: &[u8] =
        br#"{"signature":{"timestamp":"1700000000","token":"tok","signature":"abcd"}}"#;

    async fn checked(
        material: &Material,
        body: &[u8],
        answer: Result<bool, String>,
    ) -> Result<Vec<String>, String> {
        check_with(
            material,
            |field, parameters, message, signature| async move {
                assert_eq!(field, MAILGUN_FIELD);
                assert_eq!(parameters[MAILGUN_FIELD], "sealed");
                assert_eq!(message, b"1700000000tok");
                assert_eq!(signature, [0xab, 0xcd]);
                answer
            },
            received(&HeaderMap::new(), body),
        )
        .await
    }

    #[tokio::test]
    async fn a_signed_webhook_is_what_seal_says_it_is() {
        let material = mailgun(Some("sealed"));
        assert!(checked(&material, SIGNED, Ok(true)).await.is_ok());
        assert_eq!(
            checked(&material, SIGNED, Ok(false)).await.unwrap_err(),
            "the webhook's signature does not match"
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
        let material = mailgun(Some("sealed"));
        let never = |_, _, _, _| async { panic!("seal is not asked") };
        assert!(
            check_with(&material, never, received(&HeaderMap::new(), b"{}"))
                .await
                .is_err()
        );
        let not_hex = br#"{"timestamp":"1","token":"t","signature":"zz"}"#;
        let never = |_, _, _, _| async { panic!("seal is not asked") };
        assert!(
            check_with(&material, never, received(&HeaderMap::new(), not_hex))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_connection_with_nothing_to_check_against_needs_no_check() {
        let never = |_, _, _, _| async { panic!("seal is not asked") };
        assert!(
            check_with(
                &mailgun(None),
                never,
                received(&HeaderMap::new(), b"anything")
            )
            .await
            .is_ok()
        );
        let other = Material {
            connection_id: "c-1".into(),
            integration: "http_bearer".into(),
            check: None,
            secret: None,
        };
        let never = |_, _, _, _| async { panic!("seal is not asked") };
        assert!(
            check_with(&other, never, received(&HeaderMap::new(), b"anything"))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_slack_webhook_is_checked_as_slack_signs_it() {
        let material = Material {
            connection_id: "c-1".into(),
            integration: "slack_bot".into(),
            check: webhooks::declared("slack_bot"),
            secret: Some(("signing_secret".into(), serde_json::json!("sealed"))),
        };
        let at = now().to_string();
        let mut headers = HeaderMap::new();
        headers.insert("x-slack-request-timestamp", at.parse().unwrap());
        headers.insert("x-slack-signature", "v0=0a0b".parse().unwrap());
        let body = br#"{"type":"event_callback"}"#;
        let expected = format!("v0:{at}:{}", std::str::from_utf8(body).unwrap());
        let withheld = check_with(
            &material,
            |field, _, message, mac| async move {
                assert_eq!(field, "signing_secret");
                assert_eq!(message, expected.into_bytes());
                assert_eq!(mac, [0x0a, 0x0b]);
                Ok(true)
            },
            received(&headers, body),
        )
        .await
        .unwrap();
        assert!(withheld.is_empty());

        // Signed an hour ago: refused without asking.
        let mut stale = headers.clone();
        stale.insert(
            "x-slack-request-timestamp",
            (now() - 3600).to_string().parse().unwrap(),
        );
        let never = |_, _, _, _| async { panic!("seal is not asked") };
        assert!(
            check_with(&material, never, received(&stale, body))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_telegram_token_is_checked_and_not_handed_to_the_run() {
        let material = Material {
            connection_id: "c-1".into(),
            integration: "telegram_bot".into(),
            check: webhooks::declared("telegram_bot"),
            secret: Some(("webhook_secret".into(), serde_json::json!("sealed"))),
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-telegram-bot-api-secret-token", "tok".parse().unwrap());
        let withheld = check_with(
            &material,
            |_, _, message, mac| async move {
                assert!(message.is_empty());
                assert_eq!(mac, b"tok");
                Ok(true)
            },
            received(&headers, b"{}"),
        )
        .await
        .unwrap();
        assert_eq!(withheld, ["x-telegram-bot-api-secret-token"]);
    }

    #[test]
    fn what_is_kept_is_read_back_as_it_was_written() {
        let material = mailgun(Some("sealed"));
        let written = serde_json::to_value(&material).unwrap();
        assert_eq!(
            serde_json::from_value::<Material>(written).unwrap(),
            material
        );
        // Kept before checks were declared: checked as its kind declares.
        let old: Material = serde_json::from_value(serde_json::json!({
            "connection_id": "c-1",
            "integration": "mailgun",
            "secret": [MAILGUN_FIELD, "sealed"],
        }))
        .unwrap();
        assert_eq!(old.declared(), webhooks::declared("mailgun"));
    }
}
