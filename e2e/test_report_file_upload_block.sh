#!/bin/bash
# E2E Test: reports file_upload block → workflow-action execute pipeline
#
# The file_upload report block is a drop zone / file picker that launches a
# workflow with the selected file as input. The frontend sends the canonical
# FileData {content, filename, mimeType} as the workflow-action trigger value
# (context.mode=value), so the whole path rides the existing report
# workflow-action execute pipeline: idempotency, bounded wait, render-in-place.
#
# Full loop, all via the HTTP API of a self-contained server:
#   1. Create + compile a workflow whose inputSchema declares a `file` field
#      and whose Finish step echoes filename/mimeType/content back.
#   2. Validate + create a report with a file_upload block targeting it,
#      and assert validation rejects a non-`value` context mode.
#   3. Execute the block's workflow action with a base64 CSV payload and
#      assert the workflow saw the exact file bytes, the response carries an
#      in-place render, and an idempotent replay returns the same instance.
#
# Prerequisites: Postgres + docker (for an isolated Valkey) and the shared
# workflow components in target/wasm32-wasip2/release (see
# scripts/build-agent-components.sh — RUNTARA_ONLY_WORKFLOW_COMPONENTS=1 is
# enough; the workflow uses no agents).

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="report_upload_e2e_server_$$"
TEST_DB_RUNTIME="report_upload_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17720}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17721}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18721}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18722}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18723}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18724}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16391}"
TEST_DATA_DIR="$(mktemp -d -t runtara_report_upload_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="report_upload_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql \
        -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" \
        -tA "$@"
}

cleanup() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    if [ -n "${VALKEY_CONTAINER}" ]; then
        docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    fi
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME}" >/dev/null 2>&1 || true
    rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
}
trap cleanup EXIT

API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" \
        -d "$2" "${API}$1"
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: file_upload report block executes a workflow"
echo "==============================================================="

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
    if [ ! -f "${COMPONENTS_DIR}/${f}" ]; then
        print_error "Missing component ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"
        exit 1
    fi
done

print_step "Pre-flight: Postgres and docker..."
if ! psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1; then
    print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"
    exit 1
fi
if ! docker info >/dev/null 2>&1; then
    print_error "docker is required (isolated Valkey for the trigger stream)"
    exit 1
fi

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for i in {1..20}; do
    if (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null; then break; fi
    sleep 0.5
done

print_step "Creating test databases (${TEST_DB_SERVER}, ${TEST_DB_RUNTIME})..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null
SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
TENANT_ID="${TENANT}" \
SERVER_HOST=127.0.0.1 \
SERVER_PORT="${TEST_PORT_PUBLIC}" \
INTERNAL_PORT="${TEST_PORT_INTERNAL}" \
RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
DATA_DIR="${TEST_DATA_DIR}" \
RUST_LOG="warn,runtara_server=info" \
AUTH_PROVIDER=local \
SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
VALKEY_HOST=127.0.0.1 \
VALKEY_PORT="${TEST_VALKEY_PORT}" \
OTEL_SDK_DISABLED=true \
RUNTARA_SDK_BACKEND=http \
SQLX_OFFLINE="${SQLX_OFFLINE}" \
"${RUNTARA_SERVER_BIN}" >"${TEST_LOG}" 2>&1 &
SERVER_PID=$!

for i in {1..60}; do
    if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
        break
    fi
    sleep 1
    if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
        print_error "Server exited during boot."
        tail -30 "${TEST_LOG}"
        exit 1
    fi
done
echo "  Server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
print_step "Creating workflow with a 'file'-typed input..."
RESP=$(api_post /workflows/create '{"name": "file-upload-echo", "description": "echoes the uploaded file back"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
if [ -z "${WF_ID}" ]; then
    print_error "Workflow create failed: ${RESP}"
    exit 1
fi
echo "  Workflow ${WF_ID} ✓"

# A `file`-typed input is opaque to the DSL (E059: no traversal into it) —
# echo the whole FileData object and assert on its parts in the output.
DEFINITION=$(jq -n '{
  name: "file-upload-echo",
  steps: {
    finish: {
      stepType: "Finish", id: "finish",
      inputMapping: {
        file: {valueType: "reference", value: "data.file"}
      }
    }
  },
  entryPoint: "finish",
  executionPlan: [],
  variables: {},
  inputSchema: {file: {type: "file", required: true}},
  outputSchema: {}
}')
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${DEFINITION}}")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Workflow update failed: ${RESP}"
    exit 1
fi
VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')

print_step "Compiling version ${VERSION}..."
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Compile failed: ${RESP}"
    tail -40 "${TEST_LOG}"
    exit 1
fi
echo "  Compiled ✓"

#-------------------------------------------------------------------------
print_step "Validating a file_upload report definition..."
REPORT_DEF=$(jq -n --arg wf "${WF_ID}" '{
  definitionVersion: 1,
  layout: {
    id: "root", columns: 1,
    items: [{id: "root_i0", child: {id: "n_upload", type: "block", blockId: "csv_import"}}]
  },
  filters: [],
  blocks: [{
    id: "csv_import",
    type: "file_upload",
    file_upload: {
      title: "Import price list",
      description: "Drop a CSV here.",
      accept: [".csv", "text/csv"],
      trigger: "button",
      workflowAction: {
        id: "upload",
        workflowId: $wf,
        label: "Import",
        runningLabel: "Importing…",
        successMessage: "Imported.",
        context: {mode: "value", inputKey: "file"}
      }
    }
  }]
}')
RESP=$(api_post /reports/validate "{\"definition\": ${REPORT_DEF}}")
if [ "$(echo "${RESP}" | jq -r '.valid // false')" != "true" ]; then
    print_error "Expected valid definition, got: ${RESP}"
    exit 1
fi
echo "  Valid ✓"

print_step "Asserting validation rejects context.mode != 'value'..."
BAD_DEF=$(echo "${REPORT_DEF}" | jq '.blocks[0].file_upload.workflowAction.context.mode = "row"')
RESP=$(api_post /reports/validate "{\"definition\": ${BAD_DEF}}")
if [ "$(echo "${RESP}" | jq -r '.valid | tostring')" != "false" ]; then
    print_error "Expected mode=row to be rejected, got: ${RESP}"
    exit 1
fi
if ! echo "${RESP}" | jq -r '.errors[].message' | grep -q "must be 'value'"; then
    print_error "Expected a context.mode error, got: ${RESP}"
    exit 1
fi
echo "  Rejected as expected ✓"

print_step "Creating the report..."
RESP=$(api_post /reports "{\"name\": \"File upload e2e\", \"definition\": ${REPORT_DEF}}")
REPORT_ID=$(echo "${RESP}" | jq -r '.report.id // empty')
if [ -z "${REPORT_ID}" ]; then
    print_error "Report create failed: ${RESP}"
    exit 1
fi
echo "  Report ${REPORT_ID} ✓"

#-------------------------------------------------------------------------
print_step "Executing the upload action with a base64 CSV..."
CSV_CONTENT="sku,price
SKU-1,10.50
SKU-2,3.25"
CSV_B64=$(printf '%s' "${CSV_CONTENT}" | base64)
TRIGGER=$(jq -n --arg b64 "${CSV_B64}" '{
  trigger: {value: {content: $b64, filename: "prices.csv", mimeType: "text/csv"}},
  render: {filters: {}},
  waitMs: 5000
}')
IDEMPOTENCY_KEY="file-upload-e2e-$$"
RESP=$(curl -sS --max-time 60 -X POST -H "Content-Type: application/json" \
  -H "Idempotency-Key: ${IDEMPOTENCY_KEY}" \
  -d "${TRIGGER}" \
  "${API}/reports/${REPORT_ID}/blocks/csv_import/workflow-actions/upload/execute")

STATUS=$(echo "${RESP}" | jq -r '.execution.status // empty')
INSTANCE_ID=$(echo "${RESP}" | jq -r '.execution.instanceId // empty')
if [ "${STATUS}" != "completed" ]; then
    print_error "Expected completed execution, got: ${RESP}"
    tail -40 "${TEST_LOG}"
    exit 1
fi
echo "  Instance ${INSTANCE_ID} completed ✓"

print_step "Asserting the workflow received the exact file..."
OUT_FILENAME=$(echo "${RESP}" | jq -r '.execution.output.file.filename // empty')
OUT_MIME=$(echo "${RESP}" | jq -r '.execution.output.file.mimeType // empty')
OUT_CONTENT=$(echo "${RESP}" | jq -r '.execution.output.file.content // empty')
if [ "${OUT_FILENAME}" != "prices.csv" ]; then
    print_error "filename mismatch: '${OUT_FILENAME}' (resp: ${RESP})"
    exit 1
fi
if [ "${OUT_MIME}" != "text/csv" ]; then
    print_error "mimeType mismatch: '${OUT_MIME}'"
    exit 1
fi
if [ "${OUT_CONTENT}" != "${CSV_B64}" ]; then
    print_error "content mismatch: workflow saw different bytes"
    exit 1
fi
DECODED=$(printf '%s' "${OUT_CONTENT}" | base64 -d)
if [ "${DECODED}" != "${CSV_CONTENT}" ]; then
    print_error "decoded content mismatch"
    exit 1
fi
echo "  filename/mimeType/content round-tripped ✓"

print_step "Asserting the response carries an in-place render..."
if [ "$(echo "${RESP}" | jq '.render != null')" != "true" ]; then
    print_error "Expected render payload in response: ${RESP}"
    exit 1
fi
RENDER_BLOCK_TYPE=$(echo "${RESP}" | jq -r \
  '.render.blocks | (if type == "array" then .[0] else to_entries[0].value end) | (.type // .blockType) // empty' \
  2>/dev/null || true)
echo "  Render present (first block type: ${RENDER_BLOCK_TYPE:-unknown}) ✓"

print_step "Asserting idempotent replay returns the same instance..."
RESP2=$(curl -sS --max-time 60 -X POST -H "Content-Type: application/json" \
  -H "Idempotency-Key: ${IDEMPOTENCY_KEY}" \
  -d "${TRIGGER}" \
  "${API}/reports/${REPORT_ID}/blocks/csv_import/workflow-actions/upload/execute")
INSTANCE_ID2=$(echo "${RESP2}" | jq -r '.execution.instanceId // empty')
if [ "${INSTANCE_ID2}" != "${INSTANCE_ID}" ]; then
    print_error "Replay produced a different instance: ${INSTANCE_ID2} != ${INSTANCE_ID}"
    exit 1
fi
echo "  Same instance on replay ✓"

print_step "Asserting a fresh key produces a fresh run..."
RESP3=$(curl -sS --max-time 60 -X POST -H "Content-Type: application/json" \
  -H "Idempotency-Key: ${IDEMPOTENCY_KEY}-second" \
  -d "${TRIGGER}" \
  "${API}/reports/${REPORT_ID}/blocks/csv_import/workflow-actions/upload/execute")
INSTANCE_ID3=$(echo "${RESP3}" | jq -r '.execution.instanceId // empty')
STATUS3=$(echo "${RESP3}" | jq -r '.execution.status // empty')
if [ -z "${INSTANCE_ID3}" ] || [ "${INSTANCE_ID3}" == "${INSTANCE_ID}" ]; then
    print_error "Fresh key did not produce a fresh instance: ${RESP3}"
    exit 1
fi
if [ "${STATUS3}" != "completed" ]; then
    print_error "Second run did not complete: ${RESP3}"
    exit 1
fi
echo "  Fresh instance ${INSTANCE_ID3} completed ✓"

print_success "file_upload block → workflow-action execute pipeline verified end to end"
