#!/bin/bash
# E2E Test: a conversation with a workflow (docs/native-chat.md).
#
# A conversation is found by a key its client chose and carried by one run
# after another. This drives it through the API against a running server,
# with real workflows and the built-in `conversation` agent:
#
#   1. A message starts a run; the run's output is its reply; the same key
#      reaches the same conversation, which starts another run. The same
#      message id again is the same message.
#   2. A run replies from its middle (`conversation.reply`), waits for
#      input, and the next message answers it. Its reply workflow is started
#      once for each reply and each "waiting for input".
#   3. Messages sent while a run is busy wait their turn, in order, and one
#      past the bound is refused (429).
#   4. The events are a stream when asked for as one, read on by event id.
#   5. A deleted conversation is gone.
#
# Usage:  POSTGRES_PORT=55432 POSTGRES_USER=postgres ./e2e/test_conversations.sh
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
# runtara-server needs a credential encryption key; a throwaway one per run.
# shellcheck source=../scripts/credential-key.sh
source "${PROJECT_ROOT}/scripts/credential-key.sh"
ensure_credential_key

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-postgres}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD-}"

TEST_DB_SERVER="${TEST_DB_SERVER:-conversations_server_$$}"
TEST_DB_RUNTIME="${TEST_DB_RUNTIME:-conversations_runtime_$$}"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17970}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18971}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18972}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18973}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18974}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16411}"
TEST_DATA_DIR="$(mktemp -d -t runtara_conversations_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="${TENANT_ID_OVERRIDE:-conversations_$$}"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"

if [ -n "${POSTGRES_PASSWORD}" ]; then
    CREDENTIALS="${POSTGRES_USER}:${POSTGRES_PASSWORD}"
else
    CREDENTIALS="${POSTGRES_USER}"
fi
SERVER_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"
SERVER_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -X -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}
# Send a request and echo "<http status> <body>".
api_status() {
    local method="$1" path="$2" data="${3-}" body status
    if [ -n "${data}" ]; then
        body=$(curl -sS --max-time 60 -w '\n%{http_code}' -X "${method}" -H "Content-Type: application/json" -d "${data}" "${API}${path}")
    else
        body=$(curl -sS --max-time 60 -w '\n%{http_code}' -X "${method}" "${API}${path}")
    fi
    status="${body##*$'\n'}"
    echo "${status} ${body%$'\n'*}"
}

cleanup() {
    local code=$?
    if [ -n "${SERVER_PID}" ]; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
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
        RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
        TENANT_ID="${TENANT}" \
        SERVER_HOST=127.0.0.1 \
        SERVER_PORT="${TEST_PORT_PUBLIC}" \
        RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
        RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
        RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
        RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
        RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
        RUNTARA_CONVERSATION_QUEUE_BOUND=2 \
        DATA_DIR="${TEST_DATA_DIR}" \
        RUNTARA_DEV_MODE=false \
        RUST_LOG="warn,runtara_server=info" \
        AUTH_PROVIDER=local \
        VALKEY_HOST=127.0.0.1 \
        VALKEY_PORT="${TEST_VALKEY_PORT}" \
        OTEL_SDK_DISABLED=true \
        SQLX_OFFLINE=true \
        exec "${RUNTARA_SERVER_BIN}"
    ) >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!
    for _ in {1..90}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; exit 1; }
    done
    print_error "Server did not become healthy."; exit 1
}

# Create, save and compile a workflow from a graph; echo its id.
workflow() {
    local name="$1" graph="$2" resp wf version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"conversations e2e\"}")
    wf=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }
    resp=$(api_post "/workflows/${wf}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update of ${name} failed: ${resp}"; exit 1; }
    version=$(curl -sS "${API}/workflows/${wf}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Compile of ${name} failed: ${resp}"; exit 1; }
    echo "${wf}"
}

# The conversation $2 of workflow $1.
conversation() { echo "/workflows/$1/conversations/$2"; }
send() { # workflow key id text -> "<status> <body>"
    api_status POST "$(conversation "$1" "$2")/messages" "$(jq -nc --arg id "$3" --arg text "$4" '{id: $id, text: $text}')"
}
sent_state() { echo "$1" | cut -d' ' -f2- | jq -r '.data.state // empty'; }
events() { curl -sS --max-time 30 "${API}$(conversation "$1" "$2")/events?limit=500" | jq -c '.data'; }
kinds() { events "$1" "$2" | jq -c '[.[].kind | select(. != "activity")]'; }
replies() { events "$1" "$2" | jq -c '[.[] | select(.kind == "reply") | .body.text]'; }
# Wait until the jq filter $3 holds of the events of conversation $2.
wait_events() {
    local got
    for _ in {1..120}; do
        got=$(events "$1" "$2")
        if [ "$(echo "${got}" | jq "$3" 2>/dev/null)" = "true" ]; then return 0; fi
        sleep 1
    done
    print_error "Conversation $2 never satisfied: $3"; echo "${got}" | jq -c '.[] | {seq, kind, body}' >&2; exit 1
}
ended() { echo "[.[] | select(.kind == \"run_ended\")] | length == $1"; }

echo "==============================================================="
echo "E2E: a conversation with a workflow"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN}"; exit 1; }
[ -f "${COMPONENTS_DIR}/runtara_agent_conversation.wasm" ] || { print_error "Missing conversation component: run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "127.0.0.1:${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases ${TEST_DB_SERVER}, ${TEST_DB_RUNTIME}..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC} (tenant ${TENANT})..."
start_server

print_step "Creating the workflows..."
# Answers with what it returns, and says nothing before.
ECHO=$(workflow conv-echo "$(jq -n '{
    name: "conv-echo", entryPoint: "finish",
    steps: { finish: { id: "finish", stepType: "Finish", inputMapping: {
        response: { valueType: "reference", value: "data.message" } } } },
    executionPlan: [], variables: {}, inputSchema: { message: { type: "string" } }, outputSchema: {} }')")
# Replies from its middle, waits to be told something, and replies with it.
say() {
    jq -n --arg id "$1" --argjson text "$2" '{
        id: $id, stepType: "Agent", agentId: "conversation", capabilityId: "reply",
        maxRetries: 0, inputMapping: { text: $text } }'
}
TALKER=$(workflow conv-talker "$(jq -n \
    --argjson first "$(say first '{"valueType": "immediate", "value": "Working on it."}' | jq '.inputMapping += {
        replyId: {valueType: "immediate", value: "working"},
        format: {valueType: "immediate", value: "plain"},
        data: {valueType: "immediate", value: {stage: 1}} }')" \
    --argjson second "$(say second '{"valueType": "reference", "value": "steps.wait.outputs.message"}')" '{
    name: "conv-talker", durable: true, entryPoint: "first",
    steps: { first: $first,
             wait: { id: "wait", stepType: "WaitForSignal", name: "What next?", pollIntervalMs: 500,
                     responseSchema: { message: { type: "string", required: true } } },
             second: $second,
             finish: { id: "finish", stepType: "Finish", inputMapping: {
                 response: { valueType: "immediate", value: "not said: the run replied itself" } } } },
    executionPlan: [ { fromStep: "first", toStep: "wait" }, { fromStep: "wait", toStep: "second" },
                     { fromStep: "second", toStep: "finish" } ],
    variables: {}, inputSchema: {}, outputSchema: {} }')")
# Busy for a while, then answers.
SLOW=$(workflow conv-slow "$(jq -n '{
    name: "conv-slow", durable: true, entryPoint: "hold",
    steps: { hold: { id: "hold", stepType: "Delay", durationMs: { valueType: "immediate", value: 6000 } },
             finish: { id: "finish", stepType: "Finish", inputMapping: {
                 response: { valueType: "reference", value: "data.message" } } } },
    executionPlan: [ { fromStep: "hold", toStep: "finish" } ],
    variables: {}, inputSchema: { message: { type: "string" } }, outputSchema: {} }')")
# What an integration's outbound half would be: started with each reply.
ROUTE=$(workflow conv-route "$(jq -n '{
    name: "conv-route", entryPoint: "finish",
    steps: { finish: { id: "finish", stepType: "Finish", inputMapping: {
        kind: { valueType: "reference", value: "data.event.kind" },
        text: { valueType: "reference", value: "data.event.body.text" },
        key: { valueType: "reference", value: "data.conversation.key" } } } },
    executionPlan: [], variables: {}, inputSchema: { event: { type: "object" }, conversation: { type: "object" } }, outputSchema: {} }')")
echo "  echo=${ECHO} talker=${TALKER} slow=${SLOW} route=${ROUTE}"

# ---------------------------------------------------------------------------
print_step "1. A conversation is carried by one run after another..."
read -r STATUS BODY <<< "$(api_status GET "$(conversation "${ECHO}" c1)")"
[ "${STATUS}" = "404" ] || { print_error "A conversation nobody opened answered ${STATUS}: ${BODY}"; exit 1; }
read -r STATUS BODY <<< "$(api_status PUT "$(conversation "${ECHO}" c1)")"
[ "${STATUS}" = "200" ] && [ "$(echo "${BODY}" | jq -r '.data.key')" = "c1" ] || { print_error "Open failed: ${STATUS} ${BODY}"; exit 1; }
CONVERSATION_ID=$(echo "${BODY}" | jq -r '.data.id')

SENT=""
for _ in {1..90}; do
    SENT=$(send "${ECHO}" c1 m-1 hello)
    [ "$(sent_state "${SENT}")" = "started" ] && break
    sleep 2
done
[ "$(sent_state "${SENT}")" = "started" ] || { print_error "The first message did not start a run: ${SENT}"; exit 1; }
wait_events "${ECHO}" c1 "$(ended 1)"
[ "$(replies "${ECHO}" c1)" = '["hello"]' ] || { print_error "Unexpected replies: $(replies "${ECHO}" c1)"; exit 1; }
[ "$(kinds "${ECHO}" c1)" = '["message","run_started","reply","run_ended"]' ] || { print_error "Unexpected events: $(kinds "${ECHO}" c1)"; exit 1; }

SENT=$(send "${ECHO}" c1 m-2 again)
[ "$(sent_state "${SENT}")" = "started" ] || { print_error "The second message did not start a run: ${SENT}"; exit 1; }
wait_events "${ECHO}" c1 "$(ended 2)"
[ "$(replies "${ECHO}" c1)" = '["hello","again"]' ] || { print_error "Unexpected replies: $(replies "${ECHO}" c1)"; exit 1; }
RUNS=$(events "${ECHO}" c1 | jq -r '[.[] | select(.kind == "run_started") | .body.instanceId] | unique | length')
[ "${RUNS}" = "2" ] || { print_error "Expected two runs, found ${RUNS}"; exit 1; }
read -r STATUS BODY <<< "$(api_status PUT "$(conversation "${ECHO}" c1)")"
[ "$(echo "${BODY}" | jq -r '.data.id')" = "${CONVERSATION_ID}" ] || { print_error "The same key reached another conversation"; exit 1; }
[ "$(echo "${BODY}" | jq -r '.data.instanceId // "none"')" = "none" ] || { print_error "The conversation still has a run: ${BODY}"; exit 1; }
print_success "A message starts a run, its output is the reply, and the next message starts the next run ✓"

SENT=$(send "${ECHO}" c1 m-2 again)
[ "$(echo "${SENT}" | cut -d' ' -f2- | jq -r '.data.duplicate')" = "true" ] || { print_error "A message sent twice was not recognised: ${SENT}"; exit 1; }
sleep 2
[ "$(events "${ECHO}" c1 | jq -r '[.[] | select(.kind == "run_started")] | length')" = "2" ] || { print_error "A message sent twice started a run"; exit 1; }
print_success "The same message id again is the same message ✓"

# A first message that says nothing starts a run all the same: how a client
# lets the workflow speak first.
read -r STATUS BODY <<< "$(api_status POST "$(conversation "${ECHO}" opened)/messages" '{"id": "opening", "payload": {}}')"
[ "${STATUS}" = "200" ] && [ "$(echo "${BODY}" | jq -r '.data.state')" = "started" ] \
    || { print_error "A message with an empty payload did not start a run: ${STATUS} ${BODY}"; exit 1; }
wait_events "${ECHO}" opened "$(ended 1)"
[ "$(events "${ECHO}" opened | jq -r '[.[] | select(.kind == "run_ended")][0].body.status')" = "completed" ] \
    || { print_error "The run a silent message started did not complete"; exit 1; }
print_success "A first message with nothing to say starts the run ✓"

# ---------------------------------------------------------------------------
print_step "2. A run replies from its middle, waits, and is answered by the next message..."
read -r STATUS BODY <<< "$(api_status PUT "$(conversation "${TALKER}" "chat:42")" "$(jq -nc --arg r "${ROUTE}" '{replyWorkflowId: $r}')")"
[ "${STATUS}" = "200" ] && [ "$(echo "${BODY}" | jq -r '.data.replyWorkflowId')" = "${ROUTE}" ] || { print_error "Open with a reply workflow failed: ${STATUS} ${BODY}"; exit 1; }
SENT=$(send "${TALKER}" "chat:42" m-1 start)
[ "$(sent_state "${SENT}")" = "started" ] || { print_error "Talker did not start: ${SENT}"; exit 1; }
wait_events "${TALKER}" "chat:42" '[.[] | select(.kind == "waiting_for_input")] | length == 1'
[ "$(replies "${TALKER}" "chat:42")" = '["Working on it."]' ] || { print_error "The run's first reply was not said: $(replies "${TALKER}" "chat:42")"; exit 1; }
[ "$(events "${TALKER}" "chat:42" | jq "$(ended 0)")" = "true" ] || { print_error "The run ended before it was answered"; exit 1; }
read -r STATUS BODY <<< "$(api_status GET "$(conversation "${TALKER}" "chat:42")")"
[ "$(echo "${BODY}" | jq -r '.data.waitingFor.message')" = "What next?" ] || { print_error "The conversation does not say what it waits for: ${BODY}"; exit 1; }
# The reply as the workflow shaped it, in the turn of the message that
# started the run; and that message is in the log as it was said.
FIRST=$(events "${TALKER}" "chat:42" | jq -c '[.[] | select(.kind == "reply")][0] | {turn, body: (.body | del(.instanceId))}')
[ "${FIRST}" = '{"turn":"m-1","body":{"replyId":"working","text":"Working on it.","format":"plain","data":{"stage":1}}}' ] \
    || { print_error "The reply is not as the workflow said it: ${FIRST}"; exit 1; }
[ "$(events "${TALKER}" "chat:42" | jq -c '.[0] | [.kind, .turn, .body.id, .body.text]')" = '["message","m-1","m-1","start"]' ] \
    || { print_error "The message is not in the log as it was said: $(events "${TALKER}" "chat:42" | jq -c '.[0]')"; exit 1; }
REQUEST=$(events "${TALKER}" "chat:42" | jq -r '[.[] | select(.kind == "waiting_for_input")][0].body.requestId')
# An answer written for another question is not given to this one.
SENT=$(api_status POST "$(conversation "${TALKER}" "chat:42")/messages" '{"id":"m-stale","text":"for another","inReplyTo":"not-this-request"}')
[ "$(sent_state "${SENT}")" = "failed" ] || { print_error "An answer for another request was not refused: ${SENT}"; exit 1; }
[ "$(events "${TALKER}" "chat:42" | jq "$(ended 0)")" = "true" ] || { print_error "A stale answer ended the run"; exit 1; }
print_success "A reply is said while the run still runs, as the workflow shaped it; the conversation says what it waits for, and takes no answer meant for another request ✓"

SENT=$(api_status POST "$(conversation "${TALKER}" "chat:42")/messages" "$(jq -nc --arg r "${REQUEST}" '{id: "m-2", text: "the second thing", inReplyTo: $r}')")
[ "$(sent_state "${SENT}")" = "delivered" ] || { print_error "The message did not answer the run: ${SENT}"; exit 1; }
wait_events "${TALKER}" "chat:42" "$(ended 1)"
[ "$(replies "${TALKER}" "chat:42")" = '["Working on it.","the second thing"]' ] \
    || { print_error "Unexpected replies: $(replies "${TALKER}" "chat:42")"; exit 1; }
# What follows the answer is of the answer's turn.
[ "$(events "${TALKER}" "chat:42" | jq -c '[.[] | select(.kind == "reply") | .turn]')" = '["m-1","m-2"]' ] \
    || { print_error "The replies are not of their messages' turns"; exit 1; }
[ "$(events "${TALKER}" "chat:42" | jq -r '[.[] | select(.kind == "run_started")] | length')" = "1" ] || { print_error "The answer started a second run"; exit 1; }
print_success "The next message answers the waiting run; a run that replied is not answered for from its output ✓"

ROUTED=""
for _ in {1..90}; do
    ROUTED=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(json_agg(convert_from(output, 'UTF8')::json ORDER BY created_at), '[]') FROM instances \
         WHERE output IS NOT NULL AND convert_from(output, 'UTF8')::json->>'key' = 'chat:42'")
    [ "$(echo "${ROUTED}" | jq 'length')" = "3" ] && break
    sleep 1
done
echo "  reply workflow runs: ${ROUTED}"
[ "$(echo "${ROUTED}" | jq -c '[.[] | [.kind, .text]] | sort')" = '[["reply","Working on it."],["reply","the second thing"],["waiting_for_input",null]]' ] \
    || { print_error "The reply workflow was not started once for each reply and wait"; exit 1; }
print_success "The reply workflow is started once for each reply and each wait ✓"

# ---------------------------------------------------------------------------
print_step "3. Messages sent while a run is busy wait their turn, up to the bound..."
[ "$(sent_state "$(send "${SLOW}" busy m-1 one)")" = "started" ] || { print_error "Slow did not start"; exit 1; }
[ "$(sent_state "$(send "${SLOW}" busy m-2 two)")" = "queued" ] || { print_error "A message to a busy run was not queued"; exit 1; }
[ "$(sent_state "$(send "${SLOW}" busy m-3 three)")" = "queued" ] || { print_error "A second message to a busy run was not queued"; exit 1; }
read -r STATUS BODY <<< "$(send "${SLOW}" busy m-4 four)"
[ "${STATUS}" = "429" ] && [ "$(echo "${BODY}" | jq -r '.code')" = "CONVERSATION_QUEUE_FULL" ] \
    || { print_error "A message past the bound answered ${STATUS}: ${BODY}"; exit 1; }
read -r STATUS BODY <<< "$(api_status GET "$(conversation "${SLOW}" busy)")"
[ "$(echo "${BODY}" | jq -r '.data.queued')" = "2" ] || { print_error "Expected two queued: ${BODY}"; exit 1; }
wait_events "${SLOW}" busy "$(ended 3)"
[ "$(replies "${SLOW}" busy)" = '["one","two","three"]' ] || { print_error "Out of order: $(replies "${SLOW}" busy)"; exit 1; }
print_success "Queued messages each start a run in order; one past the bound is refused (429) ✓"

# ---------------------------------------------------------------------------
print_step "4. The events are a stream, read on from the last one seen..."
LAST=$(events "${ECHO}" c1 | jq -r '.[-1].seq')
STREAM=$(curl -sS -N --max-time 3 -H 'Accept: text/event-stream' "${API}$(conversation "${ECHO}" c1)/events" 2>/dev/null || true)
echo "${STREAM}" | grep -q '^event: reply' || { print_error "The stream carried no reply: ${STREAM}"; exit 1; }
[ "$(echo "${STREAM}" | grep -c '^id: ')" = "${LAST}" ] || { print_error "Expected ${LAST} events on the stream: ${STREAM}"; exit 1; }
STREAM=$(curl -sS -N --max-time 3 -H 'Accept: text/event-stream' -H "Last-Event-ID: $((LAST - 2))" "${API}$(conversation "${ECHO}" c1)/events" 2>/dev/null || true)
[ "$(echo "${STREAM}" | grep '^id: ' | tr -d '\r' | tr '\n' ' ')" = "id: $((LAST - 1)) id: ${LAST} " ] || { print_error "The stream did not read on from event $((LAST - 2)): ${STREAM}"; exit 1; }
print_success "A stream of events, resumed by event id ✓"

# ---------------------------------------------------------------------------
print_step "5. A deleted conversation is gone..."
read -r STATUS BODY <<< "$(api_status DELETE "$(conversation "${ECHO}" c1)")"
[ "${STATUS}" = "200" ] || { print_error "Delete answered ${STATUS}: ${BODY}"; exit 1; }
read -r STATUS BODY <<< "$(api_status GET "$(conversation "${ECHO}" c1)/events")"
[ "${STATUS}" = "404" ] || { print_error "A deleted conversation's events answered ${STATUS}"; exit 1; }
read -r STATUS BODY <<< "$(api_status DELETE "$(conversation "${ECHO}" c1)")"
[ "${STATUS}" = "404" ] || { print_error "Deleting twice answered ${STATUS}"; exit 1; }
print_success "Deleted, with what was said in it ✓"

echo ""
print_success "Conversations: carried across runs, replied to mid-run, queued in order, and routed ✓"
