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

# Extract major.minor for version constraints
MAJOR_MINOR=$(echo "$VERSION" | sed 's/\([0-9]*\.[0-9]*\).*/\1/')

echo "Updating version to $VERSION (constraints will use $MAJOR_MINOR)"

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$ROOT_DIR"

# Update workspace version in root Cargo.toml
echo "Updating workspace version in Cargo.toml..."
sed -i "s/^version = \"[^\"]*\"/version = \"$VERSION\"/" Cargo.toml

# Update path dependency version constraints in all crate Cargo.toml files
# These are lines like: runtara-protocol = { path = "../runtara-protocol", version = "0.1" }
echo "Updating path dependency version constraints..."

CRATE_TOMLS=$(find crates -name "Cargo.toml")
for toml in $CRATE_TOMLS; do
    # Update version constraints for runtara-* path dependencies
    # Match pattern: runtara-xxx = { path = "...", version = "X.Y" }
    if grep -q 'runtara-.*path.*version' "$toml"; then
        echo "  Updating $toml"
        sed -i "s/\(runtara-[a-z-]*\s*=\s*{[^}]*version\s*=\s*\)\"[0-9.]*\"/\1\"$MAJOR_MINOR\"/" "$toml"
    fi
done

echo ""
echo "Version updated to $VERSION"
echo "Path dependency constraints updated to $MAJOR_MINOR"
echo ""

# Verify the changes
echo "Verification:"
echo "  Workspace version:"
grep -n "^version = " Cargo.toml | head -1
echo ""
echo "  Sample path dependency constraint:"
grep -h "runtara-.*version" crates/*/Cargo.toml | head -1 || echo "  (none found)"
