#!/usr/bin/env bash
# Build a hermetic Runtara bundle for the current host platform.
#
# The bundle contains:
#   - runtara-server binary
#   - A pruned copy of the Rust toolchain (rustc + host std + wasm32-wasip2 std)
#   - Wasmtime CLI binary
#   - wac CLI binary (WebAssembly Composition — workflow compile step)
#   - cargo-component binary (cargo subcommand — workflow compile step)
#   - 23 pre-built agent components (.wasm + .meta.json each)
#   - compile-src/: source mirror of the stdlib/sdk/agent-wit crates so
#     cargo-component can build the per-workflow logic component on hosts
#     that received only the released tarball (the workflow-logic Cargo.toml
#     has absolute path = "..." deps into this tree).
#   - License files
#   - VERSION and MANIFEST.json
#
# Usage:
#   ./scripts/build-bundle.sh                   # build for the current host
#   ./scripts/build-bundle.sh --skip-build      # assemble from existing target/release
#   ./scripts/build-bundle.sh --output-dir /tmp # write bundle to custom dir
#
# Prerequisites:
#   - rustup with the version from rust-toolchain.toml installed
#   - wasm32-wasip2 target installed (rust-toolchain.toml handles this)
#   - curl (for downloading wasmtime if not cached)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$ROOT_DIR"

# ─── Defaults ────────────────────────────────────────────────────────────────

SKIP_BUILD=0
OUTPUT_DIR="${ROOT_DIR}/target/bundle"
WASMTIME_VERSION="43.0.0"
# wac (WebAssembly Composition) CLI: composes the workflow component with
# its required agent components at workflow-compile time. Upstream ships
# prebuilt binaries on the releases page.
WAC_VERSION="0.10.0"
# cargo-component: subcommand used to build the per-workflow logic
# component before wac compose. No prebuilt binaries upstream — installed
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
        --wasmtime-version) WASMTIME_VERSION="$2"; shift 2 ;;
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
    info "Wasmtime version: ${WASMTIME_VERSION}"
    info "wac version:     ${WAC_VERSION}"
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

# ─── Download wasmtime ───────────────────────────────────────────────────────

download_wasmtime() {
    step "Fetching Wasmtime ${WASMTIME_VERSION}"

    mkdir -p "$DOWNLOAD_CACHE"

    local wt_arch
    case "$ARCH" in
        x86_64)  wt_arch="x86_64" ;;
        aarch64) wt_arch="aarch64" ;;
    esac

    local wt_os
    case "$OS" in
        linux)  wt_os="linux" ;;
        darwin) wt_os="macos" ;;
    esac

    local tarball="wasmtime-v${WASMTIME_VERSION}-${wt_arch}-${wt_os}.tar.xz"
    local url="https://github.com/bytecodealliance/wasmtime/releases/download/v${WASMTIME_VERSION}/${tarball}"
    local cached="${DOWNLOAD_CACHE}/${tarball}"

    if [ -f "$cached" ]; then
        info "Using cached ${tarball}"
    else
        info "Downloading ${tarball}"
        curl -fSL -o "$cached" "$url"
    fi

    WASMTIME_TARBALL="$cached"
}

# ─── Download wac (WebAssembly Composition CLI) ─────────────────────────────

download_wac() {
    step "Fetching wac ${WAC_VERSION}"

    mkdir -p "$DOWNLOAD_CACHE"

    # Upstream binary naming uses linux-musl for both glibc and musl Linux —
    # the binary is statically linked so it runs on either.
    local wac_asset
    case "${ARCH}-${OS}" in
        x86_64-linux)   wac_asset="wac-cli-x86_64-unknown-linux-musl" ;;
        aarch64-linux)  wac_asset="wac-cli-aarch64-unknown-linux-musl" ;;
        x86_64-darwin)  wac_asset="wac-cli-x86_64-apple-darwin" ;;
        aarch64-darwin) wac_asset="wac-cli-aarch64-apple-darwin" ;;
        *) echo "No upstream wac binary for ${ARCH}-${OS}" >&2; exit 1 ;;
    esac

    local url="https://github.com/bytecodealliance/wac/releases/download/v${WAC_VERSION}/${wac_asset}"
    local cached="${DOWNLOAD_CACHE}/${wac_asset}-${WAC_VERSION}"

    if [ -f "$cached" ]; then
        info "Using cached ${wac_asset}"
    else
        info "Downloading ${wac_asset}"
        curl -fSL -o "$cached" "$url"
        chmod +x "$cached"
    fi

    WAC_BINARY="$cached"
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
        # Intentionally NOT using --locked. cargo-component v0.21.1's own
        # Cargo.lock pins wit-parser v0.219.1, which has since been yanked
        # from crates.io. Letting cargo resolve from the Cargo.toml ranges
        # picks a non-yanked wit-parser. The primary version (cargo-
        # component itself) is still pinned via --version so behavior
        # stays stable; only transitive deps shift.
        info "cargo install cargo-component --version ${CARGO_COMPONENT_VERSION} --root ${root}"
        cargo install cargo-component \
            --version "$CARGO_COMPONENT_VERSION" \
            --root "$root"
    fi

    CARGO_COMPONENT_BINARY="$bin"
}

# ─── Build agent components ─────────────────────────────────────────────────
#
# Produces target/wasm32-wasip1/release/runtara_agent_<x>.wasm plus the
# sibling .meta.json for each of the 23 agent crates. The build script
# itself depends on cargo-component + wit-deps being on PATH; we've already
# installed the right cargo-component into the cache, so prepend it.

build_agent_components() {
    if [ "$SKIP_BUILD" = "1" ]; then
        if ! find "${TARGET_DIR}/wasm32-wasip1/release" -maxdepth 1 \
                -name 'runtara_agent_*.wasm' 2>/dev/null | grep -q .; then
            echo "Error: --skip-build but no built agent components found at \
${TARGET_DIR}/wasm32-wasip1/release/runtara_agent_*.wasm" >&2
            exit 1
        fi
        info "Skipping agent component build (--skip-build)"
        return
    fi

    step "Building agent WASM components (23 crates)"
    PATH="$(dirname "$CARGO_COMPONENT_BINARY"):${PATH}" \
        "$SCRIPT_DIR/build-agent-components.sh"
}

# ─── Assemble bundle ────────────────────────────────────────────────────────

assemble_bundle() {
    step "Assembling bundle"

    local bundle="${OUTPUT_DIR}/runtara-${RUNTARA_VERSION}-${ARCH}-${OS}"
    rm -rf "$bundle"
    mkdir -p "$bundle"/{bin,toolchain/bin,toolchain/lib/rustlib,agents,licenses,compile-src/crates/agents}

    # ── runtara-server binary ──
    info "Copying runtara-server binary"
    cp "${TARGET_DIR}/release/runtara-server" "$bundle/bin/"
    strip "$bundle/bin/runtara-server" 2>/dev/null || warn "strip failed (non-critical)"

    # ── Wasmtime binary ──
    info "Extracting wasmtime binary"
    local wt_arch
    case "$ARCH" in
        x86_64)  wt_arch="x86_64" ;;
        aarch64) wt_arch="aarch64" ;;
    esac
    local wt_os
    case "$OS" in
        linux)  wt_os="linux" ;;
        darwin) wt_os="macos" ;;
    esac

    local wt_dir="wasmtime-v${WASMTIME_VERSION}-${wt_arch}-${wt_os}"
    local tmp_wt="$(mktemp -d)"
    tar -xJf "$WASMTIME_TARBALL" -C "$tmp_wt"
    cp "$tmp_wt/$wt_dir/wasmtime" "$bundle/bin/"
    rm -rf "$tmp_wt"

    # ── wac (WebAssembly Composition CLI) ──
    info "Copying wac binary"
    cp "$WAC_BINARY" "$bundle/bin/wac"
    chmod +x "$bundle/bin/wac"

    # ── cargo-component ──
    info "Copying cargo-component binary"
    cp "$CARGO_COMPONENT_BINARY" "$bundle/bin/cargo-component"
    chmod +x "$bundle/bin/cargo-component"

    # ── Agent components ──
    # Each of the 23 component agents ships as a .wasm + sibling .meta.json
    # pair. At server boot, RUNTARA_AGENT_COMPONENTS_DIR points at this
    # directory; the ComponentDispatcherService loads each pair and exposes
    # the agents to the validator and workflow runtime.
    info "Copying agent WASM components"
    local agent_src="${TARGET_DIR}/wasm32-wasip1/release"
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

    # ── Rust toolchain (pruned) ──
    info "Copying pruned Rust toolchain"
    local sysroot
    sysroot="$(rustc --print sysroot)"

    # bin: rustc, cargo, and wasm-component-ld (required for wasm32-wasip2 linking)
    cp "$sysroot/bin/rustc" "$bundle/toolchain/bin/"
    cp "$sysroot/bin/cargo" "$bundle/toolchain/bin/"

    # wasm-component-ld: installed via `cargo install wasm-component-ld`, lives in ~/.cargo/bin
    local wasm_ld
    wasm_ld="$(command -v wasm-component-ld 2>/dev/null || true)"
    if [ -n "$wasm_ld" ]; then
        cp "$wasm_ld" "$bundle/toolchain/bin/"
        info "Included wasm-component-ld from ${wasm_ld}"
    else
        warn "wasm-component-ld not found — WASM workflow compilation will fail at runtime!"
        warn "Install it with: cargo install wasm-component-ld"
    fi

    # lib: rustc shared libraries (mandatory runtime deps of rustc)
    case "$OS" in
        darwin)
            cp "$sysroot"/lib/librustc_driver-*.dylib "$bundle/toolchain/lib/"
            ;;
        linux)
            cp "$sysroot"/lib/librustc_driver-*.so "$bundle/toolchain/lib/"
            # LLVM shared lib — required by rustc on Linux
            cp "$sysroot"/lib/libLLVM*.so* "$bundle/toolchain/lib/"
            ;;
    esac

    # lib/rustlib: host target (needed for proc-macros to link against)
    cp -R "$sysroot/lib/rustlib/${HOST_TARGET}" "$bundle/toolchain/lib/rustlib/"
    # Remove sanitizer runtimes from the host target copy (not needed, saves ~8MB)
    rm -f "$bundle/toolchain/lib/rustlib/${HOST_TARGET}/lib/"*sanitizer* \
          "$bundle/toolchain/lib/rustlib/${HOST_TARGET}/lib/"*asan* \
          "$bundle/toolchain/lib/rustlib/${HOST_TARGET}/lib/"*tsan* \
          "$bundle/toolchain/lib/rustlib/${HOST_TARGET}/lib/"*lsan* 2>/dev/null || true
    # Also remove sanitizer runtimes from top-level lib/
    rm -f "$bundle/toolchain/lib/"*asan* \
          "$bundle/toolchain/lib/"*tsan* \
          "$bundle/toolchain/lib/"*lsan* 2>/dev/null || true

    # lib/rustlib: wasm32-wasip2 target (needed for workflow compilation)
    cp -R "$sysroot/lib/rustlib/wasm32-wasip2" "$bundle/toolchain/lib/rustlib/"

    # ── Compile-source tree ──
    # Required by cargo-component at workflow-compile time. Workflows are
    # built by materializing a `workflow-logic` crate whose generated
    # Cargo.toml has absolute `path = "..."` deps into the runtara workspace
    # (runtara-workflow-stdlib, runtara-sdk, runtara-sdk-macros, runtara-http
    # + per-agent wit/ dirs). Without this tree the released tarball can only
    # compile on the CI host that produced it. See
    # crates/runtara-workflows/src/components_compile.rs::workspace_root() —
    # the server picks this up via $RUNTARA_COMPILE_SOURCE_DIR (set by
    # scripts/install.sh).
    info "Assembling compile-source mirror"
    local cs="$bundle/compile-src"
    local ws_version
    ws_version="$(grep '^version' "${ROOT_DIR}/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')"

    # Synthesized workspace root. Its sole job is resolving the
    # `version.workspace = true` / `runtara-http = { workspace = true }`
    # directives in the bundled crates' Cargo.toml files. cargo doesn't
    # `cargo build` from here — the active workspace is the per-workflow
    # build dir (which has its own empty [workspace]); this manifest is read
    # passively during path-dep resolution.
    cat > "$cs/Cargo.toml" <<COMPILESRCEOF
# Generated by scripts/build-bundle.sh — DO NOT EDIT.
# Mirrors the slice of the runtara workspace that the per-workflow
# Cargo.toml depends on. Lives at \$RUNTARA_COMPILE_SOURCE_DIR.
[workspace]
resolver = "2"
members = [
    "crates/runtara-http",
    "crates/runtara-sdk",
    "crates/runtara-sdk-macros",
    "crates/runtara-workflow-stdlib",
]

[workspace.package]
edition = "2024"
version = "${ws_version}"
license = "AGPL-3.0-or-later"
repository = "https://github.com/runtarahq/runtara"

[workspace.dependencies]
runtara-http = { path = "crates/runtara-http", version = "6.0" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
chrono = { version = "0.4", features = ["serde"] }
base64 = "0.22"
COMPILESRCEOF

    # Source-bearing crates: copy Cargo.toml + src/. Skip tests/, benches/,
    # README — they don't affect the build and just add weight.
    local crate
    for crate in runtara-workflow-stdlib runtara-sdk runtara-sdk-macros runtara-http; do
        local dst="$cs/crates/$crate"
        mkdir -p "$dst"
        cp "${ROOT_DIR}/crates/$crate/Cargo.toml" "$dst/"
        cp -R "${ROOT_DIR}/crates/$crate/src" "$dst/src"
    done

    # runtara-agent-wit: only the `wit/` subtree is needed (referenced as a
    # WIT-source path dep, not a cargo crate). Carries the runtara:agent
    # contract + the 7 wasi:* dep mirrors.
    mkdir -p "$cs/crates/runtara-agent-wit"
    cp -R "${ROOT_DIR}/crates/runtara-agent-wit/wit" "$cs/crates/runtara-agent-wit/wit"

    # Per-agent wit/agent.wit files. Each declares the
    # `runtara:agent-<id>@0.3.0` package the workflow-logic crate imports.
    local agent_wit_count=0
    local agent_dir name wit_src
    for agent_dir in "${ROOT_DIR}"/crates/agents/runtara-agent-*; do
        [ -d "$agent_dir" ] || continue
        name="$(basename "$agent_dir")"
        wit_src="$agent_dir/wit/agent.wit"
        if [ -f "$wit_src" ]; then
            mkdir -p "$cs/crates/agents/$name/wit"
            cp "$wit_src" "$cs/crates/agents/$name/wit/agent.wit"
            agent_wit_count=$((agent_wit_count + 1))
        fi
    done
    if [ "$agent_wit_count" -eq 0 ]; then
        echo "Error: no per-agent wit/agent.wit files found under ${ROOT_DIR}/crates/agents/ — run a host build first so each agent's build.rs emits its wit/agent.wit" >&2
        exit 1
    fi
    info "  Compile-src: 4 crates + ${agent_wit_count} per-agent WIT"

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
  "wasmtime_version": "${WASMTIME_VERSION}",
  "wac_version": "${WAC_VERSION}",
  "cargo_component_version": "${CARGO_COMPONENT_VERSION}",
  "agent_component_count": ${wasm_count},
  "agent_wit_count": ${agent_wit_count},
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
    download_wasmtime
    download_wac
    install_cargo_component
    build_agent_components
    assemble_bundle
    create_tarball
}

main "$@"
