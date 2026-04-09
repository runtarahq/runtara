#!/usr/bin/env bash
# Runtara Bootstrap Installer
#
# Hosted at install.runtara.com — downloads the real install.sh from the
# matching GitHub release and executes it.
#
# Usage:
#   curl -fsSL https://install.runtara.com | sh
#   curl -fsSL https://install.runtara.com | sh -s -- --version 1.6.10
#   curl -fsSL https://install.runtara.com | sh -s -- --user

set -eu

GITHUB_REPO="runtarahq/runtara"

# ─── Colour helpers ─────────────────────────────────────────────────────────

if [ -t 1 ]; then
    GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'
    BOLD='\033[1m'; NC='\033[0m'
else
    GREEN='' YELLOW='' RED='' BOLD='' NC=''
fi

info() { printf "${GREEN}[+]${NC} %s\n" "$*"; }
warn() { printf "${YELLOW}[!]${NC} %s\n" "$*"; }
err()  { printf "${RED}[x]${NC} %s\n" "$*" >&2; }

# ─── Extract --version from args (if present) ──────────────────────────────

VERSION=""
PASSTHROUGH_ARGS=()

while [ $# -gt 0 ]; do
    case "$1" in
        --version)   VERSION="$2"; PASSTHROUGH_ARGS+=("$1" "$2"); shift 2 ;;
        --version=*) VERSION="${1#*=}"; PASSTHROUGH_ARGS+=("$1"); shift ;;
        *)           PASSTHROUGH_ARGS+=("$1"); shift ;;
    esac
done

# ─── Resolve version ───────────────────────────────────────────────────────

if [ -z "$VERSION" ]; then
    info "Resolving latest Runtara version..."
    VERSION="$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" \
        | sed -n 's/.*"tag_name":[[:space:]]*"v\([^"]*\)".*/\1/p')"
    if [ -z "$VERSION" ]; then
        err "Failed to resolve latest version from GitHub."
        exit 1
    fi
    # Inject resolved version so install.sh doesn't re-resolve
    PASSTHROUGH_ARGS+=("--version" "$VERSION")
fi

info "Runtara v${VERSION}"

# ─── Download and execute the real installer ────────────────────────────────

INSTALL_URL="https://github.com/${GITHUB_REPO}/releases/download/v${VERSION}/install.sh"

info "Fetching installer from release v${VERSION}..."

INSTALLER="$(curl -fsSL "$INSTALL_URL")" || {
    err "Failed to download install.sh from ${INSTALL_URL}"
    err "Check that release v${VERSION} exists: https://github.com/${GITHUB_REPO}/releases/tag/v${VERSION}"
    exit 1
}

exec sh -c "$INSTALLER" -- "${PASSTHROUGH_ARGS[@]}"
