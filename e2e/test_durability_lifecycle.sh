#!/bin/bash
# E2E Test: the durability lifecycle contract on a real server
# (docs/durability-changes.md).
#
#   1. OWNED PARK    A durable Delay parks under the run's root lease: the lease
#                    row records the park, the executions API reports
#                    executionPhase=suspended, and the wake relaunches the run
#                    under the next lease epoch, which completes it.
#   2. PAUSE         Pausing a parked run records a `paused` timeline event
#                    (not `suspended`) and reports executionPhase=paused.
#   3. PARK FAILURE  Every park attempt fails inside the database. The run is
#                    handed to recovery (termination_reason=park_failed, an
#                    immediate wake) instead of failing as crashed, parks again
#                    on the relaunch and completes.
#   4. IN-PROCESS    A non-durable Agent step's retry backoff waits inside its
#                    execution: the executions API reports
#                    executionPhase=waiting_in_process while it waits.
#
# Usage:  ./e2e/test_durability_lifecycle.sh
#
# Prereqs: Postgres (psql on PATH), docker (isolated Valkey), a debug
# runtara-server binary, and prebuilt components in target/wasm32-wasip2/release
# (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="durability_e2e_server_$$"
TEST_DB_RUNTIME="durability_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17740}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18741}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18742}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18743}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18744}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16394}"
TEST_MOCK_PORT="${TEST_MOCK_PORT:-17749}"
MOCK_PID=""
TEST_DATA_DIR="$(mktemp -d -t runtara_durability_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="durability_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
runtime_sql() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "$1"; }
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}

cleanup() {
    local code=$?
    [ -n "${SERVER_PID}" ] && kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
    [ -n "${MOCK_PID}" ] && { kill "${MOCK_PID}" 2>/dev/null; wait "${MOCK_PID}" 2>/dev/null; } || true
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    [ ${code} -ne 0 ] && [ -f "${TEST_LOG}" ] && { echo "--- server log tail ---"; tail -60 "${TEST_LOG}"; }
    rm -rf "${TEST_DATA_DIR}"
    exit ${code}
}
trap cleanup EXIT

start_server() {
    # Run from the scratch directory so no repository .env leaks in, and exec
    # so SERVER_PID is the server itself, which cleanup must stop.
    ( cd "${TEST_DATA_DIR}" && \
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
    RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
    TENANT_ID="${TENANT}" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${TEST_PORT_PUBLIC}" \
    RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
    RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
    RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
    RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
    RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
    DATA_DIR="${TEST_DATA_DIR}" \
    RUNTARA_DEV_MODE=false \
    RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info,runtara_environment=info,runtara_core=info}" \
    AUTH_PROVIDER=local \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    SQLX_OFFLINE=true \
    RUNTARA_PROXY_ALLOWED_HOSTS="127.0.0.1:${TEST_MOCK_PORT}" \
    RUNTARA_PROXY_ALLOW_HTTP_HOSTS=127.0.0.1 \
    exec "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 ) &
    SERVER_PID=$!

    for _ in {1..90}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            print_error "Server exited during boot."; exit 1
        fi
    done
    print_error "Server did not become healthy."; exit 1
}

execution() { curl -sS "${API}/workflows/instances/$1"; }
field() { execution "$1" | jq -r ".data.$2 // empty"; }
lease_row() {
    runtime_sql "SELECT owner || '|' || epoch || '|' || active || '|' || COALESCE(released_by, '') FROM invocation_root_leases WHERE instance_id = '$1'"
}
instance_row() {
    runtime_sql "SELECT COALESCE(status::text,'') || '|' || COALESCE(termination_reason::text,'') FROM instances WHERE instance_id = '$1'"
}

# Create + compile a single-Delay workflow, echo its id.
make_delay_workflow() {
    local name="$1" duration_ms="$2" durable="$3" resp wf_id definition version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"durability lifecycle\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }
    definition=$(jq -n --argjson ms "${duration_ms}" --argjson durable "${durable}" '{
      name: "durability-lifecycle",
      durable: $durable,
      entryPoint: "delay",
      steps: {
        delay: { stepType: "Delay", id: "delay", name: "Wait", durationMs: { valueType: "immediate", value: $ms } },
        finish: { stepType: "Finish", id: "finish",
                  inputMapping: { waited: { valueType: "immediate", value: true } } }
      },
      executionPlan: [ { fromStep: "delay", toStep: "finish" } ],
      variables: {}, inputSchema: {}, outputSchema: {}
    }')
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${definition}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${resp}"; exit 1; }
    version=$(curl -sS "${API}/workflows/${wf_id}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${resp}"; exit 1; }
    echo "${wf_id}"
}

execute() {
    local resp inst
    resp=$(api_post "/workflows/$1/execute" '{"inputs": {"data": {}, "variables": {}}}')
    inst=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${inst}" ] || { print_error "Execute failed: $1: ${resp}"; exit 1; }
    echo "${inst}"
}

wait_status() {
    local inst="$1" want="$2" seconds="$3" st=""
    local deadline=$(( $(date +%s) + seconds ))
    while [ "$(date +%s)" -lt "${deadline}" ]; do
        st=$(field "${inst}" status)
        [ "${st}" = "${want}" ] && return 0
        if [ "${want}" != "failed" ] && { [ "${st}" = "failed" ] || [ "${st}" = "cancelled" ]; }; then
            print_error "${inst} ended ${st} while waiting for ${want}: $(execution "${inst}" | jq -c '.data | {status, error, suspensionReason, executionPhase}')"
            exit 1
        fi
        sleep 0.5
    done
    print_error "${inst} never reached ${want} (status ${st})"; exit 1
}

echo "==============================================================="
echo "E2E: durability lifecycle (owned park, pause, park failure, in-process wait)"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
[ -f "${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm" ] || { print_error "Missing runtara_workflow_stdlib.wasm — run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
start_server
echo "  Server up (PID ${SERVER_PID})"

# ---------------------------------------------------------------------------
# Case 1 — owned park, then a relaunch under the next lease epoch.
# ---------------------------------------------------------------------------
print_step "Case 1: a durable Delay parks under the run's root lease..."
WF_PARK=$(make_delay_workflow "durability-park" 6000 true)
INST=$(execute "${WF_PARK}")
echo "  Instance ${INST}"
wait_status "${INST}" suspended 30
LEASE=$(lease_row "${INST}")
IFS='|' read -r L_OWNER L_EPOCH L_ACTIVE L_RELEASED <<< "${LEASE}"
echo "  lease owner=${L_OWNER} epoch=${L_EPOCH} active=${L_ACTIVE} released_by=${L_RELEASED}"
[ "${L_EPOCH}" = "1" ] || { print_error "the first run must hold epoch 1, got '${LEASE}'"; exit 1; }
[ "${L_ACTIVE}" = "false" ] || { print_error "a parked run must not keep its lease active: '${LEASE}'"; exit 1; }
[ "${L_RELEASED}" = "park" ] || { print_error "the lease must record that its execution parked: '${LEASE}'"; exit 1; }
[[ "${L_OWNER}" == wasm_* ]] || { print_error "the lease owner must be the runner registration, got '${L_OWNER}'"; exit 1; }
PHASE=$(field "${INST}" executionPhase)
REASON=$(field "${INST}" suspensionReason)
echo "  executionPhase=${PHASE} suspensionReason=${REASON}"
[ "${PHASE}" = "suspended" ] || { print_error "expected executionPhase=suspended, got '${PHASE}'"; exit 1; }
[ "${REASON}" = "sleeping" ] || { print_error "expected suspensionReason=sleeping, got '${REASON}'"; exit 1; }
print_success "Parked under its lease, reported as suspended ✓"

wait_status "${INST}" completed 60
LEASE=$(lease_row "${INST}")
IFS='|' read -r L_OWNER2 L_EPOCH2 L_ACTIVE2 L_RELEASED2 <<< "${LEASE}"
echo "  after wake: lease owner=${L_OWNER2} epoch=${L_EPOCH2} active=${L_ACTIVE2} released_by=${L_RELEASED2}"
[ "${L_EPOCH2}" = "2" ] || { print_error "the relaunch must claim epoch 2, got '${LEASE}'"; exit 1; }
[ "${L_OWNER2}" != "${L_OWNER}" ] || { print_error "the relaunch must own the root under a new registration"; exit 1; }
[ "${L_ACTIVE2}" = "false" ] || { print_error "a completed run must not keep its lease: '${LEASE}'"; exit 1; }
[ "${L_RELEASED2}" = "completed" ] || { print_error "the lease must record that its execution completed the run: '${LEASE}'"; exit 1; }
COMPLETED_EVENTS=$(runtime_sql "SELECT COUNT(*) FROM instance_events WHERE instance_id = '${INST}' AND event_type = 'completed'")
[ "${COMPLETED_EVENTS}" = "1" ] || { print_error "expected exactly one completed event, got ${COMPLETED_EVENTS}"; exit 1; }
[ -z "$(field "${INST}" executionPhase)" ] || { print_error "a completed run has no live phase"; exit 1; }
print_success "Woken under epoch 2 and completed; no lease left active ✓"

# ---------------------------------------------------------------------------
# Case 2 — pausing a parked run.
# ---------------------------------------------------------------------------
print_step "Case 2: pausing a parked run records a paused event..."
WF_LONG=$(make_delay_workflow "durability-pause" 600000 true)
INST=$(execute "${WF_LONG}")
echo "  Instance ${INST}"
wait_status "${INST}" suspended 30
RESP=$(api_post "/workflows/instances/${INST}/pause" '{}')
echo "  pause -> $(echo "${RESP}" | jq -c '.data // .')"
for _ in {1..20}; do
    [ "$(field "${INST}" suspensionReason)" = "paused" ] && break
    sleep 0.5
done
PHASE=$(field "${INST}" executionPhase)
REASON=$(field "${INST}" suspensionReason)
echo "  executionPhase=${PHASE} suspensionReason=${REASON}"
[ "${PHASE}" = "paused" ] || { print_error "expected executionPhase=paused, got '${PHASE}'"; exit 1; }
PAUSED_EVENTS=$(runtime_sql "SELECT COUNT(*) FROM instance_events WHERE instance_id = '${INST}' AND event_type = 'paused'")
echo "  paused events: ${PAUSED_EVENTS}"
[ "${PAUSED_EVENTS}" = "1" ] || { print_error "expected exactly one paused event, got ${PAUSED_EVENTS}"; exit 1; }
api_post "/workflows/instances/${INST}/stop" '{}' >/dev/null || true
print_success "Paused run reported as paused with its own timeline event ✓"

# ---------------------------------------------------------------------------
# Case 3 — every park attempt fails; recovery resumes the run.
# ---------------------------------------------------------------------------
print_step "Case 3: a park that never commits is recovered, not failed..."
# The run's own park sets termination_reason to a wait marker; recovery's
# suspension (park_failed) is let through. The countdown is a sequence, whose
# nextval survives the rollback the injected error causes.
runtime_sql "CREATE SEQUENCE e2e_park_faults" >/dev/null
runtime_sql "CREATE FUNCTION e2e_park_fault() RETURNS trigger AS \$\$
BEGIN
    IF OLD.status = 'running' AND NEW.status = 'suspended'
       AND NEW.termination_reason IN ('sleeping', 'waiting_signal', 'waiting_instances')
       AND nextval('e2e_park_faults') <= 5 THEN
        RAISE EXCEPTION 'injected park failure';
    END IF;
    RETURN NEW;
END \$\$ LANGUAGE plpgsql" >/dev/null
runtime_sql "CREATE TRIGGER zz_e2e_park_fault BEFORE UPDATE OF status ON instances FOR EACH ROW EXECUTE FUNCTION e2e_park_fault()" >/dev/null
INST=$(execute "${WF_PARK}")
echo "  Instance ${INST}"
SAW_RECOVERY=""
DEADLINE=$(( $(date +%s) + 90 ))
while [ "$(date +%s)" -lt "${DEADLINE}" ]; do
    ROW=$(instance_row "${INST}")
    [ "${ROW}" = "suspended|park_failed" ] && SAW_RECOVERY="yes"
    case "${ROW%%|*}" in
        completed) break ;;
        failed|cancelled) print_error "the run ended ${ROW} instead of being recovered"; exit 1 ;;
    esac
    sleep 0.2
done
runtime_sql "DROP TRIGGER zz_e2e_park_fault ON instances" >/dev/null
if [ "$(instance_row "${INST}" | cut -d'|' -f1)" != "completed" ]; then
    print_error "the recovered run never completed: $(instance_row "${INST}")"
    runtime_sql "SELECT kind || ' ' || state || ' ' || COALESCE(last_error,'') || ' attempts=' || attempt_count FROM instance_launches WHERE instance_id = '${INST}' ORDER BY created_at"
    runtime_sql "SELECT 'sleep_until=' || COALESCE(sleep_until::text,'') || ' wake_reason=' || COALESCE(wake_reason::text,'') FROM instances WHERE instance_id = '${INST}'"
    echo "lease: $(lease_row "${INST}")"
    exit 1
fi
grep -q "handing the run to recovery" "${TEST_LOG}" || { print_error "the runner never reported handing the run to recovery"; exit 1; }
FAULTS=$(runtime_sql "SELECT last_value FROM e2e_park_faults")
echo "  injected park failures: ${FAULTS}; observed park_failed recovery: ${SAW_RECOVERY:-no}"
[ "${FAULTS}" -ge 5 ] || { print_error "the park was not retried before recovery (${FAULTS} attempts)"; exit 1; }
[ -n "${SAW_RECOVERY}" ] || print_warn "the park_failed suspension was too brief to observe; the log and completion confirm recovery"
EPOCH=$(lease_row "${INST}" | cut -d'|' -f2)
echo "  final lease epoch: ${EPOCH}"
# The recovery relaunch runs under a new lease epoch. (If the Delay's deadline
# passed meanwhile, that relaunch finishes without parking again.)
[ "${EPOCH}" -ge 2 ] || { print_error "expected the recovery relaunch under a new epoch, got ${EPOCH}"; exit 1; }
print_success "Park retried, then recovered under park_failed and completed ✓"

# ---------------------------------------------------------------------------
# Case 4 — an in-process wait is reported while it lasts.
# ---------------------------------------------------------------------------
print_step "Case 4: a non-durable retry backoff is reported waiting in process..."
# A mock that always answers 503: a transient, retryable failure.
python3 -c '
import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(503); self.send_header("Content-Length", "0"); self.end_headers()
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
' "${TEST_MOCK_PORT}" &
MOCK_PID=$!
RESP=$(api_post /workflows/create '{"name": "durability-in-process", "description": "durability lifecycle"}')
WF_INPROC=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WF_INPROC}" ] || { print_error "Workflow create failed: ${RESP}"; exit 1; }
DEFINITION=$(jq -n --arg url "http://127.0.0.1:${TEST_MOCK_PORT}/flaky" '{
  name: "durability-in-process",
  durable: false,
  entryPoint: "call",
  steps: {
    call: { stepType: "Agent", id: "call", name: "Call", agentId: "http", capabilityId: "http-request",
            maxRetries: 2, retryDelay: 4000,
            inputMapping: { url: { valueType: "immediate", value: $url },
                            method: { valueType: "immediate", value: "GET" } } },
    finish: { stepType: "Finish", id: "finish",
              inputMapping: { called: { valueType: "immediate", value: true } } }
  },
  executionPlan: [ { fromStep: "call", toStep: "finish" } ],
  variables: {}, inputSchema: {}, outputSchema: {}
}')
RESP=$(api_post "/workflows/${WF_INPROC}/update" "{\"executionGraph\": ${DEFINITION}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WF_INPROC}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${WF_INPROC}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${RESP}"; exit 1; }
INST=$(execute "${WF_INPROC}")
echo "  Instance ${INST}"
SAW_WAITING=""
STATE=""
DEADLINE=$(( $(date +%s) + 40 ))
while [ "$(date +%s)" -lt "${DEADLINE}" ]; do
    STATE=$(execution "${INST}" | jq -r '.data.status + "|" + (.data.executionPhase // "")')
    [ "${STATE}" = "running|waiting_in_process" ] && { SAW_WAITING="yes"; break; }
    case "${STATE%%|*}" in completed|failed|cancelled) break ;; esac
    sleep 0.3
done
[ -n "${SAW_WAITING}" ] || { print_error "never saw executionPhase=waiting_in_process (last ${STATE})"; exit 1; }
# Retries exhaust against the mock and the run fails; it then has no live phase.
wait_status "${INST}" failed 60
[ -z "$(field "${INST}" executionPhase)" ] || { print_error "a finished run has no live phase"; exit 1; }
print_success "Reported waiting_in_process during the retry backoff ✓"

echo
print_success "Durability lifecycle holds end to end."
