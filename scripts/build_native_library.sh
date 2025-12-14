#!/bin/bash
# Build the native library cache for workflow compilation
#
# This script compiles runtara-workflow-stdlib and its dependencies
# to the musl target for static linking, then copies the artifacts
# to the library cache directory.
#
# Proc-macros (like serde_derive) must be HOST binaries, not target binaries.
# So we build the target rlibs separately and copy host proc-macros.
#
# Usage: ./scripts/build_native_library.sh

set -e

# Configuration
TARGET="${TARGET:-x86_64-unknown-linux-musl}"
DATA_DIR="${DATA_DIR:-.data}"
CACHE_DIR="${DATA_DIR}/library_cache/native"
DEPS_DIR="${CACHE_DIR}/deps"

echo "Building native library cache for target: ${TARGET}"
echo "Output directory: ${CACHE_DIR}"
echo ""

# Ensure musl target is installed
if ! rustup target list --installed | grep -q "${TARGET}"; then
    echo "Installing target ${TARGET}..."
    rustup target add "${TARGET}"
fi

# Clean previous cache
rm -rf "${CACHE_DIR}"
mkdir -p "${DEPS_DIR}"

# First, build for host to get proc-macro .so files
echo "Building host proc-macros..."
cargo build -p runtara-workflow-stdlib --release

# Build the library for the target
echo "Building runtara-workflow-stdlib for target ${TARGET}..."
cargo build -p runtara-workflow-stdlib --target "${TARGET}" --release

# Find the build output directories
BUILD_DIR="target/${TARGET}/release"
DEPS_BUILD_DIR="${BUILD_DIR}/deps"
HOST_DEPS_DIR="target/release/deps"

if [ ! -d "${DEPS_BUILD_DIR}" ]; then
    echo "Error: Build deps directory not found at ${DEPS_BUILD_DIR}"
    exit 1
fi

# Copy the main library
echo "Copying main library..."
cp "${BUILD_DIR}/libruntara_workflow_stdlib.rlib" "${CACHE_DIR}/"

# Copy all dependency rlibs from target build (except runtara_workflow_stdlib which is the main lib)
echo "Copying dependency rlibs (from target build)..."
find "${DEPS_BUILD_DIR}" -name "*.rlib" ! -name "*runtara_workflow_stdlib*" -exec cp {} "${DEPS_DIR}/" \;

# Copy host proc-macro dylibs (.so on Linux, .dylib on macOS)
# These need to be HOST binaries since they run during compilation
echo "Copying proc-macro libraries (from host build)..."
if [ -d "${HOST_DEPS_DIR}" ]; then
    # Copy .so files (Linux)
    for so_file in "${HOST_DEPS_DIR}"/*.so; do
        if [ -f "$so_file" ]; then
            cp "$so_file" "${DEPS_DIR}/"
        fi
    done
    # Copy .dylib files (macOS)
    for dylib_file in "${HOST_DEPS_DIR}"/*.dylib; do
        if [ -f "$dylib_file" ]; then
            cp "$dylib_file" "${DEPS_DIR}/"
        fi
    done
fi

# Count what we have
RLIB_COUNT=$(find "${DEPS_DIR}" -name "*.rlib" | wc -l)
SO_COUNT=$(find "${DEPS_DIR}" -name "*.so" 2>/dev/null | wc -l || echo "0")
DYLIB_COUNT=$(find "${DEPS_DIR}" -name "*.dylib" 2>/dev/null | wc -l || echo "0")

echo ""
echo "Library cache built successfully!"
echo "  Location: ${CACHE_DIR}"
echo "  Main library: libruntara_workflow_stdlib.rlib"
echo "  Dependency rlibs: ${RLIB_COUNT}"
echo "  Proc-macro .so: ${SO_COUNT}"
echo "  Proc-macro .dylib: ${DYLIB_COUNT}"
echo ""

# Verify serde_derive is present
if ls "${DEPS_DIR}"/libserde_derive*.so 2>/dev/null || ls "${DEPS_DIR}"/libserde_derive*.dylib 2>/dev/null; then
    echo "serde_derive proc-macro: PRESENT"
else
    echo "WARNING: serde_derive proc-macro NOT FOUND"
    echo "This may cause compilation issues for workflows using Serialize/Deserialize"
fi
