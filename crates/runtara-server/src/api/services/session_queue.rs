use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde_json::Value;

const QUEUE_TTL_SECS: i64 = 3600; // 1 hour
const META_TTL_SECS: i64 = 3600;

fn queue_key(org_id: &str, session_id: &str) -> String {
    format!("queue:{}:{}", org_id, session_id)
}

fn meta_key(org_id: &str, session_id: &str) -> String {
    format!("session_meta:{}:{}", org_id, session_id)
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

/// Push an event to the session queue (RPUSH + EXPIRE).
pub async fn push_event(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
    event: &Value,
) -> Result<(), redis::RedisError> {
    let key = queue_key(org_id, session_id);
    let data = serde_json::to_string(event).unwrap_or_default();
    conn.rpush::<_, _, ()>(&key, &data).await?;
    conn.expire::<_, ()>(&key, QUEUE_TTL_SECS).await?;
    Ok(())
}

/// Pop the next event from the session queue (LPOP).
pub async fn pop_event(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
) -> Result<Option<Value>, redis::RedisError> {
    let key = queue_key(org_id, session_id);
    let data: Option<String> = conn.lpop(&key, None).await?;
    Ok(data.and_then(|s| serde_json::from_str(&s).ok()))
}

/// Check if the queue has any events (non-destructive).
pub async fn has_events(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
) -> Result<bool, redis::RedisError> {
    let key = queue_key(org_id, session_id);
    let len: usize = conn.llen(&key).await?;
    Ok(len > 0)
}

/// Store session metadata (instance_id + workflow_id).
pub async fn set_session_meta(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
    instance_id: &str,
    workflow_id: &str,
) -> Result<(), redis::RedisError> {
    let key = meta_key(org_id, session_id);
    redis::pipe()
        .hset(&key, "instance_id", instance_id)
        .hset(&key, "workflow_id", workflow_id)
        .expire(&key, META_TTL_SECS)
        .query_async::<()>(conn)
        .await?;
    Ok(())
}

/// Get session metadata.
pub struct SessionMeta {
    pub instance_id: String,
    pub workflow_id: String,
}

/// Get session metadata (instance_id + workflow_id).
pub async fn get_session_meta(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
) -> Result<Option<SessionMeta>, redis::RedisError> {
    let key = meta_key(org_id, session_id);
    let values: Vec<Option<String>> = redis::pipe()
        .hget(&key, "instance_id")
        .hget(&key, "workflow_id")
        .query_async(conn)
        .await?;

    match (
        values.first().and_then(|v| v.clone()),
        values.get(1).and_then(|v| v.clone()),
    ) {
        (Some(instance_id), Some(workflow_id)) => Ok(Some(SessionMeta {
            instance_id,
            workflow_id,
        })),
        _ => Ok(None),
    }
}
