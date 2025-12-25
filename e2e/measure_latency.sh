#!/bin/bash
# E2E Latency Measurement Script
#
# Measures the end-to-end latency of launching and running a single scenario.
# This includes: API call → instance start → execution → completion → response
#
# Usage:
#   ./measure_latency.sh [--image IMAGE_ID] [--tenant TENANT_ID] [--runs N]
#
# Prerequisites:
#   - runtara-core and runtara-environment must be running
#   - jq must be installed

set -e

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUNTARA_CTL="${PROJECT_ROOT}/target/release/runtara-ctl"

# Environment defaults
export RUNTARA_ENVIRONMENT_ADDR="${RUNTARA_ENVIRONMENT_ADDR:-127.0.0.1:8002}"
export RUNTARA_SKIP_CERT_VERIFICATION="${RUNTARA_SKIP_CERT_VERIFICATION:-true}"

# Default values
IMAGE_ID=""
TENANT_ID=""
NUM_RUNS=5
POLL_INTERVAL=50  # ms - aggressive polling for accurate measurement

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --image)
            IMAGE_ID="$2"
            shift 2
            ;;
        --tenant)
            TENANT_ID="$2"
            shift 2
            ;;
        --runs)
            NUM_RUNS="$2"
            shift 2
            ;;
        --poll)
            POLL_INTERVAL="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [--image IMAGE_ID] [--tenant TENANT_ID] [--runs N] [--poll MS]"
            echo ""
            echo "Options:"
            echo "  --image    Image ID to use (auto-detected if not specified)"
            echo "  --tenant   Tenant ID (auto-detected from image if not specified)"
            echo "  --runs     Number of runs for averaging (default: 5)"
            echo "  --poll     Poll interval in ms (default: 50)"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Check prerequisites
if [ ! -f "${RUNTARA_CTL}" ]; then
    echo "ERROR: runtara-ctl not found at ${RUNTARA_CTL}"
    echo "Run: cargo build -p runtara-management-sdk --bin runtara-ctl --release"
    exit 1
fi

if ! command -v jq &> /dev/null; then
    echo "ERROR: jq is required but not installed"
    exit 1
fi

# Check environment health
echo "Checking environment health..."
if ! "${RUNTARA_CTL}" health > /dev/null 2>&1; then
    echo "ERROR: Cannot connect to runtara-environment at ${RUNTARA_ENVIRONMENT_ADDR}"
    exit 1
fi

# Auto-detect image if not specified
if [ -z "${IMAGE_ID}" ]; then
    echo "Auto-detecting latest image..."
    IMAGES_JSON=$("${RUNTARA_CTL}" list-images)
    IMAGE_ID=$(echo "${IMAGES_JSON}" | jq -r '.images[0].image_id')
    TENANT_ID=$(echo "${IMAGES_JSON}" | jq -r '.images[0].tenant_id')
    IMAGE_NAME=$(echo "${IMAGES_JSON}" | jq -r '.images[0].name')

    if [ -z "${IMAGE_ID}" ] || [ "${IMAGE_ID}" == "null" ]; then
        echo "ERROR: No images found. Please register an image first."
        exit 1
    fi
    echo "Using image: ${IMAGE_NAME} (${IMAGE_ID})"
fi

if [ -z "${TENANT_ID}" ]; then
    echo "ERROR: Tenant ID required. Use --tenant or let auto-detect work."
    exit 1
fi

echo ""
echo "=========================================="
echo "  Latency Measurement"
echo "=========================================="
echo "Environment: ${RUNTARA_ENVIRONMENT_ADDR}"
echo "Image ID:    ${IMAGE_ID}"
echo "Tenant ID:   ${TENANT_ID}"
echo "Runs:        ${NUM_RUNS}"
echo "Poll interval: ${POLL_INTERVAL}ms"
echo "=========================================="
echo ""

# Arrays to store timing data
declare -a TOTAL_TIMES
declare -a START_TIMES
declare -a EXEC_TIMES

# Run measurements
for ((i=1; i<=NUM_RUNS; i++)); do
    INSTANCE_ID="latency-test-$(date +%s%N)"
    INPUT='{"input": {"test_run": '"$i"'}}'

    # Measure total time (start → wait completion)
    START_NS=$(date +%s%N)

    # Start instance
    RESULT_ID=$("${RUNTARA_CTL}" start \
        --image "${IMAGE_ID}" \
        --tenant "${TENANT_ID}" \
        --instance-id "${INSTANCE_ID}" \
        --input "${INPUT}" 2>&1)

    AFTER_START_NS=$(date +%s%N)

    if [ -z "${RESULT_ID}" ] || [[ "${RESULT_ID}" == *"error"* ]] || [[ "${RESULT_ID}" == *"Error"* ]]; then
        echo "Run $i: FAILED to start - ${RESULT_ID}"
        continue
    fi

    # Wait for completion
    RESULT=$("${RUNTARA_CTL}" wait "${RESULT_ID}" --poll "${POLL_INTERVAL}" 2>&1)

    END_NS=$(date +%s%N)

    # Calculate times
    TOTAL_MS=$(( (END_NS - START_NS) / 1000000 ))
    START_MS=$(( (AFTER_START_NS - START_NS) / 1000000 ))

    # Get status
    STATUS=$(echo "${RESULT}" | jq -r '.status' 2>/dev/null || echo "unknown")

    # Extract timing from result if available
    STARTED_AT=$(echo "${RESULT}" | jq -r '.started_at // empty' 2>/dev/null)
    FINISHED_AT=$(echo "${RESULT}" | jq -r '.finished_at // empty' 2>/dev/null)

    if [ -n "${STARTED_AT}" ] && [ -n "${FINISHED_AT}" ] && [ "${STARTED_AT}" != "null" ] && [ "${FINISHED_AT}" != "null" ]; then
        # Calculate execution time from timestamps (if available)
        # This requires date command that supports milliseconds
        EXEC_INFO="(db times available)"
    else
        EXEC_INFO=""
    fi

    TOTAL_TIMES+=("${TOTAL_MS}")
    START_TIMES+=("${START_MS}")

    printf "Run %2d: total=%4dms  start_api=%3dms  status=%s %s\n" \
        "$i" "${TOTAL_MS}" "${START_MS}" "${STATUS}" "${EXEC_INFO}"
done

echo ""
echo "=========================================="
echo "  Results Summary"
echo "=========================================="

# Calculate statistics
if [ ${#TOTAL_TIMES[@]} -gt 0 ]; then
    # Calculate average, min, max
    SUM=0
    MIN=${TOTAL_TIMES[0]}
    MAX=${TOTAL_TIMES[0]}

    for t in "${TOTAL_TIMES[@]}"; do
        SUM=$((SUM + t))
        if [ "$t" -lt "$MIN" ]; then MIN=$t; fi
        if [ "$t" -gt "$MAX" ]; then MAX=$t; fi
    done

    AVG=$((SUM / ${#TOTAL_TIMES[@]}))

    # Calculate p50, p95 (sorted)
    SORTED=($(printf '%s\n' "${TOTAL_TIMES[@]}" | sort -n))
    P50_IDX=$(( ${#SORTED[@]} / 2 ))
    P95_IDX=$(( ${#SORTED[@]} * 95 / 100 ))
    if [ $P95_IDX -ge ${#SORTED[@]} ]; then P95_IDX=$((${#SORTED[@]} - 1)); fi

    P50=${SORTED[$P50_IDX]}
    P95=${SORTED[$P95_IDX]}

    echo "Total runs:     ${#TOTAL_TIMES[@]}"
    echo ""
    echo "Total latency (start → completion):"
    echo "  Average:      ${AVG} ms"
    echo "  Min:          ${MIN} ms"
    echo "  Max:          ${MAX} ms"
    echo "  P50:          ${P50} ms"
    echo "  P95:          ${P95} ms"
    echo ""

    # Start API timing
    if [ ${#START_TIMES[@]} -gt 0 ]; then
        START_SUM=0
        for t in "${START_TIMES[@]}"; do
            START_SUM=$((START_SUM + t))
        done
        START_AVG=$((START_SUM / ${#START_TIMES[@]}))
        echo "Start API latency (avg): ${START_AVG} ms"
    fi
else
    echo "No successful runs to analyze"
    exit 1
fi

echo ""
echo "=========================================="
