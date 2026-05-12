#![allow(unused_imports)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const VALIDATION_WASM_FINGERPRINT_VERSION: &str = "runtara-validation-wasm-v1";

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let crate_dir = Path::new(&manifest_dir);

    // Workspace root is 2 levels up from crates/runtara-server/
    let workspace_root = crate_dir.parent().unwrap().parent().unwrap();

    // Sibling crates (relative to this crate)
    let stdlib_src = crate_dir.join("../runtara-workflow-stdlib/src");
    let agents_integrations = crate_dir.join("../runtara-agents/src/agents/integrations");
    let ai_src = crate_dir.join("../runtara-ai/src");
    let http_src = crate_dir.join("../runtara-http/src");

    // Rerun if stdlib or agents source changes
    if stdlib_src.exists() {
        println!("cargo:rerun-if-changed={}", stdlib_src.display());
    }
    if agents_integrations.exists() {
        println!("cargo:rerun-if-changed={}", agents_integrations.display());
    }
    if ai_src.exists() {
        println!("cargo:rerun-if-changed={}", ai_src.display());
    }
    if http_src.exists() {
        println!("cargo:rerun-if-changed={}", http_src.display());
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
        println!("cargo:warning=   Run NATIVE_BUILD=1 cargo build -p runtara-server when needed");
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

    // Allow CI/release packaging to stamp the binary with the artifact version
    // and commit that produced it.
    let version = resolve_build_version();
    let commit = resolve_build_commit(workspace_root);
    println!("cargo:rustc-env=BUILD_VERSION={}", version);
    println!("cargo:rustc-env=BUILD_COMMIT={}", commit);
    println!("cargo:rerun-if-env-changed=BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=SMO_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    let git_head = workspace_root.join(".git/HEAD");
    if git_head.exists() {
        println!("cargo:rerun-if-changed={}", git_head.display());
    }

    // When the `embed-ui` feature is on, rust_embed needs frontend/dist to exist at
    // compile time. Surface a helpful error up-front if it's missing.
    if std::env::var("CARGO_FEATURE_EMBED_UI").is_ok() {
        let validation_wasm_rebuilt =
            build_workflow_validation_wasm_if_needed(workspace_root, crate_dir);
        if validation_wasm_rebuilt {
            rebuild_frontend_dist(crate_dir);
        }

        let dist = crate_dir.join("frontend/dist");
        let index = dist.join("index.html");
        if !index.exists() {
            panic!(
                "\n\n`embed-ui` feature is enabled but {} is missing.\n\
                 Build the frontend first:\n\n\
                 \x20   cd {} && npm ci && npm run build\n\n",
                index.display(),
                crate_dir.join("frontend").display()
            );
        }
        println!("cargo:rerun-if-changed={}", dist.display());
    }
}

fn build_workflow_validation_wasm_if_needed(workspace_root: &Path, crate_dir: &Path) -> bool {
    let wasm_crate = workspace_root.join("crates/runtara-workflow-validation-wasm");
    let output_dir = crate_dir.join("frontend/src/wasm/workflow-validation");
    let fingerprint_file = output_dir.join("runtara_workflow_validation.fingerprint");
    let required_outputs = [
        "package.json",
        "runtara_workflow_validation.d.ts",
        "runtara_workflow_validation.js",
        "runtara_workflow_validation_bg.wasm",
        "runtara_workflow_validation_bg.wasm.d.ts",
    ];

    let inputs = validation_wasm_inputs(workspace_root);
    for input in &inputs {
        if input.exists() {
            println!("cargo:rerun-if-changed={}", input.display());
        }
    }

    let fingerprint = validation_wasm_fingerprint(workspace_root, &inputs);
    let outputs_exist = required_outputs
        .iter()
        .all(|name| output_dir.join(name).is_file());
    let current_fingerprint = fs::read_to_string(&fingerprint_file)
        .map(|value| value.trim() == fingerprint)
        .unwrap_or(false);

    if outputs_exist && current_fingerprint {
        println!("cargo:warning=   ✓ Browser validation WASM is up-to-date");
        return false;
    }

    if Command::new("wasm-pack").arg("--version").output().is_err() {
        panic!(
            "\n\n`embed-ui` feature needs to rebuild browser validation WASM, \
             but `wasm-pack` is not available.\n\
             Install it first:\n\n\
             \x20   cargo install wasm-pack --locked\n\n"
        );
    }

    println!("cargo:warning=");
    println!("cargo:warning=╔════════════════════════════════════════════════════════════════╗");
    println!("cargo:warning=║  🧩 BUILDING BROWSER WORKFLOW VALIDATION WASM                  ║");
    println!("cargo:warning=╚════════════════════════════════════════════════════════════════╝");

    fs::create_dir_all(&output_dir).expect("Failed to create validation WASM output directory");

    let mut cmd = Command::new("wasm-pack");
    cmd.args(["build"])
        .arg(&wasm_crate)
        .args(["--target", "web"])
        .arg("--out-dir")
        .arg(&output_dir)
        .args(["--out-name", "runtara_workflow_validation"])
        .env(
            "CARGO_TARGET_DIR",
            workspace_root.join("target/validation-wasm-pack"),
        );

    let status = cmd
        .current_dir(workspace_root)
        .status()
        .expect("Failed to run wasm-pack for browser validation WASM");
    if !status.success() {
        panic!("wasm-pack failed while building browser validation WASM");
    }

    let generated_gitignore = output_dir.join(".gitignore");
    if generated_gitignore.exists() {
        fs::remove_file(&generated_gitignore)
            .expect("Failed to remove generated validation WASM .gitignore");
    }

    fs::write(&fingerprint_file, format!("{fingerprint}\n"))
        .expect("Failed to write validation WASM fingerprint");

    println!(
        "cargo:warning=   ✓ Browser validation WASM generated at {}",
        output_dir.display()
    );
    println!("cargo:warning=");

    true
}

fn rebuild_frontend_dist(crate_dir: &Path) {
    let frontend_dir = crate_dir.join("frontend");
    println!("cargo:warning=");
    println!("cargo:warning=╔════════════════════════════════════════════════════════════════╗");
    println!("cargo:warning=║  📦 REBUILDING FRONTEND DIST AFTER VALIDATION WASM UPDATE      ║");
    println!("cargo:warning=╚════════════════════════════════════════════════════════════════╝");

    let status = Command::new("npm")
        .args(["run", "build"])
        .current_dir(&frontend_dir)
        .status()
        .expect("Failed to run npm build for embedded frontend");

    if !status.success() {
        panic!(
            "\n\n`embed-ui` regenerated browser validation WASM but failed to rebuild \
             frontend/dist.\n\
             Build the frontend manually and retry:\n\n\
             \x20   cd {} && npm ci && npm run build\n\n",
            frontend_dir.display()
        );
    }

    println!("cargo:warning=   ✓ frontend/dist rebuilt");
    println!("cargo:warning=");
}

fn validation_wasm_inputs(workspace_root: &Path) -> Vec<PathBuf> {
    [
        "Cargo.toml",
        "Cargo.lock",
        "crates/runtara-workflow-validation-wasm/Cargo.toml",
        "crates/runtara-workflow-validation-wasm/src",
        "crates/runtara-workflows/Cargo.toml",
        "crates/runtara-workflows/src",
        "crates/runtara-dsl/Cargo.toml",
        "crates/runtara-dsl/src",
        "crates/runtara-agents/Cargo.toml",
        "crates/runtara-agents/src",
        "crates/runtara-ai/Cargo.toml",
        "crates/runtara-ai/src",
        "crates/runtara-http/Cargo.toml",
        "crates/runtara-http/src",
    ]
    .into_iter()
    .map(|path| workspace_root.join(path))
    .collect()
}

fn validation_wasm_fingerprint(workspace_root: &Path, inputs: &[PathBuf]) -> String {
    let mut files = Vec::new();
    for input in inputs {
        collect_files(input, &mut files);
    }
    files.sort();

    let mut hash = Fnv1a64::new();
    hash.write(VALIDATION_WASM_FINGERPRINT_VERSION.as_bytes());

    for file in files {
        let relative = file.strip_prefix(workspace_root).unwrap_or(&file);
        hash.write(relative.to_string_lossy().as_bytes());
        hash.write(&[0]);
        let content = fs::read(&file).unwrap_or_else(|e| {
            panic!(
                "Failed to read validation WASM input {}: {}",
                file.display(),
                e
            )
        });
        hash.write(&content);
        hash.write(&[0]);
    }

    format!("{:016x}", hash.finish())
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        files.push(path.to_path_buf());
        return;
    }

    if !path.is_dir() {
        return;
    }

    let entries = fs::read_dir(path).unwrap_or_else(|e| {
        panic!(
            "Failed to read validation WASM input dir {}: {}",
            path.display(),
            e
        )
    });
    for entry in entries {
        let entry = entry.expect("Failed to read validation WASM input dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}

struct Fnv1a64(u64);

impl Fnv1a64 {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

fn resolve_build_version() -> String {
    std::env::var("BUILD_VERSION")
        .or_else(|_| std::env::var("SMO_BUILD_VERSION"))
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap())
}

fn resolve_build_commit(workspace_root: &Path) -> String {
    std::env::var("BUILD_COMMIT")
        .ok()
        .and_then(clean_build_value)
        .or_else(|| std::env::var("GITHUB_SHA").ok().and_then(clean_build_value))
        .map(|commit| short_commit(&commit))
        .or_else(|| {
            git_output(workspace_root, &["rev-parse", "--short=12", "HEAD"])
                .and_then(clean_build_value)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn clean_build_value(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn short_commit(commit: &str) -> String {
    commit.chars().take(12).collect()
}

fn git_output(workspace_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace_root)
        .output()
        .ok()?;
    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
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
    let agents = runtara_agents::registry::get_agents();
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

    // Copy native .a libraries from WASM build scripts (e.g. wit-bindgen-rt's cabi_realloc)
    // These live in build/*/out/ and are needed by the linker at workflow compilation time.
    let wasm_build_dir = stdlib_build_dir.join(target).join("release").join("build");
    if wasm_build_dir.exists() {
        let deps_cache = final_cache_dir.join("deps");
        fs::create_dir_all(&deps_cache).ok();
        let mut a_count = 0;
        copy_build_script_outputs(&wasm_build_dir, &deps_cache, "a", &mut a_count);
        if a_count > 0 {
            println!(
                "cargo:warning=      ✓ Copied {} native .a libraries from build scripts",
                a_count
            );
        }
    }

    // Copy host proc-macro .so files (needed for workflow compilation)
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

/// Copy files with a given extension from build script output directories (build/*/out/).
fn copy_build_script_outputs(build_dir: &Path, dest: &Path, ext: &str, count: &mut usize) {
    if let Ok(entries) = fs::read_dir(build_dir) {
        for entry in entries.flatten() {
            let out_dir = entry.path().join("out");
            if out_dir.is_dir()
                && let Ok(files) = fs::read_dir(&out_dir)
            {
                for file in files.flatten() {
                    let path = file.path();
                    if path.extension().is_some_and(|e| e == ext) {
                        let dest_file = dest.join(path.file_name().unwrap());
                        if fs::copy(&path, &dest_file).is_ok() {
                            *count += 1;
                        }
                    }
                }
            }
        }
    }
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
        workspace_root.join("crates/runtara-ai/src"),
        workspace_root.join("crates/runtara-http/src"),
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
            } else if let Ok(meta) = fs::metadata(&path)
                && let Ok(modified) = meta.modified()
                && modified > threshold
            {
                return true;
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
        if !lock_path.exists()
            && let Ok(mut f) = fs::File::create(lock_path)
        {
            let _ = f.write_all(format!("{}", std::process::id()).as_bytes());
            return FileLock {
                path: lock_path.to_path_buf(),
            };
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
