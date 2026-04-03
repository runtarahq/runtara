#![allow(unused_imports)]

use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    // Rerun if stdlib source changes
    println!("cargo:rerun-if-changed=../../vendor/runtara/crates/runtara-workflow-stdlib/src");
    println!(
        "cargo:rerun-if-changed=../../vendor/runtara/crates/runtara-agents/src/agents/integrations"
    );
    // Rerun when NATIVE_BUILD changes (e.g., test step without it, then build step with it)
    println!("cargo:rerun-if-env-changed=NATIVE_BUILD");

    // Get workspace root (3 levels up from CARGO_MANIFEST_DIR)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = Path::new(&manifest_dir)
        .parent() // product
        .unwrap()
        .parent() // crates
        .unwrap()
        .parent() // workspace root
        .unwrap();

    // Use target/native_cache which is already checked by runtara-workflows
    // This is stable across builds and matches the expected location
    let stable_cache_dir = workspace_root.join("target/native_cache");

    // Pre-compile native libraries for workflow compilation
    // Skipped by default for faster builds - run ./scripts/build_native_library.sh manually
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

    // Generate specs - these go to OUT_DIR since they're embedded via include_str!
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir);
    generate_specs(out_path);

    // Export the stable cache path for the main binary to find
    println!(
        "cargo:rustc-env=NATIVE_CACHE_DIR={}",
        stable_cache_dir.display()
    );

    // Allow CI to override the version reported by the binary without modifying Cargo.toml
    // (modifying Cargo.toml invalidates cargo fingerprints and forces a full rebuild)
    if let Ok(version) = std::env::var("BUILD_VERSION") {
        println!("cargo:rustc-env=BUILD_VERSION={}", version);
    } else {
        println!(
            "cargo:rustc-env=BUILD_VERSION={}",
            std::env::var("CARGO_PKG_VERSION").unwrap()
        );
    }
    println!("cargo:rerun-if-env-changed=BUILD_VERSION");
}

/// Generate DSL and Agent specs from runtara-dsl
///
/// These are generated once at compile time and embedded into the binary
/// via include_str! in the specs handler.
fn generate_specs(out_dir: &Path) {
    use runtara_dsl::spec::{agent_openapi, dsl_schema};
    // Import runtara_agents to ensure inventory items are linked
    use runtara_agents as _;
    // Force-reference integration agent modules to ensure inventory discovers them
    use runtara_agents::integrations::ai_tools as _;
    use runtara_agents::integrations::bedrock as _;
    use runtara_agents::integrations::commerce as _;
    use runtara_agents::integrations::object_model as _;
    use runtara_agents::integrations::openai as _;
    use runtara_agents::integrations::shopify as _;
    use runtara_agents::integrations::stripe as _;

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
    // Get agents from inventory and convert to Vec<Value>
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

/// Pre-compile native libraries for workflow compilation
///
/// This compiles runtara-workflow-stdlib as a native library for the musl target,
/// which compiled workflows link against at runtime.
///
/// Uses a STABLE workspace-level cache directory (target/native_cache/) instead of
/// cargo's OUT_DIR. This prevents full rebuilds when:
/// - Cargo assigns a new build hash (happens after Ctrl+C interrupts)
/// - The build script is re-run for unrelated reasons
///
/// The nested cargo builds maintain their own incremental compilation state
/// in this stable location.
fn precompile_native_libraries(stable_cache_dir: &Path, workspace_root: &Path) {
    // Compile stdlib for WASM — scenarios are compiled to wasm32-wasip2 and executed
    // in browser or server-side WASM runtimes
    let target = "wasm32-wasip2";

    println!("cargo:warning=");
    println!("cargo:warning=╔════════════════════════════════════════════════════════════════╗");
    println!(
        "cargo:warning=║  🔧 COMPILING STDLIB ({})                ║",
        target
    );
    println!("cargo:warning=╚════════════════════════════════════════════════════════════════╝");

    // Create stable cache directory
    fs::create_dir_all(stable_cache_dir).expect("Failed to create stable cache directory");

    // Use a lock file to prevent concurrent builds from corrupting the cache
    let lock_file = stable_cache_dir.join(".build.lock");
    let _lock = acquire_file_lock(&lock_file);

    // stdlib_build is where nested cargo builds store their artifacts
    // This is STABLE across main cargo builds, so incremental compilation works
    let stdlib_build_dir = stable_cache_dir.join("stdlib_build");
    fs::create_dir_all(&stdlib_build_dir).expect("Failed to create stdlib build directory");

    // Check if we can skip the build entirely by checking modification times
    // Output directly to target/native_cache (not native subdir) to match runtara-workflows expectations
    let final_cache_dir = stable_cache_dir.to_path_buf();
    if can_skip_build(&final_cache_dir, workspace_root) {
        println!("cargo:warning=   ⚡ Native cache up-to-date, skipping build");
        println!("cargo:warning=");
        return;
    }

    // Single build step: cross-compile for target.
    // This also builds host proc-macros (needed at compile time) in target/release/deps/.
    // Using a single cargo invocation ensures the proc-macro hashes recorded in the
    // target .rlib metadata match the .so files we copy to the native cache.
    //
    // IMPORTANT: Do NOT add a separate host-only build step. Two builds produce
    // proc-macros with different SVH hashes, causing E0463 "can't find crate" errors.
    //
    // Clean host deps to prevent stale proc-macros from previous builds interfering.
    let host_deps_to_clean = stdlib_build_dir.join("release").join("deps");
    if host_deps_to_clean.exists() {
        let _ = fs::remove_dir_all(&host_deps_to_clean);
    }

    // NOTE: We use status() with inherited stdio instead of output().
    // Using output() causes a pipe buffer deadlock: cargo writes lots of output,
    // the pipe buffer (64KB) fills up, cargo blocks on write, but the parent
    // is waiting for cargo to exit - deadlock.
    println!("cargo:warning=   → Building for target {}...", target);

    // WASM: exclude C-dependent agents (xlsx, sftp, compression) via --no-default-features
    // embed-bitcode=yes enables LTO at scenario compile time for smaller binaries
    let runtara_manifest = workspace_root.join("vendor/runtara/Cargo.toml");
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
        // Use target-specific RUSTFLAGS to avoid conflicts with .cargo/config.toml
        // and parent CARGO_ENCODED_RUSTFLAGS. embed-bitcode=yes is required for LTO
        // during scenario compilation.
        cmd.env(
            format!(
                "CARGO_TARGET_{}_RUSTFLAGS",
                target.to_uppercase().replace('-', "_")
            ),
            "-C embed-bitcode=yes",
        );
        // Clear parent's encoded rustflags to prevent them overriding ours
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

    // Copy the compiled library and deps to the final location
    println!("cargo:warning=   → Copying libraries to cache...");

    let deps_dir = final_cache_dir.join("deps");

    // Clean up old libraries to prevent multiple candidates error (E0464)
    // Only remove the deps dir and rlib files, NOT stdlib_build which contains our source
    if deps_dir.exists() {
        fs::remove_dir_all(&deps_dir).expect("Failed to clean deps directory");
    }
    // Remove old rlib files in final_cache_dir (but not subdirectories like stdlib_build)
    if let Ok(entries) = fs::read_dir(&final_cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().map(|e| e == "rlib").unwrap_or(false) {
                let _ = fs::remove_file(&path);
            }
        }
    }
    fs::create_dir_all(&final_cache_dir).expect("Failed to create final cache directory");
    fs::create_dir_all(&deps_dir).expect("Failed to create deps directory");

    // Copy runtara_workflow_stdlib.rlib
    let target_release_dir = stdlib_build_dir.join(target).join("release");
    let target_deps_dir = target_release_dir.join("deps");

    // Try to find libruntara_workflow_stdlib.rlib in release dir first, then deps
    let stdlib_rlib = target_release_dir.join("libruntara_workflow_stdlib.rlib");
    if stdlib_rlib.exists() {
        fs::copy(
            &stdlib_rlib,
            final_cache_dir.join("libruntara_workflow_stdlib.rlib"),
        )
        .expect("Failed to copy libruntara_workflow_stdlib.rlib");
    } else {
        // Find in deps with hash
        let entries = fs::read_dir(&target_deps_dir).expect("Failed to read target deps dir");
        let mut found = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("libruntara_workflow_stdlib") && name_str.ends_with(".rlib") {
                fs::copy(
                    entry.path(),
                    final_cache_dir.join("libruntara_workflow_stdlib.rlib"),
                )
                .expect("Failed to copy libruntara_workflow_stdlib.rlib from deps");
                found = true;
                break;
            }
        }
        if !found {
            panic!("libruntara_workflow_stdlib.rlib not found in build output");
        }
    }

    // Copy all dependency rlibs from target build
    let entries = fs::read_dir(&target_deps_dir).expect("Failed to read target deps dir");
    let mut rlib_count = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".rlib") && !name_str.contains("runtara_workflow_stdlib") {
            fs::copy(entry.path(), deps_dir.join(&*name_str)).ok();
            rlib_count += 1;
        }
    }
    println!("cargo:warning=      Copied {} dependency rlibs", rlib_count);

    // Copy native static libraries (.a files) from build script outputs
    // These are generated by crate build.rs scripts (e.g., wit-bindgen-rt's cabi_realloc)
    let build_dir = target_release_dir.join("build");
    if build_dir.exists() {
        let mut a_count = 0;
        copy_files_recursive(&build_dir, &deps_dir, "a", &mut a_count);
        println!(
            "cargo:warning=      Copied {} native static libraries (.a)",
            a_count
        );
    }

    // Copy proc-macro .so files from host build
    let host_deps_dir = stdlib_build_dir.join("release").join("deps");
    if host_deps_dir.exists() {
        let entries = fs::read_dir(&host_deps_dir).expect("Failed to read host deps dir");
        let mut so_count = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".so") || name_str.ends_with(".dylib") {
                fs::copy(entry.path(), deps_dir.join(&*name_str)).ok();
                so_count += 1;
            }
        }
        println!(
            "cargo:warning=      Copied {} proc-macro libraries",
            so_count
        );
    }

    // Write a marker file with the build timestamp for cache validation
    let marker_file = final_cache_dir.join(".build_marker");
    fs::write(
        &marker_file,
        format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        ),
    )
    .expect("Failed to write build marker");

    println!("cargo:warning=   ✓ Native library cache ready at:");
    println!("cargo:warning=     {}", final_cache_dir.display());
    println!("cargo:warning=");
}

/// Recursively find and copy files with a given extension
fn copy_files_recursive(dir: &Path, dest: &Path, ext: &str, count: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            copy_files_recursive(&path, dest, ext, count);
        } else if path.extension().map(|e| e == ext).unwrap_or(false)
            && let Some(name) = path.file_name()
        {
            fs::copy(&path, dest.join(name)).ok();
            *count += 1;
        }
    }
}

/// Check if we can skip the native library build
///
/// Returns true if the final cache exists and is newer than all source files
fn can_skip_build(final_cache_dir: &Path, workspace_root: &Path) -> bool {
    let marker_file = final_cache_dir.join(".build_marker");

    // If no marker file, we need to build
    let Ok(marker_meta) = fs::metadata(&marker_file) else {
        return false;
    };

    let Ok(marker_time) = marker_meta.modified() else {
        return false;
    };

    // Check if any source files are newer than the marker
    let stdlib_src = workspace_root.join("vendor/runtara/crates/runtara-workflow-stdlib/src");
    let stdlib_cargo =
        workspace_root.join("vendor/runtara/crates/runtara-workflow-stdlib/Cargo.toml");
    let cargo_lock = workspace_root.join("Cargo.lock");

    // Check Cargo.toml and Cargo.lock
    for path in [&stdlib_cargo, &cargo_lock] {
        if let Ok(meta) = fs::metadata(path)
            && let Ok(mtime) = meta.modified()
            && mtime > marker_time
        {
            return false;
        }
    }

    // Check all source files recursively
    if is_any_file_newer(&stdlib_src, marker_time) {
        return false;
    }

    true
}

/// Check if any file in a directory tree is newer than the given time
fn is_any_file_newer(dir: &Path, threshold: std::time::SystemTime) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if is_any_file_newer(&path, threshold) {
                return true;
            }
        } else if let Ok(meta) = entry.metadata()
            && let Ok(mtime) = meta.modified()
            && mtime > threshold
        {
            return true;
        }
    }

    false
}

/// Simple file-based lock to prevent concurrent builds
/// Returns a guard that releases the lock when dropped
fn acquire_file_lock(lock_path: &Path) -> impl Drop {
    use std::io::Write;

    // Try to create the lock file exclusively
    // If it exists and is recent (< 10 min), wait; otherwise take over
    let max_wait = std::time::Duration::from_secs(300); // 5 min max wait
    let start = std::time::Instant::now();

    loop {
        // Check if lock is stale (older than 10 minutes)
        if let Ok(meta) = fs::metadata(lock_path)
            && let Ok(modified) = meta.modified()
            && let Ok(age) = modified.elapsed()
            && age > std::time::Duration::from_secs(600)
        {
            // Stale lock, remove it
            let _ = fs::remove_file(lock_path);
        }

        // Try to create lock file
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
        {
            Ok(mut file) => {
                // Write our PID to the lock file
                let _ = writeln!(file, "{}", std::process::id());
                break;
            }
            Err(_) => {
                // Lock exists, wait and retry
                if start.elapsed() > max_wait {
                    // Force take the lock after max wait
                    let _ = fs::remove_file(lock_path);
                    continue;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }

    // Return a guard that removes the lock on drop
    struct LockGuard(std::path::PathBuf);
    impl Drop for LockGuard {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    LockGuard(lock_path.to_path_buf())
}
