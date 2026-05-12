#!/bin/bash
# Run all E2E tests.
#
# This script is self-contained: it builds the binaries it needs, boots a
# dedicated runtara-environment (with the WASM runner, which is what
# `runtara-compile` produces) on private ports, runs every test against it,
# and tears the server down on exit. You do NOT need `./start.sh` running;
# this never touches its ports/data dir.
#
# Requirements:
#   - Postgres reachable (the dev compose at dev/docker-compose.yml provides
#     one on localhost:5432; a .env file, if present, is sourced for the
#     connection string and credentials)
#   - wasmtime on PATH or at ~/.wasmtime/bin/wasmtime
#   - psql on PATH (the SMO object-model tests use it)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${PROJECT_ROOT}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Pick up local dev configuration (DB URL, credentials, …) the same way
# start.sh does.
if [ -f "${PROJECT_ROOT}/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "${PROJECT_ROOT}/.env"
    set +a
fi

# Dedicated ports / data dir so this never collides with a `./start.sh`
# instance a developer may already have running on 8001/8002.
E2E_ENV_PORT="${E2E_ENV_PORT:-18002}"
E2E_CORE_PORT="${E2E_CORE_PORT:-18001}"
E2E_DATA_DIR="${PROJECT_ROOT}/.data/e2e"
SERVER_LOG="${E2E_DATA_DIR}/environment.log"

# Exported for the individual test scripts.
export RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:${E2E_ENV_PORT}"
export RUNTARA_SKIP_CERT_VERIFICATION="true"
export DATA_DIR="${E2E_DATA_DIR}"

RUNTARA_CTL="${PROJECT_ROOT}/target/release/runtara-ctl"
RUNTARA_COMPILE="${PROJECT_ROOT}/target/release/runtara-compile"
RUNTARA_ENVIRONMENT="${PROJECT_ROOT}/target/release/runtara-environment"

echo "=========================================="
echo "Runtara E2E Test Suite"
echo "=========================================="
echo ""

# ---------------------------------------------------------------------------
# Build binaries
# ---------------------------------------------------------------------------
echo "Building binaries (release)..."
cargo build --release \
    -p runtara-environment \
    -p runtara-management-sdk --bin runtara-ctl \
    -p runtara-workflows --bin runtara-compile
echo ""

# ---------------------------------------------------------------------------
# Boot a dedicated environment server with the WASM runner
# ---------------------------------------------------------------------------
SERVER_PID=""
cleanup() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        echo ""
        echo "Stopping e2e environment (PID ${SERVER_PID})..."
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

mkdir -p "${E2E_DATA_DIR}"

echo "Starting runtara-environment (WASM runner) on 127.0.0.1:${E2E_ENV_PORT}..."
RUNTARA_RUNNER=wasm \
RUNTARA_DATABASE_URL="${RUNTARA_DATABASE_URL:-postgres://localhost/runtara}" \
RUNTARA_ENV_HTTP_PORT="${E2E_ENV_PORT}" \
RUNTARA_CORE_ADDR="127.0.0.1:${E2E_CORE_PORT}" \
DATA_DIR="${E2E_DATA_DIR}" \
RUNTARA_SKIP_CERT_VERIFICATION="true" \
RUST_LOG="${RUST_LOG:-runtara_environment=info,runtara_core=info}" \
    "${RUNTARA_ENVIRONMENT}" > "${SERVER_LOG}" 2>&1 &
SERVER_PID=$!

# Wait for health
READY=0
for _ in $(seq 1 30); do
    if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
        break
    fi
    if "${RUNTARA_CTL}" health > /dev/null 2>&1; then
        READY=1
        break
    fi
    sleep 1
done

if [ "${READY}" -ne 1 ]; then
    echo -e "${RED}Failed to start runtara-environment. Last 30 log lines:${NC}"
    tail -30 "${SERVER_LOG}" || true
    exit 1
fi
echo "  Environment is healthy (PID ${SERVER_PID}, log: ${SERVER_LOG})"
echo ""

# ---------------------------------------------------------------------------
# Run tests
# ---------------------------------------------------------------------------
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
        TESTS_PASSED=$((TESTS_PASSED + 1))
    else
        echo -e "${RED}FAILED: ${test_name}${NC}"
        TESTS_FAILED=$((TESTS_FAILED + 1))
        FAILED_TESTS="${FAILED_TESTS}\n  - ${test_name}"
    fi
    echo ""
}

run_test "Basic Workflow" "${SCRIPT_DIR}/test_basic_workflow.sh"
run_test "Delay Workflow" "${SCRIPT_DIR}/test_delay_workflow.sh"
run_test "SMO Trigram Similarity (Tier 1)" "${SCRIPT_DIR}/test_smo_trigram_similarity.sh"
run_test "SMO Categorization Workflow (Tier 1)" "${SCRIPT_DIR}/test_smo_categorization_workflow.sh"
run_test "SMO FTS Match + TS_RANK (Tier 2)" "${SCRIPT_DIR}/test_smo_fts_match.sh"
run_test "SMO pgvector + Levenshtein (Tier 3)" "${SCRIPT_DIR}/test_smo_vector_search.sh"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo "=========================================="
echo "Test Results"
echo "=========================================="
echo -e "${GREEN}Passed: ${TESTS_PASSED}${NC}"
echo -e "${RED}Failed: ${TESTS_FAILED}${NC}"

if [ "${TESTS_FAILED}" -gt 0 ]; then
    echo -e "\nFailed tests:${FAILED_TESTS}"
    exit 1
fi

echo ""
echo -e "${GREEN}All tests passed!${NC}"
