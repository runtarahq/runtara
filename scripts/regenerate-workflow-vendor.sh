#!/usr/bin/env bash
# Regenerate workflow-build-prebuilt/ — the canonical Cargo.lock + vendor/
# pair used for hermetic (air-gapped) workflow compilation.
#
# The runtime workflow build (crates/runtara-workflows/src/components_compile.rs)
# auto-detects this dir under workspace_root() and, when present, copies
# Cargo.lock into each per-workflow build dir, writes a .cargo/config.toml
# that redirects crates-io to the vendor dir, and runs the build with
# `--frozen` + `CARGO_NET_OFFLINE=true` so no network access happens.
#
# Refresh whenever the codegen template's deps or any runtara-* dep set
# changes versions. The release pipeline should run this and ship the
# resulting `workflow-build-prebuilt/` alongside `compile-src/`.
#
# Usage:
#   ./scripts/regenerate-workflow-vendor.sh
#
# Outputs:
#   <workspace_root>/workflow-build-prebuilt/Cargo.lock
#   <workspace_root>/workflow-build-prebuilt/vendor/

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$WORKSPACE_ROOT/workflow-build-prebuilt"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

echo "==> Materializing canonical workflow Cargo.toml in $SCRATCH"
mkdir -p "$SCRATCH/src" "$SCRATCH/wit/deps"

# Mirror the wit deps the codegen template references. cargo-component reads
# wit/ at metadata time; without it, `cargo generate-lockfile` fails before
# vendoring can start.
cp -r "$WORKSPACE_ROOT/crates/runtara-agent-wit/wit/." "$SCRATCH/wit/deps/"

cat > "$SCRATCH/wit/world.wit" <<'EOF'
package runtara:workflow@0.0.1;
world workflow {
    include wasi:cli/command@0.2.3;
}
EOF

cat > "$SCRATCH/src/lib.rs" <<'EOF'
// Stub workflow used only to drive `cargo vendor`. Compiled output discarded.
EOF

# Cargo.toml mirrors the template in
# crates/runtara-workflows/src/codegen/components.rs. Path-deps point at the
# real source tree so `cargo generate-lockfile` resolves runtara-* the same
# way the runtime codegen does — keeps the vendored set in sync.
cat > "$SCRATCH/Cargo.toml" <<EOF
[package]
name = "workflow-logic"
version = "0.0.1"
edition = "2024"
publish = false

[workspace]

[lib]
crate-type = ["cdylib"]

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
wit-bindgen-rt = { version = "0.44", features = ["bitflags"] }
runtara-workflow-stdlib = { path = "$WORKSPACE_ROOT/crates/runtara-workflow-stdlib", default-features = false, features = ["wasi"] }
runtara-sdk = { path = "$WORKSPACE_ROOT/crates/runtara-sdk", default-features = false, features = ["http", "wasi"] }

[package.metadata.component]
package = "runtara:workflow-logic"

[package.metadata.component.target]
path = "wit"
world = "workflow"
EOF

echo "==> Generating Cargo.lock"
( cd "$SCRATCH" && cargo generate-lockfile >/dev/null )

echo "==> Wiping previous $OUT_DIR"
rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

echo "==> Vendoring crates.io deps into $OUT_DIR/vendor"
( cd "$SCRATCH" && cargo vendor "$OUT_DIR/vendor" >/dev/null )

cp "$SCRATCH/Cargo.lock" "$OUT_DIR/Cargo.lock"

size="$(du -sh "$OUT_DIR" | cut -f1)"
count="$(find "$OUT_DIR/vendor" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"

echo ""
echo "==> Done."
echo "    $OUT_DIR ($size, $count vendored crates)"
echo ""
echo "Next time a workflow compiles, components_compile.rs will detect this"
echo "dir and switch to hermetic --frozen + CARGO_NET_OFFLINE=true builds."
