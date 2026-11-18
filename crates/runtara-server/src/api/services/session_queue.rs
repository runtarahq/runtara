pub mod delivery;
pub mod managed;

use redis::aio::ConnectionManager;

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
