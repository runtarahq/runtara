#!/usr/bin/env bash
# Build a hermetic Runtara bundle for the current host platform.
#
# The bundle contains:
#   - runtara-server binary
#   - A pruned copy of the Rust toolchain (rustc + host std + wasm32-wasip2 std)
#   - Pre-built workflow stdlib (wasm rlibs + host proc-macros)
#   - Wasmtime CLI binary
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

    # Rust version from the active toolchain (pinned by rust-toolchain.toml)
    RUSTC_VERSION="$(rustc --version | cut -d' ' -f2)"

    # Respect CARGO_TARGET_DIR if set
    TARGET_DIR="${CARGO_TARGET_DIR:-target}"

    info "Runtara version: ${RUNTARA_VERSION}"
    info "Rustc version:   ${RUSTC_VERSION}"
    info "Wasmtime version: ${WASMTIME_VERSION}"
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

    # embed-bitcode=yes is required so that scenario compilation can use LTO
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

# ─── Assemble bundle ────────────────────────────────────────────────────────

assemble_bundle() {
    step "Assembling bundle"

    local bundle="${OUTPUT_DIR}/runtara-${RUNTARA_VERSION}-${ARCH}-${OS}"
    rm -rf "$bundle"
    mkdir -p "$bundle"/{bin,toolchain/bin,toolchain/lib/rustlib,stdlib/deps,licenses}

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
        warn "wasm-component-ld not found — WASM scenario compilation will fail at runtime!"
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

    # lib/rustlib: wasm32-wasip2 target (needed for scenario compilation)
    cp -R "$sysroot/lib/rustlib/wasm32-wasip2" "$bundle/toolchain/lib/rustlib/"

    # ── Stdlib library cache ──
    info "Copying pre-built workflow stdlib"

    # WASM rlibs
    local wasm_release="${TARGET_DIR}/wasm32-wasip2/release"
    local wasm_deps="${wasm_release}/deps"
    cp "${wasm_release}/libruntara_workflow_stdlib.rlib" "$bundle/stdlib/"
    for rlib in "$wasm_deps"/*.rlib; do
        [ -f "$rlib" ] || continue
        case "$(basename "$rlib")" in
            *runtara_workflow_stdlib*) continue ;;
        esac
        cp "$rlib" "$bundle/stdlib/deps/"
    done

    # Native static archives (.a files) needed by the WASM linker (e.g., wit_bindgen_cabi_realloc).
    # These live in the build/ directory, not deps/.
    find "${wasm_release}/build" -name "*.a" -exec cp {} "$bundle/stdlib/deps/" \; 2>/dev/null || true

    # Host proc-macro shared libraries
    local host_deps="${TARGET_DIR}/release/deps"
    case "$OS" in
        darwin) local dylib_ext="dylib" ;;
        linux)  local dylib_ext="so" ;;
    esac
    for pm in "$host_deps"/*."$dylib_ext"; do
        [ -f "$pm" ] || continue
        cp "$pm" "$bundle/stdlib/deps/"
    done

    local rlib_count
    rlib_count=$(find "$bundle/stdlib/deps" -name "*.rlib" | wc -l | tr -d ' ')
    local pm_count
    pm_count=$(find "$bundle/stdlib/deps" -name "*.${dylib_ext}" | wc -l | tr -d ' ')
    info "  Stdlib: ${rlib_count} rlibs, ${pm_count} proc-macros"

    # ── Licenses ──
    info "Copying licenses"
    cp "$ROOT_DIR/LICENSE" "$bundle/licenses/LICENSE-runtara-AGPL-3.0"
    cp "$ROOT_DIR/docs/licenses/"* "$bundle/licenses/"

    # ── VERSION ──
    echo "$RUNTARA_VERSION" > "$bundle/VERSION"

    # ── MANIFEST.json ──
    cat > "$bundle/MANIFEST.json" <<MANIFEST
{
  "runtara_version": "${RUNTARA_VERSION}",
  "rustc_version": "${RUSTC_VERSION}",
  "wasmtime_version": "${WASMTIME_VERSION}",
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
    assemble_bundle
    create_tarball
}

main "$@"
