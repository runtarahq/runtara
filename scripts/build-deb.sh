#!/bin/bash
# Build .deb package for runtara-core or runtara-environment
#
# Usage: ./scripts/build-deb.sh <package-name>
# Example: ./scripts/build-deb.sh runtara-core
#          ./scripts/build-deb.sh runtara-environment

set -e

PACKAGE_NAME="${1:-}"

if [ -z "$PACKAGE_NAME" ]; then
    echo "Usage: $0 <package-name>"
    echo "  package-name: runtara-core or runtara-environment"
    exit 1
fi

if [ "$PACKAGE_NAME" != "runtara-core" ] && [ "$PACKAGE_NAME" != "runtara-environment" ]; then
    echo "Error: Invalid package name '$PACKAGE_NAME'"
    echo "  Valid options: runtara-core, runtara-environment"
    exit 1
fi

# Get version from git tag or Cargo.toml
if [ -n "${VERSION:-}" ]; then
    DEB_VERSION="$VERSION"
elif git describe --tags --exact-match 2>/dev/null; then
    DEB_VERSION=$(git describe --tags --exact-match | sed 's/^v//')
else
    DEB_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
fi

echo "Building $PACKAGE_NAME version $DEB_VERSION"

# Paths
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
PACKAGING_DIR="$ROOT_DIR/packaging/$PACKAGE_NAME"
BUILD_DIR="$ROOT_DIR/target/deb-build/$PACKAGE_NAME"
OUTPUT_DIR="$ROOT_DIR/target"
BINARY_PATH="$ROOT_DIR/target/release/$PACKAGE_NAME"

# Check if packaging files exist
if [ ! -d "$PACKAGING_DIR" ]; then
    echo "Error: Packaging directory not found: $PACKAGING_DIR"
    exit 1
fi

# Check if binary exists
if [ ! -f "$BINARY_PATH" ]; then
    echo "Error: Binary not found: $BINARY_PATH"
    echo "Run 'cargo build --release -p $PACKAGE_NAME' first"
    exit 1
fi

# Clean and create build directory
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR/DEBIAN"
mkdir -p "$BUILD_DIR/usr/bin"
mkdir -p "$BUILD_DIR/lib/systemd/system"
mkdir -p "$BUILD_DIR/etc/runtara"

# Copy binary
cp "$BINARY_PATH" "$BUILD_DIR/usr/bin/"
chmod 755 "$BUILD_DIR/usr/bin/$PACKAGE_NAME"

# Copy systemd service
cp "$PACKAGING_DIR/$PACKAGE_NAME.service" "$BUILD_DIR/lib/systemd/system/"
chmod 644 "$BUILD_DIR/lib/systemd/system/$PACKAGE_NAME.service"

# Copy config file
if [ "$PACKAGE_NAME" = "runtara-core" ]; then
    CONFIG_FILE="core.conf"
else
    CONFIG_FILE="environment.conf"
fi
cp "$PACKAGING_DIR/$CONFIG_FILE" "$BUILD_DIR/etc/runtara/"
chmod 640 "$BUILD_DIR/etc/runtara/$CONFIG_FILE"

# Copy DEBIAN control files
sed "s/\${VERSION}/$DEB_VERSION/" "$PACKAGING_DIR/control" > "$BUILD_DIR/DEBIAN/control"
cp "$PACKAGING_DIR/conffiles" "$BUILD_DIR/DEBIAN/"
cp "$PACKAGING_DIR/postinst" "$BUILD_DIR/DEBIAN/"
cp "$PACKAGING_DIR/prerm" "$BUILD_DIR/DEBIAN/"
chmod 755 "$BUILD_DIR/DEBIAN/postinst"
chmod 755 "$BUILD_DIR/DEBIAN/prerm"

# Build .deb package
DEB_FILE="${PACKAGE_NAME}_${DEB_VERSION}_amd64.deb"
dpkg-deb --build "$BUILD_DIR" "$OUTPUT_DIR/$DEB_FILE"

echo "Successfully built: $OUTPUT_DIR/$DEB_FILE"

# Show package info
echo ""
echo "Package contents:"
dpkg-deb -c "$OUTPUT_DIR/$DEB_FILE"
