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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<OciCapabilities>,
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
    #[serde(rename = "uidMappings")]
    pub uid_mappings: Option<Vec<OciIdMapping>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "gidMappings")]
    pub gid_mappings: Option<Vec<OciIdMapping>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<OciResources>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seccomp: Option<OciSeccomp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "maskedPaths")]
    pub masked_paths: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "readonlyPaths")]
    pub readonly_paths: Option<Vec<String>>,
}

/// UID/GID mapping for user namespaces
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciIdMapping {
    #[serde(rename = "containerID")]
    pub container_id: u32,
    #[serde(rename = "hostID")]
    pub host_id: u32,
    pub size: u32,
}

/// Seccomp configuration for syscall filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OciSeccomp {
    pub default_action: String,
    pub architectures: Vec<String>,
    pub syscalls: Vec<OciSyscall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciSyscall {
    pub names: Vec<String>,
    pub action: String,
}

/// Linux capabilities configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciCapabilities {
    pub bounding: Vec<String>,
    pub effective: Vec<String>,
    pub permitted: Vec<String>,
    pub ambient: Vec<String>,
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

/// Network mode for container
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NetworkMode {
    /// Use host networking (no isolation, but allows direct QUIC access)
    #[default]
    Host,
    /// Use pasta for user-mode networking with isolation
    /// Requires pasta binary to be installed
    Pasta,
    /// Full network isolation (container has no network access)
    None,
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
    /// Run as specific user (default: 0/0 = root in container, maps to host user in rootless mode)
    pub user: (u32, u32),
    /// Network mode for the container
    pub network_mode: NetworkMode,
    /// Enable seccomp syscall filtering (default: true)
    pub enable_seccomp: bool,
    /// Drop all capabilities except minimal set (default: true)
    pub drop_capabilities: bool,
}

impl Default for BundleConfig {
    fn default() -> Self {
        Self {
            memory_limit: 512 * 1024 * 1024, // 512MB
            cpu_quota: 50000,                // 50%
            cpu_period: 100000,              // 100ms
            user: (0, 0), // Root in container (maps to host user in rootless mode)
            // Pasta networking by default for better isolation.
            // Localhost addresses are auto-transformed to gateway for connectivity.
            // Use NetworkMode::Host to bypass isolation if needed.
            network_mode: NetworkMode::Pasta,
            enable_seccomp: true,    // Seccomp filtering enabled by default
            drop_capabilities: true, // Drop dangerous capabilities by default
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
            // Mount /proc with hidepid=2 to prevent process enumeration
            OciMount {
                destination: "/proc".to_string(),
                mount_type: "proc".to_string(),
                source: "proc".to_string(),
                options: vec!["hidepid=2".to_string()],
            },
            OciMount {
                destination: "/dev".to_string(),
                mount_type: "tmpfs".to_string(),
                source: "tmpfs".to_string(),
                options: vec![
                    "nosuid".to_string(),
                    "noexec".to_string(),
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
                options: vec!["bind".to_string(), "ro".to_string(), "noexec".to_string()],
            },
            OciMount {
                destination: "/etc/hosts".to_string(),
                mount_type: "bind".to_string(),
                source: "/etc/hosts".to_string(),
                options: vec!["bind".to_string(), "ro".to_string(), "noexec".to_string()],
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
                options: vec!["bind".to_string(), "rw".to_string(), "noexec".to_string()],
            });
        }

        // If log_path is provided, add it as an environment variable
        if let Some(path) = log_path {
            env.push(format!("STDERR_LOG_PATH={}", path));
        }

        // Build namespaces list - always include basic isolation namespaces
        let mut namespaces = vec![
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
        ];

        // Add user namespace for rootless container operation
        // This is required when running crun as a non-root user
        namespaces.push(OciNamespace {
            ns_type: "user".to_string(),
        });

        // Add network namespace based on network mode
        // - Host: no network namespace (uses host networking)
        // - Pasta: network namespace (pasta will be set up externally)
        // - None: network namespace with no connectivity
        match self.config.network_mode {
            NetworkMode::Host => {
                // No network namespace = host networking
            }
            NetworkMode::Pasta | NetworkMode::None => {
                namespaces.push(OciNamespace {
                    ns_type: "network".to_string(),
                });
            }
        }

        // Set up UID/GID mappings for user namespace
        // Map container root (0) to current host user, size 1 for simple mapping
        // Use nix crate for proper UID/GID retrieval
        let host_uid = nix::unistd::getuid().as_raw();
        let host_gid = nix::unistd::getgid().as_raw();

        let uid_mappings = Some(vec![OciIdMapping {
            container_id: 0,
            host_id: host_uid,
            size: 1,
        }]);

        let gid_mappings = Some(vec![OciIdMapping {
            container_id: 0,
            host_id: host_gid,
            size: 1,
        }]);

        // Build capabilities - minimal set for running workflows
        let capabilities = if self.config.drop_capabilities {
            Some(OciCapabilities {
                // Minimal capabilities needed for basic operation
                bounding: vec![],
                effective: vec![],
                permitted: vec![],
                ambient: vec![],
            })
        } else {
            None
        };

        // Build seccomp profile - allowlist of safe syscalls
        let seccomp = if self.config.enable_seccomp {
            Some(self.generate_seccomp_profile())
        } else {
            None
        };

        // Paths to mask (hide from container)
        let masked_paths = Some(vec![
            "/proc/acpi".to_string(),
            "/proc/asound".to_string(),
            "/proc/kcore".to_string(),
            "/proc/keys".to_string(),
            "/proc/latency_stats".to_string(),
            "/proc/timer_list".to_string(),
            "/proc/timer_stats".to_string(),
            "/proc/sched_debug".to_string(),
            "/proc/scsi".to_string(),
            "/sys/firmware".to_string(),
            "/sys/devices/virtual/powercap".to_string(),
        ]);

        // Paths to make read-only
        let readonly_paths = Some(vec![
            "/proc/bus".to_string(),
            "/proc/fs".to_string(),
            "/proc/irq".to_string(),
            "/proc/sys".to_string(),
            "/proc/sysrq-trigger".to_string(),
        ]);

        let (uid, gid) = self.config.user;

        OciSpec {
            oci_version: "1.0.0".to_string(),
            process: OciProcess {
                terminal: false,
                args: vec!["/binary".to_string()],
                env,
                cwd: "/".to_string(),
                user: Some(OciUser { uid, gid }),
                capabilities,
            },
            root: OciRoot {
                path: "rootfs".to_string(),
                readonly: true,
            },
            mounts,
            linux: OciLinux {
                namespaces,
                uid_mappings,
                gid_mappings,
                resources: Some(OciResources {
                    memory: Some(OciMemory {
                        limit: self.config.memory_limit,
                    }),
                    cpu: Some(OciCpu {
                        quota: self.config.cpu_quota,
                        period: self.config.cpu_period,
                    }),
                }),
                seccomp,
                masked_paths,
                readonly_paths,
            },
        }
    }

    /// Generate a restrictive seccomp profile allowing only necessary syscalls
    fn generate_seccomp_profile(&self) -> OciSeccomp {
        OciSeccomp {
            default_action: "SCMP_ACT_ERRNO".to_string(),
            architectures: vec![
                "SCMP_ARCH_X86_64".to_string(),
                "SCMP_ARCH_AARCH64".to_string(),
            ],
            syscalls: vec![
                // File operations
                OciSyscall {
                    names: vec![
                        "read".to_string(),
                        "write".to_string(),
                        "open".to_string(),
                        "openat".to_string(),
                        "close".to_string(),
                        "stat".to_string(),
                        "fstat".to_string(),
                        "lstat".to_string(),
                        "newfstatat".to_string(),
                        "lseek".to_string(),
                        "access".to_string(),
                        "faccessat".to_string(),
                        "faccessat2".to_string(),
                        "readlink".to_string(),
                        "readlinkat".to_string(),
                        "getcwd".to_string(),
                        "dup".to_string(),
                        "dup2".to_string(),
                        "dup3".to_string(),
                        "fcntl".to_string(),
                        "flock".to_string(),
                        "fsync".to_string(),
                        "fdatasync".to_string(),
                        "ftruncate".to_string(),
                        "getdents".to_string(),
                        "getdents64".to_string(),
                        "readv".to_string(),
                        "writev".to_string(),
                        "pread64".to_string(),
                        "pwrite64".to_string(),
                        "statfs".to_string(),
                        "fstatfs".to_string(),
                        "umask".to_string(),
                        // Directory and file creation/removal
                        "mkdir".to_string(),
                        "mkdirat".to_string(),
                        "rmdir".to_string(),
                        "unlink".to_string(),
                        "unlinkat".to_string(),
                        "rename".to_string(),
                        "renameat".to_string(),
                        "renameat2".to_string(),
                        "link".to_string(),
                        "linkat".to_string(),
                        "symlink".to_string(),
                        "symlinkat".to_string(),
                        "chmod".to_string(),
                        "fchmod".to_string(),
                        "fchmodat".to_string(),
                        "chown".to_string(),
                        "fchown".to_string(),
                        "fchownat".to_string(),
                        "truncate".to_string(),
                    ],
                    action: "SCMP_ACT_ALLOW".to_string(),
                },
                // Memory management
                OciSyscall {
                    names: vec![
                        "mmap".to_string(),
                        "mprotect".to_string(),
                        "munmap".to_string(),
                        "brk".to_string(),
                        "mremap".to_string(),
                        "madvise".to_string(),
                        "membarrier".to_string(),
                    ],
                    action: "SCMP_ACT_ALLOW".to_string(),
                },
                // Process/thread management
                OciSyscall {
                    names: vec![
                        "clone".to_string(),
                        "clone3".to_string(),
                        "execve".to_string(),
                        "execveat".to_string(),
                        "exit".to_string(),
                        "exit_group".to_string(),
                        "wait4".to_string(),
                        "waitid".to_string(),
                        "getpid".to_string(),
                        "getppid".to_string(),
                        "gettid".to_string(),
                        "getuid".to_string(),
                        "getgid".to_string(),
                        "geteuid".to_string(),
                        "getegid".to_string(),
                        "getgroups".to_string(),
                        "setuid".to_string(),
                        "setgid".to_string(),
                        "setresuid".to_string(),
                        "setresgid".to_string(),
                        "setgroups".to_string(),
                        "set_tid_address".to_string(),
                        "set_robust_list".to_string(),
                        "get_robust_list".to_string(),
                        "prctl".to_string(),
                        "arch_prctl".to_string(),
                        "capget".to_string(),
                        "capset".to_string(),
                        "sched_yield".to_string(),
                        "sched_getaffinity".to_string(),
                        "sched_setaffinity".to_string(),
                        "rseq".to_string(),
                    ],
                    action: "SCMP_ACT_ALLOW".to_string(),
                },
                // Signals
                OciSyscall {
                    names: vec![
                        "rt_sigaction".to_string(),
                        "rt_sigprocmask".to_string(),
                        "rt_sigreturn".to_string(),
                        "sigaltstack".to_string(),
                        "kill".to_string(),
                        "tgkill".to_string(),
                    ],
                    action: "SCMP_ACT_ALLOW".to_string(),
                },
                // Networking (for QUIC communication with runtara-core)
                OciSyscall {
                    names: vec![
                        "socket".to_string(),
                        "socketpair".to_string(),
                        "connect".to_string(),
                        "accept".to_string(),
                        "accept4".to_string(),
                        "sendto".to_string(),
                        "recvfrom".to_string(),
                        "sendmsg".to_string(),
                        "recvmsg".to_string(),
                        "sendmmsg".to_string(),
                        "recvmmsg".to_string(),
                        "shutdown".to_string(),
                        "bind".to_string(),
                        "listen".to_string(),
                        "getsockname".to_string(),
                        "getpeername".to_string(),
                        "setsockopt".to_string(),
                        "getsockopt".to_string(),
                        "poll".to_string(),
                        "ppoll".to_string(),
                        "select".to_string(),
                        "pselect6".to_string(),
                        "epoll_create".to_string(),
                        "epoll_create1".to_string(),
                        "epoll_ctl".to_string(),
                        "epoll_wait".to_string(),
                        "epoll_pwait".to_string(),
                        "epoll_pwait2".to_string(),
                    ],
                    action: "SCMP_ACT_ALLOW".to_string(),
                },
                // Time
                OciSyscall {
                    names: vec![
                        "clock_gettime".to_string(),
                        "clock_getres".to_string(),
                        "clock_nanosleep".to_string(),
                        "nanosleep".to_string(),
                        "gettimeofday".to_string(),
                    ],
                    action: "SCMP_ACT_ALLOW".to_string(),
                },
                // Misc safe syscalls
                OciSyscall {
                    names: vec![
                        "getrandom".to_string(),
                        "uname".to_string(),
                        "sysinfo".to_string(),
                        "prlimit64".to_string(),
                        "getrlimit".to_string(),
                        "futex".to_string(),
                        "pipe".to_string(),
                        "pipe2".to_string(),
                        "eventfd".to_string(),
                        "eventfd2".to_string(),
                        "timerfd_create".to_string(),
                        "timerfd_settime".to_string(),
                        "timerfd_gettime".to_string(),
                        "ioctl".to_string(),
                    ],
                    action: "SCMP_ACT_ALLOW".to_string(),
                },
            ],
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
