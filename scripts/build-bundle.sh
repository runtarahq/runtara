#!/usr/bin/env bash
# Build a hermetic Runtara bundle for the current host platform.
#
# The bundle contains:
#   - runtara-server binary
#   - Wasmtime CLI binary
#   - 24 pre-built agent components (.wasm + .meta.json each) plus the 2 shared
#     workflow stdlib/runtime components used by direct composition
#   - License files
#   - VERSION and MANIFEST.json
#
# Workflow compilation is fully in-process (the direct WASM emitter byte-emits
# the workflow-logic module and composes the final workflow.wasm via wac-graph),
# so the bundle ships NO Rust toolchain, cargo-component, wac CLI, or source
# mirror — only the prebuilt components the server reads at runtime.
#
# Usage:
#   ./scripts/build-bundle.sh                   # build for the current host
#   ./scripts/build-bundle.sh --skip-build      # assemble from existing target/release
#   ./scripts/build-bundle.sh --output-dir /tmp # write bundle to custom dir
#
# Prerequisites (BUILD-time only — used to build the agent/shared components):
#   - rustup with the version from rust-toolchain.toml installed
#   - wasm32-wasip1 + wasm32-wasip2 targets installed (rust-toolchain.toml handles this)
#   - cargo-component (installed by this script) for building components

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$ROOT_DIR"

# ─── Defaults ────────────────────────────────────────────────────────────────

SKIP_BUILD=0
OUTPUT_DIR="${ROOT_DIR}/target/bundle"
# cargo-component: subcommand used to build the agent + shared workflow
# components at bundle-build time. No prebuilt binaries upstream — installed
# via `cargo install` into a per-version cache dir during bundle build.
# Must match scripts/build-agent-components.sh and .github/workflows/ci.yml.
CARGO_COMPONENT_VERSION="0.21.1"
DOWNLOAD_CACHE="${HOME}/.cache/runtara-bundle-build"

# ─── Parse arguments ─────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)      SKIP_BUILD=1; shift ;;
        --output-dir)      OUTPUT_DIR="$2"; shift 2 ;;
        --output-dir=*)    OUTPUT_DIR="${1#*=}"; shift ;;
        --version)         RUNTARA_VERSION_OVERRIDE="$2"; shift 2 ;;
        --version=*)       RUNTARA_VERSION_OVERRIDE="${1#*=}"; shift ;;
        *)                 echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# ─── Colour helpers ──────────────────────────────────────────────────────────

if [ -t 1 ]; then
    GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; BOLD='\033[1m'; NC='\033[0m'
else
    GREEN='' YELLOW='' BLUE='' BOLD='' NC=''
fi

info()  { printf "${GREEN}[+]${NC} %s\n" "$*"; }
warn()  { printf "${YELLOW}[!]${NC} %s\n" "$*"; }
step()  { printf "\n${BOLD}${BLUE}==> %s${NC}\n" "$*"; }

# ─── Detect platform ────────────────────────────────────────────────────────

detect_platform() {
    OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
    ARCH="$(uname -m)"

    case "$ARCH" in
        x86_64|amd64)  ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *) echo "Unsupported arch: $ARCH" >&2; exit 1 ;;
    esac

    case "$OS" in
        linux)  HOST_TARGET="${ARCH}-unknown-linux-gnu" ;;
        darwin) HOST_TARGET="${ARCH}-apple-darwin" ;;
        *) echo "Unsupported OS: $OS" >&2; exit 1 ;;
    esac

    info "Platform: ${OS} ${ARCH} (${HOST_TARGET})"
}

# ─── Resolve versions ───────────────────────────────────────────────────────

resolve_versions() {
    # Runtara version: override or from workspace Cargo.toml
    RUNTARA_VERSION="${RUNTARA_VERSION_OVERRIDE:-$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')}"
    RUNTARA_COMMIT="${BUILD_COMMIT:-$(git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)}"
    RUNTARA_BUILD_NUMBER="${BUILD_NUMBER:-${GITHUB_RUN_NUMBER:-}}"

    if [ "$RUNTARA_VERSION" = "dev" ]; then
        _CARGO_WS_VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
        RUNTARA_STAMP_VERSION="${_CARGO_WS_VERSION}-dev"
    else
        RUNTARA_STAMP_VERSION="$RUNTARA_VERSION"
    fi

    # Rust version from the active toolchain (pinned by rust-toolchain.toml)
    RUSTC_VERSION="$(rustc --version | cut -d' ' -f2)"

    # Respect CARGO_TARGET_DIR if set
    TARGET_DIR="${CARGO_TARGET_DIR:-target}"

    info "Runtara version: ${RUNTARA_VERSION} (artifact channel)"
    info "Stamped version: ${RUNTARA_STAMP_VERSION} (reported in binary/UI)"
    info "Runtara commit:  ${RUNTARA_COMMIT}"
    info "Rustc version:   ${RUSTC_VERSION}"
    info "cargo-component: ${CARGO_COMPONENT_VERSION}"
    info "Target dir:      ${TARGET_DIR}"
}

# ─── Build frontend ──────────────────────────────────────────────────────────

build_frontend() {
    local frontend_dir="${ROOT_DIR}/crates/runtara-server/frontend"
    local dist_index="${frontend_dir}/dist/index.html"

    if [ "$SKIP_BUILD" = "1" ]; then
        if [ ! -f "$dist_index" ]; then
            echo "Error: --skip-build but ${dist_index} not found" >&2
            echo "Run (cd ${frontend_dir} && npm ci && npm run build) first." >&2
            exit 1
        fi
        info "Skipping frontend build (--skip-build), using existing ${dist_index}"
        return
    fi

    if ! command -v npm >/dev/null 2>&1; then
        echo "Error: npm not found. Install Node.js to build the embedded UI." >&2
        exit 1
    fi

    step "Building frontend (embedded UI)"
    (cd "$frontend_dir" && npm ci --no-audit --no-fund && npm run build)
}

# ─── Build runtara-server ────────────────────────────────────────────────────

build_server() {
    if [ "$SKIP_BUILD" = "1" ]; then
        if [ ! -f "${TARGET_DIR}/release/runtara-server" ]; then
            echo "Error: --skip-build but target/release/runtara-server not found" >&2
            exit 1
        fi
        info "Skipping build (--skip-build), using existing target/release/runtara-server"
        return
    fi

    step "Building runtara-server (release, with embed-ui)"
    BUILD_VERSION="$RUNTARA_STAMP_VERSION" BUILD_COMMIT="$RUNTARA_COMMIT" BUILD_NUMBER="$RUNTARA_BUILD_NUMBER" \
        cargo build --release -p runtara-server --features embed-ui
}

# ─── Build workflow stdlib ───────────────────────────────────────────────────

build_stdlib() {
    if [ "$SKIP_BUILD" = "1" ]; then
        info "Skipping stdlib build (--skip-build)"
        return
    fi

    step "Building workflow stdlib (wasm32-wasip2 rlibs)"
    # Clean stale artifacts to prevent duplicate rlibs (different RUSTFLAGS produce
    # different hashes; cargo doesn't remove the old ones)
    rm -rf "${TARGET_DIR}"/wasm32-wasip2/release/deps/*.rlib "${TARGET_DIR}"/wasm32-wasip2/release/*.rlib 2>/dev/null || true

    # embed-bitcode=yes is required so that workflow compilation can use LTO
    # for cross-crate dead code elimination (see compile.rs)
    RUSTFLAGS="-C embed-bitcode=yes" \
        cargo build -p runtara-workflow-stdlib --release --target wasm32-wasip2 --no-default-features

    step "Building workflow stdlib (host proc-macros)"
    cargo build -p runtara-workflow-stdlib --release
}

# ─── Install cargo-component into a versioned cache ─────────────────────────
#
# Upstream doesn't ship prebuilt binaries — `cargo install` is the only
# option. We isolate by version under DOWNLOAD_CACHE so a re-run with the
# same version is a no-op, and bumping CARGO_COMPONENT_VERSION triggers a
# fresh install without polluting the user's ~/.cargo/bin.

install_cargo_component() {
    step "Installing cargo-component ${CARGO_COMPONENT_VERSION} (host build)"

    local root="${DOWNLOAD_CACHE}/cargo-component-${CARGO_COMPONENT_VERSION}"
    local bin="${root}/bin/cargo-component"

    if [ -x "$bin" ]; then
        info "Using cached cargo-component at ${bin}"
    else
        # --locked is required for reproducibility. Without it, cargo
        # re-resolves cargo-component's transitive deps (wit-parser,
        # wasmparser, wit-component) to whatever's newest in the Cargo.toml
        # range — and those versions affect the *component encoding* the
        # tool emits. We've observed two installs of the same 0.21.1 version
        # produce subtly different workflow.wasm bytes, causing the second
        # to trap at runtime ("cannot leave component instance" inside the
        # wasi:random shim) while the first runs cleanly. Locking pins all
        # transitive deps to cargo-component's own Cargo.lock, eliminating
        # that drift. (An earlier comment claimed --locked failed on yanked
        # wit-parser 0.219.1; that crate is still downloadable, so cargo
        # accepts it under --locked. If a future yank actually blocks the
        # install, fix it by patching the crate version in cargo-component's
        # repo, not by dropping --locked.)
        info "cargo install cargo-component --version ${CARGO_COMPONENT_VERSION} --locked --root ${root}"
        cargo install cargo-component \
            --version "$CARGO_COMPONENT_VERSION" \
            --locked \
            --root "$root"
    fi

    CARGO_COMPONENT_BINARY="$bin"
}

# ─── Build agent components ─────────────────────────────────────────────────
#
# Produces target/wasm32-wasip2/release/runtara_agent_<x>.wasm plus the
# sibling .meta.json for each of the 23 agent crates. The build script
# itself depends on cargo-component + wit-deps being on PATH; we've already
# installed the right cargo-component into the cache, so prepend it.

build_agent_components() {
    if [ "$SKIP_BUILD" = "1" ]; then
        if ! find "${TARGET_DIR}/wasm32-wasip2/release" -maxdepth 1 \
                -name 'runtara_agent_*.wasm' 2>/dev/null | grep -q .; then
            echo "Error: --skip-build but no built agent components found at \
${TARGET_DIR}/wasm32-wasip2/release/runtara_agent_*.wasm" >&2
            exit 1
        fi
        if ! find "${TARGET_DIR}/wasm32-wasip2/release" -maxdepth 1 \
                -name 'runtara_workflow_*.wasm' 2>/dev/null | grep -q .; then
            echo "Error: --skip-build but no built workflow shared components found at \
${TARGET_DIR}/wasm32-wasip2/release/runtara_workflow_*.wasm" >&2
            exit 1
        fi
        info "Skipping agent component build (--skip-build)"
        return
    fi

    step "Building agent and direct workflow WASM components"
    PATH="$(dirname "$CARGO_COMPONENT_BINARY"):${PATH}" \
        "$SCRIPT_DIR/build-agent-components.sh"
}

# ─── Assemble bundle ────────────────────────────────────────────────────────

assemble_bundle() {
    step "Assembling bundle"

    local bundle="${OUTPUT_DIR}/runtara-${RUNTARA_VERSION}-${ARCH}-${OS}"
    rm -rf "$bundle"
    mkdir -p "$bundle"/{bin,agents,licenses}

    # ── runtara-server binary ──
    info "Copying runtara-server binary"
    cp "${TARGET_DIR}/release/runtara-server" "$bundle/bin/"
    strip "$bundle/bin/runtara-server" 2>/dev/null || warn "strip failed (non-critical)"

    # ── Agent components ──
    # Each of the 23 component agents ships as a .wasm + sibling .meta.json
    # pair. At server boot, RUNTARA_AGENT_COMPONENTS_DIR points at this
    # directory; the ComponentDispatcherService loads each pair and exposes
    # the agents to the validator and workflow runtime.
    # The same directory also carries the direct workflow stdlib/runtime
    # components used by static direct composition.
    info "Copying agent and direct workflow WASM components"
    # `wasm32-wasip2/release/` is cargo-component's finalized component output.
    # Same as for workflow-logic: do NOT read from wasm32-wasip1/, which is the
    # intermediate rustc pass cargo-component leaves behind — that file is the
    # malformed Frankenstein wac silently mis-composes on linux.
    local agent_src="${TARGET_DIR}/wasm32-wasip2/release"
    local wasm_count=0
    local meta_count=0
    for f in "$agent_src"/runtara_agent_*.wasm; do
        [ -f "$f" ] || continue
        cp "$f" "$bundle/agents/"
        wasm_count=$((wasm_count + 1))
    done
    for f in "$agent_src"/runtara_agent_*.meta.json; do
        [ -f "$f" ] || continue
        cp "$f" "$bundle/agents/"
        meta_count=$((meta_count + 1))
    done
    if [ "$wasm_count" -eq 0 ] || [ "$meta_count" -eq 0 ]; then
        echo "Error: expected runtara_agent_*.{wasm,meta.json} in ${agent_src}, found ${wasm_count} wasm and ${meta_count} meta files" >&2
        exit 1
    fi
    info "  Agents: ${wasm_count} .wasm + ${meta_count} .meta.json"

    local workflow_component_count=0
    local workflow_component_meta_count=0
    for f in "$agent_src"/runtara_workflow_*.wasm; do
        [ -f "$f" ] || continue
        cp "$f" "$bundle/agents/"
        workflow_component_count=$((workflow_component_count + 1))
    done
    for f in "$agent_src"/runtara_workflow_*.meta.json; do
        [ -f "$f" ] || continue
        cp "$f" "$bundle/agents/"
        workflow_component_meta_count=$((workflow_component_meta_count + 1))
    done
    if [ "$workflow_component_count" -ne 2 ] || [ "$workflow_component_meta_count" -ne 2 ]; then
        echo "Error: expected 2 runtara_workflow_*.{wasm,meta.json} shared components in ${agent_src}, found ${workflow_component_count} wasm and ${workflow_component_meta_count} meta files" >&2
        exit 1
    fi
    info "  Direct workflow shared components: ${workflow_component_count} .wasm + ${workflow_component_meta_count} .meta.json"

    # ── Licenses ──
    info "Copying licenses"
    cp "$ROOT_DIR/LICENSE" "$bundle/licenses/LICENSE-runtara-AGPL-3.0"
    cp "$ROOT_DIR/docs/licenses/"* "$bundle/licenses/"

    # ── VERSION ──
    echo "$RUNTARA_STAMP_VERSION" > "$bundle/VERSION"

    # ── MANIFEST.json ──
    cat > "$bundle/MANIFEST.json" <<MANIFEST
{
  "runtara_version": "${RUNTARA_STAMP_VERSION}",
  "runtara_commit": "${RUNTARA_COMMIT}",
  "rustc_version": "${RUSTC_VERSION}",
  "cargo_component_version": "${CARGO_COMPONENT_VERSION}",
  "agent_component_count": ${wasm_count},
  "workflow_shared_component_count": ${workflow_component_count},
  "host_target": "${HOST_TARGET}",
  "os": "${OS}",
  "arch": "${ARCH}",
  "build_date": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
}
MANIFEST

    BUNDLE_DIR="$bundle"
    info "Bundle assembled at: ${BUNDLE_DIR}"
}

# ─── Create tarball ──────────────────────────────────────────────────────────

create_tarball() {
    step "Creating tarball"

    local basename="runtara-${RUNTARA_VERSION}-${ARCH}-${OS}"
    local tarball="${OUTPUT_DIR}/${basename}.tar.gz"

    (cd "$OUTPUT_DIR" && tar czf "${basename}.tar.gz" "${basename}/")

    # Checksum
    local sha_cmd="sha256sum"
    if ! command -v sha256sum > /dev/null 2>&1; then
        sha_cmd="shasum -a 256"
    fi
    (cd "$OUTPUT_DIR" && $sha_cmd "${basename}.tar.gz" > "${basename}.tar.gz.sha256")

    local size
    size=$(du -sh "$tarball" | cut -f1)
    info "Tarball: ${tarball} (${size})"
    info "SHA256:  ${tarball}.sha256"

    echo ""
    printf '%s  Bundle build complete!%s\n' "${GREEN}${BOLD}" "$NC"
    echo ""
    echo "  Bundle dir: ${BUNDLE_DIR}"
    echo "  Tarball:    ${tarball}"
    echo "  Size:       ${size}"
    echo ""
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
    printf '\n%s  Runtara Bundle Builder%s\n\n' "${BOLD}" "$NC"

    detect_platform
    resolve_versions
    build_frontend
    build_server
    build_stdlib
    install_cargo_component
    build_agent_components
    assemble_bundle
    create_tarball
}

main "$@"
