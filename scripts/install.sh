#!/usr/bin/env bash
# Runtara Install Script
# https://install.runtara.com
#
# Installs Rust (via rustup), Wasmtime, and the runtara-server binary
# on Debian/Ubuntu or Amazon Linux/RHEL (x86_64 / aarch64).
#
# Usage:
#   curl -fsSL https://install.runtara.com | sh
#   curl -fsSL https://install.runtara.com?version=1.3.0 | sh
#
# Non-interactive (all config via environment variables):
#   export RUNTARA_NONINTERACTIVE=1
#   export OBJECT_MODEL_DATABASE_URL=postgres://...
#   export RUNTARA_DATABASE_URL=postgres://...
#   export VALKEY_HOST=127.0.0.1
#   export OAUTH2_JWKS_URI=https://...
#   export OAUTH2_ISSUER=https://...
#   export TENANT_ID=org_abc123
#   curl -fsSL https://install.runtara.com | sh

set -eu

# ─── Exit codes ───────────────────────────────────────────────────────────────
EXIT_UNSUPPORTED_OS=10
EXIT_UNSUPPORTED_ARCH=11
EXIT_MISSING_DEPS=12
EXIT_RUST_INSTALL=20
EXIT_WASMTIME_DOWNLOAD=30
EXIT_WASMTIME_CHECKSUM=31
EXIT_BINARY_DOWNLOAD=40
EXIT_BINARY_CHECKSUM=41
EXIT_POSTGRES_CHECK=50
EXIT_VALKEY_CHECK=51
EXIT_USER_CREATION=60
EXIT_SERVICE_SETUP=70
EXIT_CONFIG_WRITE=80
EXIT_LIBRARY_BUILD=90

# ─── Defaults ─────────────────────────────────────────────────────────────────
GITHUB_REPO="runtarahq/runtara"
WASMTIME_REPO="bytecodealliance/wasmtime"
WASMTIME_VERSION="${RUNTARA_WASMTIME_VERSION:-29.0.1}"

INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="/etc/runtara"
DATA_DIR="/var/lib/runtara"
LOG_DIR="/var/log/runtara"
SYSTEMD_DIR="/lib/systemd/system"

LIBRARY_CACHE_DIR="/usr/share/runtara/library_cache/wasm"

SERVICE_USER="runtara"
SERVICE_GROUP="runtara"

# Colour helpers (disabled when piped)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    BOLD='\033[1m'
    NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' BLUE='' BOLD='' NC=''
fi

# ─── Helpers ──────────────────────────────────────────────────────────────────

info()  { printf "${GREEN}[+]${NC} %s\n" "$*"; }
warn()  { printf "${YELLOW}[!]${NC} %s\n" "$*"; }
err()   { printf "${RED}[x]${NC} %s\n" "$*" >&2; }
step()  { printf "\n${BOLD}${BLUE}==> %s${NC}\n" "$*"; }

die() {
    code=$1; shift
    err "$@"
    exit "$code"
}

need_cmd() {
    if ! command -v "$1" > /dev/null 2>&1; then
        die $EXIT_MISSING_DEPS "Required command not found: $1. $2"
    fi
}

# ─── Platform detection ──────────────────────────────────────────────────────

detect_platform() {
    ARCH="$(uname -m)"
    case "$ARCH" in
        x86_64|amd64)  ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *) die $EXIT_UNSUPPORTED_ARCH "Unsupported architecture: $ARCH. Only x86_64 and aarch64 are supported." ;;
    esac

    if [ ! -f /etc/os-release ]; then
        die $EXIT_UNSUPPORTED_OS "Cannot detect OS: /etc/os-release not found."
    fi

    # shellcheck source=/dev/null
    . /etc/os-release

    case "${ID:-}${ID_LIKE:-}" in
        *debian*|*ubuntu*)
            OS_FAMILY="debian"
            PKG_MGR="apt-get"
            ;;
        *rhel*|*centos*|*fedora*|*amzn*)
            OS_FAMILY="rhel"
            PKG_MGR="yum"
            if command -v dnf > /dev/null 2>&1; then
                PKG_MGR="dnf"
            fi
            ;;
        *)
            die $EXIT_UNSUPPORTED_OS "Unsupported OS: ${PRETTY_NAME:-$ID}. Supported: Debian/Ubuntu, Amazon Linux/RHEL."
            ;;
    esac

    info "Detected: ${PRETTY_NAME:-$ID} ($ARCH) [$OS_FAMILY]"
}

# ─── Version resolution ──────────────────────────────────────────────────────

resolve_version() {
    if [ -n "${RUNTARA_VERSION:-}" ]; then
        VERSION="$RUNTARA_VERSION"
    elif [ -n "${version:-}" ]; then
        # query-string param from ?version=x.x.x
        VERSION="$version"
    else
        step "Resolving latest runtara version"
        VERSION="$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" \
            | sed -n 's/.*"tag_name":[[:space:]]*"v\([^"]*\)".*/\1/p')"
        if [ -z "$VERSION" ]; then
            die $EXIT_BINARY_DOWNLOAD "Failed to resolve latest version from GitHub."
        fi
    fi
    info "Target version: ${VERSION}"
}

# ─── System dependencies ─────────────────────────────────────────────────────

install_system_deps() {
    step "Installing system dependencies"

    PACKAGES="curl tar gzip xz-utils make perl gcc clang lld pkg-config libssl-dev postgresql-client"
    if [ "$OS_FAMILY" = "rhel" ]; then
        PACKAGES="curl tar gzip xz make perl gcc clang lld pkgconfig openssl-devel postgresql"
    fi

    info "Installing: $PACKAGES"
    if [ "$OS_FAMILY" = "debian" ]; then
        DEBIAN_FRONTEND=noninteractive $PKG_MGR update -qq
        # shellcheck disable=SC2086
        DEBIAN_FRONTEND=noninteractive $PKG_MGR install -y -qq $PACKAGES
    else
        # shellcheck disable=SC2086
        $PKG_MGR install -y -q $PACKAGES
    fi

    # Ensure we have a tool to test Valkey connectivity
    if ! command -v redis-cli > /dev/null 2>&1 && ! command -v valkey-cli > /dev/null 2>&1; then
        info "Installing redis-cli for connectivity checks"
        if [ "$OS_FAMILY" = "debian" ]; then
            DEBIAN_FRONTEND=noninteractive $PKG_MGR install -y -qq redis-tools 2>/dev/null || true
        else
            $PKG_MGR install -y -q redis 2>/dev/null || true
        fi
    fi
}

# ─── Rust (rustup) ───────────────────────────────────────────────────────────

install_rust() {
    step "Setting up Rust toolchain"

    # Install to a system-accessible location so the runtara user can compile scenarios
    export RUSTUP_HOME="${RUSTUP_HOME:-/usr/local/rustup}"
    export CARGO_HOME="${CARGO_HOME:-/usr/local/cargo}"

    if [ -x "$CARGO_HOME/bin/rustup" ]; then
        info "Rust already installed, updating"
        "$CARGO_HOME/bin/rustup" update stable --no-self-update
    else
        info "Installing Rust via rustup"
        curl -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --no-modify-path \
            || die $EXIT_RUST_INSTALL "Rust installation failed. Check network connectivity and try again."
    fi

    # Make cargo/rustc available system-wide
    if [ ! -f /etc/profile.d/rust.sh ] || ! grep -q "$CARGO_HOME" /etc/profile.d/rust.sh 2>/dev/null; then
        cat > /etc/profile.d/rust.sh <<RUSTEOF
export RUSTUP_HOME="$RUSTUP_HOME"
export CARGO_HOME="$CARGO_HOME"
export PATH="\$CARGO_HOME/bin:\$PATH"
RUSTEOF
        chmod 644 /etc/profile.d/rust.sh
    fi

    export PATH="$CARGO_HOME/bin:$PATH"

    # Add wasm32-wasip2 target for compiling scenarios to WASM
    "$CARGO_HOME/bin/rustup" target add wasm32-wasip2

    info "Rust $(rustc --version | cut -d' ' -f2) ready, wasm32-wasip2 target installed"
}

# ─── Wasmtime ─────────────────────────────────────────────────────────────────

install_wasmtime() {
    step "Installing Wasmtime ${WASMTIME_VERSION}"

    local wasmtime_arch
    case "$ARCH" in
        x86_64)  wasmtime_arch="x86_64" ;;
        aarch64) wasmtime_arch="aarch64" ;;
    esac

    local base_url="https://github.com/${WASMTIME_REPO}/releases/download/v${WASMTIME_VERSION}"
    local tarball="wasmtime-v${WASMTIME_VERSION}-${wasmtime_arch}-linux.tar.xz"
    local sha_file="${tarball}.sha256"

    local tmp_dir
    tmp_dir="$(mktemp -d)"
    # shellcheck disable=SC2064
    trap "rm -rf '${tmp_dir}'" EXIT

    info "Downloading ${tarball}"
    curl -fSL -o "$tmp_dir/$tarball" "${base_url}/${tarball}" \
        || die $EXIT_WASMTIME_DOWNLOAD "Failed to download Wasmtime. Check https://github.com/${WASMTIME_REPO}/releases/tag/v${WASMTIME_VERSION}"

    info "Verifying checksum"
    if curl -fSL -o "$tmp_dir/$sha_file" "${base_url}/${sha_file}" 2>/dev/null; then
        (cd "$tmp_dir" && sha256sum -c "$sha_file") \
            || die $EXIT_WASMTIME_CHECKSUM "Wasmtime checksum verification failed. The download may be corrupted — retry or check the release page."
    else
        warn "No separate checksum file published for Wasmtime ${WASMTIME_VERSION}, skipping verification"
    fi

    info "Extracting to ${INSTALL_DIR}"
    tar -xJf "$tmp_dir/$tarball" -C "$tmp_dir"
    install -m 755 "$tmp_dir/wasmtime-v${WASMTIME_VERSION}-${wasmtime_arch}-linux/wasmtime" "$INSTALL_DIR/wasmtime"

    rm -rf "$tmp_dir"
    trap - EXIT

    info "Wasmtime $(wasmtime --version) installed at ${INSTALL_DIR}/wasmtime"
}

# ─── Runtara server binary ───────────────────────────────────────────────────

install_binary() {
    step "Installing runtara-server v${VERSION}"

    # If upgrading, stop existing service first
    if systemctl is-active --quiet runtara-server 2>/dev/null; then
        info "Stopping running runtara-server for upgrade"
        systemctl stop runtara-server
    fi

    # Allow pre-placed binary (for testing or air-gapped installs)
    if [ -n "${RUNTARA_BINARY_PATH:-}" ]; then
        info "Using pre-placed binary: ${RUNTARA_BINARY_PATH}"
        install -m 755 "$RUNTARA_BINARY_PATH" "$INSTALL_DIR/runtara-server"
    else
        local base_url="https://github.com/${GITHUB_REPO}/releases/download/v${VERSION}"
        local tarball="runtara-server-${VERSION}-${ARCH}-linux.tar.gz"
        local sha_file="${tarball}.sha256"

        local tmp_dir
        tmp_dir="$(mktemp -d)"

        info "Downloading ${tarball}"
        curl -fSL -o "$tmp_dir/$tarball" "${base_url}/${tarball}" \
            || die $EXIT_BINARY_DOWNLOAD "Failed to download runtara-server v${VERSION} for ${ARCH}. Check https://github.com/${GITHUB_REPO}/releases/tag/v${VERSION}"

        info "Verifying checksum"
        if curl -fSL -o "$tmp_dir/$sha_file" "${base_url}/${sha_file}" 2>/dev/null; then
            (cd "$tmp_dir" && sha256sum -c "$sha_file") \
                || die $EXIT_BINARY_CHECKSUM "Checksum verification failed. The download may be corrupted — retry or check the release page."
        else
            warn "No checksum file found, skipping verification"
        fi

        tar -xzf "$tmp_dir/$tarball" -C "$tmp_dir"
        install -m 755 "$tmp_dir/runtara-server" "$INSTALL_DIR/runtara-server"
        rm -rf "$tmp_dir"
    fi

    info "runtara-server installed at ${INSTALL_DIR}/runtara-server"
}

# ─── Stdlib library cache ─────────────────────────────────────────────────────

build_library_cache() {
    step "Building workflow stdlib library cache"

    if [ "${RUNTARA_SKIP_LIBRARY_BUILD:-0}" = "1" ]; then
        warn "Skipping library build (RUNTARA_SKIP_LIBRARY_BUILD=1)"
        mkdir -p "$LIBRARY_CACHE_DIR/deps"
        return
    fi

    info "This compiles runtara-workflow-stdlib from source (may take a few minutes)"

    local src_dir
    src_dir="$(mktemp -d)"

    # Fetch the source — either a pre-placed directory, a specific git ref, or the release tag
    if [ -n "${RUNTARA_SOURCE_DIR:-}" ]; then
        info "Using local source: ${RUNTARA_SOURCE_DIR}"
        cp -a "$RUNTARA_SOURCE_DIR/." "$src_dir/"
    else
        local tarball_url="https://github.com/${GITHUB_REPO}/archive/refs/tags/v${VERSION}.tar.gz"
        info "Fetching source for v${VERSION}"
        curl -fSL "$tarball_url" | tar -xz -C "$src_dir" --strip-components=1 \
            || die $EXIT_LIBRARY_BUILD "Failed to fetch runtara source for v${VERSION}."
    fi

    # Ensure the Rust toolchain is on PATH (it was just installed)
    export PATH="$CARGO_HOME/bin:$PATH"

    # ── Install WASI SDK (needed for C dependencies like zstd-sys, ring) ──
    local wasi_sdk_version="25"
    local wasi_sdk_dir="/opt/wasi-sdk"
    if [ ! -d "$wasi_sdk_dir" ]; then
        local wasi_sdk_arch
        case "$ARCH" in
            x86_64)  wasi_sdk_arch="x86_64" ;;
            aarch64) wasi_sdk_arch="arm64" ;;
        esac
        local wasi_sdk_tar="wasi-sdk-${wasi_sdk_version}.0-${wasi_sdk_arch}-linux.tar.gz"
        local wasi_sdk_url="https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${wasi_sdk_version}/${wasi_sdk_tar}"
        info "Installing WASI SDK ${wasi_sdk_version} (needed for C dependencies)"
        curl -fSL "$wasi_sdk_url" | tar -xz -C /opt \
            || die $EXIT_LIBRARY_BUILD "Failed to install WASI SDK."
        mv "/opt/wasi-sdk-${wasi_sdk_version}.0-${wasi_sdk_arch}-linux" "$wasi_sdk_dir"
    fi

    # ── Build stdlib for wasm32-wasip2 (target rlibs) ──
    info "Compiling runtara-workflow-stdlib for wasm32-wasip2 (release)"
    (cd "$src_dir" && \
        CC_wasm32_wasip2="${wasi_sdk_dir}/bin/clang" \
        CFLAGS_wasm32_wasip2="--sysroot=${wasi_sdk_dir}/share/wasi-sysroot" \
        cargo build \
            -p runtara-workflow-stdlib \
            --release \
            --target wasm32-wasip2 \
            --no-default-features) \
        || die $EXIT_LIBRARY_BUILD \
            "Failed to build runtara-workflow-stdlib for wasm32-wasip2." \
            "Check build output above for errors."

    # ── Build host proc-macros (dylibs for the host architecture) ──
    info "Compiling host proc-macros"
    (cd "$src_dir" && cargo build \
        -p runtara-workflow-stdlib \
        --release) \
        || die $EXIT_LIBRARY_BUILD "Failed to build host proc-macros."

    # ── Populate library cache ──
    local wasm_deps="$src_dir/target/wasm32-wasip2/release/deps"
    local host_deps="$src_dir/target/release/deps"
    local wasm_release="$src_dir/target/wasm32-wasip2/release"

    rm -rf "$LIBRARY_CACHE_DIR"
    mkdir -p "$LIBRARY_CACHE_DIR/deps"

    # Main stdlib rlib
    cp "$wasm_release/libruntara_workflow_stdlib.rlib" "$LIBRARY_CACHE_DIR/" \
        || die $EXIT_LIBRARY_BUILD "libruntara_workflow_stdlib.rlib not found in build output."

    # Target dependency rlibs (everything except the stdlib itself)
    local rlib_count=0
    for rlib in "$wasm_deps"/*.rlib; do
        [ -f "$rlib" ] || continue
        case "$(basename "$rlib")" in
            *runtara_workflow_stdlib*) continue ;;
        esac
        cp "$rlib" "$LIBRARY_CACHE_DIR/deps/"
        rlib_count=$((rlib_count + 1))
    done

    # Host proc-macro shared libraries (.so on Linux)
    local procmacro_count=0
    for so in "$host_deps"/*.so; do
        [ -f "$so" ] || continue
        cp "$so" "$LIBRARY_CACHE_DIR/deps/"
        procmacro_count=$((procmacro_count + 1))
    done

    rm -rf "$src_dir"

    info "Library cache built at ${LIBRARY_CACHE_DIR}"
    info "  Target rlibs: ${rlib_count}, host proc-macros: ${procmacro_count}"
}

# ─── System user ──────────────────────────────────────────────────────────────

create_user() {
    step "Setting up service user"

    if getent group "$SERVICE_GROUP" > /dev/null 2>&1; then
        info "Group '${SERVICE_GROUP}' already exists"
    else
        groupadd --system "$SERVICE_GROUP" \
            || die $EXIT_USER_CREATION "Failed to create group '${SERVICE_GROUP}'."
    fi

    if getent passwd "$SERVICE_USER" > /dev/null 2>&1; then
        info "User '${SERVICE_USER}' already exists"
    else
        useradd --system \
            --gid "$SERVICE_GROUP" \
            --no-create-home \
            --shell /usr/sbin/nologin \
            "$SERVICE_USER" \
            || die $EXIT_USER_CREATION "Failed to create user '${SERVICE_USER}'."
        info "Created system user '${SERVICE_USER}'"
    fi

    # Data and log directories
    mkdir -p "$DATA_DIR" "$LOG_DIR"
    chown "$SERVICE_USER:$SERVICE_GROUP" "$DATA_DIR" "$LOG_DIR"
    chmod 750 "$DATA_DIR" "$LOG_DIR"
}

# ─── Configuration ────────────────────────────────────────────────────────────

prompt_or_env() {
    local var_name="$1"
    local prompt_text="$2"
    local default_value="${3:-}"
    local required="${4:-false}"

    # If already set in the environment, use it
    eval "local current_val=\"\${${var_name}:-}\""
    if [ -n "$current_val" ]; then
        echo "$current_val"
        return
    fi

    # Non-interactive mode: use default or fail if required
    if [ "${RUNTARA_NONINTERACTIVE:-0}" = "1" ]; then
        if [ -n "$default_value" ]; then
            echo "$default_value"
            return
        elif [ "$required" = "true" ]; then
            die $EXIT_CONFIG_WRITE "${var_name} is required in non-interactive mode. Export it before running the installer."
        fi
        echo ""
        return
    fi

    # Interactive prompt
    if [ -n "$default_value" ]; then
        printf "${BOLD}%s${NC} [${default_value}]: " "$prompt_text"
    else
        printf "${BOLD}%s${NC}: " "$prompt_text"
    fi

    read -r input
    if [ -n "$input" ]; then
        echo "$input"
    elif [ -n "$default_value" ]; then
        echo "$default_value"
    elif [ "$required" = "true" ]; then
        die $EXIT_CONFIG_WRITE "${var_name} is required."
    else
        echo ""
    fi
}

collect_config() {
    step "Configuring runtara"

    if [ "${RUNTARA_NONINTERACTIVE:-0}" != "1" ]; then
        printf "\n  Runtara requires a running PostgreSQL (16+) and Valkey (7.2+) instance.\n"
        printf "  It also requires an OIDC provider for authentication.\n\n"
    fi

    # ── PostgreSQL ──
    CFG_DATABASE_URL="$(prompt_or_env "RUNTARA_DATABASE_URL" \
        "PostgreSQL URL (embedded core/environment)" \
        "postgres://runtara:password@localhost/runtara" \
        "true")"

    CFG_OBJECT_MODEL_DATABASE_URL="$(prompt_or_env "OBJECT_MODEL_DATABASE_URL" \
        "PostgreSQL URL (object model)" \
        "postgres://runtara:password@localhost/runtara_objects" \
        "true")"

    # ── Valkey ──
    CFG_VALKEY_HOST="$(prompt_or_env "VALKEY_HOST" \
        "Valkey / Redis host" \
        "127.0.0.1" \
        "true")"

    CFG_VALKEY_PORT="$(prompt_or_env "VALKEY_PORT" \
        "Valkey / Redis port" \
        "6379")"

    CFG_VALKEY_PASSWORD="$(prompt_or_env "VALKEY_PASSWORD" \
        "Valkey password (leave empty for none)" \
        "")"

    # ── OIDC ──
    CFG_JWKS_URI="$(prompt_or_env "OAUTH2_JWKS_URI" \
        "OIDC JWKS URI" \
        "" \
        "true")"

    CFG_ISSUER="$(prompt_or_env "OAUTH2_ISSUER" \
        "OIDC Issuer URL" \
        "" \
        "true")"

    CFG_AUDIENCE="$(prompt_or_env "OAUTH2_AUDIENCE" \
        "OIDC Audience (optional)" \
        "")"

    # ── Tenant ──
    CFG_TENANT_ID="$(prompt_or_env "TENANT_ID" \
        "Tenant / Organisation ID" \
        "" \
        "true")"

    # ── Ports ──
    CFG_SERVER_PORT="$(prompt_or_env "SERVER_PORT" \
        "HTTP API server port" \
        "7001")"
}

# ─── Connectivity checks ─────────────────────────────────────────────────────

check_postgres() {
    step "Checking PostgreSQL connectivity"

    local pg_url="$1"

    if command -v psql > /dev/null 2>&1; then
        if psql "$pg_url" -c "SELECT 1;" > /dev/null 2>&1; then
            info "PostgreSQL reachable: $pg_url"
        else
            die $EXIT_POSTGRES_CHECK \
                "Cannot connect to PostgreSQL at: $pg_url" \
                "Ensure the database is running, the user/password are correct, and the database exists." \
                "  Hint: createdb runtara && createuser runtara"
        fi
    else
        warn "psql not found — skipping PostgreSQL connectivity check"
    fi
}

check_valkey() {
    step "Checking Valkey / Redis connectivity"

    local host="$1"
    local port="$2"
    local password="${3:-}"
    local cli_cmd=""

    if command -v valkey-cli > /dev/null 2>&1; then
        cli_cmd="valkey-cli"
    elif command -v redis-cli > /dev/null 2>&1; then
        cli_cmd="redis-cli"
    else
        warn "Neither valkey-cli nor redis-cli found — skipping connectivity check"
        return
    fi

    local pong
    if [ -n "$password" ]; then
        pong="$($cli_cmd -h "$host" -p "$port" -a "$password" PING 2>/dev/null)" || true
    else
        pong="$($cli_cmd -h "$host" -p "$port" PING 2>/dev/null)" || true
    fi

    if [ "$pong" = "PONG" ]; then
        info "Valkey reachable at ${host}:${port}"
    else
        die $EXIT_VALKEY_CHECK \
            "Cannot connect to Valkey/Redis at ${host}:${port}." \
            "Ensure the Valkey instance is running and accessible."
    fi
}

# ─── Configuration file ──────────────────────────────────────────────────────

write_config() {
    step "Writing configuration"

    mkdir -p "$CONFIG_DIR"
    chown root:"$SERVICE_GROUP" "$CONFIG_DIR"
    chmod 750 "$CONFIG_DIR"

    local conf_file="$CONFIG_DIR/runtara-server.conf"
    if [ ! -f "$conf_file" ] || [ "${RUNTARA_FORCE_CONFIG:-0}" = "1" ]; then
        cat > "$conf_file" <<CONFEOF
# Runtara Server Configuration
# Generated by install.sh on $(date -u +"%Y-%m-%dT%H:%M:%SZ")

# Tenant / Organisation ID (required)
TENANT_ID=${CFG_TENANT_ID}

# HTTP API server
SERVER_HOST=0.0.0.0
SERVER_PORT=${CFG_SERVER_PORT}

# PostgreSQL — object model database (required)
OBJECT_MODEL_DATABASE_URL=${CFG_OBJECT_MODEL_DATABASE_URL}

# PostgreSQL — embedded runtara-core / runtara-environment (required)
RUNTARA_DATABASE_URL=${CFG_DATABASE_URL}

# Valkey / Redis (required)
VALKEY_HOST=${CFG_VALKEY_HOST}
VALKEY_PORT=${CFG_VALKEY_PORT}
$([ -n "$CFG_VALKEY_PASSWORD" ] && echo "VALKEY_PASSWORD=${CFG_VALKEY_PASSWORD}" || echo "# VALKEY_PASSWORD=")

# OIDC / JWT authentication (required)
OAUTH2_JWKS_URI=${CFG_JWKS_URI}
OAUTH2_ISSUER=${CFG_ISSUER}
$([ -n "$CFG_AUDIENCE" ] && echo "OAUTH2_AUDIENCE=${CFG_AUDIENCE}" || echo "# OAUTH2_AUDIENCE=")

# Wasmtime path
WASMTIME_PATH=${INSTALL_DIR}/wasmtime

# Workflow stdlib library cache (built from source during install)
RUNTARA_WASM_LIBRARY_DIR=${LIBRARY_CACHE_DIR}

# Data directory
DATA_DIR=${DATA_DIR}

# Logging
RUST_LOG=runtara_server=info,runtara_core=info,runtara_environment=info
CONFEOF
        info "Wrote ${conf_file}"
    else
        info "Keeping existing ${conf_file} (set RUNTARA_FORCE_CONFIG=1 to overwrite)"
    fi
    chown root:"$SERVICE_GROUP" "$conf_file"
    chmod 640 "$conf_file"
}

# ─── SELinux ──────────────────────────────────────────────────────────────────

configure_selinux() {
    # Only relevant on RHEL-family when enforcing
    if [ "$OS_FAMILY" != "rhel" ]; then
        return
    fi

    if ! command -v getenforce > /dev/null 2>&1; then
        return
    fi

    local mode
    mode="$(getenforce 2>/dev/null || echo Disabled)"
    if [ "$mode" = "Enforcing" ]; then
        step "Configuring SELinux contexts"

        if command -v semanage > /dev/null 2>&1 && command -v restorecon > /dev/null 2>&1; then
            # Allow binaries to run as a service
            semanage fcontext -a -t bin_t "${INSTALL_DIR}/runtara-server" 2>/dev/null || true
            semanage fcontext -a -t bin_t "${INSTALL_DIR}/wasmtime" 2>/dev/null || true
            restorecon -v "${INSTALL_DIR}/runtara-server" "${INSTALL_DIR}/wasmtime"

            # Allow data directory access
            semanage fcontext -a -t var_lib_t "${DATA_DIR}(/.*)?" 2>/dev/null || true
            restorecon -Rv "$DATA_DIR"

            # Allow config directory access
            semanage fcontext -a -t etc_t "${CONFIG_DIR}(/.*)?" 2>/dev/null || true
            restorecon -Rv "$CONFIG_DIR"

            info "SELinux file contexts applied"
        else
            warn "SELinux is enforcing but semanage/restorecon not found."
            warn "Install policycoreutils-python-utils: ${PKG_MGR} install -y policycoreutils-python-utils"
            warn "Then re-run this installer."
        fi
    else
        info "SELinux is ${mode} — no action needed"
    fi
}

# ─── Systemd service ─────────────────────────────────────────────────────────

install_service() {
    step "Installing systemd service"

    cat > "$SYSTEMD_DIR/runtara-server.service" <<SVCEOF
[Unit]
Description=Runtara Server
After=network.target postgresql.service
Documentation=https://runtara.com/docs

[Service]
Type=simple
User=${SERVICE_USER}
Group=${SERVICE_GROUP}
EnvironmentFile=${CONFIG_DIR}/runtara-server.conf
ExecStart=${INSTALL_DIR}/runtara-server
Restart=on-failure
RestartSec=5

# Hardening
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=${DATA_DIR} ${LOG_DIR}

[Install]
WantedBy=multi-user.target
SVCEOF

    chmod 644 "$SYSTEMD_DIR/runtara-server.service"

    systemctl daemon-reload \
        || die $EXIT_SERVICE_SETUP "Failed to reload systemd daemon."

    systemctl enable runtara-server \
        || die $EXIT_SERVICE_SETUP "Failed to enable runtara-server service."

    info "Systemd unit installed and enabled"
}

# ─── Start service ───────────────────────────────────────────────────────────

start_service() {
    step "Starting runtara-server"

    info "Starting runtara-server (migrations run automatically on first start)"
    systemctl start runtara-server \
        || die $EXIT_SERVICE_SETUP "Failed to start runtara-server. Check: journalctl -u runtara-server -n 50"

    info "Service started"
}

# ─── Summary ──────────────────────────────────────────────────────────────────

print_summary() {
    echo ""
    printf '%s  Runtara v%s installed successfully!%s\n' "${GREEN}${BOLD}" "$VERSION" "$NC"
    echo ""
    echo "  Binary:         ${INSTALL_DIR}/runtara-server"
    echo "  Wasmtime:       ${INSTALL_DIR}/wasmtime"
    echo "  Stdlib cache:   ${LIBRARY_CACHE_DIR}/"
    echo "  Config:         ${CONFIG_DIR}/runtara-server.conf"
    echo "  Data:           ${DATA_DIR}/"
    echo "  Logs:           journalctl -u runtara-server"
    echo ""
    echo "  Manage service:"
    echo "    systemctl status  runtara-server"
    echo "    systemctl restart runtara-server"
    echo "    journalctl -fu runtara-server"
    echo ""
    echo "  Rust toolchain:  source /etc/profile.d/rust.sh"
    echo "  Compile scenario: cargo build --target wasm32-wasip2 --release"
    echo ""
}

# ─── Main ─────────────────────────────────────────────────────────────────────

main() {
    printf '\n%s  Runtara Installer%s\n' "${BOLD}" "$NC"
    echo "  https://runtara.com"
    echo ""

    # Must run as root
    if [ "$(id -u)" -ne 0 ]; then
        die 1 "This script must be run as root. Try: sudo sh install.sh"
    fi

    detect_platform
    resolve_version
    install_system_deps
    install_rust
    install_wasmtime
    install_binary
    build_library_cache
    create_user
    collect_config
    check_postgres "$CFG_DATABASE_URL"
    check_valkey "$CFG_VALKEY_HOST" "$CFG_VALKEY_PORT" "$CFG_VALKEY_PASSWORD"
    write_config
    configure_selinux
    install_service
    start_service
    print_summary
}

main "$@"
