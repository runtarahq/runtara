#!/usr/bin/env bash
# Copyright (C) 2025 SyncMyOrders Sp. z o.o.
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build every runtara-agent-* crate as a WebAssembly Component and emit the
# sibling meta.json (derived from each agent's macro-emitted statics) next to
# the .wasm. Each agent ships as a pair:
#   target/wasm32-wasip1/release/runtara_agent_<id>.wasm
#   target/wasm32-wasip1/release/runtara_agent_<id>.meta.json
# (cargo-component drops binaries under wasip1 even though the target is wasip2.)
#
# Set RUNTARA_AGENT_COMPONENTS_DIR=<workspace>/target/wasm32-wasip1/release in
# the server env to load them at boot. See docs/wasm-components-migration-plan.md.

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

if [ -z "$agents" ]; then
    echo "No runtara-agent-* component crates found in Cargo.toml workspace members." >&2
    exit 1
fi

out_dir="$workspace/target/wasm32-wasip1/release"
mkdir -p "$out_dir"

count=0
for agent in $agents; do
    echo "==> $agent"
    cargo component build --release --target wasm32-wasip2 -p "$agent"
    count=$((count + 1))
done

# Single host-native pass: walks every agent crate's `agent_info()` and writes
# the JSON siblings into out_dir. Source-of-truth is each agent's Rust code
# (the macro-emitted statics), not hand-edited JSON.
echo "==> emit-meta"
cargo run --quiet -p runtara-agent-bundle-emit --bin emit-meta -- "$out_dir"

wasm_count=$(find "$out_dir" -maxdepth 1 -name 'runtara_agent_*.wasm' 2>/dev/null | wc -l | tr -d ' ')
meta_count=$(find "$out_dir" -maxdepth 1 -name 'runtara_agent_*.meta.json' 2>/dev/null | wc -l | tr -d ' ')

echo
echo "✓ Built $count component crate(s); $wasm_count .wasm + $meta_count .meta.json staged in $out_dir"
if [ "$wasm_count" -ne "$meta_count" ]; then
    echo "✗ wasm/meta count mismatch — some agent must be added to runtara-agent-bundle-emit's agent list" >&2
    exit 1
fi

echo
echo "Add this to your .env to load them on server boot:"
echo "  RUNTARA_AGENT_COMPONENTS_DIR=$out_dir"
