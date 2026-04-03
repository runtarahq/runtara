// Analytics HTTP handlers
// Provides runtime system information including memory, disk, and CPU details

use axum::{http::StatusCode, response::Json};
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
    // Reserve 20% for runtime, scenarios get 80%
    let available_for_scenarios = (available_memory as f64 * 0.8) as u64;

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
        available_for_scenarios_bytes: available_for_scenarios,
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
