#!/bin/bash
# Run all E2E tests
#
# Prerequisites:
# - runtara-core and runtara-environment must be running (use ./start.sh)
# - Binaries must be built (cargo build --release)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo "=========================================="
echo "Runtara E2E Test Suite"
echo "=========================================="
echo ""

# Build binaries if needed
echo "Checking binaries..."
if [ ! -f "${PROJECT_ROOT}/target/release/runtara-compile" ] || \
   [ ! -f "${PROJECT_ROOT}/target/release/runtara-ctl" ]; then
    echo "Building required binaries..."
    cargo build -p runtara-workflows --bin runtara-compile --release
    cargo build -p runtara-management-sdk --bin runtara-ctl --release
fi
echo ""

# Set environment for local testing
export RUNTARA_ENVIRONMENT_ADDR="${RUNTARA_ENVIRONMENT_ADDR:-127.0.0.1:7000}"
export RUNTARA_SKIP_CERT_VERIFICATION="${RUNTARA_SKIP_CERT_VERIFICATION:-true}"

TESTS_PASSED=0
TESTS_FAILED=0
FAILED_TESTS=""

run_test() {
    local test_name="$1"
    local test_script="$2"

    echo -e "${YELLOW}Running: ${test_name}${NC}"
    echo "----------------------------------------"

    if "${test_script}"; then
        echo -e "${GREEN}PASSED: ${test_name}${NC}"
        ((TESTS_PASSED++))
    else
        echo -e "${RED}FAILED: ${test_name}${NC}"
        ((TESTS_FAILED++))
        FAILED_TESTS="${FAILED_TESTS}\n  - ${test_name}"
    fi
    echo ""
}

# Run tests
run_test "Basic Workflow" "${SCRIPT_DIR}/test_basic_workflow.sh"
run_test "Delay Workflow" "${SCRIPT_DIR}/test_delay_workflow.sh"

# Summary
echo "=========================================="
echo "Test Results"
echo "=========================================="
echo -e "${GREEN}Passed: ${TESTS_PASSED}${NC}"
echo -e "${RED}Failed: ${TESTS_FAILED}${NC}"

if [ ${TESTS_FAILED} -gt 0 ]; then
    echo -e "\nFailed tests:${FAILED_TESTS}"
    exit 1
fi

echo ""
echo -e "${GREEN}All tests passed!${NC}"
