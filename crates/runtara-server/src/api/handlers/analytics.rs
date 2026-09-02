// Analytics HTTP handlers
// Provides runtime system information including memory, disk, and CPU details

use axum::{http::StatusCode, http::header, response::Json};
use serde_json::Value;
use std::path::PathBuf;
use sysinfo::Disks;

use crate::api::dto::analytics::*;

/// Get system analytics including memory, disk space, and CPU information
#[utoipa::path(
    get,
    path = "/api/runtime/analytics/system",
    responses(
        (status = 200, description = "System analytics retrieved successfully", body = SystemAnalyticsResponse),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "analytics-controller"
)]
pub async fn get_system_analytics_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
) -> (StatusCode, Json<Value>) {
    // Get memory information using sysinfo
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();

    let total_memory = sys.total_memory();
    let available_memory = sys.available_memory();
    // Reserve 20% for runtime, workflows get 80%
    let available_for_workflows = (available_memory as f64 * 0.8) as u64;

    // Get disk information for the data directory
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string());
    let data_path = PathBuf::from(&data_dir);

    // Canonicalize the path if it exists, otherwise use the original
    let canonical_path = data_path.canonicalize().unwrap_or(data_path.clone());

    let disks = Disks::new_with_refreshed_list();

    // Find the disk that contains our data directory
    let disk_info = disks
        .iter()
        .filter(|disk| canonical_path.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| DiskInfo {
            total_bytes: disk.total_space(),
            available_bytes: disk.available_space(),
            path: canonical_path.display().to_string(),
        })
        .unwrap_or_else(|| {
            // Fallback: use the first disk if we can't find the specific one
            disks
                .iter()
                .next()
                .map(|disk| DiskInfo {
                    total_bytes: disk.total_space(),
                    available_bytes: disk.available_space(),
                    path: data_dir.clone(),
                })
                .unwrap_or(DiskInfo {
                    total_bytes: 0,
                    available_bytes: 0,
                    path: data_dir,
                })
        });

    // Get CPU information
    let cpu_info = CpuInfo {
        architecture: std::env::consts::ARCH.to_string(),
        physical_cores: num_cpus::get_physical(),
        logical_cores: num_cpus::get(),
    };

    let memory_info = MemoryInfo {
        total_bytes: total_memory,
        available_bytes: available_memory,
        available_for_workflows_bytes: available_for_workflows,
    };

    let response = SystemAnalyticsResponse {
        success: true,
        message: "System analytics retrieved successfully".to_string(),
        data: SystemAnalyticsData {
            memory: memory_info,
            disk: disk_info,
            cpu: cpu_info,
        },
    };

    (
        StatusCode::OK,
        Json(serde_json::to_value(response).unwrap()),
    )
}

// ============================================================================
// Execution pipeline
// ============================================================================

/// Current occupancy of every stage of the execution pipeline.
///
/// Answers from the sampler's last snapshot rather than reading the world, so
/// this costs the same whether one viewer or fifty ask for it and no request
/// can ever cause a database query.
#[utoipa::path(
    get,
    path = "/api/runtime/analytics/pipeline",
    responses(
        (status = 200, description = "Pipeline snapshot retrieved successfully", body = crate::api::dto::pipeline::PipelineSnapshotResponse),
        (status = 503, description = "The sampler has not produced a snapshot yet", body = Value)
    ),
    tag = "analytics-controller"
)]
pub async fn get_pipeline_snapshot_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    axum::extract::State(latest): axum::extract::State<
        crate::workers::pipeline_sampler::PipelineLatest,
    >,
) -> (StatusCode, Json<Value>) {
    match latest.get() {
        Some(snapshot) => {
            let response = crate::api::dto::pipeline::PipelineSnapshotResponse {
                success: true,
                message: "Pipeline snapshot retrieved successfully".to_string(),
                data: (*snapshot).clone(),
            };
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap_or(Value::Null)),
            )
        }
        // Distinct from an empty snapshot on purpose: "the sampler has not run
        // yet" is a different fact from "the pipeline is empty", and answering
        // the first with the second would show a booting server as an idle one.
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "success": false,
                "message": "Pipeline sampler has not produced a snapshot yet",
            })),
        ),
    }
}

/// Stream pipeline snapshots as they are sampled.
///
/// Authenticated with the ordinary bearer extractor and no special-casing,
/// because the frontend consumes this with `fetch` rather than `EventSource` —
/// which cannot set headers and would otherwise push the token into a query
/// string, where it would end up in every access log.
#[utoipa::path(
    get,
    path = "/api/runtime/analytics/pipeline/stream",
    responses(
        (status = 200, description = "SSE stream of pipeline snapshots", content_type = "text/event-stream")
    ),
    tag = "analytics-controller"
)]
pub async fn stream_pipeline_handler(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    axum::extract::State(feed): axum::extract::State<
        crate::workers::pipeline_sampler::PipelineFeed,
    >,
    axum::extract::State(latest): axum::extract::State<
        crate::workers::pipeline_sampler::PipelineLatest,
    >,
) -> impl axum::response::IntoResponse {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::stream::{self, StreamExt};

    let mut rx = feed.subscribe();

    // Open with whatever is already known, so a freshly loaded page renders
    // immediately instead of sitting blank until the next tick.
    let initial = stream::iter(
        latest
            .get()
            .and_then(|snapshot| Event::default().json_data(&*snapshot).ok())
            .map(Ok::<_, std::convert::Infallible>),
    );

    let updates = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(snapshot) => {
                    if let Ok(event) = Event::default().json_data(&*snapshot) {
                        yield Ok::<_, std::convert::Infallible>(event);
                    }
                }
                // This client fell behind; the sampler and every other viewer
                // carried on. Resume from the current state rather than
                // ending the stream — a laggard wants now, not a backlog.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    let headers = [
        (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        (header::HeaderName::from_static("x-accel-buffering"), "no"),
    ];

    (
        headers,
        Sse::new(initial.chain(updates)).keep_alive(KeepAlive::default()),
    )
}
