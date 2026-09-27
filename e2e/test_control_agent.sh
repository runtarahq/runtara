#!/bin/bash
# E2E Test: the control agent on an isolated live server.
#
# Stages (select with STAGES, a comma-separated list; default: every stage
# this build implements):
#
#   1  READS    a workflow calls control:get, control:query and
#              control:list-pending-signals on other runs of the tenant and
#              gets their state, output and open requests; failures carry
#              CONTROL_* codes. The control bytes are pinned and approved at
#              boot; after an operator revokes that approval and the server
#              restarts, control calls are denied and the pinned workflow no
#              longer becomes ready.
#
#   2  MUTATIONS a workflow's send-signal step answers another run's open
#              WaitForSignal request that opted in with action.key; the
#              server is SIGKILLed while the sender sits in a durable Delay,
#              and the replay answers from its receipt instead of answering
#              twice. Pausing a waiting run through the public API pauses it
#              immediately; resuming a failed run is NotResumable; cancel and
#              pause of a run that is not a child are CONTROL_NOT_CHILD; the
#              public signal path refuses control's reserved `control:`
#              operation ids.
#
#   3  START   a parent's control:start steps admit two children (labels
#              `a` and `b`); the server is SIGKILLed while the parent sits
#              in a durable Delay, and the replayed non-durable start returns
#              the same child (replayed: true, one admission row). The parent
#              then queries its children (query with callerChildren, get),
#              pauses, resumes and cancels one and answers the other's
#              WaitForSignal; the executions API filters by
#              parentInstanceId. A reused label is CONTROL_LABEL_CONFLICT,
#              and with MAX_CONCURRENT_EXECUTIONS=5 a fifth running child is
#              CONTROL_CAPACITY_RATE_LIMITED (control's share is 4) while an
#              outside trigger is still admitted.
#
#   4  OWNERSHIP children with parentClosePolicy `cancel` are cancelled when
#              their parent fails, completes, or ends while the server is
#              down (SIGKILL, parent failed out of band, restart); a
#              `leave_running` sibling survives; a child held in admission
#              (its workflow cannot compile while compilations are off) is
#              cancelled there and never runs, and reads `cancelled`; no
#              child reads `not-started` while it runs, and no id has both
#              a run and a never-launched outcome.
#
#   5  WAITS   parallel approvals: a parent starts Finance and Legal approval
#              children (each a WaitForSignal) and control:wait-s on both
#              (`all`); it parks without a runner slot (an outside trigger
#              still runs), one answer does not wake it, the second does, in
#              either order, and it finishes with both results in answer
#              order. Pausing a parked parent holds it while its children
#              finish (D4); `any` with leave_running returns on the first
#              answer and the sibling survives (D3); a business deadline
#              settles with what finished; a SIGKILL restart while parked
#              resumes; non-durable, untimed and onError waits are E028,
#              E029 and E131.
#
# Stage 1 ends by revoking the control approval, so it runs after every
# other stage.
#
# Usage:  STAGES=1,2,3,4,5 ./e2e/test_control_agent.sh
#
# Prereqs: Postgres + docker (isolated Valkey), a built runtara-server, and
# prebuilt components (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

STAGES="${STAGES:-1,2,3,4,5}"
IMPLEMENTED_STAGES="1,2,3,4,5"
for stage in ${STAGES//,/ }; do
    case ",${IMPLEMENTED_STAGES}," in
        *",${stage},"*) ;;
        *) print_error "Stage ${stage} is not implemented yet (implemented: ${IMPLEMENTED_STAGES})"; exit 2 ;;
    esac
done
stage_enabled() { case ",${STAGES}," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="${TEST_DB_SERVER:-control_e2e_server_$$}"
TEST_DB_RUNTIME="${TEST_DB_RUNTIME:-control_e2e_runtime_$$}"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17750}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18751}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18752}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18753}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18754}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16395}"
TEST_DATA_DIR="$(mktemp -d -t runtara_control_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
BUNDLE_DIR="${TEST_DATA_DIR}/components"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="control_e2e_$$"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

if [ -n "${POSTGRES_PASSWORD}" ]; then
    CREDENTIALS="${POSTGRES_USER}:${POSTGRES_PASSWORD}"
else
    CREDENTIALS="${POSTGRES_USER}"
fi
SERVER_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -X -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}

stop_server() {
    if [ -n "${SERVER_PID}" ]; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
        SERVER_PID=""
    fi
}

crash_server() {
    if [ -n "${SERVER_PID}" ]; then
        kill -9 "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
        SERVER_PID=""
    fi
}

cleanup() {
    local code=$?
    stop_server
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    [ ${code} -ne 0 ] && [ -f "${TEST_LOG}" ] && { echo "--- server log tail ---"; tail -60 "${TEST_LOG}"; }
    rm -rf "${TEST_DATA_DIR}"
    exit ${code}
}
trap cleanup EXIT

start_server() {
    (
        # dotenvy walks parent directories; start outside the repository.
        cd "${TEST_DATA_DIR}"
        RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
        OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
        RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
        TENANT_ID="${TENANT}" \
        MAX_CONCURRENT_EXECUTIONS=5 \
        SERVER_HOST=127.0.0.1 \
        SERVER_PORT="${TEST_PORT_PUBLIC}" \
        RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
        RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
        RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
        RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
        RUNTARA_AGENT_COMPONENTS_DIR="${BUNDLE_DIR}" \
        DATA_DIR="${TEST_DATA_DIR}" \
        RUNTARA_DEV_MODE=false \
        RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info,runtara_environment=info,runtara_component_host=info}" \
        AUTH_PROVIDER=local \
        VALKEY_HOST=127.0.0.1 \
        VALKEY_PORT="${TEST_VALKEY_PORT}" \
        OTEL_SDK_DISABLED=true \
        SQLX_OFFLINE="${SQLX_OFFLINE}" \
        exec env ${SERVER_EXTRA_ENV:-} "${RUNTARA_SERVER_BIN}"
    ) >>"${TEST_LOG}" 2>&1 &
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

instance_status() { curl -sS "${API}/workflows/instances/$1" | jq -r '.data.status // .status // empty'; }
instance_row() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(status::text,''), COALESCE(convert_from(output, 'UTF8'),''), COALESCE(error,'') FROM instances WHERE instance_id = '$1'"
}
instance_output() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(convert_from(output, 'UTF8'),'null') FROM instances WHERE instance_id = '$1'"
}
compiled_pins() {
    psql_quiet -d "${TEST_DB_SERVER}" -c \
        "SELECT COALESCE(array_to_string(trusted_pins, ','),'') FROM workflow_compilations
         WHERE tenant_id = '${TENANT}' AND workflow_id = '$1' AND version = $2"
}
request_state() {
    psql_quiet -d "${TEST_DB_SERVER}" -c \
        "SELECT state || '|' || COALESCE(terminal_reason, '') FROM execution_requests WHERE tenant_id = '${TENANT}' AND instance_id = '$1'"
}

wait_status() {
    local id="$1" want="$2" limit="$3" st deadline
    deadline=$(( $(date +%s) + limit ))
    while [ "$(date +%s)" -lt "${deadline}" ]; do
        st=$(instance_status "${id}")
        case "${st}" in "${want}") echo "${st}"; return 0 ;; completed|failed|cancelled) echo "${st}"; return 0 ;; esac
        sleep 1
    done
    echo "timeout"
}

# Create a workflow, save `graph`, compile it, and echo "id version".
make_workflow() {
    local name="$1" graph="$2" resp wf_id version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"control e2e\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${resp}"; exit 1; }
    version=$(curl -sS "${API}/workflows/${wf_id}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Compile of ${name} failed: ${resp}"; exit 1; }
    echo "${wf_id} ${version}"
}

# Launch with `data`, retrying while the version is (re)compiling.
launch() {
    local wf_id="$1" data="${2:-}" resp id
    [ -n "${data}" ] || data='{}'
    for _ in {1..90}; do
        resp=$(api_post "/workflows/${wf_id}/execute" "{\"inputs\": {\"data\": ${data}, \"variables\": {}}}")
        id=$(echo "${resp}" | jq -r '.data.instanceId // empty')
        if [ -n "${id}" ]; then echo "${id}"; return 0; fi
        sleep 2
    done
    print_error "Execute never launched: ${resp}"; exit 1
}

# test_capability of the control agent; echoes the response.
test_control() {
    api_post "/agents/control/capabilities/$1/test" "{\"input\": $2}"
}
# The test endpoint reports errors as "CODE: message".
error_code() { jq -r '(.error // "") | split(":") | first'; }

control_step() {
    jq -n --arg id "$1" --arg cap "$2" --argjson mapping "$3" '{
        id: $id, stepType: "Agent", agentId: "control", capabilityId: $cap,
        maxRetries: 0, inputMapping: $mapping
    }'
}

echo "==============================================================="
echo "E2E: control agent (stages: ${STAGES})"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
[ -f "${COMPONENTS_DIR}/runtara_agent_control.wasm" ] || { print_error "Missing control component — run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Staging a private component bundle..."
mkdir -p "${BUNDLE_DIR}"
cp "${COMPONENTS_DIR}"/*.wasm "${COMPONENTS_DIR}"/*.meta.json "${BUNDLE_DIR}/"

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
start_server

if stage_enabled 1; then
    # -----------------------------------------------------------------------
    # Stage 1: reads.
    # -----------------------------------------------------------------------
    print_step "Stage 1: boot approved the installed control bytes..."
    APPROVED=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT pin FROM approved_builtin_artifacts WHERE agent_id = 'control' AND revoked_at IS NULL")
    case "${APPROVED}" in runtara:builtin-artifacts/control-h*) ;; *) print_error "No approved control pin: '${APPROVED}'"; exit 1 ;; esac
    echo "  approved: ${APPROVED}"

    print_step "Creating the runs control will read..."
    DONE_GRAPH=$(jq -n '{
        name: "control-done", durable: true, entryPoint: "finish",
        steps: { finish: { id: "finish", stepType: "Finish",
                           inputMapping: { total: { valueType: "immediate", value: 42 } } } },
        executionPlan: [], variables: {}, inputSchema: {}, outputSchema: {}
    }')
    read -r DONE_WF _ <<< "$(make_workflow control-done "${DONE_GRAPH}")"
    DONE=$(launch "${DONE_WF}")
    [ "$(wait_status "${DONE}" completed 120)" = "completed" ] || { print_error "Target run did not complete: $(instance_row "${DONE}")"; exit 1; }

    WAIT_GRAPH=$(jq -n '{
        name: "control-waiting", durable: true, entryPoint: "approve",
        steps: { approve: { id: "approve", stepType: "WaitForSignal", name: "Approve",
                            pollIntervalMs: 500,
                            responseSchema: { approved: { type: "boolean", required: true } },
                            action: { key: "finance" } },
                 finish: { id: "finish", stepType: "Finish" } },
        executionPlan: [ { fromStep: "approve", toStep: "finish" } ],
        variables: {}, inputSchema: {}, outputSchema: {}
    }')
    read -r WAIT_WF _ <<< "$(make_workflow control-waiting "${WAIT_GRAPH}")"
    WAITING=$(launch "${WAIT_WF}")
    [ "$(wait_status "${WAITING}" suspended 120)" = "suspended" ] || { print_error "Waiting run did not park: $(instance_row "${WAITING}")"; exit 1; }
    echo "  done=${DONE} waiting=${WAITING}"

    print_step "A workflow reads both runs through control..."
    GET_STEP=$(control_step get get '{"instanceId": {"valueType": "reference", "value": "data.done"}}')
    QUERY_STEP=$(control_step query query '{
        "workflowId": {"valueType": "reference", "value": "data.doneWorkflow"},
        "statuses": {"valueType": "immediate", "value": ["completed"]},
        "pageSize": {"valueType": "immediate", "value": 10}}')
    SIGNALS_STEP=$(control_step signals list-pending-signals \
        '{"instanceId": {"valueType": "reference", "value": "data.waiting"}}')
    READER_GRAPH=$(jq -n --argjson get "${GET_STEP}" --argjson query "${QUERY_STEP}" \
        --argjson signals "${SIGNALS_STEP}" \
        '{
            name: "control-reader", durable: true, entryPoint: "get",
            steps: { get: $get, query: $query, signals: $signals,
                     finish: { id: "finish", stepType: "Finish", inputMapping: {
                         get: { valueType: "reference", value: "steps.get.outputs" },
                         query: { valueType: "reference", value: "steps.query.outputs" },
                         signals: { valueType: "reference", value: "steps.signals.outputs" } } } },
            executionPlan: [ { fromStep: "get", toStep: "query" }, { fromStep: "query", toStep: "signals" },
                             { fromStep: "signals", toStep: "finish" } ],
            variables: {}, outputSchema: {},
            inputSchema: { done: { type: "string", required: true },
                           doneWorkflow: { type: "string", required: true },
                           waiting: { type: "string", required: true } }
        }')
    read -r READER_WF READER_V <<< "$(make_workflow control-reader "${READER_GRAPH}")"
    PINS=$(compiled_pins "${READER_WF}" "${READER_V}")
    echo "${PINS}" | grep -q "${APPROVED}" || { print_error "The reader does not record the approved control pin: ${PINS}"; exit 1; }
    DATA=$(jq -nc --arg done "${DONE}" --arg wf "${DONE_WF}" --arg waiting "${WAITING}" \
        '{done: $done, doneWorkflow: $wf, waiting: $waiting}')
    READER=$(launch "${READER_WF}" "${DATA}")
    [ "$(wait_status "${READER}" completed 120)" = "completed" ] || { print_error "Reader failed: $(instance_row "${READER}")"; exit 1; }
    OUT=$(instance_output "${READER}")
    echo "  output: $(echo "${OUT}" | head -c 600)"
    [ "$(echo "${OUT}" | jq -r '.get.instance.status')" = "completed" ] || { print_error "get status wrong"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.get.instance.instanceId')" = "${DONE}" ] || { print_error "get read another run"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.get.instance.workflowId')" = "${DONE_WF}" ] || { print_error "get workflowId wrong"; exit 1; }
    [ "$(echo "${OUT}" | jq -c '.get.output')" = '{"total":42}' ] || { print_error "get output wrong"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.query.total')" = "1" ] || { print_error "query total wrong"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.query.items[0].instanceId')" = "${DONE}" ] || { print_error "query item wrong"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.signals.items | length')" = "1" ] || { print_error "expected one pending signal"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.signals.items[0].signalId')" = "approve" ] || { print_error "signalId wrong"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.signals.items[0].actionKey')" = "finance" ] || { print_error "actionKey wrong"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.signals.items[0].instanceId')" = "${WAITING}" ] || { print_error "signal instance wrong"; exit 1; }
    print_success "get, query and list-pending-signals read other runs of the tenant ✓"

    print_step "Failures carry CONTROL_* codes..."
    PROBE_STEP=$(control_step get get '{"instanceId": {"valueType": "reference", "value": "data.target"}}')
    PROBE_GRAPH=$(jq -n --argjson get "${PROBE_STEP}" \
        '{
            name: "control-probe", durable: true, entryPoint: "get",
            steps: { get: $get, finish: { id: "finish", stepType: "Finish" } },
            executionPlan: [ { fromStep: "get", toStep: "finish" } ],
            variables: {}, outputSchema: {},
            inputSchema: { target: { type: "string", required: true } }
        }')
    read -r PROBE_WF _ <<< "$(make_workflow control-probe "${PROBE_GRAPH}")"
    PROBE=$(launch "${PROBE_WF}" '{"target": "no-such-run"}')
    [ "$(wait_status "${PROBE}" failed 120)" = "failed" ] || { print_error "Missing target should fail: $(instance_row "${PROBE}")"; exit 1; }
    instance_row "${PROBE}" | grep -q "CONTROL_NOT_FOUND" || { print_error "Expected CONTROL_NOT_FOUND: $(instance_row "${PROBE}")"; exit 1; }
    QUERY_PROBE_STEP=$(control_step query query '{
        "pageSize": {"valueType": "reference", "value": "data.pageSize"},
        "callerChildren": {"valueType": "reference", "value": "data.callerChildren"}}')
    QUERY_GRAPH=$(jq -n --argjson query "${QUERY_PROBE_STEP}" \
        '{
            name: "control-query-probe", durable: true, entryPoint: "query",
            steps: { query: $query, finish: { id: "finish", stepType: "Finish" } },
            executionPlan: [ { fromStep: "query", toStep: "finish" } ],
            variables: {}, outputSchema: {},
            inputSchema: { pageSize: { type: "integer", required: true },
                           callerChildren: { type: "boolean", required: true } }
        }')
    read -r QUERY_WF _ <<< "$(make_workflow control-query-probe "${QUERY_GRAPH}")"
    RUN=$(launch "${QUERY_WF}" '{"pageSize": 101, "callerChildren": false}')
    [ "$(wait_status "${RUN}" failed 120)" = "failed" ] || { print_error "pageSize 101 should fail: $(instance_row "${RUN}")"; exit 1; }
    instance_row "${RUN}" | grep -q "CONTROL_INVALID" || { print_error "Expected CONTROL_INVALID: $(instance_row "${RUN}")"; exit 1; }
    # A run without children reads an empty page of them.
    RUN=$(launch "${QUERY_WF}" '{"pageSize": 10, "callerChildren": true}')
    [ "$(wait_status "${RUN}" completed 120)" = "completed" ] || { print_error "callerChildren should read an empty page: $(instance_row "${RUN}")"; exit 1; }
    CODE=$(test_control query '{"callerChildren": true}' | error_code)
    [ "${CODE}" = "CONTROL_REQUIRES_INSTANCE" ] || { print_error "Test invocation without a run: expected CONTROL_REQUIRES_INSTANCE, got '${CODE}'"; exit 1; }
    RESP=$(test_control get "{\"instanceId\": \"${DONE}\"}")
    echo "${RESP}" | grep -q '"total":42' || { print_error "Tenant-scoped test read failed: ${RESP}"; exit 1; }
    print_success "NOT_FOUND and INVALID from runs; callerChildren reads; REQUIRES_INSTANCE from a test invocation ✓"
fi

if stage_enabled 2; then
    # -----------------------------------------------------------------------
    # Stage 2: mutations.
    # -----------------------------------------------------------------------
    input_request() {
        psql_quiet -d "${TEST_DB_RUNTIME}" -c \
            "SELECT state || '|' || request_id || '|' || COALESCE(operation_id, '') FROM instance_input_requests WHERE instance_id = '$1'"
    }
    APPROVAL_GRAPH=$(jq -n '{
        name: "control-approval", durable: true, entryPoint: "approve",
        steps: { approve: { id: "approve", stepType: "WaitForSignal", name: "Approve",
                            pollIntervalMs: 500,
                            responseSchema: { approved: { type: "boolean", required: true } },
                            action: { key: "finance" } },
                 finish: { id: "finish", stepType: "Finish", inputMapping: {
                     decision: { valueType: "reference", value: "steps.approve.outputs" } } } },
        executionPlan: [ { fromStep: "approve", toStep: "finish" } ],
        variables: {}, inputSchema: {}, outputSchema: {}
    }')
    read -r APPROVAL_WF _ <<< "$(make_workflow control-approval "${APPROVAL_GRAPH}")"
    APPROVER=$(launch "${APPROVAL_WF}")
    [ "$(wait_status "${APPROVER}" suspended 120)" = "suspended" ] || { print_error "Approver did not park: $(instance_row "${APPROVER}")"; exit 1; }

    print_step "Stage 2: a send-signal step answers the approver, then the server is SIGKILLed..."
    # The answer step is not durable, so every replay re-invokes it and only
    # the operation receipt keeps it from answering twice.
    ANSWER_STEP=$(jq -n '{
        id: "answer", stepType: "Agent", agentId: "control", capabilityId: "send-signal",
        maxRetries: 0, durable: false, inputMapping: {
            instanceId: { valueType: "reference", value: "data.target" },
            signalId: { valueType: "immediate", value: "approve" },
            actionKey: { valueType: "immediate", value: "finance" },
            payload: { valueType: "immediate", value: { approved: true } } } }')
    SENDER_GRAPH=$(jq -n --argjson answer "${ANSWER_STEP}" '{
        name: "control-sender", durable: true, entryPoint: "answer",
        steps: { answer: $answer,
                 hold: { id: "hold", stepType: "Delay",
                         durationMs: { valueType: "immediate", value: 8000 } },
                 finish: { id: "finish", stepType: "Finish", inputMapping: {
                     answer: { valueType: "reference", value: "steps.answer.outputs" } } } },
        executionPlan: [ { fromStep: "answer", toStep: "hold" }, { fromStep: "hold", toStep: "finish" } ],
        variables: {}, outputSchema: {},
        inputSchema: { target: { type: "string", required: true } }
    }')
    read -r SENDER_WF _ <<< "$(make_workflow control-sender "${SENDER_GRAPH}")"
    SENDER=$(launch "${SENDER_WF}" "$(jq -nc --arg t "${APPROVER}" '{target: $t}')")
    ACCEPTED=""
    for _ in {1..60}; do
        ACCEPTED=$(input_request "${APPROVER}")
        case "${ACCEPTED}" in accepted*) break ;; esac
        sleep 1
    done
    case "${ACCEPTED}" in accepted*"|control:"*) ;; *) print_error "The approver was not answered under a control: operation: '${ACCEPTED}' $(instance_row "${SENDER}")"; exit 1 ;; esac
    REQUEST_ID=$(echo "${ACCEPTED}" | cut -d'|' -f2)
    OPERATION_ID=$(echo "${ACCEPTED}" | cut -d'|' -f3)
    echo "  answered ${REQUEST_ID} as ${OPERATION_ID}"
    crash_server
    start_server
    [ "$(wait_status "${SENDER}" completed 180)" = "completed" ] || { print_error "Sender did not finish after the crash: $(instance_row "${SENDER}")"; exit 1; }
    OUT=$(instance_output "${SENDER}")
    echo "  sender output: ${OUT}"
    [ "$(echo "${OUT}" | jq -r '.answer.requestId')" = "${REQUEST_ID}" ] || { print_error "The replay answered another request"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.answer.replayed')" = "true" ] || { print_error "The replayed answer step should report replayed: ${OUT}"; exit 1; }
    [ "$(input_request "${APPROVER}")" = "${ACCEPTED}" ] || { print_error "The request changed after the replay: $(input_request "${APPROVER}")"; exit 1; }
    RECEIPTS=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT count(*) || '|' || string_agg(state, ',') FROM instance_control_receipts WHERE caller_instance_id = '${SENDER}'")
    [ "${RECEIPTS}" = "1|completed" ] || { print_error "Expected one completed receipt, got '${RECEIPTS}'"; exit 1; }
    [ "$(wait_status "${APPROVER}" completed 120)" = "completed" ] || { print_error "Approver did not finish: $(instance_row "${APPROVER}")"; exit 1; }
    instance_output "${APPROVER}" | grep -q '"approved":true' || { print_error "Approver saw another answer: $(instance_output "${APPROVER}")"; exit 1; }
    AUDIT=$(psql_quiet -d "${TEST_DB_SERVER}" -c \
        "SELECT count(*) FROM audit_events WHERE tenant_id = '${TENANT}' AND event_type = 'control.send_signal' AND resource_id = '${APPROVER}' AND payload::text NOT LIKE '%approved%'")
    [ "${AUDIT}" -ge 1 ] || { print_error "send-signal was not audited without its payload"; exit 1; }
    print_success "send-signal answered once across a SIGKILL replay (replayed: true, one receipt) ✓"

    print_step "Stage 2: pausing a waiting run is immediate; resume relaunches it..."
    PAUSED=$(launch "${APPROVAL_WF}")
    [ "$(wait_status "${PAUSED}" suspended 120)" = "suspended" ] || { print_error "Run did not park: $(instance_row "${PAUSED}")"; exit 1; }
    RESP=$(api_post "/workflows/instances/${PAUSED}/pause" '{}')
    [ "$(echo "${RESP}" | jq -r '.data.outcome')" = "applied" ] || { print_error "Pause of a waiting run should apply at once: ${RESP}"; exit 1; }
    REASON=$(test_control get "{\"instanceId\": \"${PAUSED}\"}" | jq -r '.output.instance.suspensionReason // .result.instance.suspensionReason // empty')
    [ "${REASON}" = "paused" ] || { print_error "Expected suspensionReason paused, got '${REASON}'"; exit 1; }
    RESP=$(api_post "/workflows/instances/${PAUSED}/pause" '{}')
    [ "$(echo "${RESP}" | jq -r '.data.outcome')" = "unchanged" ] || { print_error "A second pause should be unchanged: ${RESP}"; exit 1; }
    RESP=$(api_post "/workflows/instances/${PAUSED}/resume" '{}')
    [ "$(echo "${RESP}" | jq -r '.data.outcome')" = "applied" ] || { print_error "Resume failed: ${RESP}"; exit 1; }
    print_success "A waiting run paused at once (applied, suspensionReason paused) and resumed ✓"

    print_step "Stage 2: lifecycle commands reach children only; the reserved prefix is refused..."
    for cap in cancel pause; do
        STEP=$(jq -n --arg cap "${cap}" '{
            id: "command", stepType: "Agent", agentId: "control", capabilityId: $cap,
            maxRetries: 0, inputMapping: {
                instanceId: { valueType: "reference", value: "data.target" } } }')
        GRAPH=$(jq -n --argjson step "${STEP}" --arg name "control-${cap}-probe" '{
            name: $name, durable: true, entryPoint: "command",
            steps: { command: $step, finish: { id: "finish", stepType: "Finish" } },
            executionPlan: [ { fromStep: "command", toStep: "finish" } ],
            variables: {}, outputSchema: {},
            inputSchema: { target: { type: "string", required: true } }
        }')
        read -r CMD_WF _ <<< "$(make_workflow "control-${cap}-probe" "${GRAPH}")"
        RUN=$(launch "${CMD_WF}" "$(jq -nc --arg t "${PAUSED}" '{target: $t}')")
        [ "$(wait_status "${RUN}" failed 120)" = "failed" ] || { print_error "${cap} of a non-child should fail: $(instance_row "${RUN}")"; exit 1; }
        instance_row "${RUN}" | grep -q "CONTROL_NOT_CHILD" || { print_error "Expected CONTROL_NOT_CHILD from ${cap}: $(instance_row "${RUN}")"; exit 1; }
        FAILED_RUN="${RUN}"
    done
    [ "$(instance_status "${PAUSED}")" != "cancelled" ] || { print_error "A refused cancel still cancelled the run"; exit 1; }
    CODE=$(test_control pause "{\"instanceId\": \"${PAUSED}\"}" | error_code)
    [ "${CODE}" = "CONTROL_REQUIRES_INSTANCE" ] || { print_error "A test invocation of pause should need a run, got '${CODE}'"; exit 1; }
    RESP=$(curl -sS -o /dev/null -w "%{http_code}" -X POST -H "Content-Type: application/json" \
        -d '{}' "${API}/workflows/instances/${FAILED_RUN}/resume")
    [ "${RESP}" = "400" ] || { print_error "Resuming a failed run should be 400 NotResumable, got ${RESP}"; exit 1; }
    OPEN=$(launch "${APPROVAL_WF}")
    [ "$(wait_status "${OPEN}" suspended 120)" = "suspended" ] || { print_error "Run did not park: $(instance_row "${OPEN}")"; exit 1; }
    OPEN_REQUEST=$(input_request "${OPEN}" | cut -d'|' -f2)
    RESP=$(curl -sS -o /dev/null -w "%{http_code}" -X POST -H "Content-Type: application/json" \
        -d "{\"requestId\": \"${OPEN_REQUEST}\", \"operationId\": \"control:forged\", \"payload\": {\"approved\": true}}" \
        "${API}/signals/${OPEN}")
    [ "${RESP}" = "400" ] || { print_error "A public control: operation id should be refused, got ${RESP}"; exit 1; }
    case "$(input_request "${OPEN}")" in open*) ;; *) print_error "The forged answer was accepted"; exit 1 ;; esac
    print_success "cancel/pause of a non-child: CONTROL_NOT_CHILD; failed run NotResumable; control: prefix refused ✓"
fi

if stage_enabled 3; then
    # -----------------------------------------------------------------------
    # Stage 3: start.
    # -----------------------------------------------------------------------
    children_of() {
        psql_quiet -d "${TEST_DB_SERVER}" -c \
            "SELECT count(*) FROM execution_requests WHERE tenant_id = '${TENANT}' AND parent_instance_id = '$1'"
    }
    child_labelled() {
        psql_quiet -d "${TEST_DB_SERVER}" -c \
            "SELECT instance_id FROM execution_requests WHERE tenant_id = '${TENANT}' AND parent_instance_id = '$1' AND run_label = '$2'"
    }
    start_step() {
        # id, workflow reference, label, policy, durable
        jq -n --arg id "$1" --arg label "$3" --arg policy "$4" --argjson durable "$5" --arg wf "$2" '{
            id: $id, stepType: "Agent", agentId: "control", capabilityId: "start",
            maxRetries: 0, durable: $durable, inputMapping: {
                workflowId: { valueType: "reference", value: $wf },
                runLabel: { valueType: "immediate", value: $label },
                parentClosePolicy: { valueType: "immediate", value: $policy },
                inputs: { valueType: "immediate", value: { data: { n: 1 }, variables: {} } } } }'
    }
    CHILD_GRAPH=$(jq -n '{
        name: "control-child", durable: true, entryPoint: "approve",
        steps: { approve: { id: "approve", stepType: "WaitForSignal", name: "Approve",
                            pollIntervalMs: 500,
                            responseSchema: { approved: { type: "boolean", required: true } } },
                 finish: { id: "finish", stepType: "Finish", inputMapping: {
                     decision: { valueType: "reference", value: "steps.approve.outputs" } } } },
        executionPlan: [ { fromStep: "approve", toStep: "finish" } ],
        variables: {}, outputSchema: {}, inputSchema: { n: { type: "integer" } }
    }')
    read -r CHILD_WF _ <<< "$(make_workflow control-child "${CHILD_GRAPH}")"

    print_step "Stage 3: a parent starts two children, then the server is SIGKILLed..."
    # startA is not durable, so the replay after the crash re-invokes it and
    # only the admission's idempotency keeps it from starting a second child.
    # `settle` runs after the restart, so the children have launched and
    # parked on their WaitForSignal before the parent commands them.
    STEPS=$(jq -n \
        --argjson a "$(start_step startA data.child a cancel false)" \
        --argjson b "$(start_step startB data.child b leave_running true)" \
        '{
            startA: $a, startB: $b,
            hold: { id: "hold", stepType: "Delay", durationMs: { valueType: "immediate", value: 8000 } },
            settle: { id: "settle", stepType: "Delay", durationMs: { valueType: "immediate", value: 8000 } },
            query: { id: "query", stepType: "Agent", agentId: "control", capabilityId: "query", maxRetries: 0,
                     inputMapping: { callerChildren: { valueType: "immediate", value: true },
                                     pageSize: { valueType: "immediate", value: 10 } } },
            getA: { id: "getA", stepType: "Agent", agentId: "control", capabilityId: "get", maxRetries: 0,
                    inputMapping: { instanceId: { valueType: "reference", value: "steps.startA.outputs.instanceId" } } },
            pauseA: { id: "pauseA", stepType: "Agent", agentId: "control", capabilityId: "pause", maxRetries: 0,
                      inputMapping: { instanceId: { valueType: "reference", value: "steps.startA.outputs.instanceId" } } },
            resumeA: { id: "resumeA", stepType: "Agent", agentId: "control", capabilityId: "resume", maxRetries: 0,
                       inputMapping: { instanceId: { valueType: "reference", value: "steps.startA.outputs.instanceId" } } },
            signalB: { id: "signalB", stepType: "Agent", agentId: "control", capabilityId: "send-signal", maxRetries: 0,
                       inputMapping: { instanceId: { valueType: "reference", value: "steps.startB.outputs.instanceId" },
                                       signalId: { valueType: "immediate", value: "approve" },
                                       payload: { valueType: "immediate", value: { approved: true } } } },
            cancelA: { id: "cancelA", stepType: "Agent", agentId: "control", capabilityId: "cancel", maxRetries: 0,
                       inputMapping: { instanceId: { valueType: "reference", value: "steps.startA.outputs.instanceId" },
                                       graceMs: { valueType: "immediate", value: 0 } } },
            finish: { id: "finish", stepType: "Finish", inputMapping: {
                startA: { valueType: "reference", value: "steps.startA.outputs" },
                startB: { valueType: "reference", value: "steps.startB.outputs" },
                query: { valueType: "reference", value: "steps.query.outputs" },
                getA: { valueType: "reference", value: "steps.getA.outputs" },
                pauseA: { valueType: "reference", value: "steps.pauseA.outputs" },
                resumeA: { valueType: "reference", value: "steps.resumeA.outputs" },
                signalB: { valueType: "reference", value: "steps.signalB.outputs" },
                cancelA: { valueType: "reference", value: "steps.cancelA.outputs" } } }
        }')
    PARENT_GRAPH=$(jq -n --argjson steps "${STEPS}" '{
        name: "control-parent", durable: true, entryPoint: "startA", steps: $steps,
        executionPlan: [ { fromStep: "startA", toStep: "startB" }, { fromStep: "startB", toStep: "hold" },
                         { fromStep: "hold", toStep: "settle" }, { fromStep: "settle", toStep: "query" }, { fromStep: "query", toStep: "getA" },
                         { fromStep: "getA", toStep: "pauseA" }, { fromStep: "pauseA", toStep: "resumeA" },
                         { fromStep: "resumeA", toStep: "signalB" }, { fromStep: "signalB", toStep: "cancelA" },
                         { fromStep: "cancelA", toStep: "finish" } ],
        variables: {}, outputSchema: {}, inputSchema: { child: { type: "string", required: true } }
    }')
    read -r PARENT_WF _ <<< "$(make_workflow control-parent "${PARENT_GRAPH}")"
    PARENT=$(launch "${PARENT_WF}" "$(jq -nc --arg c "${CHILD_WF}" '{child: $c}')")
    for _ in {1..60}; do
        [ "$(children_of "${PARENT}")" = "2" ] && break
        sleep 0.5
    done
    [ "$(children_of "${PARENT}")" = "2" ] || { print_error "The parent did not admit two children: $(instance_row "${PARENT}")"; exit 1; }
    CHILD_A=$(child_labelled "${PARENT}" a)
    CHILD_B=$(child_labelled "${PARENT}" b)
    echo "  parent=${PARENT} a=${CHILD_A} b=${CHILD_B}"
    # Crash once both children have launched and parked, while the parent is
    # still in its durable Delay.
    for child in "${CHILD_A}" "${CHILD_B}"; do
        [ "$(wait_status "${child}" suspended 60)" = "suspended" ] || { print_error "Child ${child} did not launch and park: $(instance_row "${child}")"; exit 1; }
    done
    [ "$(instance_status "${PARENT}")" != "completed" ] || { print_error "The parent finished before the crash"; exit 1; }
    crash_server
    start_server
    [ "$(wait_status "${PARENT}" completed 240)" = "completed" ] || { print_error "The parent did not finish after the crash: $(instance_row "${PARENT}")"; exit 1; }
    OUT=$(instance_output "${PARENT}")
    echo "  parent output: $(echo "${OUT}" | head -c 800)"
    [ "$(echo "${OUT}" | jq -r '.startA.instanceId')" = "${CHILD_A}" ] || { print_error "The replayed start returned another child"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.startA.replayed')" = "true" ] || { print_error "The replayed start should report replayed: ${OUT}"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.startB.instanceId')" = "${CHILD_B}" ] || { print_error "startB returned another child"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.startA.workflowId')" = "${CHILD_WF}" ] || { print_error "startA workflowId wrong"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.startA.runLabel')" = "a" ] || { print_error "startA runLabel wrong"; exit 1; }
    [ "$(children_of "${PARENT}")" = "2" ] || { print_error "The replay admitted another child: $(children_of "${PARENT}")"; exit 1; }
    print_success "start admitted two children; the SIGKILL replay returned the same child (replayed: true) ✓"

    print_step "Stage 3: the parent read, commanded and answered its children..."
    [ "$(echo "${OUT}" | jq -r '.query.total')" = "2" ] || { print_error "query(callerChildren) should see two children"; exit 1; }
    [ "$(echo "${OUT}" | jq -r --arg p "${PARENT}" '[.query.items[] | select(.parentInstanceId == $p)] | length')" = "2" ] \
        || { print_error "query items should name the parent"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.getA.instance.parentInstanceId')" = "${PARENT}" ] || { print_error "get should report parentInstanceId"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.pauseA.outcome')" = "applied" ] || { print_error "Pausing the waiting child should apply at once"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.resumeA.outcome')" = "applied" ] || { print_error "Resuming the paused child failed"; exit 1; }
    case "$(echo "${OUT}" | jq -r '.cancelA.outcome')" in applied|requested) ;; *) print_error "Cancelling the child failed"; exit 1 ;; esac
    [ -n "$(echo "${OUT}" | jq -r '.signalB.requestId // empty')" ] || { print_error "send-signal to the child failed"; exit 1; }
    [ "$(wait_status "${CHILD_B}" completed 120)" = "completed" ] || { print_error "Child b did not finish: $(instance_row "${CHILD_B}")"; exit 1; }
    instance_output "${CHILD_B}" | grep -q '"approved":true' || { print_error "Child b saw another answer"; exit 1; }
    [ "$(wait_status "${CHILD_A}" cancelled 120)" = "cancelled" ] || { print_error "Child a was not cancelled: $(instance_row "${CHILD_A}")"; exit 1; }
    LISTED=$(curl -sS "${API}/executions?parentInstanceId=${PARENT}&size=10")
    [ "$(echo "${LISTED}" | jq -r --arg p "${PARENT}" '[.data.content[] | select(.parentInstanceId == $p)] | length')" = "2" ] \
        || { print_error "The executions API should list two children: $(echo "${LISTED}" | head -c 400)"; exit 1; }
    [ "$(curl -sS "${API}/workflows/instances/${CHILD_B}" | jq -r '.data.parentInstanceId')" = "${PARENT}" ] \
        || { print_error "The instance API should report parentInstanceId"; exit 1; }
    print_success "query/get see the children; pause, resume, cancel and send-signal reached them; the API filters by parent ✓"

    print_step "Stage 3: a reused label conflicts..."
    DUP_GRAPH=$(jq -n \
        --argjson one "$(start_step one data.child dup cancel true)" \
        --argjson two "$(start_step two data.child dup cancel true)" '{
        name: "control-duplicate-label", durable: true, entryPoint: "one",
        steps: { one: $one, two: $two, finish: { id: "finish", stepType: "Finish" } },
        executionPlan: [ { fromStep: "one", toStep: "two" }, { fromStep: "two", toStep: "finish" } ],
        variables: {}, outputSchema: {}, inputSchema: { child: { type: "string", required: true } }
    }')
    read -r DUP_WF _ <<< "$(make_workflow control-duplicate-label "${DUP_GRAPH}")"
    DUP=$(launch "${DUP_WF}" "$(jq -nc --arg c "${CHILD_WF}" '{child: $c}')")
    [ "$(wait_status "${DUP}" failed 120)" = "failed" ] || { print_error "A reused label should fail the run: $(instance_row "${DUP}")"; exit 1; }
    instance_row "${DUP}" | grep -q "CONTROL_LABEL_CONFLICT" || { print_error "Expected CONTROL_LABEL_CONFLICT: $(instance_row "${DUP}")"; exit 1; }
    [ "$(children_of "${DUP}")" = "1" ] || { print_error "The conflicting start admitted a child"; exit 1; }
    print_success "A reused run label is CONTROL_LABEL_CONFLICT and admits nothing ✓"

    print_step "Stage 3: control's children hold at most their share (4 of 5)..."
    # A child that keeps running (an in-process agent sleep, not a parked
    # Delay), so it holds its slot while the parent starts the next one.
    BUSY_GRAPH=$(jq -n '{
        name: "control-busy", durable: true, entryPoint: "busy",
        steps: { busy: { id: "busy", stepType: "Agent", agentId: "utils", capabilityId: "delay-in-ms",
                         maxRetries: 0, inputMapping: { delay_value: { valueType: "immediate", value: 20000 } } },
                 finish: { id: "finish", stepType: "Finish" } },
        executionPlan: [ { fromStep: "busy", toStep: "finish" } ],
        variables: {}, outputSchema: {}, inputSchema: { n: { type: "integer" } }
    }')
    read -r BUSY_WF _ <<< "$(make_workflow control-busy "${BUSY_GRAPH}")"
    FAN_STEPS='{}'
    FAN_PLAN='[]'
    for i in 1 2 3 4 5; do
        FAN_STEPS=$(echo "${FAN_STEPS}" | jq --argjson step "$(start_step "s${i}" data.child "busy-${i}" leave_running true)" --arg id "s${i}" '. + {($id): $step}')
        next="s$((i + 1))"; [ "${i}" = "5" ] && next="finish"
        FAN_PLAN=$(echo "${FAN_PLAN}" | jq --arg from "s${i}" --arg to "${next}" '. + [{fromStep: $from, toStep: $to}]')
    done
    FAN_GRAPH=$(jq -n --argjson steps "${FAN_STEPS}" --argjson plan "${FAN_PLAN}" '{
        name: "control-fan-out", durable: true, entryPoint: "s1",
        steps: ($steps + { finish: { id: "finish", stepType: "Finish" } }),
        executionPlan: $plan, variables: {}, outputSchema: {},
        inputSchema: { child: { type: "string", required: true } }
    }')
    read -r FAN_WF _ <<< "$(make_workflow control-fan-out "${FAN_GRAPH}")"
    OUTSIDE_GRAPH=$(jq -n '{
        name: "control-outside", durable: true, entryPoint: "finish",
        steps: { finish: { id: "finish", stepType: "Finish" } },
        executionPlan: [], variables: {}, inputSchema: {}, outputSchema: {}
    }')
    read -r OUTSIDE_WF _ <<< "$(make_workflow control-outside "${OUTSIDE_GRAPH}")"
    FAN=$(launch "${FAN_WF}" "$(jq -nc --arg c "${BUSY_WF}" '{child: $c}')")
    [ "$(wait_status "${FAN}" failed 120)" = "failed" ] || { print_error "The fifth child should not fit: $(instance_row "${FAN}")"; exit 1; }
    instance_row "${FAN}" | grep -q "CONTROL_CAPACITY_RATE_LIMITED" || { print_error "Expected CONTROL_CAPACITY_RATE_LIMITED: $(instance_row "${FAN}")"; exit 1; }
    [ "$(children_of "${FAN}")" = "4" ] || { print_error "Expected four admitted children, got $(children_of "${FAN}")"; exit 1; }
    RESP=$(api_post "/workflows/${OUTSIDE_WF}/execute" '{"inputs": {"data": {}, "variables": {}}}')
    OUTSIDE=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
    [ -n "${OUTSIDE}" ] || { print_error "An outside trigger should be admitted while control holds its share: ${RESP}"; exit 1; }
    [ "$(wait_status "${OUTSIDE}" completed 120)" = "completed" ] || { print_error "The outside run did not finish: $(instance_row "${OUTSIDE}")"; exit 1; }
    print_success "The fifth running child is CONTROL_CAPACITY_RATE_LIMITED; an outside trigger still runs ✓"
fi

if stage_enabled 4; then
    # -----------------------------------------------------------------------
    # Stage 4: ownership.
    # -----------------------------------------------------------------------
    labelled_child() {
        psql_quiet -d "${TEST_DB_SERVER}" -c \
            "SELECT instance_id FROM execution_requests WHERE tenant_id = '${TENANT}' AND parent_instance_id = '$1' AND run_label = '$2'"
    }
    own_start_step() {
        # id, label, policy
        jq -n --arg id "$1" --arg label "$2" --arg policy "$3" '{
            id: $id, stepType: "Agent", agentId: "control", capabilityId: "start",
            maxRetries: 0, durable: true, inputMapping: {
                workflowId: { valueType: "reference", value: "data.child" },
                runLabel: { valueType: "immediate", value: $label },
                parentClosePolicy: { valueType: "immediate", value: $policy },
                inputs: { valueType: "immediate", value: { data: {}, variables: {} } } } }'
    }
    own_get_step() {
        jq -n --arg id "$1" --arg ref "$2" '{
            id: $id, stepType: "Agent", agentId: "control", capabilityId: "get", maxRetries: 0,
            inputMapping: { instanceId: { valueType: "reference", value: $ref } } }'
    }
    # control:get of a run through the test endpoint; echoes its status.
    # control:get of a run through the (rate-limited) test endpoint; echoes
    # its status.
    control_status() {
        local resp
        for _ in {1..10}; do
            resp=$(test_control get "{\"instanceId\": \"$1\"}")
            case "${resp}" in *"Rate limit exceeded"*) sleep 1 ;; *) break ;; esac
        done
        echo "${resp}" \
            | jq -r '[.. | objects | select(has("instance")) | .instance.status][0] // "unreadable"' 2>/dev/null \
            || echo "unreadable"
    }
    no_running_not_started() {
        local id st
        for id in "$@"; do
            st=$(control_status "${id}")
            [ "${st}" != "not_started" ] || { print_error "Child ${id} reads not-started while it runs"; exit 1; }
        done
    }
    # Wait for a child to be cancelled, checking it never reads not-started.
    wait_cancelled() {
        local id="$1" limit="$2" st deadline
        deadline=$(( $(date +%s) + limit ))
        while [ "$(date +%s)" -lt "${deadline}" ]; do
            st=$(instance_status "${id}")
            [ "${st}" = "cancelled" ] && return 0
            case "${st}" in completed|failed) print_error "Child ${id} ended ${st}, not cancelled"; exit 1 ;; esac
            no_running_not_started "${id}"
            sleep 2
        done
        print_error "Child ${id} was not cancelled: $(instance_row "${id}")"; exit 1
    }
    make_workflow_uncompiled() {
        local name="$1" graph="$2" resp wf_id
        resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"control e2e\"}")
        wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
        [ -n "${wf_id}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }
        resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
        [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${resp}"; exit 1; }
        echo "${wf_id}"
    }

    # Earlier stages leave busy children holding control's share of the
    # concurrency limit for a while; start once they have finished.
    for _ in {1..120}; do
        [ "$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM instances WHERE tenant_id = '${TENANT}' AND status IN ('pending', 'running')")" = "0" ] && break
        sleep 1
    done

    OWNED_GRAPH=$(jq -n '{
        name: "control-owned", durable: true, entryPoint: "approve",
        steps: { approve: { id: "approve", stepType: "WaitForSignal", name: "Approve",
                            pollIntervalMs: 500,
                            responseSchema: { approved: { type: "boolean", required: true } } },
                 finish: { id: "finish", stepType: "Finish" } },
        executionPlan: [ { fromStep: "approve", toStep: "finish" } ],
        variables: {}, outputSchema: {}, inputSchema: {}
    }')
    read -r OWNED_WF _ <<< "$(make_workflow control-owned "${OWNED_GRAPH}")"
    CHILD_DATA=$(jq -nc --arg c "${OWNED_WF}" '{child: $c, missing: "no-such-run"}')
    PARENT_INPUTS='{"child": {"type": "string", "required": true}, "missing": {"type": "string", "required": true}}'

    print_step "Stage 4: a parent fails; its cancel child follows, its leave_running child stays..."
    STEPS=$(jq -n \
        --argjson c "$(own_start_step startC c cancel)" \
        --argjson l "$(own_start_step startL l leave_running)" \
        --argjson boom "$(own_get_step boom data.missing)" '{
        startC: $c, startL: $l, boom: $boom,
        hold: { id: "hold", stepType: "Delay", durationMs: { valueType: "immediate", value: 4000 } },
        finish: { id: "finish", stepType: "Finish" } }')
    FAILING_GRAPH=$(jq -n --argjson steps "${STEPS}" --argjson inputs "${PARENT_INPUTS}" '{
        name: "control-failing-parent", durable: true, entryPoint: "startC", steps: $steps,
        executionPlan: [ { fromStep: "startC", toStep: "startL" }, { fromStep: "startL", toStep: "hold" },
                         { fromStep: "hold", toStep: "boom" }, { fromStep: "boom", toStep: "finish" } ],
        variables: {}, outputSchema: {}, inputSchema: $inputs }')
    read -r FAILING_WF _ <<< "$(make_workflow control-failing-parent "${FAILING_GRAPH}")"
    P_FAIL=$(launch "${FAILING_WF}" "${CHILD_DATA}")
    for _ in {1..60}; do [ -n "$(labelled_child "${P_FAIL}" l)" ] && break; sleep 0.5; done
    C_FAIL=$(labelled_child "${P_FAIL}" c); L_FAIL=$(labelled_child "${P_FAIL}" l)
    [ -n "${C_FAIL}" ] && [ -n "${L_FAIL}" ] || { print_error "The failing parent did not start both children"; exit 1; }
    for child in "${C_FAIL}" "${L_FAIL}"; do
        [ "$(wait_status "${child}" suspended 90)" = "suspended" ] || { print_error "Child ${child} did not park: $(instance_row "${child}")"; exit 1; }
        ST=$(control_status "${child}")
        [ "${ST}" = "suspended" ] || { print_error "control:get should read the parked child as suspended, got '${ST}': $(test_control get "{\"instanceId\": \"${child}\"}" | head -c 400)"; exit 1; }
    done
    [ "$(wait_status "${P_FAIL}" failed 120)" = "failed" ] || { print_error "The parent should fail: $(instance_row "${P_FAIL}")"; exit 1; }
    wait_cancelled "${C_FAIL}" 90
    grep -q "parent ${P_FAIL} terminated (failed)" "${TEST_LOG}" || { print_error "No parent-close reason in the log"; exit 1; }
    grep -q "platform:parent-close" "${TEST_LOG}" || { print_error "The cascade did not act as platform:parent-close"; exit 1; }
    print_success "Parent failed: the cancel child was cancelled (parent ${P_FAIL} terminated (failed)) ✓"

    print_step "Stage 4: a parent completes; its cancel child follows..."
    STEPS=$(jq -n \
        --argjson c "$(own_start_step startC c cancel)" \
        --argjson l "$(own_start_step startL l leave_running)" \
        --argjson getC "$(own_get_step getC steps.startC.outputs.instanceId)" \
        --argjson getL "$(own_get_step getL steps.startL.outputs.instanceId)" '{
        startC: $c, startL: $l, getC: $getC, getL: $getL,
        hold: { id: "hold", stepType: "Delay", durationMs: { valueType: "immediate", value: 4000 } },
        finish: { id: "finish", stepType: "Finish", inputMapping: {
            getC: { valueType: "reference", value: "steps.getC.outputs" },
            getL: { valueType: "reference", value: "steps.getL.outputs" } } } }')
    DONE_PARENT_GRAPH=$(jq -n --argjson steps "${STEPS}" --argjson inputs "${PARENT_INPUTS}" '{
        name: "control-completing-parent", durable: true, entryPoint: "startC", steps: $steps,
        executionPlan: [ { fromStep: "startC", toStep: "startL" }, { fromStep: "startL", toStep: "hold" },
                         { fromStep: "hold", toStep: "getC" }, { fromStep: "getC", toStep: "getL" },
                         { fromStep: "getL", toStep: "finish" } ],
        variables: {}, outputSchema: {}, inputSchema: $inputs }')
    read -r DONE_PARENT_WF _ <<< "$(make_workflow control-completing-parent "${DONE_PARENT_GRAPH}")"
    P_DONE=$(launch "${DONE_PARENT_WF}" "${CHILD_DATA}")
    [ "$(wait_status "${P_DONE}" completed 120)" = "completed" ] || { print_error "The parent should complete: $(instance_row "${P_DONE}")"; exit 1; }
    OUT=$(instance_output "${P_DONE}")
    C_DONE=$(echo "${OUT}" | jq -r '.getC.instance.instanceId'); L_DONE=$(echo "${OUT}" | jq -r '.getL.instance.instanceId')
    for status in "$(echo "${OUT}" | jq -r '.getC.instance.status')" "$(echo "${OUT}" | jq -r '.getL.instance.status')"; do
        case "${status}" in not_started|null|"") print_error "The parent read a live child as '${status}': ${OUT}"; exit 1 ;; esac
    done
    wait_cancelled "${C_DONE}" 90
    print_success "Parent completed: the cancel child was cancelled; the parent never read a live child as not-started ✓"

    print_step "Stage 4: the parent ends while the server is down (SIGKILL)..."
    STEPS=$(jq -n --argjson c "$(own_start_step startC c cancel)" '{
        startC: $c,
        hold: { id: "hold", stepType: "Delay", durationMs: { valueType: "immediate", value: 120000 } },
        finish: { id: "finish", stepType: "Finish" } }')
    CRASH_GRAPH=$(jq -n --argjson steps "${STEPS}" --argjson inputs "${PARENT_INPUTS}" '{
        name: "control-crashing-parent", durable: true, entryPoint: "startC", steps: $steps,
        executionPlan: [ { fromStep: "startC", toStep: "hold" }, { fromStep: "hold", toStep: "finish" } ],
        variables: {}, outputSchema: {}, inputSchema: $inputs }')
    read -r CRASH_WF _ <<< "$(make_workflow control-crashing-parent "${CRASH_GRAPH}")"
    P_CRASH=$(launch "${CRASH_WF}" "${CHILD_DATA}")
    for _ in {1..60}; do [ -n "$(labelled_child "${P_CRASH}" c)" ] && break; sleep 0.5; done
    C_CRASH=$(labelled_child "${P_CRASH}" c)
    [ "$(wait_status "${C_CRASH}" suspended 90)" = "suspended" ] || { print_error "Child did not park: $(instance_row "${C_CRASH}")"; exit 1; }
    [ "$(wait_status "${P_CRASH}" suspended 90)" = "suspended" ] || { print_error "Parent did not park in its Delay: $(instance_row "${P_CRASH}")"; exit 1; }
    crash_server
    # No parent code runs: the parent is ended out of band while the platform
    # is down, and only the parent link can carry the policy.
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "UPDATE instances SET status = 'failed', finished_at = NOW(), sleep_until = NULL, error = 'e2e: ended while the platform was down' WHERE instance_id = '${P_CRASH}'" >/dev/null
    start_server
    wait_cancelled "${C_CRASH}" 90
    [ "$(instance_status "${L_FAIL}")" = "suspended" ] || { print_error "The leave_running child did not survive: $(instance_row "${L_FAIL}")"; exit 1; }
    [ "$(instance_status "${L_DONE}")" = "suspended" ] || { print_error "The leave_running child did not survive: $(instance_row "${L_DONE}")"; exit 1; }
    no_running_not_started "${L_FAIL}" "${L_DONE}"
    print_success "After a SIGKILL restart the cascade cancelled the child; leave_running children survived ✓"

    print_step "Stage 4: a child held in admission is cancelled there and never runs..."
    HELD_GRAPH=$(jq -n '{
        name: "control-held", durable: true, entryPoint: "finish",
        steps: { finish: { id: "finish", stepType: "Finish" } },
        executionPlan: [], variables: {}, inputSchema: {}, outputSchema: {} }')
    STEPS=$(jq -n \
        --argjson h "$(own_start_step startH h cancel)" \
        --argjson boom "$(own_get_step boom data.missing)" '{
        startH: $h, boom: $boom,
        hold: { id: "hold", stepType: "Delay", durationMs: { valueType: "immediate", value: 3000 } },
        finish: { id: "finish", stepType: "Finish" } }')
    HOLD_GRAPH=$(jq -n --argjson steps "${STEPS}" --argjson inputs "${PARENT_INPUTS}" '{
        name: "control-holding-parent", durable: true, entryPoint: "startH", steps: $steps,
        executionPlan: [ { fromStep: "startH", toStep: "hold" }, { fromStep: "hold", toStep: "boom" },
                         { fromStep: "boom", toStep: "finish" } ],
        variables: {}, outputSchema: {}, inputSchema: $inputs }')
    read -r HOLD_WF _ <<< "$(make_workflow control-holding-parent "${HOLD_GRAPH}")"
    # With compilations off the held child's workflow, saved only now, can
    # never compile, so it stays in admission (retried as not compiled yet)
    # while its parent runs and fails.
    crash_server
    SERVER_EXTRA_ENV="MAX_CONCURRENT_COMPILATIONS=0" start_server
    HELD_WF=$(make_workflow_uncompiled control-held "${HELD_GRAPH}")
    P_HOLD=$(launch "${HOLD_WF}" "$(jq -nc --arg c "${HELD_WF}" '{child: $c, missing: "no-such-run"}')")
    [ "$(wait_status "${P_HOLD}" failed 120)" = "failed" ] || { print_error "The holding parent should fail: $(instance_row "${P_HOLD}")"; exit 1; }
    H=$(labelled_child "${P_HOLD}" h)
    [ -n "${H}" ] || { print_error "The holding parent did not admit its child"; exit 1; }
    ST=""
    for _ in {1..60}; do
        ST=$(request_state "${H}")
        case "${ST}" in cancelled*) break ;; esac
        sleep 1
    done
    [ "${ST}" = "cancelled|parent ${P_HOLD} terminated (failed)" ] || { print_error "The held child was not cancelled in admission: '${ST}'"; exit 1; }
    [ -z "$(instance_row "${H}")" ] || { print_error "The held child ran: $(instance_row "${H}")"; exit 1; }
    [ "$(control_status "${H}")" = "cancelled" ] || { print_error "The held child should read cancelled: $(test_control get "{\"instanceId\": \"${H}\"}")"; exit 1; }
    OUTCOME=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT outcome || '|' || reason FROM instance_external_outcomes WHERE instance_id = '${H}'")
    [ "${OUTCOME}" = "cancelled|parent ${P_HOLD} terminated (failed)" ] || { print_error "Unexpected outcome: '${OUTCOME}'"; exit 1; }
    crash_server
    start_server
    sleep 3
    [ -z "$(instance_row "${H}")" ] || { print_error "The held child ran after the restart: $(instance_row "${H}")"; exit 1; }
    BOTH=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT count(*) FROM instance_external_outcomes AS o JOIN instances AS i ON i.instance_id = o.instance_id")
    [ "${BOTH}" = "0" ] || { print_error "${BOTH} ids have both a run and a never-launched outcome"; exit 1; }
    print_success "The held child was cancelled in admission, never ran, and reads cancelled ✓"
fi

if stage_enabled 5; then
    # -----------------------------------------------------------------------
    # Stage 5: parallel approvals through control:wait.
    # -----------------------------------------------------------------------
    labelled() {
        psql_quiet -d "${TEST_DB_SERVER}" -c \
            "SELECT instance_id FROM execution_requests WHERE tenant_id = '${TENANT}' AND parent_instance_id = '$1' AND run_label = '$2'"
    }
    open_request() {
        psql_quiet -d "${TEST_DB_RUNTIME}" -c \
            "SELECT request_id FROM instance_input_requests WHERE instance_id = '$1' AND state = 'open'"
    }
    parked_reason() {
        psql_quiet -d "${TEST_DB_RUNTIME}" -c \
            "SELECT COALESCE(termination_reason::text, '') FROM instances WHERE instance_id = '$1'"
    }
    busy_runs() {
        psql_quiet -d "${TEST_DB_RUNTIME}" -c \
            "SELECT count(*) FROM instances WHERE tenant_id = '${TENANT}' AND status IN ('pending', 'running')"
    }
    # Answer a child's open WaitForSignal request through the public API.
    answer() {
        local child="$1" approved="$2" request resp
        request=""
        for _ in {1..60}; do
            request=$(open_request "${child}")
            [ -n "${request}" ] && break
            sleep 1
        done
        [ -n "${request}" ] || { print_error "Child ${child} has no open request: $(instance_row "${child}")"; exit 1; }
        resp=$(api_post "/signals/${child}" "$(jq -nc --arg r "${request}" --arg o "e2e-answer-${child}" --argjson a "${approved}" \
            '{requestId: $r, operationId: $o, payload: {approved: $a}}')")
        [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Answering ${child} failed: ${resp}"; exit 1; }
    }
    approvals_step() {
        # id, label, policy
        jq -n --arg id "$1" --arg label "$2" --arg policy "$3" '{
            id: $id, stepType: "Agent", agentId: "control", capabilityId: "start",
            maxRetries: 0, durable: true, inputMapping: {
                workflowId: { valueType: "reference", value: "data.approver" },
                runLabel: { valueType: "immediate", value: $label },
                parentClosePolicy: { valueType: "immediate", value: $policy },
                inputs: { valueType: "immediate", value: { data: {}, variables: {} } } } }'
    }
    # A parent that starts the Finance and Legal approvals, then waits.
    approvals_graph() {
        # name, policy, mode, with-deadline
        jq -n --arg name "$1" --argjson finance "$(approvals_step finance finance "$2")" \
            --argjson legal "$(approvals_step legal legal "$2")" --arg mode "$3" --argjson timed "$4" '
            ({ instanceIds: { valueType: "composite", value: [
                   { valueType: "reference", value: "steps.finance.outputs.instanceId" },
                   { valueType: "reference", value: "steps.legal.outputs.instanceId" } ] },
               mode: { valueType: "immediate", value: $mode } }
             + (if $timed then { deadline: { valueType: "reference", value: "data.deadline" } } else {} end)) as $mapping
            | {
            name: $name, durable: true, entryPoint: "finance",
            steps: { finance: $finance, legal: $legal,
                     wait: { id: "wait", stepType: "Agent", agentId: "control", capabilityId: "wait",
                             maxRetries: 0, durable: true, timeout: 600000, inputMapping: $mapping },
                     finish: { id: "finish", stepType: "Finish", inputMapping: {
                         result: { valueType: "reference", value: "steps.wait.outputs" },
                         finance: { valueType: "reference", value: "steps.finance.outputs.instanceId" },
                         legal: { valueType: "reference", value: "steps.legal.outputs.instanceId" } } } },
            executionPlan: [ { fromStep: "finance", toStep: "legal" }, { fromStep: "legal", toStep: "wait" },
                             { fromStep: "wait", toStep: "finish" } ],
            variables: {}, outputSchema: {},
            inputSchema: ({ approver: { type: "string", required: true } }
                          + (if $timed then { deadline: { type: "integer", required: true } } else {} end))
        }'
    }
    # Launch a parent; wait until it parks on both approvals; echo "parent finance legal".
    parked_parent() {
        local wf="$1" data="$2" parent finance legal
        parent=$(launch "${wf}" "${data}")
        for _ in {1..60}; do [ -n "$(labelled "${parent}" legal)" ] && break; sleep 0.5; done
        finance=$(labelled "${parent}" finance); legal=$(labelled "${parent}" legal)
        [ -n "${finance}" ] && [ -n "${legal}" ] || { print_error "Parent ${parent} did not start both approvals"; exit 1; }
        for child in "${finance}" "${legal}"; do
            [ "$(wait_status "${child}" suspended 90)" = "suspended" ] || { print_error "Approval ${child} did not park: $(instance_row "${child}")"; exit 1; }
        done
        [ "$(wait_status "${parent}" suspended 90)" = "suspended" ] || { print_error "Parent ${parent} did not park: $(instance_row "${parent}")"; exit 1; }
        for _ in {1..30}; do [ "$(parked_reason "${parent}")" = "waiting_instances" ] && break; sleep 0.5; done
        [ "$(parked_reason "${parent}")" = "waiting_instances" ] || { print_error "Parent ${parent} is not waiting on its children: '$(parked_reason "${parent}")'"; exit 1; }
        echo "${parent} ${finance} ${legal}"
    }
    # `parked_parent` runs in a command substitution, where its `exit` only
    # leaves the subshell: stop here when it did not name all three runs.
    parked_or_exit() { [ -n "${3:-}" ] || { print_error "No parked parent"; exit 1; }; }

    # Earlier stages leave children holding control's share for a while.
    for _ in {1..120}; do [ "$(busy_runs)" = "0" ] && break; sleep 1; done

    APPROVER_GRAPH=$(jq -n '{
        name: "control-approver", durable: true, entryPoint: "approve",
        steps: { approve: { id: "approve", stepType: "WaitForSignal", name: "Approve",
                            pollIntervalMs: 500,
                            responseSchema: { approved: { type: "boolean", required: true } } },
                 finish: { id: "finish", stepType: "Finish", inputMapping: {
                     decision: { valueType: "reference", value: "steps.approve.outputs" } } } },
        executionPlan: [ { fromStep: "approve", toStep: "finish" } ],
        variables: {}, outputSchema: {}, inputSchema: {}
    }')
    read -r APPROVER_WF _ <<< "$(make_workflow control-approver "${APPROVER_GRAPH}")"
    read -r ALL_WF _ <<< "$(make_workflow control-approvals-all "$(approvals_graph control-approvals-all cancel all false)")"
    read -r ANY_WF _ <<< "$(make_workflow control-approvals-any "$(approvals_graph control-approvals-any leave_running any false)")"
    read -r TIMED_WF _ <<< "$(make_workflow control-approvals-timed "$(approvals_graph control-approvals-timed cancel all true)")"
    APPROVER_DATA=$(jq -nc --arg a "${APPROVER_WF}" '{approver: $a}')

    for order in finance-first legal-first; do
        print_step "Stage 5: parallel approvals, answered ${order}..."
        read -r PARENT FINANCE LEGAL <<< "$(parked_parent "${ALL_WF}" "${APPROVER_DATA}")"
        parked_or_exit "${PARENT:-}" "${FINANCE:-}" "${LEGAL:-}"
        # Parked with both approvals parked: nothing holds a runner slot, so an
        # outside trigger still runs under MAX_CONCURRENT_EXECUTIONS=5.
        [ "$(busy_runs)" = "0" ] || { print_error "A parked parent or approval holds a slot: $(busy_runs) busy"; exit 1; }
        REASON=$(test_control get "{\"instanceId\": \"${PARENT}\"}" | jq -r '.output.instance.suspensionReason // .result.instance.suspensionReason // empty')
        [ "${REASON}" = "waiting_instances" ] || { print_error "Expected suspensionReason waiting_instances, got '${REASON}'"; exit 1; }
        if [ "${order}" = "finance-first" ]; then FIRST="${FINANCE}"; SECOND="${LEGAL}"; else FIRST="${LEGAL}"; SECOND="${FINANCE}"; fi
        answer "${FIRST}" true
        [ "$(wait_status "${FIRST}" completed 90)" = "completed" ] || { print_error "The first approval did not finish: $(instance_row "${FIRST}")"; exit 1; }
        sleep 3
        [ "$(instance_status "${PARENT}")" = "suspended" ] || { print_error "One answer woke an \`all\` wait: $(instance_row "${PARENT}")"; exit 1; }
        answer "${SECOND}" false
        [ "$(wait_status "${PARENT}" completed 120)" = "completed" ] || { print_error "The parent did not finish: $(instance_row "${PARENT}")"; exit 1; }
        OUT=$(instance_output "${PARENT}")
        [ "$(echo "${OUT}" | jq -r '.result.resolution')" = "satisfied" ] || { print_error "Unexpected resolution: ${OUT}"; exit 1; }
        [ "$(echo "${OUT}" | jq -r '.result.finished | map(.instanceId) | join(",")')" = "${FIRST},${SECOND}" ] \
            || { print_error "Finished should list the approvals in answer order: ${OUT}"; exit 1; }
        [ "$(echo "${OUT}" | jq -r --arg f "${FINANCE}" '.result.finished[] | select(.instanceId == $f) | .output.decision.approved')" \
            = "$([ "${order}" = "finance-first" ] && echo true || echo false)" ] || { print_error "Finance's decision is missing: ${OUT}"; exit 1; }
        [ "$(echo "${OUT}" | jq -r '.result.remaining | length')" = "0" ] || { print_error "Nothing should remain: ${OUT}"; exit 1; }
        [ "$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM instance_waits WHERE waiter_instance_id = '${PARENT}'")" = "0" ] \
            || { print_error "The settled wait was not released"; exit 1; }
        [ "$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM instance_agent_continuations WHERE instance_id = '${PARENT}'")" = "0" ] \
            || { print_error "The continuation was not released"; exit 1; }
        print_success "Both approvals answered (${order}): the parent parked slot-free and resumed with both results ✓"
    done

    print_step "Stage 5: pausing a parked parent holds it (D4)..."
    read -r PARENT FINANCE LEGAL <<< "$(parked_parent "${ALL_WF}" "${APPROVER_DATA}")"
    parked_or_exit "${PARENT:-}" "${FINANCE:-}" "${LEGAL:-}"
    RESP=$(api_post "/workflows/instances/${PARENT}/pause" '{}')
    [ "$(echo "${RESP}" | jq -r '.data.outcome')" = "applied" ] || { print_error "Pausing the parked parent should apply at once: ${RESP}"; exit 1; }
    answer "${FINANCE}" true
    answer "${LEGAL}" true
    [ "$(wait_status "${LEGAL}" completed 90)" = "completed" ] || { print_error "Legal did not finish: $(instance_row "${LEGAL}")"; exit 1; }
    [ "$(wait_status "${FINANCE}" completed 90)" = "completed" ] || { print_error "Finance did not finish: $(instance_row "${FINANCE}")"; exit 1; }
    sleep 5
    [ "$(instance_status "${PARENT}")" = "suspended" ] || { print_error "A paused parent was woken by its children: $(instance_row "${PARENT}")"; exit 1; }
    REASON=$(test_control get "{\"instanceId\": \"${PARENT}\"}" | jq -r '.output.instance.suspensionReason // .result.instance.suspensionReason // empty')
    [ "${REASON}" = "paused" ] || { print_error "Expected suspensionReason paused, got '${REASON}'"; exit 1; }
    RESP=$(api_post "/workflows/instances/${PARENT}/resume" '{}')
    [ "$(echo "${RESP}" | jq -r '.data.outcome')" = "applied" ] || { print_error "Resume failed: ${RESP}"; exit 1; }
    [ "$(wait_status "${PARENT}" completed 120)" = "completed" ] || { print_error "The resumed parent did not finish: $(instance_row "${PARENT}")"; exit 1; }
    [ "$(instance_output "${PARENT}" | jq -r '.result.resolution')" = "satisfied" ] || { print_error "Unexpected: $(instance_output "${PARENT}")"; exit 1; }
    print_success "A paused parent stayed paused while its children finished, and resumed to their results ✓"

    print_step "Stage 5: \`any\` with leave_running returns on the first answer (D3)..."
    read -r PARENT FINANCE LEGAL <<< "$(parked_parent "${ANY_WF}" "${APPROVER_DATA}")"
    parked_or_exit "${PARENT:-}" "${FINANCE:-}" "${LEGAL:-}"
    answer "${LEGAL}" true
    [ "$(wait_status "${PARENT}" completed 120)" = "completed" ] || { print_error "The any-parent did not finish: $(instance_row "${PARENT}")"; exit 1; }
    OUT=$(instance_output "${PARENT}")
    [ "$(echo "${OUT}" | jq -r '.result.mode + "|" + .result.resolution')" = "any|satisfied" ] || { print_error "Unexpected: ${OUT}"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.result.finished | map(.instanceId) | join(",")')" = "${LEGAL}" ] || { print_error "Only legal finished: ${OUT}"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.result.remaining | join(",")')" = "${FINANCE}" ] || { print_error "Finance remains: ${OUT}"; exit 1; }
    sleep 8
    [ "$(instance_status "${FINANCE}")" = "suspended" ] || { print_error "The leave_running approval did not survive its parent: $(instance_row "${FINANCE}")"; exit 1; }
    answer "${FINANCE}" true
    [ "$(wait_status "${FINANCE}" completed 90)" = "completed" ] || { print_error "The surviving approval did not finish: $(instance_row "${FINANCE}")"; exit 1; }
    print_success "\`any\` returned on the first answer; the leave_running sibling kept running ✓"

    print_step "Stage 5: a business deadline returns what finished so far..."
    DEADLINE=$(( $(date +%s) * 1000 + 25000 ))
    read -r PARENT FINANCE LEGAL <<< "$(parked_parent "${TIMED_WF}" "$(jq -nc --arg a "${APPROVER_WF}" --argjson d "${DEADLINE}" '{approver: $a, deadline: $d}')")"
    parked_or_exit "${PARENT:-}" "${FINANCE:-}" "${LEGAL:-}"
    answer "${FINANCE}" true
    [ "$(wait_status "${FINANCE}" completed 90)" = "completed" ] || { print_error "Finance did not finish: $(instance_row "${FINANCE}")"; exit 1; }
    [ "$(wait_status "${PARENT}" completed 120)" = "completed" ] || { print_error "The timed parent did not finish at its deadline: $(instance_row "${PARENT}")"; exit 1; }
    [ "$(( $(date +%s) * 1000 ))" -ge "${DEADLINE}" ] || { print_error "The timed parent finished before its deadline"; exit 1; }
    OUT=$(instance_output "${PARENT}")
    [ "$(echo "${OUT}" | jq -r '.result.resolution')" = "deadline" ] || { print_error "Expected resolution deadline: ${OUT}"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.result.finished | map(.instanceId) | join(",")')" = "${FINANCE}" ] || { print_error "Only finance finished: ${OUT}"; exit 1; }
    [ "$(echo "${OUT}" | jq -r '.result.remaining | join(",")')" = "${LEGAL}" ] || { print_error "Legal remains: ${OUT}"; exit 1; }
    print_success "The deadline settled the wait with finance finished and legal remaining ✓"

    print_step "Stage 5: a restart while parked resumes the parent..."
    read -r PARENT FINANCE LEGAL <<< "$(parked_parent "${ALL_WF}" "${APPROVER_DATA}")"
    parked_or_exit "${PARENT:-}" "${FINANCE:-}" "${LEGAL:-}"
    crash_server
    start_server
    answer "${FINANCE}" true
    answer "${LEGAL}" true
    [ "$(wait_status "${PARENT}" completed 180)" = "completed" ] || { print_error "The parent did not resume after the restart: $(instance_row "${PARENT}")"; exit 1; }
    [ "$(instance_output "${PARENT}" | jq -r '.result.finished | length')" = "2" ] || { print_error "Unexpected: $(instance_output "${PARENT}")"; exit 1; }
    print_success "A parent parked across a SIGKILL restart resumed to both results ✓"

    print_step "Stage 5: bad placements are validation errors (E028, E029, E131)..."
    bad_wait() {
        # extra step fields
        jq -n --argjson extra "$1" '{
            id: "wait", stepType: "Agent", agentId: "control", capabilityId: "wait", maxRetries: 0,
            durable: true, timeout: 600000,
            inputMapping: { instanceIds: { valueType: "immediate", value: ["x"] } } } + $extra'
    }
    expect_code() {
        local code="$1" graph="$2" resp wf_id
        resp=$(api_post /workflows/create "{\"name\": \"control-bad-${code}\", \"description\": \"control e2e\"}")
        wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
        resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
        if ! echo "${resp}" | grep -q "${code}"; then
            resp=$(api_post "/workflows/${wf_id}/versions/1/compile" '{}' 300)
        fi
        echo "${resp}" | grep -q "${code}" || { print_error "Expected ${code}: $(echo "${resp}" | head -c 600)"; exit 1; }
    }
    single_graph() {
        jq -n --argjson step "$1" '{ name: "control-bad", durable: true, entryPoint: "wait",
            steps: { wait: $step, finish: { id: "finish", stepType: "Finish" } },
            executionPlan: [ { fromStep: "wait", toStep: "finish" } ],
            variables: {}, inputSchema: {}, outputSchema: {} }'
    }
    expect_code E028 "$(single_graph "$(bad_wait '{"durable": false}')")"
    expect_code E029 "$(single_graph "$(bad_wait '{"timeout": null}' | jq 'del(.timeout)')")"
    ON_ERROR=$(jq -n --argjson wait "$(bad_wait '{}')" '{ name: "control-bad", durable: true, entryPoint: "boom",
        steps: { boom: { id: "boom", stepType: "Agent", agentId: "control", capabilityId: "get", maxRetries: 0,
                         inputMapping: { instanceId: { valueType: "immediate", value: "no-such-run" } } },
                 wait: $wait, finish: { id: "finish", stepType: "Finish" } },
        executionPlan: [ { fromStep: "boom", toStep: "finish" },
                         { fromStep: "boom", toStep: "wait", label: "onError" },
                         { fromStep: "wait", toStep: "finish" } ],
        variables: {}, inputSchema: {}, outputSchema: {} }')
    expect_code E131 "${ON_ERROR}"
    print_success "Non-durable (E028), untimed (E029) and onError (E131) waits are refused ✓"
fi

if stage_enabled 1; then
    print_step "Revoking the approved control digest and restarting..."
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "UPDATE approved_builtin_artifacts SET revoked_at = now(), revoked_reason = 'e2e' WHERE pin = '${APPROVED}'" >/dev/null
    stop_server
    start_server
    [ "$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM approved_builtin_artifacts WHERE pin = '${APPROVED}' AND revoked_at IS NOT NULL")" = "1" ] \
        || { print_error "Boot undid the revocation"; exit 1; }
    CODE=$(test_control get "{\"instanceId\": \"${DONE}\"}" | error_code)
    [ "${CODE}" = "CONTROL_DENIED" ] || { print_error "Revoked control should be denied, got '${CODE}'"; exit 1; }
    RESP=$(api_post "/workflows/${READER_WF}/execute" "{\"inputs\": {\"data\": ${DATA}, \"variables\": {}}}")
    INST=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
    if [ -n "${INST}" ]; then
        ST=""
        for _ in {1..60}; do
            ST=$(request_state "${INST}")
            case "${ST}" in terminal*) break ;; esac
            case "$(instance_status "${INST}")" in completed) print_error "A revoked control artifact ran to completion"; exit 1 ;; failed) ST="failed"; break ;; esac
            sleep 2
        done
        case "${ST}" in terminal*|failed) ;; *) print_error "The pinned reader should not run after revocation, got '${ST}'"; exit 1 ;; esac
        echo "  reader launch after revocation: ${ST}"
        [ -z "$(instance_row "${INST}")" ] || { print_error "A run started on a revoked control artifact: $(instance_row "${INST}")"; exit 1; }
    else
        echo "  reader launch refused: $(echo "${RESP}" | head -c 300)"
    fi
    # Not ready: the recompile pins the same revoked bytes, and that is recorded
    # as a terminal failure carrying the pin.
    COMPILED=$(psql_quiet -d "${TEST_DB_SERVER}" -c \
        "SELECT compilation_status || '|' || COALESCE(array_to_string(trusted_pins, ','),'')
         FROM workflow_compilations WHERE tenant_id = '${TENANT}' AND workflow_id = '${READER_WF}' AND version = ${READER_V}")
    echo "  reader compilation: $(echo "${COMPILED}" | cut -c1-120)"
    case "${COMPILED}" in failed*"${APPROVED}"*) ;; *) print_error "The revoked pin should leave the reader not ready: ${COMPILED}"; exit 1 ;; esac
    print_success "Revoked digest: control calls denied, the pinned workflow no longer runs ✓"
fi

echo
print_success "Control agent stages ${STAGES} passed."
