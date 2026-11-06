//! Background consumer for the per-tenant control-event stream, `{tenant}:control` (SYN-939).
//!
//! The control plane (`runtara-cloud`) is the only component that observes a member being
//! removed or disabled, but runtara is the only one that owns `api_keys`. So on removal the
//! control plane appends a `member_removed` event to this Valkey stream, and this consumer —
//! running in the runtara process that serves the tenant — performs the durable revocation that
//! makes the user's keys read as revoked everywhere, not just be denied at request time.
//!
//! It mirrors the trigger worker ([`crate::workers::trigger_worker`]): a dedicated (non-shared)
//! blocking `ConnectionManager`, a consumer group, `XAUTOCLAIM` to reclaim entries a crashed
//! consumer left pending, and the shutdown-aware select around the blocking read. The revoke
//! itself is terminal, timestamp-bounded and idempotent (see
//! [`crate::api::handlers::api_keys::revoke_user_keys_before`]), so at-least-once delivery is
//! safe and an entry is only `XACK`ed *after* the revoke commits.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::shutdown::ShutdownSignal;
use crate::valkey::ValkeyConfig;
use crate::valkey::client::ValkeyClient;
use crate::valkey::events::ValkeyEvent;
use crate::valkey::stream::{StreamAcker, StreamConsumer};

/// Consumer group that revokes keys on member removal.
pub const CONTROL_CONSUMER_GROUP: &str = "runtara-control-revoker";

/// The per-tenant control stream key. Tenant-first, matching `{tenant}:member:{sub}` and
/// `{tenant}:token:revoked:{jti}` — so a `RUNTARA_VALKEY_TENANT_SCOPED` process keeps it under
/// its served-tenant prefix.
pub fn control_stream_key(tenant_id: &str) -> String {
    format!("{tenant_id}:control")
}

const BLOCK_TIMEOUT_MS: usize = 5000;
const BATCH_SIZE: usize = 50;
/// Min idle before an undelivered/unacked entry is reclaimed via `XAUTOCLAIM`.
const PENDING_RETRY_DELAY_MS: u64 = 60_000;

/// Run the control-event consumer for one tenant until shutdown. Spawn one per served tenant,
/// inside `scoped(tenant, …)` so the revoke `UPDATE public.api_keys` runs under the tenant's
/// row-level-security scope.
pub async fn run(
    pool: PgPool,
    valkey_config: ValkeyConfig,
    tenant_id: String,
    shutdown: ShutdownSignal,
) {
    let consumer_name = format!("control-{}", Uuid::new_v4());
    let stream_key = control_stream_key(&tenant_id);

    // Dedicated blocking connection (never the shared manager — a BLOCKing XREADGROUP would
    // head-of-line-block every other caller on the multiplexed connection).
    let client = match ValkeyClient::new(valkey_config).await {
        Ok(client) => client,
        Err(e) => {
            tracing::error!(error = %e, tenant_id = %tenant_id, "control consumer: Valkey connect failed, not starting");
            return;
        }
    };
    let revoke_conn = client.get_connection();

    // Backstop for control events that were trimmed from the capped stream (MAXLEN ~) before this
    // consumer ever read them — e.g. runtara was down long enough for the stream to roll over.
    // Runs once, before we begin consuming new events.
    reconcile_revocations(&pool, &revoke_conn, &tenant_id).await;

    let mut consumer = StreamConsumer::new(
        client.get_connection(),
        stream_key.clone(),
        CONTROL_CONSUMER_GROUP.to_string(),
        consumer_name.clone(),
    );
    if let Err(e) = consumer.initialize_consumer_group().await {
        tracing::error!(error = %e, tenant_id = %tenant_id, "control consumer: group init failed, not starting");
        return;
    }
    let acker = consumer.acker();
    let mut autoclaim_start_id = "0-0".to_string();

    tracing::info!(tenant_id = %tenant_id, stream_key = %stream_key, "control consumer listening");

    loop {
        if shutdown.is_shutting_down() {
            return;
        }

        // PHASE 1 — reclaim entries a crashed/earlier consumer left pending (XAUTOCLAIM), so a
        // revoke that was delivered but not acked is retried rather than lost.
        match consumer
            .claim_pending_events(PENDING_RETRY_DELAY_MS, BATCH_SIZE, &autoclaim_start_id)
            .await
        {
            Ok((pending, next_start_id)) => {
                autoclaim_start_id = next_start_id;
                for (entry_id, event) in pending {
                    process_event(&pool, &revoke_conn, &tenant_id, &acker, &entry_id, &event).await;
                }
            }
            Err(e) => {
                if format!("{e}").contains("NOGROUP") {
                    // Valkey lost the group (restart / flush) — recreate and reset pagination.
                    let _ = consumer.initialize_consumer_group().await;
                    autoclaim_start_id = "0-0".to_string();
                } else {
                    tracing::warn!(error = %e, tenant_id = %tenant_id, "control consumer: claim_pending failed");
                }
            }
        }

        // PHASE 2 — new events, racing the shutdown signal. Abandoning an in-flight XREADGROUP is
        // safe: delivered entries sit in the PEL and PHASE 1 reclaims them next run.
        let read = tokio::select! {
            biased;
            _ = shutdown.clone().wait() => None,
            read = consumer.read_events(BLOCK_TIMEOUT_MS, BATCH_SIZE) => Some(read),
        };
        let Some(read) = read else {
            return;
        };
        match read {
            Ok(events) => {
                for (entry_id, event) in events {
                    process_event(&pool, &revoke_conn, &tenant_id, &acker, &entry_id, &event).await;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, tenant_id = %tenant_id, "control consumer: read failed");
                // Brief backoff so a persistent Valkey error doesn't spin, still shutdown-aware.
                tokio::select! {
                    biased;
                    _ = shutdown.clone().wait() => return,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                }
            }
        }
    }
}

/// Handle one control event. Acks only after a successful revoke (or when the entry is not
/// actionable — malformed/unknown — so it isn't redelivered forever). A revoke error leaves the
/// entry unacked, so `XAUTOCLAIM` retries it.
async fn process_event(
    pool: &PgPool,
    revoke_conn: &redis::aio::ConnectionManager,
    tenant_id: &str,
    acker: &StreamAcker,
    entry_id: &str,
    event: &ValkeyEvent,
) {
    let event_type = event.raw_data.get("type").map(String::as_str);
    match event_type {
        Some("member_removed") | Some("member_disabled") => {
            let sub = event.raw_data.get("sub").map(String::as_str);
            let at = event
                .raw_data
                .get("at")
                .and_then(|s| s.parse::<i64>().ok())
                .and_then(DateTime::<Utc>::from_timestamp_millis);
            match (sub, at) {
                (Some(sub), Some(at)) => {
                    match crate::api::handlers::api_keys::revoke_user_keys_before(
                        pool,
                        Some(revoke_conn),
                        tenant_id,
                        sub,
                        Some(at),
                    )
                    .await
                    {
                        Ok(count) => {
                            tracing::info!(tenant_id = %tenant_id, sub = %sub, revoked = count, "control consumer: revoked user keys on removal");
                            ack(acker, entry_id).await;
                        }
                        // Leave unacked on a DB/Valkey error → reclaimed and retried by XAUTOCLAIM.
                        Err(e) => {
                            tracing::warn!(error = %e, tenant_id = %tenant_id, sub = %sub, "control consumer: revoke failed, will retry");
                        }
                    }
                }
                _ => {
                    tracing::warn!(tenant_id = %tenant_id, entry_id = %entry_id, "control consumer: malformed member event (missing sub/at), dropping");
                    ack(acker, entry_id).await;
                }
            }
        }
        // Unknown/unhandled type: ack so it is not redelivered forever. Other consumers (or
        // future handlers) own their own types; this consumer only revokes.
        other => {
            tracing::debug!(tenant_id = %tenant_id, event_type = ?other, "control consumer: ignoring event type");
            ack(acker, entry_id).await;
        }
    }
}

/// Startup reconcile sweep: revoke the keys of any user who still holds an active key but is no
/// longer a member of the tenant. This is the backstop for `member_removed` events lost to stream
/// trimming (§9): the per-request membership gate already *denies* such keys (their
/// `{tenant}:member:{sub}` key is gone), and this makes the durable `is_revoked` flag catch up so
/// they also read as revoked everywhere.
///
/// Best-effort and conservative. A failure listing users, or a Valkey error checking one user's
/// membership, skips that work rather than failing startup or revoking on uncertainty — the gate
/// still protects in the meantime, and the next startup retries. A key is revoked only when its
/// issuer's membership key is confirmed **absent**. The coarser signal (current membership) means
/// a removed-then-re-added user is treated as a member, so a lost event's precise, timestamp-
/// bounded revocation of their pre-removal keys is not reconstructed here — only the common
/// "removed and still gone" case is. `revoke_user_keys_before` is idempotent, so re-running is safe.
async fn reconcile_revocations(
    pool: &PgPool,
    valkey: &redis::aio::ConnectionManager,
    tenant_id: &str,
) {
    let users = match sqlx::query_scalar::<_, String>(
        r#"
        SELECT DISTINCT issuing_user_id
        FROM public.api_keys
        WHERE org_id = $1 AND is_revoked = FALSE AND issuing_user_id IS NOT NULL
        "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    {
        Ok(users) => users,
        Err(e) => {
            tracing::warn!(error = %e, tenant_id = %tenant_id, "reconcile sweep: could not list active-key users, skipping");
            return;
        }
    };

    for user in users {
        let member_key = format!("{tenant_id}:member:{user}");
        let mut conn = valkey.clone();
        let exists: Result<bool, _> = redis::cmd("EXISTS")
            .arg(&member_key)
            .query_async(&mut conn)
            .await;
        match exists {
            // Still a member — leave the keys alone.
            Ok(true) => {}
            // Confirmed non-member with active keys → a control event was missed; revoke them now.
            Ok(false) => {
                match crate::api::handlers::api_keys::revoke_user_keys_before(
                    pool,
                    Some(valkey),
                    tenant_id,
                    &user,
                    None,
                )
                .await
                {
                    Ok(count) if count > 0 => {
                        tracing::warn!(tenant_id = %tenant_id, sub = %user, revoked = count, "reconcile sweep: revoked keys of former member (missed control event)");
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, tenant_id = %tenant_id, sub = %user, "reconcile sweep: revoke failed, leaving for next startup");
                    }
                }
            }
            // Transient read error for this user — never revoke on uncertainty; next startup retries.
            Err(e) => {
                tracing::warn!(error = %e, tenant_id = %tenant_id, sub = %user, "reconcile sweep: membership check failed, leaving keys");
            }
        }
    }
}

async fn ack(acker: &StreamAcker, entry_id: &str) {
    if let Err(e) = acker.acknowledge_event(entry_id).await {
        tracing::warn!(error = %e, entry_id = %entry_id, "control consumer: XACK failed (entry will be reclaimed)");
    }
}
