// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Phase 3 components-mode compile pipeline.
//!
//! When `CompilationInput.compile_mode == CompileMode::Components`, the
//! standard `compile_workflow` delegates here. The pipeline:
//!
//!   1. Calls the components codegen (`codegen::components`) to materialize
//!      the workflow-logic crate (lib.rs + Cargo.toml + wit/world.wit +
//!      workflow.wac) into a per-workflow build dir.
//!   2. Resolves the workspace path placeholders in Cargo.toml.
//!   3. Runs `cargo component build --release --target wasm32-wasip2` to
//!      produce `workflow-logic.wasm`.
//!   4. Stages the required agent components in
//!      `$DATA_DIR/agent-cas/` from `RUNTARA_AGENT_COMPONENTS_DIR` (or the
//!      default bundle dir).
//!   5. Runs `wac compose` to link workflow-logic with the required agents
//!      into a single composed `workflow.wasm`.
//!   6. Computes the SHA-256 of the composed artifact and returns the
//!      `NativeCompilationResult` the existing runner already understands.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use runtara_dsl::ExecutionGraph;
use sha2::{Digest, Sha256};

use crate::codegen::ast::context::EmitContext;
use crate::codegen::components::{self, AgentRequirement, CodegenArtifacts};
use crate::compile::{
    ChildDependency, ChildWorkflowInput, CompilationInput, NativeCompilationResult,
    ProgressCallback,
};

/// Fire a progress event if a callback is attached. Cheap no-op otherwise.
fn report(progress: &Option<ProgressCallback>, stage: &str, message: &str) {
    if let Some(cb) = progress {
        cb(stage, message);
    }
}

/// Compile a workflow under `CompileMode::Components`. Mirrors
/// `compile_workflow`'s public contract — same input, same `NativeCompilationResult`.
pub fn compile_workflow_components(input: CompilationInput) -> io::Result<NativeCompilationResult> {
    let CompilationInput {
        tenant_id,
        workflow_id,
        version,
        execution_graph,
        track_events,
        child_workflows,
        connection_service_url,
        agent_catalog,
        progress_callback,
    } = input;

    // Fall back to the statically-linked agent registry if the caller didn't
    // supply a runtime catalog. The server normally hands in
    // `ComponentDispatcherService::catalog()`; CLI / test paths leave it
    // unset and get the embedded set.
    let catalog = agent_catalog.unwrap_or_else(|| {
        std::sync::Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
            runtara_agents::registry::get_agents(),
        ))
    });

    report(&progress_callback, "generating", "Generating workflow code");

    // 1. Codegen — produce the four artifacts.
    let artifacts = run_codegen(
        &execution_graph,
        track_events,
        &child_workflows,
        connection_service_url.as_deref(),
        &tenant_id,
        catalog,
    )?;

    // 2. Materialize the workflow-logic crate.
    let build_dir = build_dir_for(&tenant_id, &workflow_id, version);
    fs::create_dir_all(build_dir.join("src"))?;
    fs::create_dir_all(build_dir.join("wit"))?;
    let cargo_toml =
        resolve_cargo_toml_placeholders(&artifacts.cargo_toml, &artifacts.agents_required)?;
    fs::write(build_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(build_dir.join("src/lib.rs"), &artifacts.lib_rs)?;
    fs::write(build_dir.join("wit/world.wit"), &artifacts.world_wit)?;
    fs::write(build_dir.join("workflow.wac"), &artifacts.wac_source)?;

    // cargo-component's wit-parser resolves `runtara:agent` (and the wasi:*
    // package set the world `include`s) by reading `wit/deps/`. Mirror the
    // whole tree from the agent-wit crate so the worker's wit/ is
    // self-contained — much simpler than relying on cargo-component's path
    // dependencies to side-effect the deps directory.
    stage_wit_deps(&build_dir.join("wit/deps"))?;

    // Each used agent's per-agent WIT package (`runtara:agent-<id>@0.3.0`)
    // also has to be in the workflow's wit/deps/ so cargo-component can
    // resolve the anonymous import `runtara:agent-<id>/capabilities@0.3.0`
    // emitted by the components codegen.
    stage_per_agent_wits(&build_dir.join("wit/deps"), &artifacts.agents_required)?;

    // 3. Stage the agent-cas (copies missing .wasm files from the bundle dir).
    let cas_dir = stage_agent_cas(&artifacts.agents_required)?;

    report(
        &progress_callback,
        "building",
        "Compiling workflow components",
    );

    // 4. Build the workflow-logic component.
    let workflow_logic_wasm = run_cargo_component_build(&build_dir, &progress_callback)?;

    report(
        &progress_callback,
        "composing",
        "Linking workflow components",
    );

    // 5. Compose with the agent CAS.
    let composed_wasm = run_wac_compose(
        &build_dir,
        &cas_dir,
        &workflow_logic_wasm,
        &artifacts.agents_required,
    )?;

    // 6. Pack the result the existing runner expects.
    let bytes = fs::read(&composed_wasm)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let checksum = format!("{:x}", hasher.finalize());

    let child_dependencies: Vec<ChildDependency> = child_workflows
        .iter()
        .map(|c| ChildDependency {
            step_id: c.step_id.clone(),
            child_workflow_id: c.workflow_id.clone(),
            child_version_requested: c.version_requested.clone(),
            child_version_resolved: c.version_resolved,
        })
        .collect();

    let graph_json = serde_json::to_value(&execution_graph).unwrap_or(serde_json::Value::Null);
    let has_side_effects = crate::compile::workflow_has_side_effects(&graph_json);

    // Crate source size = the per-workflow files we materialized in step 2.
    // Sums Cargo.toml, src/lib.rs, wit/world.wit, workflow.wac. Excludes
    // `wit/deps/` (staged copies of shared WIT, identical across workflows)
    // and `target/` (build artifacts).
    let package_size = package_source_size(&build_dir);

    Ok(NativeCompilationResult {
        binary_size: bytes.len(),
        binary_path: composed_wasm,
        binary_checksum: checksum,
        build_dir,
        package_size,
        has_side_effects,
        child_dependencies,
        default_variables: serde_json::to_value(&execution_graph.variables)
            .unwrap_or(serde_json::Value::Null),
    })
}

/// Sum the byte size of the workflow-specific source files. Failures on
/// individual files are silent (count as 0) — this is a UX metric, not a
/// load-bearing number, so a missing file shouldn't fail the compile.
fn package_source_size(build_dir: &Path) -> usize {
    const PACKAGE_FILES: &[&str] = &["Cargo.toml", "src/lib.rs", "wit/world.wit", "workflow.wac"];
    PACKAGE_FILES
        .iter()
        .map(|rel| {
            fs::metadata(build_dir.join(rel))
                .map(|m| m.len() as usize)
                .unwrap_or(0)
        })
        .sum()
}

// ---------------------------------------------------------------------------
// Codegen wrapper
// ---------------------------------------------------------------------------

fn run_codegen(
    graph: &ExecutionGraph,
    track_events: bool,
    child_workflows: &[ChildWorkflowInput],
    connection_service_url: Option<&str>,
    tenant_id: &str,
    catalog: std::sync::Arc<runtara_dsl::agent_meta::AgentCatalog>,
) -> io::Result<CodegenArtifacts> {
    let child_graphs: HashMap<String, ExecutionGraph> = child_workflows
        .iter()
        .map(|c| {
            let key = format!("{}::{}", c.workflow_id, c.version_resolved);
            (key, c.execution_graph.clone())
        })
        .collect();
    let step_to_child_ref: HashMap<String, (String, i32)> = child_workflows
        .iter()
        .map(|c| {
            (
                c.step_id.clone(),
                (c.workflow_id.clone(), c.version_resolved),
            )
        })
        .collect();

    let mut ctx = EmitContext::with_child_workflows(
        track_events,
        child_graphs,
        step_to_child_ref,
        connection_service_url.map(str::to_string),
        Some(tenant_id.to_string()),
    );
    ctx.set_catalog(catalog);
    ctx.rate_limit_budget_ms = graph.rate_limit_budget_ms;
    ctx.durable = graph.durable.unwrap_or(true);

    components::emit_components_artifacts(graph, &mut ctx).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Components-mode codegen failed: {e}"),
        )
    })
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

fn data_dir() -> PathBuf {
    // Make the path absolute. cargo-component runs with `current_dir =
    // build_dir`, so a relative `CARGO_TARGET_DIR` like `.data/.../target`
    // would re-resolve against the new cwd and nest one level deeper. By
    // anchoring DATA_DIR (and every path derived from it) to the *original*
    // cwd at first use, the build dir + target dir stay where the caller
    // expects.
    let raw = PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string()));
    if raw.is_absolute() {
        raw
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&raw))
            .unwrap_or(raw)
    }
}

fn build_dir_for(tenant_id: &str, workflow_id: &str, version: u32) -> PathBuf {
    data_dir()
        .join("workflow-builds-components")
        .join(tenant_id)
        .join(workflow_id)
        .join(version.to_string())
}

fn agent_cas_dir() -> PathBuf {
    data_dir().join("agent-cas")
}

/// Workspace root used to resolve absolute paths to the stdlib / sdk /
/// agent-wit sources cargo-component needs when building workflow-logic.
///
/// Three layers, in order:
///   1. `$RUNTARA_COMPILE_SOURCE_DIR` — explicit override. Honored above
///      everything else so custom deployments can point at a non-default
///      layout. `scripts/install.sh` sets this in the systemd
///      EnvironmentFile, but other deployments (Docker without the
///      install-test Dockerfile, ECS, manual launches) frequently don't —
///      hence the auto-detect below.
///   2. `<install>/compile-src/` next to the binary — the released bundle
///      layout is `<install>/bin/runtara-server` + `<install>/compile-src/`,
///      so the binary can find its own bundle without anyone setting an
///      env var. Resolved from `current_exe()`, which is the canonical
///      path on Linux/macOS (symlinks like `/usr/local/bin/runtara-server`
///      still resolve back to the install root).
///   3. `env!("CARGO_MANIFEST_DIR")` — compile-time fallback for `cargo run`
///      against an in-tree checkout. The CI-baked path
///      (`/home/runner/work/runtara/runtara/...`) shows up here on a
///      released binary, which is why steps 1+2 exist.
fn workspace_root() -> PathBuf {
    if let Ok(dir) = std::env::var("RUNTARA_COMPILE_SOURCE_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(install_root) = exe.parent().and_then(Path::parent)
    {
        let candidate = install_root.join("compile-src");
        if candidate.is_dir() {
            return candidate;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Copy `crates/runtara-agent-wit/wit/` into the workflow's `wit/deps/` so
/// cargo-component's wit-parser can resolve `runtara:agent` and the wasi:*
/// packages it transitively pulls in. Idempotent — overwrites any existing
/// dep tree from a previous compile.
fn stage_wit_deps(deps_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(deps_dir)?;
    let src_root = workspace_root().join("crates/runtara-agent-wit/wit");

    // Hard-fail with a setup-actionable message if the compile-source tree
    // isn't where we expect it. Without this, a misconfigured deployment
    // surfaces as `Compilation failed: No such file or directory (os error 2)`
    // ~50 ms in, with no clue about which file is missing.
    if !src_root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "agent-wit source tree missing at {} — set RUNTARA_COMPILE_SOURCE_DIR \
                 to point at the bundle's `compile-src/` directory (e.g. \
                 /opt/runtara-<ver>/compile-src), or run from an in-tree checkout",
                src_root.display()
            ),
        ));
    }

    // Copy the runtara-agent.wit file itself into deps/runtara-agent/.
    let runtara_dst = deps_dir.join("runtara-agent");
    fs::create_dir_all(&runtara_dst)?;
    fs::copy(
        src_root.join("runtara-agent.wit"),
        runtara_dst.join("runtara-agent.wit"),
    )?;

    // Mirror the wasi:* deps that the agent world includes via wit-deps.
    let src_deps = src_root.join("deps");
    if src_deps.is_dir() {
        copy_dir_recursive(&src_deps, deps_dir)?;
    }
    Ok(())
}

/// For each agent the workflow imports, copy its `wit/agent.wit` (declaring
/// the unique per-agent package `runtara:agent-<id>@0.3.0`) into
/// `wit/deps/runtara-agent-<id>/agent.wit`. cargo-component's wit-parser
/// uses the directory name as a dep id and reads the contained .wit files
/// to populate its package map.
fn stage_per_agent_wits(deps_dir: &Path, required: &[AgentRequirement]) -> io::Result<()> {
    let ws = workspace_root();
    for req in required {
        let src = ws.join(format!(
            "crates/agents/runtara-agent-{}/wit/agent.wit",
            req.agent_id
        ));
        if !src.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "per-agent WIT missing for `{}` at {} — run \
                     `cargo build -p runtara-agent-{}` to trigger the build.rs that \
                     generates it, or check that the crate is in the workspace",
                    req.agent_id,
                    src.display(),
                    req.agent_id
                ),
            ));
        }
        let dst_dir = deps_dir.join(format!("runtara-agent-{}", req.agent_id));
        fs::create_dir_all(&dst_dir)?;
        fs::copy(&src, dst_dir.join("agent.wit"))?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

fn resolve_cargo_toml_placeholders(
    toml: &str,
    required: &[AgentRequirement],
) -> io::Result<String> {
    let ws = workspace_root();
    let stdlib_path = ws.join("crates/runtara-workflow-stdlib");
    let sdk_path = ws.join("crates/runtara-sdk");
    let agent_wit_path = ws.join("crates/runtara-agent-wit/wit");
    let deps_root = agent_wit_path.join("deps");
    let mut out = toml
        .replace("{{STDLIB_PATH}}", &stdlib_path.display().to_string())
        .replace("{{SDK_PATH}}", &sdk_path.display().to_string())
        .replace("{{AGENT_WIT_PATH}}", &agent_wit_path.display().to_string())
        .replace(
            "{{WASI_CLI_PATH}}",
            &deps_root.join("cli").display().to_string(),
        )
        .replace(
            "{{WASI_IO_PATH}}",
            &deps_root.join("io").display().to_string(),
        )
        .replace(
            "{{WASI_CLOCKS_PATH}}",
            &deps_root.join("clocks").display().to_string(),
        )
        .replace(
            "{{WASI_RANDOM_PATH}}",
            &deps_root.join("random").display().to_string(),
        )
        .replace(
            "{{WASI_FILESYSTEM_PATH}}",
            &deps_root.join("filesystem").display().to_string(),
        )
        .replace(
            "{{WASI_SOCKETS_PATH}}",
            &deps_root.join("sockets").display().to_string(),
        );
    for req in required {
        let agent_wit = ws.join(format!("crates/agents/runtara-agent-{}/wit", req.agent_id));
        let token = format!("{{{{AGENT_PER_WIT_PATH:{}}}}}", req.agent_id);
        out = out.replace(&token, &agent_wit.display().to_string());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// agent-cas staging
// ---------------------------------------------------------------------------

fn agent_bundle_dir() -> PathBuf {
    if let Ok(env_dir) = std::env::var("RUNTARA_AGENT_COMPONENTS_DIR") {
        return PathBuf::from(env_dir);
    }
    workspace_root().join("target/wasm32-wasip1/release")
}

/// Copy each required agent's `.wasm` from the bundle dir into the persistent
/// CAS under `$DATA_DIR/agent-cas/`. wac's `-d` flag finds packages by name
/// using a `<namespace>/<name>-<version>.wasm` layout, but it also accepts a
/// flat directory of files named `<namespace>:<name>.wasm` — we use the
/// latter and let wac match against the world's import names.
fn stage_agent_cas(required: &[AgentRequirement]) -> io::Result<PathBuf> {
    let cas = agent_cas_dir();
    fs::create_dir_all(&cas)?;
    let bundle = agent_bundle_dir();

    for req in required {
        // Bundle filename: runtara_agent_<snake>.wasm
        let snake = req.agent_id.replace('-', "_");
        let src = bundle.join(format!("runtara_agent_{snake}.wasm"));
        if !src.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "agent component `{}` missing — expected at {} (set RUNTARA_AGENT_COMPONENTS_DIR or run scripts/build-agent-components.sh)",
                    req.agent_id,
                    src.display()
                ),
            ));
        }
        // CAS filename: <namespace>-<name>.wasm so wac's `-d` lookup works.
        // package = "runtara:agent-<id>" → file = "runtara-agent-<id>.wasm"
        let dst = cas.join(format!("{}.wasm", req.package.replace(':', "-")));
        // Always overwrite — keeps the CAS in sync if a developer rebuilt an
        // agent without bumping the agent crate version.
        fs::copy(&src, &dst)?;
    }
    Ok(cas)
}

// ---------------------------------------------------------------------------
// `cargo component build` + `wac compose`
// ---------------------------------------------------------------------------

fn run_cargo_component_build(
    build_dir: &Path,
    progress: &Option<ProgressCallback>,
) -> io::Result<PathBuf> {
    // When a progress callback is wired up, stream cargo's JSON message
    // format so we can surface per-crate "Compiling foo" events to the user.
    // No callback? Inherit stdio and let the user see the usual cargo
    // output, matching prior behavior for CLI / test callers.
    let want_progress = progress.is_some();

    let mut cmd = Command::new("cargo");
    cmd.current_dir(build_dir)
        .arg("component")
        .arg("build")
        .arg("--release")
        .arg("--target")
        .arg("wasm32-wasip2")
        // Per-tenant shared target dir if RUNTARA_COMPONENTS_TARGET_DIR is set
        // (used by test fixtures to amortize the ~30s cold build across many
        // workflows in one process). Otherwise default to per-workflow.
        .env(
            "CARGO_TARGET_DIR",
            std::env::var_os("RUNTARA_COMPONENTS_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| build_dir.join("target")),
        );

    let status = if want_progress {
        // Resolve the total package count up front so the progress message
        // can be a fraction. `cargo metadata` over the in-tree manifest is
        // cheap (~100 ms warm) and gives us every transitive dep — close
        // enough for a ticking denominator. If it fails we fall back to a
        // bare running count rather than blocking the build.
        let total_packages = count_workflow_packages(build_dir);

        cmd.arg("--message-format=json-render-diagnostics")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn().map_err(|e| {
            io::Error::other(format!(
                "cargo component build failed to launch (is cargo-component installed?): {e}"
            ))
        })?;

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let mut compiled: usize = 0;
            for line in reader.lines() {
                let Ok(line) = line else { continue };
                // Only `compiler-artifact` events carry a crate name; other
                // messages (warnings, build-finished, …) we let pass. We
                // don't surface the crate name itself — too noisy — just a
                // ticking count of dependencies that have finished building.
                if parse_cargo_artifact_name(&line).is_some() {
                    compiled += 1;
                    let msg = match total_packages {
                        Some(total) if total > 0 => {
                            format!("Building dependencies ({}/{})", compiled.min(total), total)
                        }
                        _ => format!("Building dependencies ({})", compiled),
                    };
                    report(progress, "building", &msg);
                }
            }
        }

        child
            .wait()
            .map_err(|e| io::Error::other(format!("cargo component build wait failed: {e}")))?
    } else {
        cmd.status().map_err(|e| {
            io::Error::other(format!(
                "cargo component build failed to launch (is cargo-component installed?): {e}"
            ))
        })?
    };
    if !status.success() {
        return Err(io::Error::other(format!(
            "cargo component build returned non-zero status {} (build dir: {})",
            status,
            build_dir.display()
        )));
    }
    let target_root = std::env::var_os("RUNTARA_COMPONENTS_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("target"));
    let wasm = target_root
        .join("wasm32-wasip1/release/workflow_logic.wasm")
        .canonicalize()
        .or_else(|_| {
            target_root
                .join("wasm32-wasip2/release/workflow_logic.wasm")
                .canonicalize()
        })
        .map_err(|e| {
            io::Error::other(format!(
                "cargo component build succeeded but workflow_logic.wasm not found under {}: {e}",
                build_dir.join("target").display()
            ))
        })?;
    Ok(wasm)
}

fn run_wac_compose(
    build_dir: &Path,
    cas_dir: &Path,
    workflow_logic_wasm: &Path,
    required: &[AgentRequirement],
) -> io::Result<PathBuf> {
    let out = build_dir.join("workflow.wasm");
    let mut cmd = Command::new("wac");
    cmd.arg("compose")
        .arg(build_dir.join("workflow.wac"))
        .arg("-d")
        .arg(format!(
            "runtara:workflow-logic={}",
            workflow_logic_wasm.display()
        ));
    // Map each required agent package to its .wasm in the CAS so wac can
    // resolve `new runtara:agent-<id> { ... }` instantiations.
    for req in required {
        let agent_wasm = cas_dir.join(format!("{}.wasm", req.package.replace(':', "-")));
        cmd.arg("-d").arg(format!(
            "{pkg}={path}",
            pkg = req.package,
            path = agent_wasm.display()
        ));
    }
    cmd.arg("-o").arg(&out);
    let status = cmd.status().map_err(|e| {
        io::Error::other(format!(
            "wac compose failed to launch (is wac-cli installed? `cargo install wac-cli --locked`): {e}"
        ))
    })?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "wac compose returned non-zero status {} (wac script: {}, cas: {})",
            status,
            build_dir.join("workflow.wac").display(),
            cas_dir.display()
        )));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Cargo JSON message parsing
// ---------------------------------------------------------------------------

/// Pull the crate name out of a `cargo --message-format=json` line if it's a
/// `compiler-artifact` event. Anything else (diagnostics, build-script-executed,
/// build-finished, …) returns `None`. Done manually to avoid pulling in
/// `cargo_metadata` for one field.
fn parse_cargo_artifact_name(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("reason")?.as_str()? != "compiler-artifact" {
        return None;
    }
    let name = v.get("target")?.get("name")?.as_str()?;
    Some(name.to_string())
}

/// Count packages (incl. transitive deps) cargo will resolve for the
/// generated workflow-logic crate. Used purely as a denominator for the
/// "Building dependencies (3/N)" progress message — best-effort, returns
/// `None` and we degrade to a bare count if anything goes wrong. Subtracts
/// 1 to exclude `workflow-logic` itself.
fn count_workflow_packages(build_dir: &Path) -> Option<usize> {
    // `--no-deps` is a flag (no `=false` form); omitting it means cargo
    // emits every transitive package, which is what we want for the
    // denominator. Run with the same CARGO_TARGET_DIR as the build so
    // metadata uses the populated lockfile rather than regenerating one.
    let target_dir = std::env::var_os("RUNTARA_COMPONENTS_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("target"));
    let output = Command::new("cargo")
        .current_dir(build_dir)
        .arg("metadata")
        .arg("--format-version=1")
        .env("CARGO_TARGET_DIR", &target_dir)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let total = json.get("packages")?.as_array()?.len();
    Some(total.saturating_sub(1))
}

#[cfg(test)]
mod cargo_parse_tests {
    use super::parse_cargo_artifact_name;

    #[test]
    fn extracts_crate_name_from_compiler_artifact() {
        let line = r#"{"reason":"compiler-artifact","package_id":"foo 0.1.0","manifest_path":"/x/Cargo.toml","target":{"kind":["lib"],"crate_types":["lib"],"name":"foo","src_path":"/x/src/lib.rs","edition":"2024","doc":true,"doctest":true,"test":true},"profile":{"opt_level":"3","debuginfo":0,"debug_assertions":false,"overflow_checks":false,"test":false},"features":[],"filenames":[],"executable":null,"fresh":false}"#;
        assert_eq!(parse_cargo_artifact_name(line), Some("foo".to_string()));
    }

    #[test]
    fn ignores_non_artifact_messages() {
        let line = r#"{"reason":"build-finished","success":true}"#;
        assert!(parse_cargo_artifact_name(line).is_none());
    }

    #[test]
    fn handles_malformed_json() {
        assert!(parse_cargo_artifact_name("not json").is_none());
    }
}
