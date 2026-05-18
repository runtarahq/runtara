#!/usr/bin/env bash
# Copyright (C) 2025 SyncMyOrders Sp. z o.o.
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build every runtara-agent-* crate as a WebAssembly Component.
# Outputs land at target/wasm32-wasip1/release/runtara_agent_<name>.wasm
# (cargo-component drops them under wasip1 even though the target is wasip2).
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
# package (runtara-agent-wit), the proc-macro crate (runtara-agent-macro), and
# the shared utilities crate (runtara-agent-common) — those aren't components.
agents=$(grep -E '^\s*"crates/runtara-agent-' Cargo.toml \
    | sed -E 's@.*"crates/(runtara-agent-[^"]+)".*@\1@' \
    | grep -v '^runtara-agent-wit$' \
    | grep -v '^runtara-agent-macro$' \
    | grep -v '^runtara-agent-common$' \
    || true)

if [ -z "$agents" ]; then
    echo "No runtara-agent-* component crates found in Cargo.toml workspace members." >&2
    exit 1
fi

count=0
for agent in $agents; do
    echo "==> $agent"
    cargo component build --release --target wasm32-wasip2 -p "$agent"
    count=$((count + 1))
done

out_dir="$workspace/target/wasm32-wasip1/release"
wasm_count=$(find "$out_dir" -maxdepth 1 -name 'runtara_agent_*.wasm' 2>/dev/null | wc -l | tr -d ' ')

echo
echo "✓ Built $count component crate(s); $wasm_count .wasm in $out_dir"
echo
echo "Add this to your .env to load them on server boot:"
echo "  RUNTARA_AGENT_COMPONENTS_DIR=$out_dir"
