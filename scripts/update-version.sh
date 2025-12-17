#!/bin/bash
# Copyright (C) 2025 SyncMyOrders Sp. z o.o.
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Update version in all Cargo.toml files from git tag
#
# Usage: ./scripts/update-version.sh <version>
# Example: ./scripts/update-version.sh 1.0.21

set -euo pipefail

if [ $# -ne 1 ]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 1.0.21"
    exit 1
fi

VERSION="$1"

# Validate version format (semver)
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$ ]]; then
    echo "Error: Invalid version format. Expected semver (e.g., 1.0.21 or 1.0.21-beta.1)"
    exit 1
fi

echo "Updating version to $VERSION"

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$ROOT_DIR"

# Update workspace version in root Cargo.toml
# All crates use version.workspace = true, so we only need to update the root
echo "Updating workspace version in Cargo.toml..."
sed -i "s/^version = \"[^\"]*\"/version = \"$VERSION\"/" Cargo.toml

echo "Version updated to $VERSION"
echo ""
echo "Updated files:"
echo "  - Cargo.toml (workspace version)"
echo ""
echo "All crates inherit version from workspace.package.version"

# Verify the changes
echo ""
echo "Verification:"
grep -n "^version = " Cargo.toml | head -1
