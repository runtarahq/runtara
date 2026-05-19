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

if ! command -v cargo-component >/dev/null 2>&1; then
    echo "error: cargo-component is required" >&2
    echo "       install with: cargo install cargo-component" >&2
    exit 1
fi

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace"

# Discover every workspace member named runtara-agent-* and skip the WIT
# package, the proc-macro crate, the shared utilities crate, and the
# build-time emit-meta binary — those aren't components.
agents=$(grep -E '^\s*"crates/runtara-agent-' Cargo.toml \
    | sed -E 's@.*"crates/(runtara-agent-[^"]+)".*@\1@' \
    | grep -v '^runtara-agent-wit$' \
    | grep -v '^runtara-agent-macro$' \
    | grep -v '^runtara-agent-common$' \
    | grep -v '^runtara-agent-bundle-emit$' \
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
