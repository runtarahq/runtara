//! Local audit-event recording (SYN-437 Phase 1.11).
//!
//! runtara writes audit events to its own per-tenant `audit_events` table; smo-management
//! later ingests them into the unified audit log (transport TBD). The column shape is the
//! cross-service contract in `docs/security/user-management-contracts.md` §6 — runtara's
//! table *is* the wire format, so the schema here and on the smo-management side must match.

use serde_json::{Value, json};
use sqlx::PgPool;

/// `source` value for events runtara records.
pub const SOURCE: &str = "runtara";

/// An audit event to record. `event_type` is required (e.g. `token.create`); resource
/// type/id and payload are optional context.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub event_type: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub payload: Value,
}

impl AuditEvent {
    pub fn new(event_type: impl Into<String>) -> Self {
        Self {
            event_type: event_type.into(),
            resource_type: None,
            resource_id: None,
            payload: json!({}),
        }
    }

    /// Attach the affected resource's type and id (e.g. `("api_key", "<uuid>")`).
    pub fn resource(
        mut self,
        resource_type: impl Into<String>,
        resource_id: impl Into<String>,
    ) -> Self {
        self.resource_type = Some(resource_type.into());
        self.resource_id = Some(resource_id.into());
        self
    }

    /// Attach an event-type-specific JSON payload.
    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }
}

/// Record an audit event for `tenant_id`, acting as `actor_user_id` (`None` for system
/// actions).
///
/// Best-effort: a write failure is logged and swallowed — audit logging must never break the
/// action it records.
pub async fn emit(pool: &PgPool, tenant_id: &str, actor_user_id: Option<&str>, event: AuditEvent) {
    let result = sqlx::query(
        r#"
        INSERT INTO audit_events
            (tenant_id, actor_user_id, source, event_type, resource_type, resource_id, payload)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(tenant_id)
    .bind(actor_user_id)
    .bind(SOURCE)
    .bind(&event.event_type)
    .bind(event.resource_type.as_deref())
    .bind(event.resource_id.as_deref())
    .bind(&event.payload)
    .execute(pool)
    .await;

    if let Err(e) = result {
        tracing::warn!(
            error = %e,
            event_type = %event.event_type,
            "failed to write audit event"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_event_has_empty_defaults() {
        let event = AuditEvent::new("token.create");
        assert_eq!(event.event_type, "token.create");
        assert_eq!(event.resource_type, None);
        assert_eq!(event.resource_id, None);
        assert_eq!(event.payload, json!({}));
    }

    #[test]
    fn builder_sets_resource_and_payload() {
        let event = AuditEvent::new("workflow.update")
            .resource("workflow", "wf-1")
            .payload(json!({ "field": "name" }));
        assert_eq!(event.resource_type.as_deref(), Some("workflow"));
        assert_eq!(event.resource_id.as_deref(), Some("wf-1"));
        assert_eq!(event.payload, json!({ "field": "name" }));
    }
}
