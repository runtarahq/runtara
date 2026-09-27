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
# Later stages (ownership, waits) are added by their slices. Stage 1 ends by
# revoking the control approval, so it runs after every other stage.
#
# Usage:  STAGES=1,2,3 ./e2e/test_control_agent.sh
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

STAGES="${STAGES:-1,2,3}"
IMPLEMENTED_STAGES="1,2,3"
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
        exec "${RUNTARA_SERVER_BIN}"
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
