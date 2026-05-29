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
    let agents_src = crate_dir.join("../runtara-agents/src/agents");
    let ai_src = crate_dir.join("../runtara-ai/src");
    let http_src = crate_dir.join("../runtara-http/src");

    // Rerun if stdlib or agents source changes
    if stdlib_src.exists() {
        println!("cargo:rerun-if-changed={}", stdlib_src.display());
    }
    if agents_src.exists() {
        println!("cargo:rerun-if-changed={}", agents_src.display());
    }
    if ai_src.exists() {
        println!("cargo:rerun-if-changed={}", ai_src.display());
    }
    if http_src.exists() {
        println!("cargo:rerun-if-changed={}", http_src.display());
    }

    // Generate specs — these go to OUT_DIR since they're embedded via include_str!
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir);
    generate_specs(out_path);

    // Allow CI/release packaging to stamp the binary with the artifact version
    // and commit that produced it.
    let version = resolve_build_version();
    let commit = resolve_build_commit(workspace_root);
    let build_number = resolve_build_number();
    println!("cargo:rustc-env=BUILD_VERSION={}", version);
    println!("cargo:rustc-env=BUILD_COMMIT={}", commit);
    println!("cargo:rustc-env=BUILD_NUMBER={}", build_number);
    println!("cargo:rerun-if-env-changed=BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=SMO_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-env-changed=BUILD_NUMBER");
    println!("cargo:rerun-if-env-changed=GITHUB_RUN_NUMBER");
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
        clean_workflow_validation_wasm_output(&output_dir, &required_outputs, true);
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
    clean_workflow_validation_wasm_output(&output_dir, &required_outputs, false);

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

fn clean_workflow_validation_wasm_output(
    output_dir: &Path,
    required_outputs: &[&str],
    keep_current: bool,
) {
    if !output_dir.exists() {
        return;
    }

    for entry in fs::read_dir(output_dir).expect("Failed to read validation WASM output directory")
    {
        let entry = entry.expect("Failed to read validation WASM output directory entry");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let should_keep = keep_current
            && (name == "runtara_workflow_validation.fingerprint"
                || required_outputs
                    .iter()
                    .any(|required| *required == name.as_ref()));

        if should_keep {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path)
                .expect("Failed to remove stale validation WASM output directory");
        } else {
            fs::remove_file(&path).expect("Failed to remove stale validation WASM output file");
        }
    }
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

fn resolve_build_number() -> String {
    std::env::var("BUILD_NUMBER")
        .or_else(|_| std::env::var("GITHUB_RUN_NUMBER"))
        .unwrap_or_default()
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
    use runtara_dsl::spec::dsl_schema;

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

    println!("cargo:warning=   ✓ All specs generated at:");
    println!("cargo:warning=     {}", specs_dir.display());
    println!("cargo:warning=");
}
