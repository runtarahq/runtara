// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The placement API's routes ([`crate::protocol`]). Who may call them is
//! decided before a request gets here: the listener serves only the server's
//! certificate ([`crate::tls::serve`]).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::protocol::{MAX_WAIT_MS, Place, Placed, Refusal};
use crate::runs::{PlaceError, Runs};

/// What the routes act on.
#[derive(Clone)]
pub struct Api {
    pub runs: Arc<Runs>,
}

fn refuse(status: StatusCode, error: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(Refusal {
            error: error.to_owned(),
            message: message.into(),
        }),
    )
        .into_response()
}

async fn place(State(api): State<Api>, Json(place): Json<Place>) -> Response {
    match api.runs.place(place).await {
        Ok(run) => Json(Placed { run }).into_response(),
        Err(PlaceError::Invalid(message)) => refuse(StatusCode::BAD_REQUEST, "invalid", message),
        Err(PlaceError::Capacity) => refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "capacity",
            "every run slot is taken",
        ),
        Err(PlaceError::Internal(message)) => {
            tracing::error!(%message, "placing a run failed");
            refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "the executor could not start the run",
            )
        }
    }
}

#[derive(Deserialize)]
struct Wait {
    #[serde(default)]
    wait_ms: u64,
}

async fn state(
    State(api): State<Api>,
    Path(run): Path<String>,
    Query(wait): Query<Wait>,
) -> Response {
    let wait = Duration::from_millis(wait.wait_ms.min(MAX_WAIT_MS));
    match api.runs.state(&run, wait).await {
        Some(state) => Json(state).into_response(),
        None => refuse(StatusCode::NOT_FOUND, "not_found", "no such run"),
    }
}

async fn stop(State(api): State<Api>, Path(run): Path<String>) -> Response {
    if api.runs.stop(&run) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        refuse(StatusCode::NOT_FOUND, "not_found", "no such run")
    }
}

/// The placement API.
pub fn router(api: Api) -> Router {
    Router::new()
        .route("/v1/runs", post(place))
        .route("/v1/runs/{run}", get(state).delete(stop))
        .with_state(api)
}
