#!/bin/bash
# E2E Test: queryable workflow state (SetState / GetState) on an isolated
# live server.
#
#   1. Published state: approval runs publish {order, stage, dueAt}; readers
#      see it while the runs wait, through the single-run executions endpoint,
#      POST /executions/query filters, and a reader workflow's control
#      `query` (state filter) and `get-state`. Approving one moves it out of
#      the filter.
#   2. Replay: a read-modify-write loop (GetState -> SetState -> durable
#      Delay) parks and replays on every iteration and still appends exactly
#      once per iteration; the write log holds one row per write.
#   3. Local state: an embedded child keeps its own state (the same loop, and
#      a durable Split whose cached result carries the state its body wrote);
#      the parent publishes nothing. A non-durable workflow keeps local state.
#   4. Refusals: an undeclared field is E135 at save; a value that does not
#      match stateSchema fails the step with STATE_INVALID_VALUE.
#
# Usage:  ./e2e/test_workflow_state.sh
#
# Prereqs: Postgres (POSTGRES_HOST/PORT/USER/PASSWORD), docker (isolated
# Valkey), a built runtara-server, and prebuilt components
# (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-postgres}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-}"

TEST_DB_SERVER="${TEST_DB_SERVER:-state_e2e_server_$$}"
TEST_DB_RUNTIME="${TEST_DB_RUNTIME:-state_e2e_runtime_$$}"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17780}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18781}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18782}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18783}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18784}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16398}"
TEST_DATA_DIR="$(mktemp -d -t runtara_state_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="state_e2e_$$"

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
BASE="http://127.0.0.1:${TEST_PORT_PUBLIC}"
API="${BASE}/api/runtime"

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -X -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}
api_get() {
    curl -sS --max-time 60 "${API}$1"
}

stop_server() {
    if [ -n "${SERVER_PID}" ]; then
        kill "${SERVER_PID}" 2>/dev/null || true
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
    [ ${code} -ne 0 ] && [ -f "${TEST_LOG}" ] && { echo "--- server log tail ---"; tail -80 "${TEST_LOG}"; }
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
        SERVER_HOST=127.0.0.1 \
        SERVER_PORT="${TEST_PORT_PUBLIC}" \
        RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
        RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
        RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
        RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
        RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
        DATA_DIR="${TEST_DATA_DIR}" \
        RUNTARA_DEV_MODE=true \
        RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info}" \
        AUTH_PROVIDER=local \
        VALKEY_HOST=127.0.0.1 \
        VALKEY_PORT="${TEST_VALKEY_PORT}" \
        OTEL_SDK_DISABLED=true \
        SQLX_OFFLINE="${SQLX_OFFLINE}" \
        exec "${RUNTARA_SERVER_BIN}"
    ) >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!

    for _ in {1..90}; do
        if curl -sS -o /dev/null -w "%{http_code}" "${BASE}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            print_error "Server exited during boot."; exit 1
        fi
    done
    print_error "Server did not become healthy."; exit 1
}

fail() { print_error "$1"; exit 1; }

# Assert jq filter $2 over JSON $1 is `true`, or fail with message $3.
expect_true() {
    local got
    got=$(echo "$1" | jq -r "$2" 2>/dev/null || echo "jq-error")
    [ "${got}" = "true" ] || fail "$3 (filter: $2)
$(echo "$1" | head -c 3000)"
}

instance() { api_get "/workflows/instances/$1"; }
instance_status() { instance "$1" | jq -r '.data.status // empty'; }
instance_output() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(convert_from(output, 'UTF8'),'null') FROM instances WHERE instance_id = '$1'"
}
instance_error() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(error,'') FROM instances WHERE instance_id = '$1'"
}

# Wait for run $1 to reach status $2 (or any terminal one); echo the status.
wait_status() {
    local id="$1" want="$2" limit="${3:-120}" st deadline
    deadline=$(( $(date +%s) + limit ))
    while [ "$(date +%s)" -lt "${deadline}" ]; do
        st=$(instance_status "${id}")
        case "${st}" in "${want}") echo "${st}"; return 0 ;; completed|failed|cancelled) echo "${st}"; return 0 ;; esac
        sleep 1
    done
    echo "timeout"
}

# Create a workflow, save `graph`, compile it; echo "id version".
make_workflow() {
    local name="$1" graph="$2" resp wf_id version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"state e2e\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || fail "Workflow create failed: ${resp}"
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || fail "Update of ${name} failed: ${resp}"
    version=$(api_get "/workflows/${wf_id}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || fail "Compile of ${name} failed: ${resp}"
    echo "${wf_id} ${version}"
}

# Launch workflow $1 with data $2, retrying while it compiles; echo the id.
launch() {
    local wf_id="$1" data="${2:-}" resp id
    [ -n "${data}" ] || data='{}'
    for _ in {1..90}; do
        resp=$(api_post "/workflows/${wf_id}/execute" "{\"inputs\": {\"data\": ${data}, \"variables\": {}}}")
        id=$(echo "${resp}" | jq -r '.data.instanceId // empty')
        if [ -n "${id}" ]; then echo "${id}"; return 0; fi
        sleep 2
    done
    fail "Execute never launched: ${resp}"
}

# Answer the open WaitForSignal action of run $2 of workflow $1.
approve() {
    local wf="$1" iid="$2" actions action request resp
    for _ in {1..30}; do
        actions=$(api_get "/workflows/${wf}/instances/${iid}/actions")
        action=$(echo "${actions}" | jq -r '(.data.actions // .actions // [])[0].actionId // empty')
        request=$(echo "${actions}" | jq -r '(.data.actions // .actions // [])[0].requestId // empty')
        [ -n "${action}" ] && break
        sleep 1
    done
    [ -n "${action}" ] || fail "No open action on ${iid}: ${actions}"
    resp=$(api_post "/workflows/${wf}/instances/${iid}/actions/${action}/submit" \
        "$(jq -nc --arg r "${request}" --arg o "$(uuidgen)" '{requestId: $r, operationId: $o, payload: {approved: true}}')")
    expect_true "${resp}" '(.success // true) != false' "Approving ${iid} failed"
}

echo "==============================================================="
echo "E2E: queryable workflow state"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || fail "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"
[ -f "${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm" ] || fail "Missing components in ${COMPONENTS_DIR} — run scripts/build-agent-components.sh"
[ -f "${COMPONENTS_DIR}/runtara_agent_control.wasm" ] || fail "Missing the control agent in ${COMPONENTS_DIR}"
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || fail "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"
docker info >/dev/null 2>&1 || fail "docker required (isolated Valkey)"
command -v uuidgen >/dev/null || fail "uuidgen required"

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases ${TEST_DB_SERVER} and ${TEST_DB_RUNTIME}..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
start_server

imm() { jq -nc --argjson v "$1" '{valueType: "immediate", value: $v}'; }
ref() { jq -nc --arg v "$1" '{valueType: "reference", value: $v}'; }

# A While body: read the state, append "x" to `trail`, then a durable Delay
# that parks the run; the wake replays it from the start.
LOOP_BODY=$(jq -n '{
    entryPoint: "get",
    steps: {
        get: { id: "get", stepType: "GetState" },
        put: { id: "put", stepType: "SetState", values: {
            trail: { valueType: "template",
                     value: "{% if steps.get.outputs.trail %}{{ steps.get.outputs.trail }}{% endif %}x" } } },
        nap: { id: "nap", stepType: "Delay", durationMs: { valueType: "immediate", value: 1000 } },
        done: { id: "done", stepType: "Finish" }
    },
    executionPlan: [
        { fromStep: "get", toStep: "put" },
        { fromStep: "put", toStep: "nap" },
        { fromStep: "nap", toStep: "done" }
    ]
}')
LOOP_GRAPH=$(jq -n --argjson body "${LOOP_BODY}" '{
    name: "state-loop", entryPoint: "loop",
    stateSchema: { trail: { type: "string", label: "Trail" } },
    steps: {
        loop: { id: "loop", stepType: "While",
                condition: { type: "operation", op: "LT", arguments: [
                    { valueType: "reference", value: "loop.index" },
                    { valueType: "immediate", value: 3 } ] },
                config: { maxIterations: 10 },
                subgraph: $body },
        final: { id: "final", stepType: "GetState" },
        finish: { id: "finish", stepType: "Finish", inputMapping: {
            trail: { valueType: "reference", value: "steps.final.outputs.trail" } } }
    },
    executionPlan: [
        { fromStep: "loop", toStep: "final" },
        { fromStep: "final", toStep: "finish" }
    ]
}')

# ---------------------------------------------------------------------------
# 1. Published state and its readers.
# ---------------------------------------------------------------------------
print_step "1. Published state: approvals wait with {order, stage, dueAt}..."
APPROVAL_GRAPH=$(jq -n '{
    name: "state-approval", entryPoint: "open",
    inputSchema: {
        order: { type: "string" },
        dueAt: { type: "string", format: "datetime" }
    },
    stateSchema: {
        order: { type: "string", label: "Order" },
        stage: { type: "string", label: "Stage", enum: ["received", "approval", "approved"] },
        dueAt: { type: "string", format: "datetime", label: "Due" }
    },
    steps: {
        open: { id: "open", stepType: "SetState", values: {
            order: { valueType: "reference", value: "data.order" },
            stage: { valueType: "immediate", value: "approval" },
            dueAt: { valueType: "reference", value: "data.dueAt" } } },
        wait: { id: "wait", stepType: "WaitForSignal" },
        close: { id: "close", stepType: "SetState", values: {
            stage: { valueType: "immediate", value: "approved" } } },
        read: { id: "read", stepType: "GetState" },
        finish: { id: "finish", stepType: "Finish", inputMapping: {
            order: { valueType: "reference", value: "steps.read.outputs.order" },
            stage: { valueType: "reference", value: "steps.read.outputs.stage" } } }
    },
    executionPlan: [
        { fromStep: "open", toStep: "wait" },
        { fromStep: "wait", toStep: "close" },
        { fromStep: "close", toStep: "read" },
        { fromStep: "read", toStep: "finish" }
    ]
}')
read -r APPROVAL_WF _ <<<"$(make_workflow state-approval "${APPROVAL_GRAPH}")"
A1=$(launch "${APPROVAL_WF}" '{"order": "o-1", "dueAt": "2026-09-29T10:00:00+02:00"}')
A2=$(launch "${APPROVAL_WF}" '{"order": "o-2", "dueAt": "2026-10-05T10:00:00Z"}')
for run in "${A1}" "${A2}"; do
    [ "$(wait_status "${run}" suspended 120)" = "suspended" ] || fail "Approval ${run} did not park"
done

RUN=$(instance "${A1}")
expect_true "${RUN}" '.data.state == {"order": "o-1", "stage": "approval", "dueAt": "2026-09-29T08:00:00.000Z"}' "GET instance state of a waiting run (datetime in UTC)"
expect_true "${RUN}" '.data.stateUpdatedAt != null' "stateUpdatedAt missing"

query_total() {
    api_post /executions/query "$(jq -nc --arg wf "${APPROVAL_WF}" --argjson state "$1" '{workflowId: $wf, state: $state}')" \
        | jq -r '.data.totalElements // .data.total // (.data.content | length)'
}
[ "$(query_total '[{"field":"stage","op":"eq","value":"approval"}]')" = "2" ] || fail "state filter stage=approval should list 2 runs"
[ "$(query_total '[{"field":"dueAt","op":"lt","value":"2026-10-01T00:00:00+02:00"}]')" = "1" ] || fail "dueAt range filter should list 1 run"
[ "$(query_total '[{"field":"order","op":"in","value":["o-2","o-9"]}]')" = "1" ] || fail "in filter should list 1 run"
[ "$(query_total '[{"field":"nope","op":"exists"}]')" = "0" ] || fail "a missing field must not match"
RESP=$(api_post /executions/query '{"state":[{"field":"stage","op":"like","value":"x"}]}')
expect_true "${RESP}" '.success == false' "An unknown filter operator must be refused"
RESP=$(api_post /executions/query "$(jq -nc --arg wf "${APPROVAL_WF}" '{workflowId: $wf, state: [{field: "stage", op: "eq", value: "approval"}]}')")
expect_true "${RESP}" '[.data.content[] | has("state")] | any | not' "A listing must never return state"
print_success "The executions API returns and filters published state"

READER_GRAPH=$(jq -n --arg wf "${APPROVAL_WF}" '{
    name: "state-reader", entryPoint: "q",
    steps: {
        q: { id: "q", stepType: "Agent", agentId: "control", capabilityId: "query", maxRetries: 0,
             inputMapping: {
                 workflowId: { valueType: "immediate", value: $wf },
                 state: { valueType: "immediate", value: [{ field: "stage", op: "eq", value: "approval" }] },
                 sortBy: { valueType: "immediate", value: "created_at" },
                 order: { valueType: "immediate", value: "asc" },
                 pageSize: { valueType: "immediate", value: 10 } } },
        g: { id: "g", stepType: "Agent", agentId: "control", capabilityId: "get-state", maxRetries: 0,
             inputMapping: { instanceId: { valueType: "reference", value: "steps.q.outputs.items[0].instanceId" } } },
        finish: { id: "finish", stepType: "Finish", inputMapping: {
            total: { valueType: "reference", value: "steps.q.outputs.total" },
            order: { valueType: "reference", value: "steps.g.outputs.state.order" },
            version: { valueType: "reference", value: "steps.g.outputs.instance.version" } } }
    },
    executionPlan: [ { fromStep: "q", toStep: "g" }, { fromStep: "g", toStep: "finish" } ]
}')
read -r READER_WF _ <<<"$(make_workflow state-reader "${READER_GRAPH}")"
R=$(launch "${READER_WF}")
[ "$(wait_status "${R}" completed 120)" = "completed" ] || fail "Reader run did not complete: $(instance_error "${R}")"
OUT=$(instance_output "${R}")
expect_true "${OUT}" '.total == 2 and .order == "o-1" and .version != null' "Reader workflow saw the published state"
print_success "A workflow lists runs by state (control query) and reads one (get-state)"

approve "${APPROVAL_WF}" "${A1}"
[ "$(wait_status "${A1}" completed 120)" = "completed" ] || fail "Approved run did not complete: $(instance_error "${A1}")"
expect_true "$(instance_output "${A1}")" '.order == "o-1" and .stage == "approved"' "GetState after the approval"
expect_true "$(instance "${A1}")" '.data.state.stage == "approved"' "Finished run keeps its state"
[ "$(query_total '[{"field":"stage","op":"eq","value":"approval"}]')" = "1" ] || fail "The approved run must leave the approval filter"
print_success "Approving moves a run out of the filter; a finished run keeps its state"

# ---------------------------------------------------------------------------
# 2. Replay: read-modify-write through parks.
# ---------------------------------------------------------------------------
print_step "2. Read-modify-write loop parks and replays on every iteration..."
read -r LOOP_WF _ <<<"$(make_workflow state-loop "${LOOP_GRAPH}")"
L=$(launch "${LOOP_WF}")
[ "$(wait_status "${L}" completed 120)" = "completed" ] || fail "Loop run did not complete: $(instance_error "${L}")"
expect_true "$(instance_output "${L}")" '.trail == "xxx"' "The loop appended once per iteration"
expect_true "$(instance "${L}")" '.data.state == {"trail": "xxx"}' "Published loop state"
WRITES=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM instance_state_writes WHERE instance_id = '${L}'")
[ "${WRITES}" = "3" ] || fail "Expected 3 logged writes for 3 iterations, found ${WRITES}"
# Each durable Delay parks the run and the wake relaunches it from the start,
# so every later pass replays the earlier SetState/GetState steps.
LAUNCHES=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM instance_launches WHERE instance_id = '${L}'")
[ "${LAUNCHES}" -ge 4 ] || fail "The loop should have launched once plus once per park, found ${LAUNCHES}"
echo "  loop run launched ${LAUNCHES} times; writes ${WRITES}"
print_success "Replayed SetState changes nothing; replayed GetState reads what it read"

# ---------------------------------------------------------------------------
# 3. Local state: an embedded child and a non-durable workflow.
# ---------------------------------------------------------------------------
print_step "3. Local state in an embedded child and a non-durable workflow..."
SPLIT_CHILD_GRAPH=$(jq -n '{
    name: "state-split-child", entryPoint: "each",
    inputSchema: { items: { type: "array" } },
    stateSchema: { last: { type: "string" } },
    steps: {
        each: { id: "each", stepType: "Split",
                config: { value: { valueType: "reference", value: "data.items" } },
                subgraph: {
                    entryPoint: "mark",
                    steps: {
                        mark: { id: "mark", stepType: "SetState", values: {
                            last: { valueType: "reference", value: "data" } } },
                        done: { id: "done", stepType: "Finish" } },
                    executionPlan: [ { fromStep: "mark", toStep: "done" } ] } },
        nap: { id: "nap", stepType: "Delay", durationMs: { valueType: "immediate", value: 1000 } },
        after: { id: "after", stepType: "GetState" },
        finish: { id: "finish", stepType: "Finish", inputMapping: {
            last: { valueType: "reference", value: "steps.after.outputs.last" } } }
    },
    executionPlan: [
        { fromStep: "each", toStep: "nap" },
        { fromStep: "nap", toStep: "after" },
        { fromStep: "after", toStep: "finish" }
    ]
}')
read -r SPLIT_CHILD_WF _ <<<"$(make_workflow state-split-child "${SPLIT_CHILD_GRAPH}")"
read -r LOOP_CHILD_WF _ <<<"$(make_workflow state-loop-child "$(echo "${LOOP_GRAPH}" | jq -c '.name = "state-loop-child"')")"
PARENT_GRAPH=$(jq -n --arg loop "${LOOP_CHILD_WF}" --arg split "${SPLIT_CHILD_WF}" '{
    name: "state-parent", entryPoint: "loop",
    steps: {
        loop: { id: "loop", stepType: "EmbedWorkflow", childWorkflowId: $loop, childVersion: "latest" },
        split: { id: "split", stepType: "EmbedWorkflow", childWorkflowId: $split, childVersion: "latest",
                 inputMapping: { items: { valueType: "immediate", value: ["a", "b", "c"] } } },
        finish: { id: "finish", stepType: "Finish", inputMapping: {
            trail: { valueType: "reference", value: "steps.loop.outputs.trail" },
            last: { valueType: "reference", value: "steps.split.outputs.last" } } }
    },
    executionPlan: [ { fromStep: "loop", toStep: "split" }, { fromStep: "split", toStep: "finish" } ]
}')
RESP=$(api_post /workflows/create '{"name": "state-parent", "description": "state e2e"}')
PARENT_WF=$(echo "${RESP}" | jq -r '.data.id')
RESP=$(api_post "/workflows/${PARENT_WF}/update" "{\"executionGraph\": ${PARENT_GRAPH}}")
expect_true "${RESP}" '.success == true' "Saving the embedding parent failed"
expect_true "${RESP}" '[.warnings[]? | select(contains("[W083]"))] | length == 2' "Embedding children with state steps must warn W083"
PARENT_V=$(api_get "/workflows/${PARENT_WF}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max')
RESP=$(api_post "/workflows/${PARENT_WF}/versions/${PARENT_V}/compile" '{}' 900)
expect_true "${RESP}" '.success == true' "Compiling the embedding parent failed"
P=$(launch "${PARENT_WF}")
[ "$(wait_status "${P}" completed 180)" = "completed" ] || fail "Parent run did not complete: $(instance_error "${P}")"
expect_true "$(instance_output "${P}")" '.trail == "xxx" and .last == "c"' "Embedded children keep working local state (loop and Split snapshot)"
expect_true "$(instance "${P}")" '.data.state == null' "The parent must not publish its children's state"
ROWS=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM instance_state WHERE instance_id = '${P}'")
[ "${ROWS}" = "0" ] || fail "An embedding parent must have no published state row"
print_success "Embedded children keep local state; replay restores a Split's writes"

# A non-durable workflow cannot Delay: the loop body only reads and appends.
NON_DURABLE=$(echo "${LOOP_GRAPH}" | jq -c '.name = "state-non-durable" | .durable = false
    | del(.steps.loop.subgraph.steps.nap)
    | .steps.loop.subgraph.executionPlan = [
        {fromStep: "get", toStep: "put"}, {fromStep: "put", toStep: "done"}]')
RESP=$(api_post /workflows/create '{"name": "state-non-durable", "description": "state e2e"}')
ND_WF=$(echo "${RESP}" | jq -r '.data.id')
RESP=$(api_post "/workflows/${ND_WF}/update" "{\"executionGraph\": ${NON_DURABLE}}")
expect_true "${RESP}" '[.warnings[]? | select(contains("[W082]"))] | length >= 1' "Non-durable state steps must warn W082"
ND_V=$(api_get "/workflows/${ND_WF}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max')
RESP=$(api_post "/workflows/${ND_WF}/versions/${ND_V}/compile" '{}' 900)
expect_true "${RESP}" '.success == true' "Compiling the non-durable workflow failed"
N=$(launch "${ND_WF}")
[ "$(wait_status "${N}" completed 120)" = "completed" ] || fail "Non-durable run did not complete: $(instance_error "${N}")"
expect_true "$(instance_output "${N}")" '.trail == "xxx"' "A non-durable workflow keeps local state"
expect_true "$(instance "${N}")" '.data.state == null' "A non-durable workflow publishes nothing"
print_success "A non-durable workflow keeps local state and publishes nothing"

# ---------------------------------------------------------------------------
# 4. Refusals.
# ---------------------------------------------------------------------------
print_step "4. E135 at save; STATE_INVALID_VALUE at run time..."
BAD=$(echo "${APPROVAL_GRAPH}" | jq -c '.steps.open.values.priority = {valueType: "immediate", value: 1}')
RESP=$(api_post "/workflows/${APPROVAL_WF}/update" "{\"executionGraph\": ${BAD}}")
echo "${RESP}" | grep -q "E135" || fail "An undeclared state field must be E135: ${RESP}"

TYPED_GRAPH=$(jq -n '{
    name: "state-typed", entryPoint: "set",
    inputSchema: { count: { type: "string" } },
    stateSchema: { count: { type: "integer" } },
    steps: {
        set: { id: "set", stepType: "SetState", values: {
            count: { valueType: "reference", value: "data.count" } } },
        finish: { id: "finish", stepType: "Finish" }
    },
    executionPlan: [ { fromStep: "set", toStep: "finish" } ]
}')
read -r TYPED_WF _ <<<"$(make_workflow state-typed "${TYPED_GRAPH}")"
T=$(launch "${TYPED_WF}" '{"count": "three"}')
[ "$(wait_status "${T}" failed 120)" = "failed" ] || fail "A mistyped state value must fail the run"
instance_error "${T}" | grep -q "STATE_INVALID_VALUE" || fail "Expected STATE_INVALID_VALUE: $(instance_error "${T}")"
print_success "Undeclared fields and mistyped values are refused"

echo
print_success "All queryable workflow state checks passed."
