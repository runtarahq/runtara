#![allow(unused_imports)]

use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let crate_dir = Path::new(&manifest_dir);

    // Workspace root is 2 levels up from crates/runtara-server/
    let workspace_root = crate_dir.parent().unwrap().parent().unwrap();

    // Sibling crates (relative to this crate)
    let stdlib_src = crate_dir.join("../runtara-workflow-stdlib/src");
    let agents_integrations = crate_dir.join("../runtara-agents/src/agents/integrations");

    // Rerun if stdlib or agents source changes
    if stdlib_src.exists() {
        println!("cargo:rerun-if-changed={}", stdlib_src.display());
    }
    if agents_integrations.exists() {
        println!("cargo:rerun-if-changed={}", agents_integrations.display());
    }
    println!("cargo:rerun-if-env-changed=NATIVE_BUILD");

    // Stable cache for compiled native libraries
    let stable_cache_dir = workspace_root.join("target/native_cache");

    // Pre-compile native libraries for workflow compilation
    // Skipped by default for faster builds
    // Set NATIVE_BUILD=1 to enable (useful for CI/CD or initial setup)
    if std::env::var("NATIVE_BUILD")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        precompile_native_libraries(&stable_cache_dir, workspace_root);
    } else {
        println!("cargo:warning=   ⚡ Native library compilation skipped (default)");
        println!("cargo:warning=   Run ./scripts/build_native_library.sh manually when needed");
    }

    // Generate specs — these go to OUT_DIR since they're embedded via include_str!
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir);
    generate_specs(out_path);

    // Export the stable cache path for the main binary to find
    println!(
        "cargo:rustc-env=NATIVE_CACHE_DIR={}",
        stable_cache_dir.display()
    );

    // Allow CI to override the version
    let version = std::env::var("BUILD_VERSION")
        .or_else(|_| std::env::var("SMO_BUILD_VERSION"))
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=BUILD_VERSION={}", version);
    println!("cargo:rerun-if-env-changed=BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=SMO_BUILD_VERSION");
}

/// Generate DSL and Agent specs from runtara-dsl
fn generate_specs(out_dir: &Path) {
    use runtara_agents as _;
    use runtara_agents::integrations::ai_tools as _;
    use runtara_agents::integrations::bedrock as _;
    use runtara_agents::integrations::commerce as _;
    use runtara_agents::integrations::object_model as _;
    use runtara_agents::integrations::openai as _;
    use runtara_agents::integrations::shopify as _;
    use runtara_agents::integrations::stripe as _;
    use runtara_dsl::spec::{agent_openapi, dsl_schema};

    println!("cargo:warning=");
    println!("cargo:warning=╔════════════════════════════════════════════════════════════════╗");
    println!("cargo:warning=║  📋 GENERATING SPECS FROM RUNTARA-DSL                          ║");
    println!("cargo:warning=╚════════════════════════════════════════════════════════════════╝");

    let specs_dir = out_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs directory");

    // Generate DSL schema
    println!("cargo:warning=   → Generating DSL schema...");
    let dsl_schema = dsl_schema::generate_dsl_schema();
    let dsl_schema_json =
        serde_json::to_string_pretty(&dsl_schema).expect("Failed to serialize DSL schema");
    fs::write(specs_dir.join("dsl_schema.json"), &dsl_schema_json)
        .expect("Failed to write DSL schema");
    println!(
        "cargo:warning=      ✓ DSL schema: {} bytes",
        dsl_schema_json.len()
    );

    // Generate DSL changelog
    println!("cargo:warning=   → Generating DSL changelog...");
    let dsl_changelog = dsl_schema::get_dsl_changelog();
    let dsl_changelog_json =
        serde_json::to_string_pretty(&dsl_changelog).expect("Failed to serialize DSL changelog");
    fs::write(specs_dir.join("dsl_changelog.json"), &dsl_changelog_json)
        .expect("Failed to write DSL changelog");
    println!(
        "cargo:warning=      ✓ DSL changelog: {} bytes",
        dsl_changelog_json.len()
    );

    // Generate Agent OpenAPI spec
    println!("cargo:warning=   → Generating Agent OpenAPI spec...");
    let agents = runtara_dsl::agent_meta::get_agents();
    let agents_json: Vec<serde_json::Value> = agents
        .iter()
        .map(|a| serde_json::to_value(a).expect("Failed to serialize agent"))
        .collect();
    let agent_spec = agent_openapi::generate_agent_openapi_spec(agents_json);
    let agent_spec_json =
        serde_json::to_string_pretty(&agent_spec).expect("Failed to serialize Agent spec");
    fs::write(specs_dir.join("agent_openapi.json"), &agent_spec_json)
        .expect("Failed to write Agent spec");
    println!(
        "cargo:warning=      ✓ Agent OpenAPI: {} bytes ({} agents)",
        agent_spec_json.len(),
        agents.len()
    );

    // Generate Agent changelog
    println!("cargo:warning=   → Generating Agent changelog...");
    let agent_changelog = agent_openapi::get_agent_changelog();
    let agent_changelog_json = serde_json::to_string_pretty(&agent_changelog)
        .expect("Failed to serialize Agent changelog");
    fs::write(
        specs_dir.join("agent_changelog.json"),
        &agent_changelog_json,
    )
    .expect("Failed to write Agent changelog");
    println!(
        "cargo:warning=      ✓ Agent changelog: {} bytes",
        agent_changelog_json.len()
    );

    println!("cargo:warning=   ✓ All specs generated at:");
    println!("cargo:warning=     {}", specs_dir.display());
    println!("cargo:warning=");
}

fn precompile_native_libraries(stable_cache_dir: &Path, workspace_root: &Path) {
    let target = "wasm32-wasip2";

    println!("cargo:warning=");
    println!("cargo:warning=╔════════════════════════════════════════════════════════════════╗");
    println!(
        "cargo:warning=║  🔧 COMPILING STDLIB ({})                ║",
        target
    );
    println!("cargo:warning=╚════════════════════════════════════════════════════════════════╝");

    fs::create_dir_all(stable_cache_dir).expect("Failed to create stable cache directory");

    let lock_file = stable_cache_dir.join(".build.lock");
    let _lock = acquire_file_lock(&lock_file);

    let stdlib_build_dir = stable_cache_dir.join("stdlib_build");
    fs::create_dir_all(&stdlib_build_dir).expect("Failed to create stdlib build directory");

    let final_cache_dir = stable_cache_dir.to_path_buf();
    if can_skip_build(&final_cache_dir, workspace_root) {
        println!("cargo:warning=   ⚡ Native cache up-to-date, skipping build");
        println!("cargo:warning=");
        return;
    }

    let host_deps_to_clean = stdlib_build_dir.join("release").join("deps");
    if host_deps_to_clean.exists() {
        let _ = fs::remove_dir_all(&host_deps_to_clean);
    }

    println!("cargo:warning=   → Building for target {}...", target);

    // The runtara workspace Cargo.toml is at the workspace root
    let runtara_manifest = workspace_root.join("Cargo.toml");
    let mut cmd = Command::new("cargo");
    cmd.args([
        "build",
        "--manifest-path",
        &runtara_manifest.to_string_lossy(),
        "-p",
        "runtara-workflow-stdlib",
        "--release",
        "--target",
        target,
    ]);
    if target.contains("wasm") {
        cmd.arg("--no-default-features");
        cmd.env(
            format!(
                "CARGO_TARGET_{}_RUSTFLAGS",
                target.to_uppercase().replace('-', "_")
            ),
            "-C embed-bitcode=yes",
        );
        cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
    }
    let target_status = cmd
        .current_dir(workspace_root)
        .env("CARGO_TARGET_DIR", &stdlib_build_dir)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .expect("Failed to build runtara-workflow-stdlib for target");

    if !target_status.success() {
        panic!(
            "runtara-workflow-stdlib target build failed with status: {}",
            target_status
        );
    }

    println!("cargo:warning=      ✓ Build completed");

    // Copy artifacts to the stable cache
    let target_release = stdlib_build_dir.join(target).join("release");

    if target_release.exists() {
        let mut count = 0;
        copy_files_recursive(&target_release, &final_cache_dir, "rlib", &mut count);
        copy_files_recursive(&target_release, &final_cache_dir, "wasm", &mut count);
        println!("cargo:warning=      ✓ Copied {} artifacts to cache", count);
    }

    // Copy host proc-macro .so files (needed for scenario compilation)
    let host_release_deps = stdlib_build_dir.join("release").join("deps");
    if host_release_deps.exists() {
        let deps_cache = final_cache_dir.join("deps");
        fs::create_dir_all(&deps_cache).ok();
        let mut so_count = 0;
        copy_files_recursive(&host_release_deps, &deps_cache, "so", &mut so_count);
        copy_files_recursive(&host_release_deps, &deps_cache, "dylib", &mut so_count);
        if so_count > 0 {
            println!(
                "cargo:warning=      ✓ Copied {} proc-macro libraries",
                so_count
            );
        }
    }

    println!("cargo:warning=");
}

fn copy_files_recursive(dir: &Path, dest: &Path, ext: &str, count: &mut usize) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().map(|n| n == "deps").unwrap_or(false) {
                    let dest_deps = dest.join("deps");
                    fs::create_dir_all(&dest_deps).ok();
                    copy_files_recursive(&path, &dest_deps, ext, count);
                }
            } else if path.extension().is_some_and(|e| e == ext) {
                let dest_file = dest.join(path.file_name().unwrap());
                if fs::copy(&path, &dest_file).is_ok() {
                    *count += 1;
                }
            }
        }
    }
}

fn can_skip_build(final_cache_dir: &Path, workspace_root: &Path) -> bool {
    let rlib = final_cache_dir.join("libruntara_workflow_stdlib.rlib");
    if !rlib.exists() {
        return false;
    }

    let Ok(rlib_meta) = fs::metadata(&rlib) else {
        return false;
    };
    let Ok(rlib_modified) = rlib_meta.modified() else {
        return false;
    };

    // Check if any source file is newer than the cached rlib
    let src_dirs = [
        workspace_root.join("crates/runtara-workflow-stdlib/src"),
        workspace_root.join("crates/runtara-agents/src"),
        workspace_root.join("crates/runtara-sdk/src"),
    ];

    for src_dir in &src_dirs {
        if src_dir.exists() && is_any_file_newer(src_dir, rlib_modified) {
            return false;
        }
    }

    true
}

fn is_any_file_newer(dir: &Path, threshold: std::time::SystemTime) -> bool {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if is_any_file_newer(&path, threshold) {
                    return true;
                }
            } else if let Ok(meta) = fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    if modified > threshold {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn acquire_file_lock(lock_path: &Path) -> impl Drop {
    use std::io::Write;

    struct FileLock {
        path: std::path::PathBuf,
    }

    impl Drop for FileLock {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    // Simple spin-lock with file existence
    for _ in 0..60 {
        if !lock_path.exists() {
            if let Ok(mut f) = fs::File::create(lock_path) {
                let _ = f.write_all(format!("{}", std::process::id()).as_bytes());
                return FileLock {
                    path: lock_path.to_path_buf(),
                };
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Timeout — force acquire
    if let Ok(mut f) = fs::File::create(lock_path) {
        let _ = f.write_all(format!("{}", std::process::id()).as_bytes());
    }
    FileLock {
        path: lock_path.to_path_buf(),
    }
}
