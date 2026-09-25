pub mod delivery;
pub mod managed;

use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde_json::Value;

fn queue_key(org_id: &str, session_id: &str) -> String {
    format!("queue:{}:{}", org_id, session_id)
}

fn activity_dedup_key(identity: &str) -> String {
    format!("channel_activity_dedup:{identity}")
}

/// Reserve a one-time dedup key for an inbound activity (SET NX EX).
///
/// Returns `true` when the key was newly reserved (process this delivery) and
/// `false` when it already existed (a duplicate — Teams redelivers at-least-once
/// if the endpoint takes >~15s). Fails open (`true`) when Valkey is unreachable,
/// so a backend blip never drops real messages; the deterministic-instance-id
/// backstop still prevents a double execution in that window.
pub async fn reserve_activity_dedup(
    conn: &mut ConnectionManager,
    identity: &str,
    ttl_secs: i64,
) -> bool {
    let key = activity_dedup_key(identity);
    let set: redis::RedisResult<Option<String>> = redis::cmd("SET")
        .arg(&key)
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(ttl_secs)
        .query_async(conn)
        .await;
    match set {
        // "OK" means the key was set (fresh); nil means it already existed.
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(_) => true,
    }
}

/// Release a previously reserved dedup key so a genuine redelivery can retry.
///
/// Called when processing a reserved activity FAILED: with ack-fast the webhook
/// has already returned 200, but if Teams redelivers for any other reason
/// (e.g. it never saw our ack) the tombstone would otherwise drop a message we
/// never actually handled. Best-effort — a lost DEL just falls back to the
/// natural TTL expiry.
pub async fn release_activity_dedup(conn: &mut ConnectionManager, identity: &str) {
    let key = activity_dedup_key(identity);
    let _: redis::RedisResult<i64> = redis::cmd("DEL").arg(&key).query_async(conn).await;
}

/// An actor-buffered message. Its original execution is frozen before discovery;
/// `None` is a fresh idle/startup message, never an implicit managed response.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct BufferedEvent {
    pub message_id: String,
    pub instance_id: Option<String>,
    pub target: Option<managed::InputTarget>,
    pub payload: Value,
}
pub struct PeekedEvent {
    pub event: BufferedEvent,
    encoded: String,
}

pub async fn push_event(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
    instance_id: Option<&str>,
    payload: &Value,
) -> managed::QueueResult<()> {
    let event = BufferedEvent {
        message_id: uuid::Uuid::new_v4().to_string(),
        instance_id: instance_id.map(str::to_owned),
        target: None,
        payload: payload.clone(),
    };
    let encoded = serde_json::to_string(&event).expect("buffered event");
    // No expiry may discard a reply while discovery/delivery is unavailable.
    redis::pipe()
        .atomic()
        .rpush(queue_key(org_id, session_id), encoded)
        .persist(queue_key(org_id, session_id))
        .query_async::<()>(conn)
        .await?;
    Ok(())
}

pub async fn peek_event(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
) -> managed::QueueResult<Option<PeekedEvent>> {
    let raw: Option<String> = conn.lindex(queue_key(org_id, session_id), 0).await?;
    raw.map(|encoded| {
        let event: BufferedEvent =
            serde_json::from_str(&encoded).map_err(|_| managed::QueueError::Corrupt)?;
        if uuid::Uuid::parse_str(&event.message_id).is_err()
            || event
                .target
                .as_ref()
                .is_some_and(|target| Some(&target.instance_id) != event.instance_id.as_ref())
        {
            return Err(managed::QueueError::Corrupt);
        }
        Ok(PeekedEvent { event, encoded })
    })
    .transpose()
}

/// Freeze target before the managed-queue handoff. Even if destination retention
/// later expires, this source can only replay the same request and operation.
pub async fn bind_event(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
    source: &PeekedEvent,
    target: &managed::InputTarget,
) -> managed::QueueResult<PeekedEvent> {
    if source.event.instance_id.as_ref() != Some(&target.instance_id)
        || source
            .event
            .target
            .as_ref()
            .is_some_and(|existing| existing != target)
    {
        return Err(managed::QueueError::Conflict);
    }
    let mut event = source.event.clone();
    event.target = Some(target.clone());
    let encoded = serde_json::to_string(&event).expect("buffered event");
    let changed: bool = redis::Script::new("if redis.call('LINDEX', KEYS[1], 0) ~= ARGV[1] then return 0 end redis.call('LSET', KEYS[1], 0, ARGV[2]) return 1")
        .key(queue_key(org_id, session_id)).arg(&source.encoded).arg(&encoded).invoke_async(conn).await?;
    if !changed {
        return Err(managed::QueueError::Conflict);
    }
    Ok(PeekedEvent { event, encoded })
}

/// Remove exactly the observed head after durable handoff (or field consumption).
/// A stale acknowledgement cannot consume the following, even identical, reply.
pub async fn acknowledge_event(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
    source: &PeekedEvent,
) -> managed::QueueResult<()> {
    let changed: bool = redis::Script::new("if redis.call('LINDEX', KEYS[1], 0) ~= ARGV[1] then return 0 end redis.call('LPOP', KEYS[1]) return 1")
        .key(queue_key(org_id, session_id)).arg(&source.encoded).invoke_async(conn).await?;
    if !changed {
        return Err(managed::QueueError::Conflict);
    }
    Ok(())
}

/// Existing idle-phase startup intake. It cannot consume an execution's reply.
/// Durable launch handoff is a separate follow-up; managed response delivery
/// must use peek/bind/acknowledge instead.
pub async fn take_startup_event(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
) -> managed::QueueResult<Option<Value>> {
    let Some(source) = peek_event(conn, org_id, session_id).await? else {
        return Ok(None);
    };
    if source.event.instance_id.is_some() || source.event.target.is_some() {
        return Err(managed::QueueError::Conflict);
    }
    acknowledge_event(conn, org_id, session_id, &source).await?;
    Ok(Some(source.event.payload))
}

pub async fn has_events(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
) -> managed::QueueResult<bool> {
    Ok(conn.llen::<_, usize>(queue_key(org_id, session_id)).await? > 0)
}

/// Configure an authorized session route. Unresolved managed replies prevent
/// changing the execution before the caller has resolved those responses.
pub async fn set_session_meta(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
    instance_id: &str,
    workflow_id: &str,
) -> managed::QueueResult<()> {
    let scope = managed::QueueScope::new(org_id, session_id)?;
    managed::configure_route(
        conn,
        &scope,
        &managed::SessionRoute {
            instance_id: instance_id.into(),
            workflow_id: workflow_id.into(),
        },
    )
    .await
}

pub struct SessionMeta {
    pub instance_id: String,
    pub workflow_id: String,
}
pub async fn get_session_meta(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
) -> managed::QueueResult<Option<SessionMeta>> {
    let scope = managed::QueueScope::new(org_id, session_id)?;
    match managed::session_route(conn, &scope).await {
        Ok(route) => Ok(Some(SessionMeta {
            instance_id: route.instance_id,
            workflow_id: route.workflow_id,
        })),
        Err(managed::QueueError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}
