/// Analytics-related DTOs
use serde::Serialize;
use utoipa::ToSchema;

// ============================================================================
// Response Types
// ============================================================================

/// Memory information for the runtime system
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInfo {
    /// Total system memory in bytes
    pub total_bytes: u64,
    /// Currently available memory in bytes
    pub available_bytes: u64,
    /// Memory available for scenarios (80% of available, 20% reserved for runtime)
    pub available_for_scenarios_bytes: u64,
}

/// Disk space information for the data directory
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    /// Total disk space in bytes
    pub total_bytes: u64,
    /// Available disk space in bytes
    pub available_bytes: u64,
    /// Path to the data directory being measured
    pub path: String,
}

/// CPU information for the runtime system
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CpuInfo {
    /// CPU architecture (e.g., "x86_64", "aarch64")
    pub architecture: String,
    /// Number of physical CPU cores
    pub physical_cores: usize,
    /// Number of logical CPU cores (including hyperthreading)
    pub logical_cores: usize,
}

/// System analytics data containing memory, disk, and CPU information
#[derive(Debug, Serialize, ToSchema)]
pub struct SystemAnalyticsData {
    /// Memory information
    pub memory: MemoryInfo,
    /// Disk space information for the data directory
    pub disk: DiskInfo,
    /// CPU information
    pub cpu: CpuInfo,
}

/// Response for system analytics endpoint
#[derive(Debug, Serialize, ToSchema)]
pub struct SystemAnalyticsResponse {
    pub success: bool,
    pub message: String,
    pub data: SystemAnalyticsData,
}
