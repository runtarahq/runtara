pub mod delivery;
pub mod managed;

use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde_json::Value;

fn queue_key(org_id: &str, session_id: &str) -> String {
    format!("queue:{}:{}", org_id, session_id)
}

/// Replies received while an execution runs are buffered per execution, apart
/// from idle messages that start the next run. Neither can block the other.
fn reply_key(org_id: &str, session_id: &str, instance_id: &str) -> String {
    format!("{}:{}", queue_key(org_id, session_id), instance_id)
}

/// Backstop for reply buffers orphaned by a crashed actor. Live actors hand
/// replies off or report them undeliverable long before this.
const REPLY_BUFFER_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// An actor-buffered message. `instance_id` is `None` for a fresh idle/startup
/// message, never an implicit managed response. A reply records the request
/// that was open and prompted when it arrived; it is only ever delivered to
/// that request, never to whichever request is open later.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct BufferedEvent {
    pub message_id: String,
    pub instance_id: Option<String>,
    pub target: Option<managed::InputTarget>,
    #[serde(default)]
    pub for_request: Option<String>,
    pub payload: Value,
}
pub struct PeekedEvent {
    pub event: BufferedEvent,
    encoded: String,
    key: String,
}

/// `message_id` becomes the managed-queue message and operation id at handoff;
/// a channel reply passes its intake id so a recovered handoff is deduplicated.
pub async fn push_event(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
    instance_id: Option<&str>,
    for_request: Option<&str>,
    message_id: Option<uuid::Uuid>,
    payload: &Value,
) -> managed::QueueResult<()> {
    if for_request.is_some() && instance_id.is_none() {
        return Err(managed::QueueError::Invalid);
    }
    let event = BufferedEvent {
        message_id: message_id.unwrap_or_else(uuid::Uuid::new_v4).to_string(),
        instance_id: instance_id.map(str::to_owned),
        target: None,
        for_request: for_request.map(str::to_owned),
        payload: payload.clone(),
    };
    let encoded = serde_json::to_string(&event).expect("buffered event");
    let mut pipe = redis::pipe();
    pipe.atomic();
    match instance_id {
        Some(instance) => {
            let key = reply_key(org_id, session_id, instance);
            pipe.rpush(&key, encoded)
                .expire(&key, REPLY_BUFFER_TTL_SECS);
        }
        None => {
            // Startup messages wait for the idle loop without expiring.
            let key = queue_key(org_id, session_id);
            pipe.rpush(&key, encoded).persist(&key);
        }
    }
    pipe.query_async::<()>(conn).await?;
    Ok(())
}

/// Peek the head of an execution's reply buffer, or of the startup buffer.
pub async fn peek_event(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
    instance_id: Option<&str>,
) -> managed::QueueResult<Option<PeekedEvent>> {
    let key = match instance_id {
        Some(instance) => reply_key(org_id, session_id, instance),
        None => queue_key(org_id, session_id),
    };
    let raw: Option<String> = conn.lindex(&key, 0).await?;
    raw.map(|encoded| {
        let event: BufferedEvent =
            serde_json::from_str(&encoded).map_err(|_| managed::QueueError::Corrupt)?;
        if uuid::Uuid::parse_str(&event.message_id).is_err()
            || event.instance_id.as_deref() != instance_id
            || event
                .target
                .as_ref()
                .is_some_and(|target| Some(&target.instance_id) != event.instance_id.as_ref())
        {
            return Err(managed::QueueError::Corrupt);
        }
        Ok(PeekedEvent {
            event,
            encoded,
            key,
        })
    })
    .transpose()
}

/// Freeze the target before the managed-queue handoff. Only the request the
/// reply was received for can be chosen. Even if destination retention later
/// expires, this source can only replay the same request and operation.
pub async fn bind_event(
    conn: &mut ConnectionManager,
    source: &PeekedEvent,
    target: &managed::InputTarget,
) -> managed::QueueResult<PeekedEvent> {
    if source.event.instance_id.as_ref() != Some(&target.instance_id)
        || source.event.for_request.as_ref() != Some(&target.request_id)
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
        .key(&source.key).arg(&source.encoded).arg(&encoded).invoke_async(conn).await?;
    if !changed {
        return Err(managed::QueueError::Conflict);
    }
    Ok(PeekedEvent {
        event,
        encoded,
        key: source.key.clone(),
    })
}

/// Remove exactly the observed head after durable handoff, field consumption or
/// an explicit undeliverable notice. A stale acknowledgement cannot consume the
/// following, even identical, reply.
pub async fn acknowledge_event(
    conn: &mut ConnectionManager,
    source: &PeekedEvent,
) -> managed::QueueResult<()> {
    let changed: bool = redis::Script::new("if redis.call('LINDEX', KEYS[1], 0) ~= ARGV[1] then return 0 end redis.call('LPOP', KEYS[1]) return 1")
        .key(&source.key).arg(&source.encoded).invoke_async(conn).await?;
    if !changed {
        return Err(managed::QueueError::Conflict);
    }
    Ok(())
}

/// Idle-phase startup intake. Replies live in per-execution buffers, so this
/// can neither consume nor be blocked by an execution's reply.
/// Durable launch handoff is a separate follow-up; managed response delivery
/// must use peek/bind/acknowledge instead.
pub async fn take_startup_event(
    conn: &mut ConnectionManager,
    org_id: &str,
    session_id: &str,
) -> managed::QueueResult<Option<Value>> {
    let Some(source) = peek_event(conn, org_id, session_id, None).await? else {
        return Ok(None);
    };
    acknowledge_event(conn, &source).await?;
    Ok(Some(source.event.payload))
}

/// Whether an idle/startup message is waiting to start the next run.
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
