#!/bin/bash
# E2E Test: guest workflow recovery across an Environment restart.
#
# Reproduces the bug "Long-running guests killed by Environment restart with no
# resume / re-queue" and verifies the three target behaviors:
#
#   MODE=graceful  (G-A) SIGTERM the stack mid-run -> drain suspends the guest ->
#                  restart -> wake scheduler relaunches -> completed, no dup rows.
#   MODE=abrupt    (G-B) SIGKILL the stack mid-run (no drain) -> restart ->
#                  recover_orphaned_containers must route into recovery ->
#                  relaunched -> completed, no dup rows.
#                  (On UNPATCHED code this is expected to FAIL: the instance
#                  dead-ends at status=failed "Process terminated during
#                  Environment restart". That is the bug.)
#
# The workflow is a Split (sequential) over N items; each iteration does a
# durable object_model:create-instance plus a utils:delay-in-ms to keep the
# guest *running* (not self-suspended) so it can be killed mid-loop. Because the
# engine is replay-from-start with checkpoints as a result cache, completed
# create-instance steps are served from cache on relaunch -> the physical row
# count must equal N with no duplicates.
#
# Usage:  MODE=graceful ./e2e/test_recovery_environment_restart.sh
#         MODE=abrupt   ./e2e/test_recovery_environment_restart.sh
#
# Prereqs: Postgres + docker (isolated Valkey) and prebuilt components in
# target/wasm32-wasip2/release (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

MODE="${MODE:-graceful}"            # graceful | abrupt
N_ITEMS="${N_ITEMS:-20}"            # Split iterations
DELAY_MS="${DELAY_MS:-700}"         # per-iteration pacing delay
KILL_AFTER_ROWS="${KILL_AFTER_ROWS:-4}"  # kill once this many rows exist
GRACE_MS="${GRACE_MS:-3000}"        # drain grace (short, so force-stop is quick)

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="rec_e2e_server_$$"
TEST_DB_RUNTIME="rec_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17710}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17711}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18711}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18712}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18713}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18714}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16391}"
TEST_DATA_DIR="$(mktemp -d -t runtara_rec_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="rec_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}

cleanup() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill -9 "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    if [ "${KEEP_DB:-0}" = "1" ]; then
        echo "KEEP_DB=1 — leaving ${TEST_DB_SERVER}/${TEST_DB_RUNTIME} and ${TEST_DATA_DIR}"
        return
    fi
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME}" >/dev/null 2>&1 || true
    rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
}
trap cleanup EXIT

start_server() {
    # Start (or restart) runtara-server with identical env so the wake scheduler
    # and orphan recovery see the same DBs / data dir / Valkey across restarts.
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
    RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
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
    RUNTARA_DEV_MODE=false \
    RUNTARA_SHUTDOWN_GRACE_MS="${GRACE_MS}" \
    RUNTARA_AUTO_RECOVER="${RUNTARA_AUTO_RECOVER:-true}" \
    RUNTARA_MAX_AUTO_RESTARTS="${RUNTARA_MAX_AUTO_RESTARTS:-5}" \
    RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info,runtara_environment=info,runtara_core=info}" \
    AUTH_PROVIDER=local \
    SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    RUNTARA_SDK_BACKEND=http \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!

    for i in {1..60}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            print_error "Server exited during boot."; tail -40 "${TEST_LOG}"; exit 1
        fi
    done
    print_error "Server did not become healthy."; tail -40 "${TEST_LOG}"; exit 1
}

instance_status() {
    curl -sS "${API}/workflows/instances/$1" | jq -r '.data.status // .status // empty'
}
instance_error() {
    curl -sS "${API}/workflows/instances/$1" | jq -r '.data.error // .error // empty'
}
row_count() {
    psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT COUNT(*) FROM ${ROW_TABLE}" 2>/dev/null | tr -d '[:space:]'
}
distinct_count() {
    psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT COUNT(DISTINCT idx) FROM ${ROW_TABLE}" 2>/dev/null | tr -d '[:space:]'
}

echo "==============================================================="
echo "E2E: Environment-restart recovery  (MODE=${MODE}, N=${N_ITEMS})"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
for f in runtara_agent_object_model.wasm runtara_agent_utils.wasm runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
    [ -f "${COMPONENTS_DIR}/${f}" ] || { print_error "Missing component ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"; exit 1; }
done
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for i in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server (boot 1) on :${TEST_PORT_PUBLIC}..."
start_server
echo "  Server up (PID ${SERVER_PID})"

print_step "Creating object schema RecoveryItem..."
RESP=$(api_post /object-model/schemas '{
  "name": "RecoveryItem",
  "tableName": "recovery_item_e2e",
  "columns": [{"name": "idx", "type": "integer"}]
}')
SCHEMA_ID=$(echo "${RESP}" | jq -r '.schemaId // empty')
[ -n "${SCHEMA_ID}" ] || { print_error "Schema create failed: ${RESP}"; exit 1; }
# Discover the physical table name (robust against any prefixing).
ROW_TABLE=$(psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT tablename FROM pg_tables WHERE tablename = 'recovery_item_e2e' LIMIT 1" | tr -d '[:space:]')
[ -n "${ROW_TABLE}" ] || { print_error "Physical table recovery_item_e2e not found after schema create"; psql_quiet -d "${TEST_DB_SERVER}" -c "\dt" ; exit 1; }
echo "  Schema ${SCHEMA_ID}, table ${ROW_TABLE} ✓"

print_step "Creating postgres connection..."
RESP=$(api_post /connections "{\"title\": \"rec-e2e store\", \"integrationId\": \"postgres\", \"connectionParameters\": {\"database_url\": \"${SERVER_DB_URL}\"}}")
CONN_ID=$(echo "${RESP}" | jq -r '.connection_id // .connectionId // empty')
[ -n "${CONN_ID}" ] || { print_error "Connection create failed: ${RESP}"; exit 1; }
echo "  Connection ${CONN_ID} ✓"

print_step "Creating workflow..."
RESP=$(api_post /workflows/create '{"name": "recovery-harness", "description": "Split with durable create-instance per item; used to test restart recovery"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WF_ID}" ] || { print_error "Workflow create failed: ${RESP}"; exit 1; }
echo "  Workflow ${WF_ID} ✓"

print_step "Pushing workflow definition (Split -> [create-instance, delay] x N)..."
DEFINITION=$(jq -n --arg conn "${CONN_ID}" --argjson delay "${DELAY_MS}" '{
  name: "recovery-harness",
  steps: {
    split: {
      stepType: "Split", id: "split",
      config: { value: {valueType: "reference", value: "data.items"}, sequential: true },
      subgraph: {
        name: "process one item",
        steps: {
          createRow: {
            stepType: "Agent", id: "createRow",
            agentId: "object_model", capabilityId: "create-if-not-exists",
            connectionId: $conn,
            inputMapping: {
              schema_name: {valueType: "immediate", value: "RecoveryItem"},
              match_filters: {valueType: "reference", value: "item"},
              data: {valueType: "reference", value: "item"}
            }
          },
          pace: {
            stepType: "Agent", id: "pace",
            agentId: "utils", capabilityId: "delay-in-ms",
            inputMapping: { delay_value: {valueType: "immediate", value: $delay} }
          },
          iterFinish: {
            stepType: "Finish", id: "iterFinish",
            inputMapping: { created: {valueType: "reference", value: "steps.createRow.outputs.instance_id"} }
          }
        },
        entryPoint: "createRow",
        executionPlan: [
          {fromStep: "createRow", toStep: "pace"},
          {fromStep: "pace", toStep: "iterFinish"}
        ]
      }
    },
    finish: {
      stepType: "Finish", id: "finish",
      inputMapping: { results: {valueType: "reference", value: "steps.split.outputs"} }
    }
  },
  entryPoint: "split",
  executionPlan: [ {fromStep: "split", toStep: "finish"} ],
  variables: {},
  inputSchema: { items: {type: "array", description: "items to create"} },
  outputSchema: {}
}')
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${DEFINITION}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Workflow update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
echo "  Definition pushed (version ${VERSION}) ✓"

print_step "Compiling version ${VERSION}..."
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1; }
echo "  Compiled ✓"

# Build the items array [{idx:0},...,{idx:N-1}]
ITEMS=$(jq -n --argjson n "${N_ITEMS}" '[range(0;$n) | {idx: .}]')

print_step "Executing workflow with ${N_ITEMS} items..."
RESP=$(api_post "/workflows/${WF_ID}/execute" "{\"inputs\": {\"data\": {\"items\": ${ITEMS}}}}")
INSTANCE_ID=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${INSTANCE_ID}" ] || { print_error "Execute failed: ${RESP}"; exit 1; }
echo "  Instance ${INSTANCE_ID}"

print_step "Waiting until >= ${KILL_AFTER_ROWS} rows exist (run is mid-flight)..."
ROWS_AT_KILL=0
for i in {1..120}; do
    C=$(row_count); C=${C:-0}
    S=$(instance_status "${INSTANCE_ID}")
    if [ "${C}" -ge "${KILL_AFTER_ROWS}" ]; then ROWS_AT_KILL=${C}; break; fi
    case "${S}" in completed|failed) print_error "Instance reached '${S}' before kill window (rows=${C}); increase N_ITEMS/DELAY_MS"; exit 1 ;; esac
    sleep 0.5
done
[ "${ROWS_AT_KILL}" -ge "${KILL_AFTER_ROWS}" ] || { print_error "Never reached ${KILL_AFTER_ROWS} rows"; exit 1; }
echo "  Mid-flight: ${ROWS_AT_KILL} rows created, instance status=$(instance_status "${INSTANCE_ID}") ✓"

#-------------------------------------------------------------------------
if [ "${MODE}" = "graceful" ]; then
    print_step "GRACEFUL kill: SIGTERM server PID ${SERVER_PID} (grace ${GRACE_MS}ms)..."
    kill -TERM "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
    echo "  Server exited after drain."
    print_step "Post-drain instance status (expect suspended)..."
    # Server is down; read drain outcome from the runtime DB directly.
    DRAIN_STATUS=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT status FROM instances WHERE instance_id='${INSTANCE_ID}'" | tr -d '[:space:]')
    DRAIN_REASON=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT termination_reason FROM instances WHERE instance_id='${INSTANCE_ID}'" | tr -d '[:space:]')
    echo "  runtime DB: status=${DRAIN_STATUS} termination_reason=${DRAIN_REASON}"
    # G-C: clean checkpoint-and-pause assertions.
    CKPTS_AT_SUSPEND=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT COUNT(*) FROM checkpoints WHERE instance_id='${INSTANCE_ID}'" | tr -d '[:space:]')
    CKPTS_AT_SUSPEND=${CKPTS_AT_SUSPEND:-0}
    SUSPEND_ERR=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT COALESCE(error,'') FROM instances WHERE instance_id='${INSTANCE_ID}'")
    echo "  checkpoints recorded at suspend: ${CKPTS_AT_SUSPEND}"
    echo "  suspend error column: '${SUSPEND_ERR}'"
    if echo "${SUSPEND_ERR}" | grep -q "Force-stopped after grace period"; then
        print_error "G-C: instance was FORCE-STOPPED at grace expiry, not a clean checkpoint-pause"
        GC_FORCE_STOPPED=1
    else
        echo "  suspend kind: CLEAN checkpoint-pause (not force-stopped)"
        GC_FORCE_STOPPED=0
    fi
    if [ "${DRAIN_STATUS}" != "suspended" ]; then
        print_error "G-C: expected status=suspended after graceful drain, got '${DRAIN_STATUS}'"
        exit 1
    fi
    if [ "${CKPTS_AT_SUSPEND}" -lt "${ROWS_AT_KILL}" ]; then
        print_error "G-C: only ${CKPTS_AT_SUSPEND} checkpoints at suspend, expected >= ${ROWS_AT_KILL} (progress not checkpointed before pause)"
        exit 1
    fi
    if [ "${GC_FORCE_STOPPED}" = "1" ]; then
        exit 1
    fi
    print_success "G-C: graceful drain reached ${CKPTS_AT_SUSPEND} checkpoints and paused cleanly (suspended, not force-stopped)"
else
    print_step "ABRUPT kill: SIGKILL server PID ${SERVER_PID} (no drain)..."
    kill -9 "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
    echo "  Server hard-killed."
    KILL_STATUS=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT status FROM instances WHERE instance_id='${INSTANCE_ID}'" | tr -d '[:space:]')
    echo "  runtime DB: status=${KILL_STATUS} (expected still 'running' — no drain ran)"
fi
SERVER_PID=""

#-------------------------------------------------------------------------
print_step "Restarting runtara-server (boot 2)..."
start_server
echo "  Server up (PID ${SERVER_PID})"

print_step "Waiting for the instance to be recovered and complete (up to ~150s)..."
FINAL=""
for i in {1..150}; do
    S=$(instance_status "${INSTANCE_ID}")
    case "${S}" in completed) FINAL="completed"; break ;; failed) FINAL="failed"; break ;; esac
    sleep 1
done

echo ""
echo "---------------------------------------------------------------"
echo "RESULT (MODE=${MODE})"
echo "  rows at kill : ${ROWS_AT_KILL}"
echo "  final status : ${FINAL:-<timeout>} (last='$(instance_status "${INSTANCE_ID}")')"
echo "  final error  : $(instance_error "${INSTANCE_ID}")"
echo "  row count    : $(row_count) / ${N_ITEMS}"
echo "  distinct idx : $(distinct_count) / ${N_ITEMS}"
echo "  recovery_attempts : $(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT recovery_attempts FROM instances WHERE instance_id='${INSTANCE_ID}'" | tr -d '[:space:]')"
echo "---------------------------------------------------------------"

FINAL_ROWS=$(row_count); FINAL_ROWS=${FINAL_ROWS:-0}
FINAL_DISTINCT=$(distinct_count); FINAL_DISTINCT=${FINAL_DISTINCT:-0}

# Negative case: when EXPECT_FAIL=1 (e.g. RUNTARA_AUTO_RECOVER=false) the
# instance must NOT be recovered — it should end `failed` with the
# environment_restart reason and no further rows created.
if [ "${EXPECT_FAIL:-0}" = "1" ]; then
    FINAL_STATUS=$(instance_status "${INSTANCE_ID}")
    FINAL_ERR=$(instance_error "${INSTANCE_ID}")
    FINAL_REASON=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT termination_reason FROM instances WHERE instance_id='${INSTANCE_ID}'" | tr -d '[:space:]')
    echo "  EXPECT_FAIL: status=${FINAL_STATUS} reason=${FINAL_REASON} error='${FINAL_ERR}'"
    if [ "${FINAL_STATUS}" != "failed" ]; then
        print_error "EXPECT_FAIL: instance should be 'failed' (auto-recovery disabled), got '${FINAL_STATUS}'"
        exit 1
    fi
    if [ "${FINAL_REASON}" != "environment_restart" ]; then
        print_error "EXPECT_FAIL: termination_reason should be 'environment_restart', got '${FINAL_REASON}'"
        exit 1
    fi
    print_success "Auto-recovery disabled: instance failed terminally with environment_restart (not relaunched)."
    exit 0
fi

if [ "${FINAL}" != "completed" ]; then
    print_error "Instance did not complete (status='$(instance_status "${INSTANCE_ID}")'). Recovery did not happen."
    tail -60 "${TEST_LOG}"
    exit 1
fi
if [ "${FINAL_ROWS}" != "${N_ITEMS}" ]; then
    print_error "Row count ${FINAL_ROWS} != ${N_ITEMS} (lost or duplicated work on replay)"
    exit 1
fi
if [ "${FINAL_DISTINCT}" != "${N_ITEMS}" ]; then
    print_error "Distinct idx ${FINAL_DISTINCT} != ${N_ITEMS} (DUPLICATE side effects on replay)"
    exit 1
fi

print_success "Recovered across ${MODE} restart: completed, ${FINAL_ROWS}/${N_ITEMS} rows, no duplicates."
