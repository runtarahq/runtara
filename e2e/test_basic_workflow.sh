#!/bin/bash
# E2E Test: Basic Workflow Execution
#
# Tests the complete workflow lifecycle:
# 1. Compile a simple workflow to a native binary
# 2. Register the binary as an image with Environment
# 3. Start an instance with input
# 4. Wait for completion
# 5. Verify the output matches expected result
#
# Prerequisites:
# - runtara-core and runtara-environment must be running (use ./start.sh)
# - RUNTARA_ENVIRONMENT_ADDR and RUNTARA_SKIP_CERT_VERIFICATION should be set

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKFLOW_FILE="${SCRIPT_DIR}/workflows/simple_passthrough.json"
TENANT_ID="e2e-test"
SCENARIO_ID="simple-passthrough"
IMAGE_NAME="e2e-simple-passthrough-$(date +%s)"

# Binary paths
RUNTARA_COMPILE="${PROJECT_ROOT}/target/release/runtara-compile"
RUNTARA_CTL="${PROJECT_ROOT}/target/release/runtara-ctl"

# Environment defaults for local testing
export RUNTARA_ENVIRONMENT_ADDR="${RUNTARA_ENVIRONMENT_ADDR:-127.0.0.1:7000}"
export RUNTARA_SKIP_CERT_VERIFICATION="${RUNTARA_SKIP_CERT_VERIFICATION:-true}"
export DATA_DIR="${DATA_DIR:-.data}"

# Test input and expected output
# Wrap the actual input in {"input": ...} because workflow accesses data.input
INPUT_JSON='{"input": {"message": "Hello from E2E test", "number": 42}}'
EXPECTED_OUTPUT_FIELD="message"
EXPECTED_OUTPUT_VALUE="Hello from E2E test"

# Cleanup on exit
cleanup() {
    if [ -n "${TEMP_BINARY}" ] && [ -f "${TEMP_BINARY}" ]; then
        rm -f "${TEMP_BINARY}"
    fi
    if [ -n "${IMAGE_ID}" ]; then
        echo -e "${YELLOW}Cleaning up: deleting image ${IMAGE_ID}${NC}"
        "${RUNTARA_CTL}" delete-image "${IMAGE_ID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

print_step() {
    echo -e "${GREEN}[STEP]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

# Check prerequisites
echo "=========================================="
echo "E2E Test: Basic Workflow Execution"
echo "=========================================="
echo ""

if [ ! -f "${RUNTARA_COMPILE}" ]; then
    print_error "runtara-compile not found at ${RUNTARA_COMPILE}"
    echo "Run: cargo build -p runtara-workflows --bin runtara-compile --release"
    exit 1
fi

if [ ! -f "${RUNTARA_CTL}" ]; then
    print_error "runtara-ctl not found at ${RUNTARA_CTL}"
    echo "Run: cargo build -p runtara-management-sdk --bin runtara-ctl --release"
    exit 1
fi

if [ ! -f "${WORKFLOW_FILE}" ]; then
    print_error "Workflow file not found at ${WORKFLOW_FILE}"
    exit 1
fi

# Step 1: Check Environment health
print_step "Checking Environment health..."
if ! "${RUNTARA_CTL}" health > /dev/null 2>&1; then
    print_error "Cannot connect to runtara-environment at ${RUNTARA_ENVIRONMENT_ADDR}"
    echo "Make sure runtara-core and runtara-environment are running (use ./start.sh)"
    exit 1
fi
echo "  Environment is healthy"

# Step 2: Compile the workflow
print_step "Compiling workflow..."
TEMP_BINARY=$(mktemp)
if ! "${RUNTARA_COMPILE}" \
    --workflow "${WORKFLOW_FILE}" \
    --tenant "${TENANT_ID}" \
    --scenario "${SCENARIO_ID}" \
    --output "${TEMP_BINARY}" 2>&1; then
    print_error "Compilation failed"
    exit 1
fi
echo "  Compiled to: ${TEMP_BINARY}"
echo "  Size: $(stat -c%s "${TEMP_BINARY}" 2>/dev/null || stat -f%z "${TEMP_BINARY}") bytes"

# Step 3: Register the image
print_step "Registering image with Environment..."
IMAGE_ID=$("${RUNTARA_CTL}" register \
    --binary "${TEMP_BINARY}" \
    --tenant "${TENANT_ID}" \
    --name "${IMAGE_NAME}" \
    --description "E2E test image")

if [ -z "${IMAGE_ID}" ]; then
    print_error "Failed to register image"
    exit 1
fi
echo "  Image ID: ${IMAGE_ID}"

# Step 4: Start an instance
print_step "Starting instance..."
INSTANCE_ID=$("${RUNTARA_CTL}" start \
    --image "${IMAGE_ID}" \
    --tenant "${TENANT_ID}" \
    --input "${INPUT_JSON}")

if [ -z "${INSTANCE_ID}" ]; then
    print_error "Failed to start instance"
    exit 1
fi
echo "  Instance ID: ${INSTANCE_ID}"

# Step 5: Wait for completion
print_step "Waiting for instance completion..."
RESULT=$("${RUNTARA_CTL}" wait "${INSTANCE_ID}" --poll 200)

if [ -z "${RESULT}" ]; then
    print_error "Failed to get instance result"
    exit 1
fi

# Parse status
STATUS=$(echo "${RESULT}" | jq -r '.status')
echo "  Status: ${STATUS}"

if [ "${STATUS}" != "completed" ]; then
    print_error "Instance did not complete successfully. Status: ${STATUS}"
    echo "  Full result: ${RESULT}"
    exit 1
fi

# Step 6: Verify output
print_step "Verifying output..."
OUTPUT=$(echo "${RESULT}" | jq -r '.output')
echo "  Output: ${OUTPUT}"

# Check that input was passed through to output
# The workflow maps input to .result in the Finish step
OUTPUT_VALUE=$(echo "${OUTPUT}" | jq -r ".result.${EXPECTED_OUTPUT_FIELD}")

if [ "${OUTPUT_VALUE}" != "${EXPECTED_OUTPUT_VALUE}" ]; then
    print_error "Output mismatch!"
    echo "  Expected: ${EXPECTED_OUTPUT_VALUE}"
    echo "  Got: ${OUTPUT_VALUE}"
    exit 1
fi

echo ""
print_success "All checks passed!"
echo ""
echo "Summary:"
echo "  - Workflow compiled successfully"
echo "  - Image registered: ${IMAGE_ID}"
echo "  - Instance completed: ${INSTANCE_ID}"
echo "  - Output verified: input was correctly passed through"
echo ""
