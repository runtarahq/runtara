// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! OCI bundle creation helper for E2E tests.
//!
//! Creates minimal OCI bundles suitable for running compiled workflows
//! in crun or other OCI-compatible runtimes.

use std::fs;
use std::io;
use std::path::Path;

/// Create an OCI bundle at the specified path.
///
/// The bundle contains:
/// - rootfs/binary: The compiled workflow binary
/// - rootfs/data/input.json: Input file (if input is provided)
/// - config.json: OCI runtime specification
///
/// # Arguments
/// * `bundle_path` - Path where the bundle will be created
/// * `binary_path` - Path to the compiled workflow binary
/// * `env_vars` - Additional environment variables for the container
/// * `input` - Optional input JSON to write to /data/input.json
///
/// # Returns
/// Ok(()) on success, or an IO error
pub fn create_oci_bundle_with_input(
    bundle_path: &Path,
    binary_path: &Path,
    env_vars: &[(&str, &str)],
    input: Option<&str>,
) -> io::Result<()> {
    let rootfs = bundle_path.join("rootfs");
    fs::create_dir_all(&rootfs)?;

    // Copy binary to rootfs
    let binary_dest = rootfs.join("binary");
    fs::copy(binary_path, &binary_dest)?;

    // Set executable permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary_dest, fs::Permissions::from_mode(0o755))?;
    }

    // Create /data directory with input.json if provided
    let data_dir = rootfs.join("data");
    fs::create_dir_all(&data_dir)?;
    if let Some(input_json) = input {
        fs::write(data_dir.join("input.json"), input_json)?;
    }

    // Generate OCI config
    let config = generate_oci_config(env_vars);
    fs::write(bundle_path.join("config.json"), config)?;

    Ok(())
}

/// Create an OCI bundle at the specified path (legacy API without input file).
pub fn create_oci_bundle(
    bundle_path: &Path,
    binary_path: &Path,
    env_vars: &[(&str, &str)],
) -> io::Result<()> {
    create_oci_bundle_with_input(bundle_path, binary_path, env_vars, None)
}

/// Generate a minimal OCI runtime specification.
fn generate_oci_config(env_vars: &[(&str, &str)]) -> String {
    // Build environment variables array
    let mut env_array: Vec<String> = vec!["PATH=/usr/bin:/bin".to_string(), "HOME=/".to_string()];

    for (key, value) in env_vars {
        env_array.push(format!("{}={}", key, value));
    }

    let env_json = serde_json::to_string(&env_array).unwrap_or_else(|_| "[]".to_string());

    format!(
        r#"{{
    "ociVersion": "1.0.0",
    "process": {{
        "terminal": false,
        "user": {{
            "uid": 0,
            "gid": 0
        }},
        "args": ["/binary"],
        "env": {},
        "cwd": "/"
    }},
    "root": {{
        "path": "rootfs",
        "readonly": false
    }},
    "mounts": [
        {{
            "destination": "/proc",
            "type": "proc",
            "source": "proc"
        }},
        {{
            "destination": "/dev",
            "type": "tmpfs",
            "source": "tmpfs",
            "options": ["nosuid", "strictatime", "mode=755", "size=65536k"]
        }},
        {{
            "destination": "/tmp",
            "type": "tmpfs",
            "source": "tmpfs",
            "options": ["nosuid", "nodev", "size=65536k"]
        }}
    ],
    "linux": {{
        "namespaces": [
            {{ "type": "pid" }},
            {{ "type": "mount" }},
            {{ "type": "network" }}
        ],
        "resources": {{
            "memory": {{
                "limit": 536870912
            }},
            "cpu": {{
                "shares": 512
            }}
        }}
    }}
}}"#,
        env_json
    )
}

/// Check if crun is available on the system.
pub fn crun_available() -> bool {
    std::process::Command::new("crun")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a container using crun.
///
/// # Arguments
/// * `bundle_path` - Path to the OCI bundle
/// * `container_id` - Unique ID for the container
///
/// # Returns
/// The output of the container process
pub fn run_container(bundle_path: &Path, container_id: &str) -> io::Result<std::process::Output> {
    let output = std::process::Command::new("crun")
        .args([
            "run",
            "--bundle",
            bundle_path.to_str().unwrap(),
            container_id,
        ])
        .output()?;

    // Clean up container (ignore errors)
    let _ = std::process::Command::new("crun")
        .args(["delete", "--force", container_id])
        .output();

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_generate_oci_config() {
        let config = generate_oci_config(&[("FOO", "bar")]);
        assert!(config.contains("ociVersion"));
        assert!(config.contains("FOO=bar"));
    }

    #[test]
    fn test_create_oci_bundle() {
        let temp_dir = TempDir::new().unwrap();
        let bundle_path = temp_dir.path().join("bundle");

        // Create a dummy binary
        let binary_dir = temp_dir.path().join("bin");
        fs::create_dir_all(&binary_dir).unwrap();
        let binary_path = binary_dir.join("test_binary");
        fs::write(&binary_path, b"#!/bin/sh\necho hello").unwrap();

        // Create bundle
        create_oci_bundle(&bundle_path, &binary_path, &[("TEST_VAR", "test_value")]).unwrap();

        // Verify bundle structure
        assert!(bundle_path.join("rootfs/binary").exists());
        assert!(bundle_path.join("config.json").exists());

        // Verify config contains our env var
        let config = fs::read_to_string(bundle_path.join("config.json")).unwrap();
        assert!(config.contains("TEST_VAR=test_value"));
    }
}
