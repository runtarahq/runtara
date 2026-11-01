#![allow(unused_imports)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod build_metadata;

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
    for git_path in build_metadata::git_rerun_paths(workspace_root) {
        println!("cargo:rerun-if-changed={}", git_path.display());
    }

    // `embed-ui` embeds the runtara-frontend release pinned in frontend.lock.
    // rust_embed needs it on disk at compile time; the script fetches it (and
    // checks its checksum) unless that release is already there.
    if std::env::var("CARGO_FEATURE_EMBED_UI").is_ok() {
        let lock = crate_dir.join("frontend.lock");
        println!("cargo:rerun-if-changed={}", lock.display());
        println!("cargo:rerun-if-env-changed=RUNTARA_FRONTEND_TARBALL");
        let status = Command::new("bash")
            .arg(workspace_root.join("scripts/fetch-frontend.sh"))
            .status()
            .expect("Failed to run scripts/fetch-frontend.sh");
        if !status.success() {
            panic!(
                "\n\n`embed-ui` needs the runtara-frontend release pinned in {}, \
                 and scripts/fetch-frontend.sh could not provide it (see above).\n\n",
                lock.display()
            );
        }
        println!(
            "cargo:rerun-if-changed={}",
            crate_dir.join("frontend-dist").display()
        );
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
