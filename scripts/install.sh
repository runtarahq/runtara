#!/usr/bin/env bash
# Runtara Installer
#
# Installs Runtara from a pre-built hermetic bundle.
#
# Usage:
#   # From GitHub releases (latest):
#   curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | sh
#
#   # From a local bundle tarball:
#   ./scripts/install-bundle.sh --bundle /path/to/runtara-1.6.3-aarch64-darwin.tar.gz
#
#   # From a local extracted bundle directory:
#   ./scripts/install-bundle.sh --bundle-dir /path/to/runtara-1.6.3-aarch64-darwin
#
#   # User-mode install (no sudo):
#   ./scripts/install-bundle.sh --user --bundle-dir /path/to/bundle
#
#   # Non-interactive:
#   RUNTARA_NONINTERACTIVE=1 RUNTARA_DATABASE_URL=postgres://... ./scripts/install-bundle.sh --bundle-dir ...

set -eu

# ─── Exit codes ──────────────────────────────────────────────────────────────

EXIT_UNSUPPORTED_OS=10
EXIT_UNSUPPORTED_ARCH=11
EXIT_MISSING_DEPS=12
EXIT_DOWNLOAD=40
EXIT_CHECKSUM=41
EXIT_USER_CREATION=60
EXIT_SERVICE_SETUP=70
EXIT_CONFIG_WRITE=80

# ─── Defaults ────────────────────────────────────────────────────────────────

GITHUB_REPO="runtarahq/runtara"
INSTALL_MODE=""  # "system" or "user", auto-detected if not set
BUNDLE_TARBALL=""
BUNDLE_DIR=""
SKIP_SERVICE=0
DO_UNINSTALL=0
DO_PURGE=0
DO_RUN=0

# ─── Colour helpers ──────────────────────────────────────────────────────────

if [ -t 1 ]; then
    RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
    BLUE='\033[0;34m'; BOLD='\033[1m'; NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' BLUE='' BOLD='' NC=''
fi

info()  { printf "${GREEN}[+]${NC} %s\n" "$*"; }
warn()  { printf "${YELLOW}[!]${NC} %s\n" "$*"; }
err()   { printf "${RED}[x]${NC} %s\n" "$*" >&2; }
step()  { printf "\n${BOLD}${BLUE}==> %s${NC}\n" "$*"; }

die() {
    local code=$1; shift
    err "$@"
    exit "$code"
}

# ─── Parse arguments ────────────────────────────────────────────────────────

while [ $# -gt 0 ]; do
    case "$1" in
        --user)          INSTALL_MODE="user"; shift ;;
        --system)        INSTALL_MODE="system"; shift ;;
        --bundle)        BUNDLE_TARBALL="$2"; shift 2 ;;
        --bundle=*)      BUNDLE_TARBALL="${1#*=}"; shift ;;
        --bundle-dir)    BUNDLE_DIR="$2"; shift 2 ;;
        --bundle-dir=*)  BUNDLE_DIR="${1#*=}"; shift ;;
        --skip-service)  SKIP_SERVICE=1; shift ;;
        --run)           DO_RUN=1; SKIP_SERVICE=1; shift ;;
        --uninstall)     DO_UNINSTALL=1; shift ;;
        --purge)         DO_PURGE=1; shift ;;
        --version)       RUNTARA_VERSION="$2"; shift 2 ;;
        --version=*)     RUNTARA_VERSION="${1#*=}"; shift ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# ─── Detect platform ────────────────────────────────────────────────────────

detect_platform() {
    ARCH="$(uname -m)"
    case "$ARCH" in
        x86_64|amd64)  ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *) die $EXIT_UNSUPPORTED_ARCH "Unsupported architecture: $ARCH" ;;
    esac

    OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
    case "$OS" in
        linux|darwin) ;;
        *) die $EXIT_UNSUPPORTED_OS "Unsupported OS: $OS. Supported: Linux, macOS." ;;
    esac

    info "Detected: ${OS} ${ARCH}"
}

# ─── Resolve install mode and paths ─────────────────────────────────────────

resolve_paths() {
    # Auto-detect mode if not set
    if [ -z "$INSTALL_MODE" ]; then
        if [ "$(id -u)" -eq 0 ]; then
            INSTALL_MODE="system"
        else
            INSTALL_MODE="user"
        fi
    fi

    if [ "$INSTALL_MODE" = "system" ]; then
        RUNTARA_DIR="/opt/runtara"
        CONFIG_DIR="/etc/runtara"
        DATA_DIR="/var/lib/runtara"
        LOG_DIR="/var/log/runtara"
        SYMLINK_DIR="/usr/local/bin"
        SERVICE_USER="runtara"
        SERVICE_GROUP="runtara"
    else
        RUNTARA_DIR="${HOME}/.runtara"
        CONFIG_DIR="${HOME}/.config/runtara"
        DATA_DIR="${HOME}/.local/share/runtara"
        LOG_DIR=""
        SYMLINK_DIR=""
        SERVICE_USER=""
        SERVICE_GROUP=""
    fi

    if [ "$OS" = "darwin" ] && [ "$INSTALL_MODE" = "user" ]; then
        CONFIG_DIR="${HOME}/Library/Application Support/runtara"
        DATA_DIR="${HOME}/Library/Application Support/runtara/data"
        LOG_DIR="${HOME}/Library/Logs/runtara"
    fi

    info "Install mode: ${INSTALL_MODE}"
    info "Bundle dir:   ${RUNTARA_DIR}"
    info "Config dir:   ${CONFIG_DIR}"
    info "Data dir:     ${DATA_DIR}"
}

# ─── Uninstall ───────────────────────────────────────────────────────────────

do_uninstall() {
    step "Uninstalling Runtara"

    # Stop service
    if [ "$OS" = "linux" ]; then
        if [ "$INSTALL_MODE" = "system" ]; then
            systemctl stop runtara-server 2>/dev/null || true
            systemctl disable runtara-server 2>/dev/null || true
            rm -f /etc/systemd/system/runtara-server.service
            systemctl daemon-reload 2>/dev/null || true
        else
            systemctl --user stop runtara-server 2>/dev/null || true
            systemctl --user disable runtara-server 2>/dev/null || true
            rm -f "${HOME}/.config/systemd/user/runtara-server.service"
            systemctl --user daemon-reload 2>/dev/null || true
        fi
    elif [ "$OS" = "darwin" ]; then
        if [ "$INSTALL_MODE" = "system" ]; then
            launchctl bootout system /Library/LaunchDaemons/com.runtara.server.plist 2>/dev/null || true
            rm -f /Library/LaunchDaemons/com.runtara.server.plist
        else
            launchctl bootout "gui/$(id -u)" "${HOME}/Library/LaunchAgents/com.runtara.server.plist" 2>/dev/null || true
            rm -f "${HOME}/Library/LaunchAgents/com.runtara.server.plist"
        fi
    fi

    # Remove bundle
    if [ -d "$RUNTARA_DIR" ]; then
        rm -rf "$RUNTARA_DIR"
        info "Removed ${RUNTARA_DIR}"
    fi

    # Remove symlinks
    if [ -n "$SYMLINK_DIR" ]; then
        rm -f "${SYMLINK_DIR}/runtara-server"
    fi

    # Purge config + data
    if [ "$DO_PURGE" = "1" ]; then
        if [ -d "$CONFIG_DIR" ]; then
            rm -rf "$CONFIG_DIR"
            info "Purged ${CONFIG_DIR}"
        fi
        if [ -d "$DATA_DIR" ]; then
            rm -rf "$DATA_DIR"
            info "Purged ${DATA_DIR}"
        fi
    else
        info "Config (${CONFIG_DIR}) and data (${DATA_DIR}) preserved. Use --purge to remove."
    fi

    info "Runtara uninstalled."
    exit 0
}

# ─── Download or locate the bundle ──────────────────────────────────────────

resolve_bundle() {
    # If --bundle-dir was given, use it directly
    if [ -n "$BUNDLE_DIR" ]; then
        if [ ! -d "$BUNDLE_DIR" ]; then
            die $EXIT_DOWNLOAD "Bundle directory not found: ${BUNDLE_DIR}"
        fi
        info "Using local bundle: ${BUNDLE_DIR}"
        return
    fi

    # If --bundle tarball was given, extract it
    if [ -n "$BUNDLE_TARBALL" ]; then
        if [ ! -f "$BUNDLE_TARBALL" ]; then
            die $EXIT_DOWNLOAD "Bundle tarball not found: ${BUNDLE_TARBALL}"
        fi
        step "Extracting bundle tarball"
        local tmp_extract
        tmp_extract="$(mktemp -d)"
        tar xzf "$BUNDLE_TARBALL" -C "$tmp_extract"
        # The tarball contains a single directory
        BUNDLE_DIR="$(find "$tmp_extract" -mindepth 1 -maxdepth 1 -type d | head -1)"
        if [ -z "$BUNDLE_DIR" ] || [ ! -f "$BUNDLE_DIR/VERSION" ]; then
            die $EXIT_DOWNLOAD "Invalid bundle tarball: no VERSION file found inside"
        fi
        info "Extracted to: ${BUNDLE_DIR}"
        return
    fi

    # Otherwise, download from GitHub
    step "Resolving latest Runtara version"
    if [ -z "${RUNTARA_VERSION:-}" ]; then
        RUNTARA_VERSION="$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" \
            | sed -n 's/.*"tag_name":[[:space:]]*"v\([^"]*\)".*/\1/p')"
        if [ -z "$RUNTARA_VERSION" ]; then
            die $EXIT_DOWNLOAD "Failed to resolve latest version from GitHub."
        fi
    fi
    info "Target version: ${RUNTARA_VERSION}"

    # Check if already installed at this version
    if [ -f "${RUNTARA_DIR}/VERSION" ]; then
        local installed
        installed="$(cat "${RUNTARA_DIR}/VERSION")"
        if [ "$installed" = "$RUNTARA_VERSION" ]; then
            info "Runtara v${RUNTARA_VERSION} is already installed. Nothing to do."
            exit 0
        fi
        info "Upgrading from v${installed} to v${RUNTARA_VERSION}"
    fi

    local tarball="runtara-${RUNTARA_VERSION}-${ARCH}-${OS}.tar.gz"
    local base_url="https://github.com/${GITHUB_REPO}/releases/download/v${RUNTARA_VERSION}"

    step "Downloading ${tarball}"
    local tmp_dl
    tmp_dl="$(mktemp -d)"
    curl -fSL -o "${tmp_dl}/${tarball}" "${base_url}/${tarball}" \
        || die $EXIT_DOWNLOAD "Failed to download ${tarball}. Check https://github.com/${GITHUB_REPO}/releases/tag/v${RUNTARA_VERSION}"

    # Verify checksum
    info "Verifying checksum"
    if curl -fSL -o "${tmp_dl}/${tarball}.sha256" "${base_url}/${tarball}.sha256" 2>/dev/null; then
        local sha_cmd="sha256sum"
        if ! command -v sha256sum > /dev/null 2>&1; then
            sha_cmd="shasum -a 256"
        fi
        (cd "$tmp_dl" && $sha_cmd -c "${tarball}.sha256") \
            || die $EXIT_CHECKSUM "Checksum verification failed."
    else
        warn "No checksum file found, skipping verification"
    fi

    # Extract
    step "Extracting"
    tar xzf "${tmp_dl}/${tarball}" -C "$tmp_dl"
    BUNDLE_DIR="$(find "$tmp_dl" -mindepth 1 -maxdepth 1 -type d | head -1)"
    if [ -z "$BUNDLE_DIR" ] || [ ! -f "$BUNDLE_DIR/VERSION" ]; then
        die $EXIT_DOWNLOAD "Invalid bundle: no VERSION file found"
    fi
    info "Extracted to: ${BUNDLE_DIR}"
}

# ─── Install the bundle ─────────────────────────────────────────────────────

install_bundle() {
    step "Installing bundle"

    local new_dir="${RUNTARA_DIR}.new"
    rm -rf "$new_dir"
    cp -R "$BUNDLE_DIR" "$new_dir"

    # Atomic swap
    if [ -d "$RUNTARA_DIR" ]; then
        info "Stopping service for upgrade"
        stop_service 2>/dev/null || true
        mv "$RUNTARA_DIR" "${RUNTARA_DIR}.old"
    fi

    mv "$new_dir" "$RUNTARA_DIR"
    info "Installed to ${RUNTARA_DIR}"

    # Symlinks
    if [ -n "$SYMLINK_DIR" ]; then
        ln -sf "${RUNTARA_DIR}/bin/runtara-server" "${SYMLINK_DIR}/runtara-server"
        info "Symlinked runtara-server → ${SYMLINK_DIR}/runtara-server"
    fi

    # Create data/log/config directories
    mkdir -p "$CONFIG_DIR" "$DATA_DIR"
    if [ -n "$LOG_DIR" ]; then
        mkdir -p "$LOG_DIR"
    fi

    # Ownership (system mode)
    if [ "$INSTALL_MODE" = "system" ] && [ -n "$SERVICE_USER" ]; then
        create_service_user
        chown -R "${SERVICE_USER}:${SERVICE_GROUP}" "$DATA_DIR"
        if [ -n "$LOG_DIR" ]; then
            chown -R "${SERVICE_USER}:${SERVICE_GROUP}" "$LOG_DIR"
        fi
    fi

    # Clean up old bundle on success
    rm -rf "${RUNTARA_DIR}.old" 2>/dev/null || true

    info "Bundle installed"
}

# ─── Service user (system mode) ─────────────────────────────────────────────

create_service_user() {
    if [ "$OS" = "linux" ]; then
        if ! getent group "$SERVICE_GROUP" > /dev/null 2>&1; then
            groupadd --system "$SERVICE_GROUP"
        fi
        if ! getent passwd "$SERVICE_USER" > /dev/null 2>&1; then
            useradd --system --gid "$SERVICE_GROUP" --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
            info "Created system user '${SERVICE_USER}'"
        fi
    elif [ "$OS" = "darwin" ]; then
        # macOS: create _runtara user via dscl
        if ! dscl . -read /Users/_runtara > /dev/null 2>&1; then
            local max_uid
            max_uid=$(dscl . -list /Users UniqueID | awk '{print $2}' | sort -n | tail -1)
            local new_uid=$((max_uid + 1))
            dscl . -create /Users/_runtara
            dscl . -create /Users/_runtara UserShell /usr/bin/false
            dscl . -create /Users/_runtara UniqueID "$new_uid"
            dscl . -create /Users/_runtara PrimaryGroupID 20
            dscl . -create /Users/_runtara RealName "Runtara Service"
            info "Created macOS service user '_runtara'"
            SERVICE_USER="_runtara"
        fi
    fi
}

# ─── Configuration ───────────────────────────────────────────────────────────

write_config() {
    local conf_file="${CONFIG_DIR}/runtara-server.conf"

    if [ -f "$conf_file" ]; then
        info "Keeping existing config: ${conf_file}"
        return
    fi

    step "Writing configuration"
    info "Config file: ${conf_file}"

    # Use the same prompt-or-env logic from the old installer
    # but simplified: if RUNTARA_NONINTERACTIVE=1, use defaults/env vars

    local db_url="${RUNTARA_DATABASE_URL:-postgres://runtara:password@localhost/runtara}"
    local obj_db_url="${OBJECT_MODEL_DATABASE_URL:-postgres://runtara:password@localhost/runtara_objects}"
    local valkey_host="${VALKEY_HOST:-127.0.0.1}"
    local valkey_port="${VALKEY_PORT:-6379}"
    local valkey_pass="${VALKEY_PASSWORD:-}"
    local jwks_uri="${OAUTH2_JWKS_URI:-}"
    local issuer="${OAUTH2_ISSUER:-}"
    local audience="${OAUTH2_AUDIENCE:-}"
    local tenant_id="${TENANT_ID:-}"
    local server_port="${SERVER_PORT:-7001}"

    cat > "$conf_file" <<CONFEOF
# Runtara Server Configuration
# Generated by install-bundle.sh on $(date -u +"%Y-%m-%dT%H:%M:%SZ")

TENANT_ID=${tenant_id}
SERVER_HOST=0.0.0.0
SERVER_PORT=${server_port}

OBJECT_MODEL_DATABASE_URL=${obj_db_url}
RUNTARA_DATABASE_URL=${db_url}

VALKEY_HOST=${valkey_host}
VALKEY_PORT=${valkey_port}
$([ -n "$valkey_pass" ] && echo "VALKEY_PASSWORD=${valkey_pass}" || echo "# VALKEY_PASSWORD=")

$([ -n "$jwks_uri" ] && echo "OAUTH2_JWKS_URI=${jwks_uri}" || echo "# OAUTH2_JWKS_URI=")
$([ -n "$issuer" ] && echo "OAUTH2_ISSUER=${issuer}" || echo "# OAUTH2_ISSUER=")
$([ -n "$audience" ] && echo "OAUTH2_AUDIENCE=${audience}" || echo "# OAUTH2_AUDIENCE=")

WASMTIME_PATH=${RUNTARA_DIR}/bin/wasmtime
RUNTARA_WASM_LIBRARY_DIR=${RUNTARA_DIR}/stdlib
DATA_DIR=${DATA_DIR}
RUST_LOG=runtara_server=info,runtara_core=info,runtara_environment=info
CONFEOF

    chmod 640 "$conf_file"
    info "Wrote ${conf_file}"
}

# ─── Service management ─────────────────────────────────────────────────────

stop_service() {
    if [ "$OS" = "linux" ]; then
        if [ "$INSTALL_MODE" = "system" ]; then
            systemctl stop runtara-server 2>/dev/null || true
        else
            systemctl --user stop runtara-server 2>/dev/null || true
        fi
    elif [ "$OS" = "darwin" ]; then
        if [ "$INSTALL_MODE" = "system" ]; then
            launchctl bootout system /Library/LaunchDaemons/com.runtara.server.plist 2>/dev/null || true
        else
            launchctl bootout "gui/$(id -u)" "${HOME}/Library/LaunchAgents/com.runtara.server.plist" 2>/dev/null || true
        fi
    fi
}

install_service() {
    if [ "$SKIP_SERVICE" = "1" ]; then
        info "Skipping service install (--skip-service)"
        return
    fi

    step "Installing service"

    # PATH for the service: bundled toolchain first, then system paths
    local svc_path="${RUNTARA_DIR}/toolchain/bin:${RUNTARA_DIR}/bin:/usr/local/bin:/usr/bin:/bin"

    if [ "$OS" = "linux" ]; then
        install_systemd_service "$svc_path"
    elif [ "$OS" = "darwin" ]; then
        install_launchd_service "$svc_path"
    fi
}

install_systemd_service() {
    local svc_path="$1"
    local unit_dir unit_file sctl_args

    if [ "$INSTALL_MODE" = "system" ]; then
        unit_dir="/etc/systemd/system"
        sctl_args=""
    else
        unit_dir="${HOME}/.config/systemd/user"
        mkdir -p "$unit_dir"
        sctl_args="--user"
    fi

    unit_file="${unit_dir}/runtara-server.service"

    local user_lines=""
    if [ "$INSTALL_MODE" = "system" ] && [ -n "$SERVICE_USER" ]; then
        user_lines="User=${SERVICE_USER}
Group=${SERVICE_GROUP}"
    fi

    cat > "$unit_file" <<SVCEOF
[Unit]
Description=Runtara Server
After=network-online.target
Wants=network-online.target
Documentation=https://runtara.com/docs

[Service]
Type=simple
${user_lines}
EnvironmentFile=${CONFIG_DIR}/runtara-server.conf
Environment="PATH=${svc_path}"
Environment="LD_LIBRARY_PATH=${RUNTARA_DIR}/toolchain/lib"
Environment="DYLD_LIBRARY_PATH=${RUNTARA_DIR}/toolchain/lib"
ExecStart=${RUNTARA_DIR}/bin/runtara-server
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes

[Install]
WantedBy=multi-user.target
SVCEOF

    chmod 644 "$unit_file"

    # shellcheck disable=SC2086
    systemctl $sctl_args daemon-reload
    # shellcheck disable=SC2086
    systemctl $sctl_args enable runtara-server

    info "Systemd unit installed: ${unit_file}"
}

install_launchd_service() {
    local svc_path="$1"
    local plist_dir plist_file

    if [ "$INSTALL_MODE" = "system" ]; then
        plist_dir="/Library/LaunchDaemons"
        plist_file="${plist_dir}/com.runtara.server.plist"
    else
        plist_dir="${HOME}/Library/LaunchAgents"
        mkdir -p "$plist_dir"
        plist_file="${plist_dir}/com.runtara.server.plist"
    fi

    local log_out="${LOG_DIR:-/tmp}/runtara-server.log"
    local log_err="${LOG_DIR:-/tmp}/runtara-server.err"

    cat > "$plist_file" <<PLISTEOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.runtara.server</string>
    <key>ProgramArguments</key>
    <array>
        <string>${RUNTARA_DIR}/bin/runtara-server</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>${svc_path}</string>
        <key>RUNTARA_CONFIG</key>
        <string>${CONFIG_DIR}/runtara-server.conf</string>
        <key>DYLD_LIBRARY_PATH</key>
        <string>${RUNTARA_DIR}/toolchain/lib</string>
    </dict>
    <key>StandardOutPath</key>
    <string>${log_out}</string>
    <key>StandardErrorPath</key>
    <string>${log_err}</string>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
PLISTEOF

    info "LaunchAgent plist installed: ${plist_file}"
}

start_service() {
    if [ "$SKIP_SERVICE" = "1" ]; then
        return
    fi

    step "Starting runtara-server"

    if [ "$OS" = "linux" ]; then
        if [ "$INSTALL_MODE" = "system" ]; then
            systemctl start runtara-server
        else
            systemctl --user start runtara-server
        fi
    elif [ "$OS" = "darwin" ]; then
        if [ "$INSTALL_MODE" = "system" ]; then
            launchctl bootstrap system /Library/LaunchDaemons/com.runtara.server.plist
        else
            launchctl bootstrap "gui/$(id -u)" "${HOME}/Library/LaunchAgents/com.runtara.server.plist"
        fi
    fi

    info "Service started"
}

# ─── Summary ─────────────────────────────────────────────────────────────────

print_summary() {
    local version
    version="$(cat "${RUNTARA_DIR}/VERSION")"

    echo ""
    printf '%s  Runtara v%s installed successfully!%s\n' "${GREEN}${BOLD}" "$version" "$NC"
    echo ""
    echo "  Binary:     ${RUNTARA_DIR}/bin/runtara-server"
    echo "  Toolchain:  ${RUNTARA_DIR}/toolchain/bin/rustc"
    echo "  Stdlib:     ${RUNTARA_DIR}/stdlib/"
    echo "  Wasmtime:   ${RUNTARA_DIR}/bin/wasmtime"
    echo "  Config:     ${CONFIG_DIR}/runtara-server.conf"
    echo "  Data:       ${DATA_DIR}/"
    echo ""

    if [ "$OS" = "linux" ]; then
        if [ "$INSTALL_MODE" = "system" ]; then
            echo "  Manage service:"
            echo "    systemctl status  runtara-server"
            echo "    systemctl restart runtara-server"
            echo "    journalctl -fu runtara-server"
        else
            echo "  Manage service:"
            echo "    systemctl --user status  runtara-server"
            echo "    systemctl --user restart runtara-server"
            echo "    journalctl --user -fu runtara-server"
        fi
    elif [ "$OS" = "darwin" ]; then
        echo "  Logs:       ${LOG_DIR:-/tmp}/runtara-server.log"
        echo "  Manage service:"
        echo "    launchctl list | grep runtara"
    fi
    echo ""
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
    printf '\n%s  Runtara Installer%s\n' "${BOLD}" "$NC"
    echo "  https://runtara.com"
    echo ""

    detect_platform
    resolve_paths

    if [ "$DO_UNINSTALL" = "1" ]; then
        do_uninstall
    fi

    resolve_bundle
    install_bundle
    write_config
    install_service
    start_service
    print_summary

    if [ "$DO_RUN" = "1" ]; then
        step "Running runtara-server in foreground"
        exec "$RUNTARA_DIR/bin/runtara-server"
    fi
}

main "$@"
