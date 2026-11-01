#!/usr/bin/env bash
# Fetch the runtara-frontend release that runtara-server embeds (--features
# embed-ui), as pinned in crates/runtara-server/frontend.lock, into
# crates/runtara-server/frontend-dist/. The tarball's SHA-256 must match the
# lock. Does nothing when that release is already there, so build.rs can call
# it on every build and an offline build works once it has run.
#
# Usage:
#   scripts/fetch-frontend.sh
#   RUNTARA_FRONTEND_TARBALL=/path/to/runtara-frontend-X.Y.Z.tar.gz scripts/fetch-frontend.sh
#     (use a local copy of the pinned release instead of downloading it)

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK="${ROOT_DIR}/crates/runtara-server/frontend.lock"
DEST="${ROOT_DIR}/crates/runtara-server/frontend-dist"
# Next to the directory, not in it: everything in it is embedded and served.
MARKER="${DEST}.release"

lock_value() {
    sed -n "s/^$1[[:space:]]*=[[:space:]]*//p" "$LOCK" | tr -d '[:space:]'
}

VERSION="$(lock_value version)"
SHA256="$(lock_value sha256)"
if [ -z "$VERSION" ] || [ -z "$SHA256" ]; then
    echo "error: ${LOCK} must set version and sha256" >&2
    exit 1
fi

if [ -f "${DEST}/index.html" ] && [ "$(cat "$MARKER" 2>/dev/null)" = "${VERSION} ${SHA256}" ]; then
    exit 0
fi

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
TARBALL="${TMP}/runtara-frontend-${VERSION}.tar.gz"

if [ -n "${RUNTARA_FRONTEND_TARBALL:-}" ]; then
    cp "$RUNTARA_FRONTEND_TARBALL" "$TARBALL"
else
    URL="https://github.com/runtarahq/runtara-frontend/releases/download/v${VERSION}/runtara-frontend-${VERSION}.tar.gz"
    echo "Fetching runtara-frontend ${VERSION}" >&2
    curl -fsSL --retry 3 -o "$TARBALL" "$URL"
fi

ACTUAL="$(sha256_of "$TARBALL")"
if [ "$ACTUAL" != "$SHA256" ]; then
    echo "error: runtara-frontend ${VERSION} has SHA-256 ${ACTUAL}, ${LOCK} pins ${SHA256}" >&2
    exit 1
fi

rm -rf "$DEST" "$MARKER"
mkdir -p "$DEST"
tar -xzf "$TARBALL" -C "$DEST"
if [ ! -f "${DEST}/index.html" ]; then
    echo "error: runtara-frontend ${VERSION} has no index.html" >&2
    exit 1
fi
echo "${VERSION} ${SHA256}" > "$MARKER"
echo "runtara-frontend ${VERSION} in ${DEST}" >&2
