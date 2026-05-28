#!/usr/bin/env bash
# Copyright (C) 2025 SyncMyOrders Sp. z o.o.
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build every runtara-agent-* crate as a WebAssembly Component, then build the
# shared workflow stdlib/runtime components used by the direct workflow emitter.
# Each agent ships as a pair:
#   target/wasm32-wasip2/release/runtara_agent_<id>.wasm
#   target/wasm32-wasip2/release/runtara_agent_<id>.meta.json
# The direct shared workflow components ship as:
#   target/wasm32-wasip2/release/runtara_workflow_stdlib.wasm
#   target/wasm32-wasip2/release/runtara_workflow_stdlib.meta.json
#   target/wasm32-wasip2/release/runtara_workflow_runtime.wasm
#   target/wasm32-wasip2/release/runtara_workflow_runtime.meta.json
#
# cargo-component's `--target wasm32-wasip2` writes the finalized component
# under `wasm32-wasip2/`. It also leaves an intermediate file under
# `wasm32-wasip1/` (rustc's core wasm output, before component encoding); on
# darwin that intermediate is coincidentally usable, but on linux it's a
# malformed Frankenstein that traps at runtime inside the preview1 adapter's
# `cabi_import_realloc`. Always read from `wasm32-wasip2/`.
#
# Set RUNTARA_AGENT_COMPONENTS_DIR=<workspace>/target/wasm32-wasip2/release in
# the server env to load agents at boot and to let direct composition find the
# shared workflow components. See docs/wasm-components-migration-plan.md.

set -euo pipefail

# Auto-install the host-side tools we need. Set RUNTARA_NO_INSTALL_TOOLS=1 to
# fail fast instead of installing — useful in CI where you want the install
# steps to be explicit.
ensure_tool() {
    local cmd="$1"
    local crate="$2"
    local version="${3:-}"
    if command -v "$cmd" >/dev/null 2>&1; then
        return 0
    fi
    if [ "${RUNTARA_NO_INSTALL_TOOLS:-}" = "1" ]; then
        echo "error: \`$cmd\` is required but not installed (RUNTARA_NO_INSTALL_TOOLS=1)" >&2
        if [ -n "$version" ]; then
            echo "       install with: cargo install $crate --version $version --locked" >&2
        else
            echo "       install with: cargo install $crate --locked" >&2
        fi
        exit 1
    fi
    echo "==> installing $cmd via cargo install $crate${version:+ --version $version}"
    if [ -n "$version" ]; then
        cargo install "$crate" --version "$version" --locked
    else
        cargo install "$crate" --locked
    fi
}

# cargo-component compiles each agent crate's cdylib into a Component-Model
# .wasm. Pinned to match the version this codebase was built against.
ensure_tool cargo-component cargo-component 0.21.1
# wit-deps resolves the wasi:* dependencies pinned by
# crates/runtara-agent-wit/wit/deps.toml. Only needed for first-time
# checkouts or after a deps.toml bump; the lockfile is committed.
ensure_tool wit-deps wit-deps-cli
# wac-cli is reserved for the Phase 3 composition step (single bundled
# .wasm). Not strictly needed by this script today, so we warn rather
# than fail when it's missing.
if ! command -v wac >/dev/null 2>&1; then
    echo "note: \`wac\` (Phase 3 composition tool) not installed — skipping."
    echo "      install with: cargo install wac-cli --locked"
fi

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace"

# Discover every workspace member under crates/agents/. Component agents
# live there as a self-contained subsystem; runtara-agent-wit, -macro,
# -bundle-emit stay at crates/ root because they're infrastructure shared
# with the host crates, not components themselves.
agents=$(grep -E '^\s*"crates/agents/runtara-agent-' Cargo.toml \
    | sed -E 's@.*"crates/agents/(runtara-agent-[^"]+)".*@\1@' \
    || true)

if [ -z "$agents" ] && [ "${RUNTARA_ONLY_WORKFLOW_COMPONENTS:-}" != "1" ]; then
    echo "No runtara-agent-* component crates found in Cargo.toml workspace members." >&2
    exit 1
fi

# Honor $CARGO_TARGET_DIR so docker-mounted / out-of-tree builds land where
# cargo actually wrote the .wasm files. Without this the meta.json siblings
# end up under $workspace/target/... while cargo dropped the binaries under
# $CARGO_TARGET_DIR/..., and the downstream bundle assembly errors out with
# "expected runtara_agent_*.{wasm,meta.json}, found N wasm and 0 meta files".
target_dir="${CARGO_TARGET_DIR:-$workspace/target}"
out_dir="$target_dir/wasm32-wasip2/release"
mkdir -p "$out_dir"

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

workspace_version="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"

emit_workflow_component_meta() {
    local crate="$1"
    local package="$2"
    local world="$3"
    local interface="$4"
    local wasm="$5"
    local meta="$6"

    local checksum
    checksum="$(sha256_file "$wasm")"
    local size
    size="$(wc -c < "$wasm" | tr -d ' ')"

    cat > "$meta" <<METAEOF
{
  "schemaVersion": 1,
  "kind": "workflow-component",
  "package": "${package}",
  "witVersion": "0.1.0",
  "crate": "${crate}",
  "crateVersion": "${workspace_version}",
  "world": "${world}",
  "exports": [
    "${interface}"
  ],
  "wasm": "$(basename "$wasm")",
  "sha256": "${checksum}",
  "sizeBytes": ${size}
}
METAEOF
}

count=0
wasm_count=0
meta_count=0
if [ "${RUNTARA_ONLY_WORKFLOW_COMPONENTS:-}" != "1" ]; then
    for agent in $agents; do
        echo "==> $agent"
        cargo component build --release --target wasm32-wasip2 -p "$agent"
        count=$((count + 1))
    done
fi

echo "==> runtara-workflow-stdlib"
cargo component build --release --target wasm32-wasip2 -p runtara-workflow-stdlib --no-default-features --features direct-component
emit_workflow_component_meta \
    "runtara-workflow-stdlib" \
    "runtara:workflow-stdlib" \
    "workflow-stdlib" \
    "runtara:workflow-stdlib/json@0.1.0" \
    "$out_dir/runtara_workflow_stdlib.wasm" \
    "$out_dir/runtara_workflow_stdlib.meta.json"

echo "==> runtara-workflow-runtime"
cargo component build --release --target wasm32-wasip2 -p runtara-workflow-runtime --no-default-features --features wasi
emit_workflow_component_meta \
    "runtara-workflow-runtime" \
    "runtara:workflow-runtime" \
    "workflow-runtime" \
    "runtara:workflow-runtime/runtime@0.1.0" \
    "$out_dir/runtara_workflow_runtime.wasm" \
    "$out_dir/runtara_workflow_runtime.meta.json"

if [ "${RUNTARA_ONLY_WORKFLOW_COMPONENTS:-}" != "1" ]; then
    # Single host-native pass: walks every agent crate's `agent_info()` and writes
    # the JSON siblings into out_dir. Source-of-truth is each agent's Rust code
    # (the macro-emitted statics), not hand-edited JSON.
    echo "==> emit-meta"
    cargo run --quiet -p runtara-agent-bundle-emit --bin emit-meta -- "$out_dir"

    wasm_count=$(find "$out_dir" -maxdepth 1 -name 'runtara_agent_*.wasm' 2>/dev/null | wc -l | tr -d ' ')
    meta_count=$(find "$out_dir" -maxdepth 1 -name 'runtara_agent_*.meta.json' 2>/dev/null | wc -l | tr -d ' ')
fi
workflow_wasm_count=$(find "$out_dir" -maxdepth 1 -name 'runtara_workflow_*.wasm' 2>/dev/null | wc -l | tr -d ' ')
workflow_meta_count=$(find "$out_dir" -maxdepth 1 -name 'runtara_workflow_*.meta.json' 2>/dev/null | wc -l | tr -d ' ')

echo
if [ "${RUNTARA_ONLY_WORKFLOW_COMPONENTS:-}" != "1" ]; then
    echo "✓ Built $count agent component crate(s); $wasm_count .wasm + $meta_count .meta.json staged in $out_dir"
fi
echo "✓ Built shared workflow components; $workflow_wasm_count .wasm + $workflow_meta_count .meta.json staged in $out_dir"
if [ "${RUNTARA_ONLY_WORKFLOW_COMPONENTS:-}" != "1" ] && [ "$wasm_count" -ne "$meta_count" ]; then
    echo "✗ wasm/meta count mismatch — some agent must be added to runtara-agent-bundle-emit's agent list" >&2
    exit 1
fi
if [ "$workflow_wasm_count" -ne 2 ] || [ "$workflow_meta_count" -ne 2 ]; then
    echo "✗ expected 2 shared workflow .wasm files and 2 shared workflow .meta.json files" >&2
    exit 1
fi

echo
echo "Add this to your .env to load agents and direct workflow components on server boot:"
echo "  RUNTARA_AGENT_COMPONENTS_DIR=$out_dir"
