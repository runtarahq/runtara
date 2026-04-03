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

/// Store session metadata (instance_id + scenario_id).
pub async fn set_session_meta(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
    instance_id: &str,
    scenario_id: &str,
) -> Result<(), redis::RedisError> {
    let key = meta_key(org_id, session_id);
    redis::pipe()
        .hset(&key, "instance_id", instance_id)
        .hset(&key, "scenario_id", scenario_id)
        .expire(&key, META_TTL_SECS)
        .query_async::<()>(conn)
        .await?;
    Ok(())
}

/// Get session metadata.
pub struct SessionMeta {
    pub instance_id: String,
    pub scenario_id: String,
}

/// Get session metadata (instance_id + scenario_id).
pub async fn get_session_meta(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
) -> Result<Option<SessionMeta>, redis::RedisError> {
    let key = meta_key(org_id, session_id);
    let values: Vec<Option<String>> = redis::pipe()
        .hget(&key, "instance_id")
        .hget(&key, "scenario_id")
        .query_async(conn)
        .await?;

    match (
        values.first().and_then(|v| v.clone()),
        values.get(1).and_then(|v| v.clone()),
    ) {
        (Some(instance_id), Some(scenario_id)) => Ok(Some(SessionMeta {
            instance_id,
            scenario_id,
        })),
        _ => Ok(None),
    }
}
