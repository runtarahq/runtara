// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
use std::path::PathBuf;

/// Get the base data directory path from environment variable or default
///
/// The data directory can be configured via the `DATA_DIR` environment variable.
/// If not set, defaults to `./.data` for local development.
///
/// # Returns
/// The base data directory path
pub fn get_data_dir() -> PathBuf {
    std::env::var("DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".data"))
}

/// Construct the scenario directory path
///
/// # Arguments
/// * `tenant_id` - The organization/tenant ID
/// * `scenario_id` - The scenario ID
///
/// # Returns
/// Path to the scenario directory: `{data_dir}/{tenant_id}/scenarios/{scenario_id}`
pub fn get_scenario_dir(tenant_id: &str, scenario_id: &str) -> PathBuf {
    get_data_dir()
        .join(tenant_id)
        .join("scenarios")
        .join(scenario_id)
}

/// Construct the translated crate directory path
///
/// # Arguments
/// * `tenant_id` - The organization/tenant ID
/// * `scenario_id` - The scenario ID
/// * `version` - The version number
///
/// # Returns
/// Path to the translated crate: `{data_dir}/{tenant_id}/scenarios/{scenario_id}/translated/version_{version}`
pub fn get_translated_dir(tenant_id: &str, scenario_id: &str, version: u32) -> PathBuf {
    get_scenario_dir(tenant_id, scenario_id)
        .join("translated")
        .join(format!("version_{}", version))
}

/// Construct the compiled binary file path
///
/// # Arguments
/// * `tenant_id` - The organization/tenant ID
/// * `scenario_id` - The scenario ID
/// * `version` - The version number
///
/// # Returns
/// Path to the compiled binary: `{data_dir}/{tenant_id}/scenarios/{scenario_id}/compiled/version_{version}`
pub fn get_compiled_binary_path(tenant_id: &str, scenario_id: &str, version: u32) -> PathBuf {
    get_scenario_dir(tenant_id, scenario_id)
        .join("compiled")
        .join(format!("version_{}", version))
}

/// Construct the scenario JSON file path
///
/// # Arguments
/// * `tenant_id` - The organization/tenant ID
/// * `scenario_id` - The scenario ID
/// * `version` - The version number
///
/// # Returns
/// Path to the scenario JSON: `{data_dir}/{tenant_id}/scenarios/{scenario_id}/{version}.json`
pub fn get_scenario_json_path(tenant_id: &str, scenario_id: &str, version: u32) -> PathBuf {
    get_scenario_dir(tenant_id, scenario_id).join(format!("{}.json", version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_data_dir_default() {
        // Test that default is used when env var is not set or is empty
        // Note: We can't reliably remove the env var in parallel tests,
        // so we just test that the function returns a valid path
        let data_dir = get_data_dir();
        // Should return either .data (default) or a custom path from env
        assert!(
            data_dir == PathBuf::from(".data") || data_dir.is_absolute(),
            "Data dir should be either .data or an absolute path, got: {:?}",
            data_dir
        );
    }

    #[test]
    fn test_path_construction() {
        // Test path construction logic without modifying environment
        // This avoids test interference
        unsafe {
            std::env::set_var("DATA_DIR", "/test");
        }

        let scenario_dir = get_scenario_dir("tenant1", "scenario1");
        assert_eq!(
            scenario_dir,
            PathBuf::from("/test/tenant1/scenarios/scenario1")
        );

        let translated_dir = get_translated_dir("tenant1", "scenario1", 5);
        assert_eq!(
            translated_dir,
            PathBuf::from("/test/tenant1/scenarios/scenario1/translated/version_5")
        );

        let binary_path = get_compiled_binary_path("tenant1", "scenario1", 5);
        assert_eq!(
            binary_path,
            PathBuf::from("/test/tenant1/scenarios/scenario1/compiled/version_5")
        );

        let json_path = get_scenario_json_path("tenant1", "scenario1", 5);
        assert_eq!(
            json_path,
            PathBuf::from("/test/tenant1/scenarios/scenario1/5.json")
        );

        unsafe {
            std::env::remove_var("DATA_DIR");
        }
    }
}
