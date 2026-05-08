#!/bin/bash
# Copyright (C) 2025 SyncMyOrders Sp. z o.o.
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Release script: bump version, update Cargo.toml files and Cargo.lock, commit, tag, and push.
# The tag push triggers the release CI workflow which builds GitHub release artifacts.
#
# Usage: ./scripts/release.sh <patch|minor|major>
# Example: ./scripts/release.sh patch   # 1.3.1 -> 1.3.2

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$ROOT_DIR"

# --- Parse arguments ---
BUMP_TYPE="${1:-}"
if [[ ! "$BUMP_TYPE" =~ ^(patch|minor|major)$ ]]; then
    echo "Usage: $0 <patch|minor|major>"
    echo "Example: $0 patch"
    exit 1
fi

# --- Ensure clean working tree ---
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Error: Working tree is not clean. Commit or stash your changes first."
    exit 1
fi

# --- Read current version from latest git tag ---
LATEST_TAG=$(git tag --list 'v[0-9]*.[0-9]*.[0-9]*' --sort=-v:refname | head -1)
if [ -z "$LATEST_TAG" ]; then
    echo "Error: No version tags found (expected v*.*.* format)."
    exit 1
fi
CURRENT_VERSION="${LATEST_TAG#v}"
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"

echo "Current version: $CURRENT_VERSION"

# --- Bump version ---
case "$BUMP_TYPE" in
    major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
    minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
    patch) PATCH=$((PATCH + 1)) ;;
esac

NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"
TAG="v${NEW_VERSION}"

echo "New version:     $NEW_VERSION"
echo "Tag:             $TAG"
echo ""

# --- Check tag doesn't already exist ---
if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "Error: Tag $TAG already exists."
    exit 1
fi

# --- Update versions in Cargo.toml files ---
"$SCRIPT_DIR/update-version.sh" "$NEW_VERSION"

# --- Update and verify Cargo.lock ---
echo "Updating Cargo.lock workspace package versions..."
cargo update --workspace

echo "Verifying Cargo.lock is current..."
cargo metadata --locked --format-version 1 >/dev/null

# --- Commit and tag ---
git add -A
git commit -m "Release $NEW_VERSION"
git tag "$TAG"

echo ""
echo "Created commit and tag $TAG."
echo ""
read -rp "Push commit and tag to origin? [y/N] " CONFIRM
if [[ "$CONFIRM" =~ ^[Yy]$ ]]; then
    git push origin HEAD "$TAG"
    echo "Pushed. Release CI will build GitHub release artifacts."
else
    echo "Skipped push. Run manually:"
    echo "  git push origin HEAD $TAG"
fi
