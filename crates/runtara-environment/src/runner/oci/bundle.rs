// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! OCI bundle management.
//!
//! Creates and manages OCI bundles for crun execution.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::runner::RunnerError;

type Result<T> = std::result::Result<T, RunnerError>;

/// OCI runtime specification (config.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OciSpec {
    /// OCI specification version.
    pub oci_version: String,
    /// Process configuration.
    pub process: OciProcess,
    /// Root filesystem configuration.
    pub root: OciRoot,
    /// Mount points.
    pub mounts: Vec<OciMount>,
    /// Linux-specific configuration.
    pub linux: OciLinux,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciMount {
    pub destination: String,
    #[serde(rename = "type")]
    pub mount_type: String,
    pub source: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciProcess {
    /// Whether to allocate a terminal (must be false for detached execution)
    #[serde(default)]
    pub terminal: bool,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<OciUser>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciUser {
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciRoot {
    pub path: String,
    pub readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciLinux {
    pub namespaces: Vec<OciNamespace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<OciResources>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciNamespace {
    #[serde(rename = "type")]
    pub ns_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciResources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<OciMemory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<OciCpu>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciMemory {
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciCpu {
    pub quota: i64,
    pub period: u64,
}

/// Bundle configuration
#[derive(Debug, Clone)]
pub struct BundleConfig {
    /// Memory limit in bytes (default: 512MB)
    pub memory_limit: u64,
    /// CPU quota (microseconds per period, default: 50000 = 50%)
    pub cpu_quota: i64,
    /// CPU period (microseconds, default: 100000 = 100ms)
    pub cpu_period: u64,
    /// Run as specific user
    pub user: Option<(u32, u32)>,
}

impl Default for BundleConfig {
    fn default() -> Self {
        Self {
            memory_limit: 512 * 1024 * 1024, // 512MB
            cpu_quota: 50000,                // 50%
            cpu_period: 100000,              // 100ms
            user: None,
        }
    }
}

/// Manages OCI bundles for instances
pub struct BundleManager {
    bundles_dir: PathBuf,
    config: BundleConfig,
}

impl BundleManager {
    /// Create a new bundle manager
    pub fn new(bundles_dir: PathBuf, config: BundleConfig) -> Self {
        Self {
            bundles_dir,
            config,
        }
    }

    /// Get bundle directory for an instance
    pub fn bundle_path(&self, instance_id: &str) -> PathBuf {
        self.bundles_dir.join(instance_id)
    }

    /// Check if bundle exists
    pub fn bundle_exists(&self, instance_id: &str) -> bool {
        let bundle_dir = self.bundle_path(instance_id);
        bundle_dir.join("config.json").exists() && bundle_dir.join("rootfs/binary").exists()
    }

    /// Create or update an OCI bundle for an instance
    pub fn prepare_bundle(&self, instance_id: &str, binary: &[u8]) -> Result<PathBuf> {
        let bundle_dir = self.bundle_path(instance_id);
        let rootfs_dir = bundle_dir.join("rootfs");

        // Create directories
        fs::create_dir_all(&rootfs_dir)?;

        // Write binary
        let binary_path = rootfs_dir.join("binary");
        fs::write(&binary_path, binary)?;
        fs::set_permissions(&binary_path, Permissions::from_mode(0o755))?;

        // Generate and write config.json with default env
        let config = self.generate_oci_config(vec!["PATH=/usr/bin".to_string()], None, None);
        let config_json = serde_json::to_string_pretty(&config)?;
        fs::write(bundle_dir.join("config.json"), config_json)?;

        Ok(bundle_dir)
    }

    /// Update the bundle's config.json with runtime environment variables
    pub fn update_bundle_env(
        &self,
        instance_id: &str,
        env: &HashMap<String, String>,
        log_path: Option<&str>,
    ) -> Result<()> {
        let bundle_dir = self.bundle_path(instance_id);
        self.update_bundle_env_at_path(&bundle_dir, env, log_path)
    }

    /// Update config.json at the given bundle path
    pub fn update_bundle_env_at_path(
        &self,
        bundle_path: &Path,
        env: &HashMap<String, String>,
        log_path: Option<&str>,
    ) -> Result<()> {
        let config_path = bundle_path.join("config.json");
        self.write_config_to_path(&config_path, env, log_path)
    }

    /// Write a config.json to a specific path (for per-instance configs)
    pub fn write_config_to_path(
        &self,
        config_path: &Path,
        env: &HashMap<String, String>,
        log_path: Option<&str>,
    ) -> Result<()> {
        // Build env list in OCI format (KEY=value)
        let mut env_list = vec!["PATH=/usr/bin".to_string()];
        for (key, value) in env {
            env_list.push(format!("{}={}", key, value));
        }

        // Extract DATA_DIR for mounting
        let data_dir = env.get("DATA_DIR").map(|s| s.as_str());

        tracing::debug!(
            config_path = %config_path.display(),
            env_count = env.len(),
            data_dir = ?data_dir,
            log_path = ?log_path,
            "Writing OCI config.json"
        );

        // Ensure parent directory exists
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Generate and write config.json
        let config = self.generate_oci_config(env_list, data_dir, log_path);
        let config_json = serde_json::to_string_pretty(&config)?;
        fs::write(config_path, config_json)?;

        Ok(())
    }

    /// Generate OCI runtime configuration
    fn generate_oci_config(
        &self,
        mut env: Vec<String>,
        data_dir: Option<&str>,
        log_path: Option<&str>,
    ) -> OciSpec {
        let mut mounts = vec![
            OciMount {
                destination: "/proc".to_string(),
                mount_type: "proc".to_string(),
                source: "proc".to_string(),
                options: vec![],
            },
            OciMount {
                destination: "/dev".to_string(),
                mount_type: "tmpfs".to_string(),
                source: "tmpfs".to_string(),
                options: vec![
                    "nosuid".to_string(),
                    "strictatime".to_string(),
                    "mode=755".to_string(),
                    "size=65536k".to_string(),
                ],
            },
            // Mount host networking config for DNS resolution
            OciMount {
                destination: "/etc/resolv.conf".to_string(),
                mount_type: "bind".to_string(),
                source: "/etc/resolv.conf".to_string(),
                options: vec!["bind".to_string(), "ro".to_string()],
            },
            OciMount {
                destination: "/etc/hosts".to_string(),
                mount_type: "bind".to_string(),
                source: "/etc/hosts".to_string(),
                options: vec!["bind".to_string(), "ro".to_string()],
            },
            OciMount {
                destination: "/dev/null".to_string(),
                mount_type: "bind".to_string(),
                source: "/dev/null".to_string(),
                options: vec!["bind".to_string(), "rw".to_string()],
            },
        ];

        // Add data directory mount if provided
        if let Some(dir) = data_dir {
            mounts.push(OciMount {
                destination: dir.to_string(),
                mount_type: "bind".to_string(),
                source: dir.to_string(),
                options: vec!["bind".to_string(), "rw".to_string()],
            });
        }

        // If log_path is provided, add it as an environment variable
        if let Some(path) = log_path {
            env.push(format!("STDERR_LOG_PATH={}", path));
        }

        OciSpec {
            oci_version: "1.0.0".to_string(),
            process: OciProcess {
                terminal: false,
                args: vec!["/binary".to_string()],
                env,
                cwd: "/".to_string(),
                user: self.config.user.map(|(uid, gid)| OciUser { uid, gid }),
            },
            root: OciRoot {
                path: "rootfs".to_string(),
                readonly: true,
            },
            mounts,
            linux: OciLinux {
                // No network namespace = host networking
                namespaces: vec![
                    OciNamespace {
                        ns_type: "pid".to_string(),
                    },
                    OciNamespace {
                        ns_type: "mount".to_string(),
                    },
                    OciNamespace {
                        ns_type: "ipc".to_string(),
                    },
                    OciNamespace {
                        ns_type: "uts".to_string(),
                    },
                ],
                resources: Some(OciResources {
                    memory: Some(OciMemory {
                        limit: self.config.memory_limit,
                    }),
                    cpu: Some(OciCpu {
                        quota: self.config.cpu_quota,
                        period: self.config.cpu_period,
                    }),
                }),
            },
        }
    }

    /// Delete a bundle
    pub fn delete_bundle(&self, instance_id: &str) -> Result<()> {
        let bundle_dir = self.bundle_path(instance_id);
        if bundle_dir.exists() {
            fs::remove_dir_all(&bundle_dir)?;
        }
        Ok(())
    }
}

// ============================================================================
// Standalone functions for compile-time bundle creation
// ============================================================================

/// Generate a default OCI runtime configuration.
pub fn generate_default_oci_config() -> OciSpec {
    let manager = BundleManager::new(PathBuf::new(), BundleConfig::default());
    manager.generate_oci_config(vec!["PATH=/usr/bin".to_string()], None, None)
}

/// Create an OCI bundle at the specified path from a binary.
pub fn create_bundle_at_path(bundle_path: &Path, binary_path: &Path) -> std::io::Result<()> {
    let rootfs_dir = bundle_path.join("rootfs");

    // Create directories
    fs::create_dir_all(&rootfs_dir)?;

    // Copy binary to rootfs/binary
    let binary_dest = rootfs_dir.join("binary");
    fs::copy(binary_path, &binary_dest)?;
    fs::set_permissions(&binary_dest, Permissions::from_mode(0o755))?;

    // Generate and write config.json
    let config = generate_default_oci_config();
    let config_json = serde_json::to_string_pretty(&config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    fs::write(bundle_path.join("config.json"), config_json)?;

    Ok(())
}

/// Check if a bundle exists at the given path.
pub fn bundle_exists_at_path(bundle_path: &Path) -> bool {
    bundle_path.join("config.json").exists() && bundle_path.join("rootfs/binary").exists()
}
