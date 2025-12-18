// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Management QUIC server for runtara-core.
//!
//! Accepts connections from Environment and routes protocol messages to management handlers.
//!
//! This server handles:
//! - Health checks
//! - Signal delivery (Environment proxies these)
//! - Instance status queries
//! - Instance listing
//!
//! Note: Start/stop/register operations are handled by Environment.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, error, info, instrument, warn};

use runtara_protocol::frame::Frame;
use runtara_protocol::management_proto::{
    RpcError, RpcRequest, RpcResponse, rpc_request::Request, rpc_response::Response,
};
use runtara_protocol::server::{ConnectionHandler, RuntaraServer, StreamHandler};

use crate::management_handlers::{
    ManagementHandlerState, handle_get_checkpoint, handle_get_instance_status, handle_health_check,
    handle_list_checkpoints, handle_list_instances, handle_send_custom_signal, handle_send_signal,
};

/// Shared state for management server
pub type ManagementServerState = ManagementHandlerState;

/// Run the management QUIC server
#[instrument(skip(state))]
pub async fn run_management_server(
    bind_addr: SocketAddr,
    state: Arc<ManagementServerState>,
) -> Result<()> {
    let server = RuntaraServer::localhost(bind_addr)?;

    info!(addr = %bind_addr, "Management QUIC server starting");

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
async fn handle_connection(conn: ConnectionHandler, state: Arc<ManagementServerState>) {
    info!("New management connection accepted");

    conn.run(move |stream: StreamHandler| {
        let state = state.clone();
        async move {
            if let Err(e) = handle_stream(stream, state).await {
                error!("Stream error: {}", e);
            }
        }
    })
    .await;

    debug!("Management connection closed");
}

/// Handle a single stream (request/response)
async fn handle_stream(mut stream: StreamHandler, state: Arc<ManagementServerState>) -> Result<()> {
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
        "Received management request: {:?}",
        std::mem::discriminant(&request)
    );

    // Route to appropriate handler based on request type
    let response = match request {
        Request::HealthCheck(req) => match handle_health_check(&state, req).await {
            Ok(resp) => Response::HealthCheck(resp),
            Err(e) => Response::Error(RpcError {
                code: "HEALTH_CHECK_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::SendSignal(req) => match handle_send_signal(&state, req).await {
            Ok(resp) => Response::SendSignal(resp),
            Err(e) => Response::Error(RpcError {
                code: "SEND_SIGNAL_ERROR".to_string(),
                message: e.to_string(),
            }),
        },
        Request::SendCustomSignal(req) => match handle_send_custom_signal(&state, req).await {
            Ok(resp) => Response::SendCustomSignal(resp),
            Err(e) => Response::Error(RpcError {
                code: "SEND_CUSTOM_SIGNAL_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::GetInstanceStatus(req) => match handle_get_instance_status(&state, req).await {
            Ok(resp) => Response::GetInstanceStatus(resp),
            Err(e) => Response::Error(RpcError {
                code: "GET_INSTANCE_STATUS_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::ListInstances(req) => match handle_list_instances(&state, req).await {
            Ok(resp) => Response::ListInstances(resp),
            Err(e) => Response::Error(RpcError {
                code: "LIST_INSTANCES_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::ListCheckpoints(req) => match handle_list_checkpoints(&state, req).await {
            Ok(resp) => Response::ListCheckpoints(resp),
            Err(e) => Response::Error(RpcError {
                code: "LIST_CHECKPOINTS_ERROR".to_string(),
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
    fn test_management_server_compiles() {
        // Basic compilation test
    }
}
