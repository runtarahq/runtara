//! Managed input operations on the internal instance protocol.
use super::*;
use runtara_core::persistence::inputs::{
    InputAuthority, InputClosure, InputError, InputRequestSpec, InputResult, InputState, request_id,
};

#[derive(Deserialize)]
pub(super) struct RegisterInputBody {
    tenant_id: String,
    descriptor: Value,
    deadline_ms: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct InputBody {
    tenant_id: String,
    signal_id: String,
}

fn error_response(error: InputError) -> Response {
    let status = match error {
        InputError::NotFound => StatusCode::NOT_FOUND,
        InputError::InvalidRequest | InputError::InvalidPayload(_) => StatusCode::BAD_REQUEST,
        InputError::Storage(_) => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::CONFLICT,
    };
    (
        status,
        Json(json!({"code": "INPUT_REQUEST_ERROR", "message": error.to_string()})),
    )
        .into_response()
}

fn state_response(state: InputState) -> Response {
    Json(match state {
        InputState::Open => json!({"state": "open"}),
        InputState::Accepted { receipt } => json!({"state": "accepted", "value": receipt.payload}),
        InputState::Closed { reason, .. } => json!({"state": "closed", "value": reason.as_str()}),
    })
    .into_response()
}

pub(super) async fn register(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<RegisterInputBody>,
) -> Response {
    let result: InputResult<()> = async {
        let spec = InputRequestSpec::from_descriptor(
            &serde_json::to_vec(&body.descriptor).map_err(|_| InputError::InvalidRequest)?,
            body.deadline_ms,
        )?;
        let inputs = state
            .persistence
            .input_requests()
            .ok_or_else(|| InputError::Storage("managed inputs unavailable".into()))?;
        let authority = InputAuthority::Root {
            tenant_id: body.tenant_id,
            instance_id,
        };
        inputs.register_input(&authority, &spec).await?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Json(SuccessResponse { success: true }).into_response(),
        Err(error) => error_response(error),
    }
}

async fn read(
    state: &InstanceHandlerState,
    instance_id: String,
    body: InputBody,
    close: bool,
) -> InputResult<InputState> {
    let inputs = state
        .persistence
        .input_requests()
        .ok_or_else(|| InputError::Storage("managed inputs unavailable".into()))?;
    let authority = InputAuthority::Root {
        tenant_id: body.tenant_id,
        instance_id,
    };
    let id = request_id(&body.signal_id);
    let record = if close {
        inputs
            .close_input(&authority, &id, InputClosure::Abandoned)
            .await?
    } else {
        inputs.poll_input(&authority, &id).await?
    };
    if record.spec.signal_id != body.signal_id {
        return Err(InputError::IdentityConflict);
    }
    Ok(record.state)
}

pub(super) async fn poll(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<InputBody>,
) -> Response {
    match read(&state, instance_id, body, false).await {
        Ok(state) => state_response(state),
        Err(error) => error_response(error),
    }
}

pub(super) async fn close(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<InputBody>,
) -> Response {
    match read(&state, instance_id, body, true).await {
        Ok(state) => state_response(state),
        Err(error) => error_response(error),
    }
}
