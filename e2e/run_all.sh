#!/bin/bash
# Run all E2E tests
#
# Prerequisites:
# - runtara-server must be running (use ./start.sh)
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
#
# Workflow compile→register→run coverage moved to the in-process cargo suites
# (runtara-workflows `direct_wasm_execute` and `validation_integration_test`,
# both CI-gated) when the standalone runtara-compile CLI was removed. These
# remaining e2e scripts drive a full runtara-server over HTTP for object-model /
# SQL features.
run_test "SMO Trigram Similarity (Tier 1)" "${SCRIPT_DIR}/test_smo_trigram_similarity.sh"
run_test "SMO FTS Match + TS_RANK (Tier 2)" "${SCRIPT_DIR}/test_smo_fts_match.sh"
run_test "SMO pgvector + Levenshtein (Tier 3)" "${SCRIPT_DIR}/test_smo_vector_search.sh"
run_test "Object-model raw SQL (query-sql / execute-sql)" "${SCRIPT_DIR}/test_obm_sql_workflow.sh"
run_test "Negative duration on suspend/relaunch (regression)" "${SCRIPT_DIR}/test_negative_duration_on_resume.sh"
run_test "Stale artifact + trigger replay idempotency (regression)" "${SCRIPT_DIR}/test_trigger_replay_idempotency.sh"
run_test "Pending input across concurrent branches (regression)" "${SCRIPT_DIR}/test_pending_input_concurrent_branches.sh"

# Microsoft Teams. These boot their OWN isolated runtara-server + Valkey (docker)
# on dedicated high ports and mock the Bot Framework, so they are self-contained
# and do not depend on ./start.sh. They need docker + python3 + openssl.
run_test "Teams outbound send-message (mock Bot Connector)" "${SCRIPT_DIR}/test_teams_send_message.sh"
run_test "Teams inbound webhook JWT + dedup (mock authority)" "${SCRIPT_DIR}/test_teams_inbound_webhook.sh"
run_test "Channel session re-flush provenance guard" "${SCRIPT_DIR}/test_channel_reflush_provenance.sh"

# Connection named endpoints. Also self-contained (own server + Valkey); needs
# docker + python3 + jq. Nothing egresses — every case fail-closes at the proxy.
run_test "Connection named endpoints (QuickBooks Online)" "${SCRIPT_DIR}/test_connection_named_endpoint.sh"

# The instance protocol used to be covered here by two scripts driving the
# standalone runtara-core binary. Core is a library now — runtara-server owns
# that listener — so the coverage moved into Rust: the status-code mapping into
# the router tests in `runtara-server/src/core_runtime/http_server.rs`, and the
# drain + concurrency-cap behaviour into
# `runtara-server/tests/core_instance_api.rs`, which runs under the CI gate
# these scripts never ran in.

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
