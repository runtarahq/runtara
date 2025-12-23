// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance QUIC server for runtara-core.
//!
//! Accepts connections from instances and routes protocol messages to instance handlers.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, error, info, instrument, warn};

use runtara_protocol::frame::Frame;
use runtara_protocol::instance_proto::{
    RpcError, RpcRequest, RpcResponse, rpc_request::Request, rpc_response::Response,
};
use runtara_protocol::server::{ConnectionHandler, RuntaraServer, StreamHandler};

use crate::instance_handlers::{
    InstanceHandlerState, handle_checkpoint, handle_get_checkpoint, handle_get_instance_status,
    handle_instance_event, handle_poll_signals, handle_register_instance, handle_retry_attempt,
    handle_signal_ack, handle_sleep,
};

/// Shared state for instance server
pub type InstanceServerState = InstanceHandlerState;

/// Run the instance QUIC server
#[instrument(skip(state))]
pub async fn run_instance_server(
    bind_addr: SocketAddr,
    state: Arc<InstanceServerState>,
) -> Result<()> {
    let server = RuntaraServer::localhost(bind_addr)?;

    info!(addr = %bind_addr, "Instance QUIC server starting");

    server
        .run(move |conn: ConnectionHandler| {
            let state = state.clone();
            async move {
                handle_connection(conn, state).await;
            }
        })
        .await?;

    Ok(())
}

/// Handle a single connection
#[instrument(skip(conn, state), fields(remote = %conn.remote_address()))]
pub async fn handle_connection(conn: ConnectionHandler, state: Arc<InstanceServerState>) {
    info!("New instance connection accepted");

    conn.run(move |stream: StreamHandler| {
        let state = state.clone();
        async move {
            if let Err(e) = handle_stream(stream, state).await {
                error!("Stream error: {}", e);
            }
        }
    })
    .await;

    debug!("Instance connection closed");
}

/// Handle a single stream (request/response)
async fn handle_stream(mut stream: StreamHandler, state: Arc<InstanceServerState>) -> Result<()> {
    // Read the request frame
    let request_frame = stream.read_frame().await?;

    // Decode as RpcRequest wrapper
    let rpc_request: RpcRequest = request_frame.decode()?;

    let request = match rpc_request.request {
        Some(req) => req,
        None => {
            warn!("Received empty RpcRequest");
            let response = RpcResponse {
                response: Some(Response::Error(RpcError {
                    code: "EMPTY_REQUEST".to_string(),
                    message: "RpcRequest contained no request".to_string(),
                })),
            };
            stream.write_frame(&Frame::response(&response)?).await?;
            stream.finish()?;
            return Ok(());
        }
    };

    debug!(
        "Received instance request: {:?}",
        std::mem::discriminant(&request)
    );

    // Route to appropriate handler based on request type
    let response = match request {
        Request::RegisterInstance(req) => match handle_register_instance(&state, req).await {
            Ok(resp) => Response::RegisterInstance(resp),
            Err(e) => Response::Error(RpcError {
                code: "REGISTER_INSTANCE_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::Checkpoint(req) => match handle_checkpoint(&state, req).await {
            Ok(resp) => Response::Checkpoint(resp),
            Err(e) => Response::Error(RpcError {
                code: "CHECKPOINT_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::GetCheckpoint(req) => match handle_get_checkpoint(&state, req).await {
            Ok(resp) => Response::GetCheckpoint(resp),
            Err(e) => Response::Error(RpcError {
                code: "GET_CHECKPOINT_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::Sleep(req) => match handle_sleep(&state, req).await {
            Ok(resp) => Response::Sleep(resp),
            Err(e) => Response::Error(RpcError {
                code: "SLEEP_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::InstanceEvent(event) => {
            // Some instance events (completed/failed/suspended) return a response
            // Others (heartbeat/custom) are fire-and-forget
            match handle_instance_event(&state, event).await {
                Ok(Some(resp)) => Response::InstanceEvent(resp),
                Ok(None) => {
                    // Fire-and-forget event (heartbeat, custom)
                    stream.finish()?;
                    return Ok(());
                }
                Err(e) => Response::Error(RpcError {
                    code: "INSTANCE_EVENT_ERROR".to_string(),
                    message: e.to_string(),
                }),
            }
        }

        Request::InstanceEventBatch(batch) => {
            // Process all events in the batch (fire-and-forget for batches)
            // Note: Batches are typically used for heartbeats/custom events
            for event in batch.events {
                if let Err(e) = handle_instance_event(&state, event).await {
                    error!("Failed to handle instance event in batch: {}", e);
                }
            }
            stream.finish()?;
            return Ok(());
        }

        Request::GetInstanceStatus(req) => match handle_get_instance_status(&state, req).await {
            Ok(resp) => Response::GetInstanceStatus(resp),
            Err(e) => Response::Error(RpcError {
                code: "GET_INSTANCE_STATUS_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::PollSignals(req) => match handle_poll_signals(&state, req).await {
            Ok(resp) => Response::PollSignals(resp),
            Err(e) => Response::Error(RpcError {
                code: "POLL_SIGNALS_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::SignalAck(ack) => {
            // Signal acknowledgements are fire-and-forget
            if let Err(e) = handle_signal_ack(&state, ack).await {
                error!("Failed to handle signal ack: {}", e);
            }
            stream.finish()?;
            return Ok(());
        }

        Request::RetryAttempt(event) => {
            // Retry attempt events are fire-and-forget (audit trail)
            if let Err(e) = handle_retry_attempt(&state, event).await {
                error!("Failed to handle retry attempt event: {}", e);
            }
            stream.finish()?;
            return Ok(());
        }
    };

    // Send response
    let rpc_response = RpcResponse {
        response: Some(response),
    };
    stream.write_frame(&Frame::response(&rpc_response)?).await?;
    stream.finish()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_instance_server_compiles() {
        // Basic compilation test
    }
}
