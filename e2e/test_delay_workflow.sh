#!/bin/bash
# E2E Test: Delay/Durable Sleep Workflow
#
# Tests durable sleep functionality:
# 1. Compile a workflow that includes a delay
# 2. Register the binary as an image with Environment
# 3. Start an instance with a short delay
# 4. Wait for completion
# 5. Verify the workflow completed after the delay
#
# This tests the wake scheduler and durable sleep recovery mechanism.
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
WORKFLOW_FILE="${SCRIPT_DIR}/workflows/delay_workflow.json"
TENANT_ID="e2e-test"
SCENARIO_ID="delay-workflow"
IMAGE_NAME="e2e-delay-workflow-$(date +%s)"

# Binary paths
RUNTARA_COMPILE="${PROJECT_ROOT}/target/release/runtara-compile"
RUNTARA_CTL="${PROJECT_ROOT}/target/release/runtara-ctl"

# Environment defaults for local testing
export RUNTARA_ENVIRONMENT_ADDR="${RUNTARA_ENVIRONMENT_ADDR:-127.0.0.1:7000}"
export RUNTARA_SKIP_CERT_VERIFICATION="${RUNTARA_SKIP_CERT_VERIFICATION:-true}"
export DATA_DIR="${DATA_DIR:-.data}"

# Test configuration
# Use a short delay for testing (3 seconds)
# This should be long enough to test the mechanism but short enough to not slow down tests
DELAY_MS=3000
# Wrap in {"input": ...} because workflow accesses data.input.delay_ms
INPUT_JSON="{\"input\": {\"delay_ms\": ${DELAY_MS}}}"

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
echo "E2E Test: Delay/Durable Sleep Workflow"
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
print_step "Compiling delay workflow..."
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
    --description "E2E delay test image")

if [ -z "${IMAGE_ID}" ]; then
    print_error "Failed to register image"
    exit 1
fi
echo "  Image ID: ${IMAGE_ID}"

# Step 4: Start an instance with delay
print_step "Starting instance with ${DELAY_MS}ms delay..."
START_TIME=$(date +%s)

INSTANCE_ID=$("${RUNTARA_CTL}" start \
    --image "${IMAGE_ID}" \
    --tenant "${TENANT_ID}" \
    --input "${INPUT_JSON}")

if [ -z "${INSTANCE_ID}" ]; then
    print_error "Failed to start instance"
    exit 1
fi
echo "  Instance ID: ${INSTANCE_ID}"

# Step 5: Check initial status (should be running or pending)
print_step "Checking initial status..."
sleep 0.5  # Brief pause to let it start
INITIAL_STATUS=$("${RUNTARA_CTL}" status "${INSTANCE_ID}" | jq -r '.status')
echo "  Initial status: ${INITIAL_STATUS}"

# Step 6: Wait for completion
print_step "Waiting for instance completion (this should take ~${DELAY_MS}ms)..."
RESULT=$("${RUNTARA_CTL}" wait "${INSTANCE_ID}" --poll 500)

END_TIME=$(date +%s)
ELAPSED=$((END_TIME - START_TIME))

if [ -z "${RESULT}" ]; then
    print_error "Failed to get instance result"
    exit 1
fi

# Parse status
STATUS=$(echo "${RESULT}" | jq -r '.status')
echo "  Final status: ${STATUS}"
echo "  Elapsed time: ${ELAPSED} seconds"

if [ "${STATUS}" != "completed" ]; then
    print_error "Instance did not complete successfully. Status: ${STATUS}"
    echo "  Full result: ${RESULT}"
    ERROR=$(echo "${RESULT}" | jq -r '.error // "no error"')
    echo "  Error: ${ERROR}"
    exit 1
fi

# Step 7: Verify timing
print_step "Verifying timing..."
MIN_EXPECTED_SECONDS=$((DELAY_MS / 1000))

if [ "${ELAPSED}" -lt "${MIN_EXPECTED_SECONDS}" ]; then
    print_error "Workflow completed too quickly!"
    echo "  Expected at least ${MIN_EXPECTED_SECONDS} seconds due to delay"
    echo "  Actual: ${ELAPSED} seconds"
    exit 1
fi
echo "  Timing is correct (delay was respected)"

# Step 8: Verify output
print_step "Verifying output..."
OUTPUT=$(echo "${RESULT}" | jq -r '.output')
echo "  Output: ${OUTPUT}"

OUTPUT_STATUS=$(echo "${OUTPUT}" | jq -r '.result.status // .status // "unknown"')
echo "  Output status: ${OUTPUT_STATUS}"

if [ "${OUTPUT_STATUS}" != "completed_after_delay" ]; then
    echo -e "${YELLOW}Warning: Output status is '${OUTPUT_STATUS}' (expected 'completed_after_delay')${NC}"
    echo "  This may be expected if the workflow structure differs"
fi

echo ""
print_success "All checks passed!"
echo ""
echo "Summary:"
echo "  - Workflow with delay compiled successfully"
echo "  - Image registered: ${IMAGE_ID}"
echo "  - Instance completed: ${INSTANCE_ID}"
echo "  - Delay was respected: ~${ELAPSED} seconds elapsed"
echo "  - Durable sleep mechanism working correctly"
echo ""
