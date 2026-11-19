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
        RUNTARA_CONVERSATION_STREAMS=3 \
        RUNTARA_MCP_SESSION_STORE=local \
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
# Says three things, a while apart.
pause() { jq -n --arg id "$1" '{ id: $id, stepType: "Delay", durationMs: { valueType: "immediate", value: 1200 } }'; }
PACED=$(workflow conv-paced "$(jq -n \
    --argjson one "$(say one '{"valueType": "immediate", "value": "one"}')" \
    --argjson two "$(say two '{"valueType": "immediate", "value": "two"}')" \
    --argjson three "$(say three '{"valueType": "immediate", "value": "three"}')" \
    --argjson p1 "$(pause p1)" --argjson p2 "$(pause p2)" '{
    name: "conv-paced", durable: true, entryPoint: "one",
    steps: { one: $one, p1: $p1, two: $two, p2: $p2, three: $three,
             finish: { id: "finish", stepType: "Finish", inputMapping: {
                 response: { valueType: "immediate", value: "not said" } } } },
    executionPlan: [ { fromStep: "one", toStep: "p1" }, { fromStep: "p1", toStep: "two" },
                     { fromStep: "two", toStep: "p2" }, { fromStep: "p2", toStep: "three" },
                     { fromStep: "three", toStep: "finish" } ],
    variables: {}, inputSchema: {}, outputSchema: {} }')")
# Reads what was said in its conversation and hands it back as data.
HISTORIAN=$(workflow conv-historian "$(jq -n '{
    name: "conv-historian", entryPoint: "history",
    steps: { history: { id: "history", stepType: "Agent", agentId: "conversation", capabilityId: "history",
                        maxRetries: 0, inputMapping: { limit: { valueType: "immediate", value: 10 } } },
             tell: { id: "tell", stepType: "Agent", agentId: "conversation", capabilityId: "reply",
                     maxRetries: 0, inputMapping: {
                         text: { valueType: "immediate", value: "remembered" },
                         data: { valueType: "reference", value: "steps.history.outputs.said" } } },
             finish: { id: "finish", stepType: "Finish", inputMapping: {} } },
    executionPlan: [ { fromStep: "history", toStep: "tell" }, { fromStep: "tell", toStep: "finish" } ],
    variables: {}, inputSchema: {}, outputSchema: {} }')")
# An adapter's outbound half, as it would be: started with each reply, and
# told what the inbound half kept about the chat.
OUTBOUND=$(workflow conv-outbound "$(jq -n '{
    name: "conv-outbound", entryPoint: "finish",
    steps: { finish: { id: "finish", stepType: "Finish", inputMapping: {
        outbound: { valueType: "immediate", value: true },
        kind: { valueType: "reference", value: "data.event.kind" },
        text: { valueType: "reference", value: "data.event.body.text" },
        chat: { valueType: "reference", value: "data.conversation.metadata.chat" },
        seq: { valueType: "reference", value: "data.event.seq" } } } },
    executionPlan: [], variables: {}, inputSchema: { event: { type: "object" }, conversation: { type: "object" } }, outputSchema: {} }')")
# An adapter's inbound half: a platform's webhook, handed to the workflow
# that answers it, in the conversation the platform's chat is.
INBOUND=$(workflow conv-inbound "$(jq -n --arg target "${HISTORIAN}" --arg out "${OUTBOUND}" '{
    name: "conv-inbound", entryPoint: "send",
    steps: { send: { id: "send", stepType: "Agent", agentId: "conversation", capabilityId: "send",
                     maxRetries: 0, inputMapping: {
                         workflowId: { valueType: "immediate", value: $target },
                         key: { valueType: "reference", value: "data.body.key" },
                         messageId: { valueType: "reference", value: "data.body.update" },
                         text: { valueType: "reference", value: "data.body.text" },
                         replyWorkflowId: { valueType: "immediate", value: $out },
                         metadata: { valueType: "reference", value: "data.body.meta" } } },
             finish: { id: "finish", stepType: "Finish", inputMapping: {
                 state: { valueType: "reference", value: "steps.send.outputs.state" },
                 duplicate: { valueType: "reference", value: "steps.send.outputs.duplicate" } } } },
    executionPlan: [ { fromStep: "send", toStep: "finish" } ],
    variables: {}, inputSchema: { body: { type: "object" } }, outputSchema: {} }')")
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
# It says how to come back, and when everything kept has been sent.
echo "${STREAM}" | grep -q '^retry: 2000' || { print_error "The stream does not say when to connect again: ${STREAM}"; exit 1; }
[ "$(echo "${STREAM}" | grep -c '^event: caught_up')" = "1" ] || { print_error "The stream does not say it has caught up: ${STREAM}"; exit 1; }
echo "${STREAM}" | grep -A1 '^event: caught_up' | grep -q "\"lastSeq\":${LAST}" || { print_error "caught_up does not say the last event: ${STREAM}"; exit 1; }
print_success "A stream of events, resumed by event id, that says when it has caught up ✓"

# ---------------------------------------------------------------------------
print_step "4b. What a run says reaches an open stream as it is said, not on a clock..."
api_status PUT "$(conversation "${PACED}" live)" '{}' >/dev/null
ARRIVED="${TEST_DATA_DIR}/arrived.txt"
# Each line of the stream, with the millisecond it arrived.
(curl -sS -N --max-time 25 -H 'Accept: text/event-stream' "${API}$(conversation "${PACED}" live)/events" 2>/dev/null \
    | python3 -u -c 'import sys, time
for line in sys.stdin:
    print(int(time.time() * 1000), line.rstrip(), flush=True)' > "${ARRIVED}") &
STREAM_PID=$!
sleep 1
SENT=$(send "${PACED}" live m-1 go)
[ "$(sent_state "${SENT}")" = "started" ] || { print_error "Paced did not start: ${SENT}"; kill "${STREAM_PID}" 2>/dev/null; exit 1; }
for _ in {1..60}; do
    grep -q 'event: run_ended' "${ARRIVED}" 2>/dev/null && break
    sleep 0.5
done
kill "${STREAM_PID}" 2>/dev/null || true; wait "${STREAM_PID}" 2>/dev/null || true
grep -q 'event: run_ended' "${ARRIVED}" || { print_error "The run's end did not reach the stream: $(cat "${ARRIVED}")"; exit 1; }
PACED_RUN=$(events "${PACED}" live | jq -r '[.[] | select(.kind == "run_started")][0].body.instanceId')
# When each reply was said (the run's own event), and when it arrived.
SAID_AT=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c \
    "SELECT string_agg(((extract(epoch FROM created_at) * 1000)::bigint)::text, ' ' ORDER BY id) FROM instance_events \
     WHERE instance_id = '${PACED_RUN}' AND subtype = 'conversation_reply'")
ARRIVED_AT=$(grep 'event: reply' "${ARRIVED}" | awk '{print $1}' | tr '\n' ' ')
LAGS=$(python3 -c 'import sys
said = [int(x) for x in sys.argv[1].split()]; arrived = [int(x) for x in sys.argv[2].split()]
assert len(said) == 3 and len(arrived) == 3, (said, arrived)
print(" ".join(str(a - s) for s, a in zip(said, arrived)))' "${SAID_AT}" "${ARRIVED_AT}") \
    || { print_error "Three replies were not said and received: said at '${SAID_AT}', arrived at '${ARRIVED_AT}'"; exit 1; }
echo "  each reply reached the stream this many ms after it was said: ${LAGS}"
for lag in ${LAGS}; do
    [ "${lag}" -lt 400 ] || { print_error "A reply took ${lag} ms to reach an open stream"; exit 1; }
done
# The replies came one by one, a pause apart, and not together at the end.
SPREAD=$(python3 -c 'import sys
a = [int(x) for x in sys.argv[1].split()]; print(a[-1] - a[0])' "${ARRIVED_AT}")
[ "${SPREAD}" -gt 1800 ] || { print_error "The replies arrived together (${SPREAD} ms apart), not as they were said"; exit 1; }
print_success "Each reply reached the open stream within 400 ms of being said (${LAGS} ms) ✓"

# A client that makes one request at a time waits in that request.
LAST=$(events "${PACED}" live | jq -r '.[-1].seq')
STARTED=$(python3 -c 'import time; print(int(time.time() * 1000))')
WAITED=$(curl -sS --max-time 30 "${API}$(conversation "${PACED}" live)/events?after=${LAST}&wait=1" | jq -c '.data')
TOOK=$(( $(python3 -c 'import time; print(int(time.time() * 1000))') - STARTED ))
[ "${WAITED}" = "[]" ] && [ "${TOOK}" -ge 900 ] && [ "${TOOK}" -lt 5000 ] \
    || { print_error "Waiting for an event that did not come answered ${WAITED} after ${TOOK} ms"; exit 1; }
# And a whole exchange is one request: the message, and its turn.
read -r STATUS BODY <<< "$(api_status POST "$(conversation "${ECHO}" one-request)/messages?wait=60" '{"id":"m-1","text":"in one"}')"
[ "$(echo "${BODY}" | jq -c '[.data.state, .data.turn.settled, ([.data.turn.events[] | select(.kind == "reply") | .body.text])]')" = '["started",true,["in one"]]' ] \
    || { print_error "Sending and waiting did not answer with the turn: ${STATUS} ${BODY}"; exit 1; }
[ "$(echo "${BODY}" | jq -c '[.data.turn.events[].kind | select(. != "activity")]')" = '["message","run_started","reply","run_ended"]' ] \
    || { print_error "The turn is not the message's events: ${BODY}"; exit 1; }
print_success "One request waits for an event; one request sends a message and answers with its turn ✓"

# A tenant holds a bounded number of streams open.
HELD_PIDS=()
for _ in 1 2 3; do
    curl -sS -N --max-time 20 -o /dev/null -H 'Accept: text/event-stream' "${API}$(conversation "${PACED}" live)/events" 2>/dev/null &
    HELD_PIDS+=($!)
done
sleep 1
CODE=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 -H 'Accept: text/event-stream' "${API}$(conversation "${PACED}" live)/events")
for pid in "${HELD_PIDS[@]}"; do kill "${pid}" 2>/dev/null || true; done
wait "${HELD_PIDS[@]}" 2>/dev/null || true
[ "${CODE}" = "429" ] || { print_error "A stream past the tenant's bound answered ${CODE}, not 429"; exit 1; }
sleep 1
CODE=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 2 -H 'Accept: text/event-stream' "${API}$(conversation "${PACED}" live)/events" || true)
[ "${CODE}" = "200" ] || { print_error "With the streams closed a new one answered ${CODE}"; exit 1; }
print_success "A stream past the tenant's bound is refused, and a place is given back when one closes ✓"

# ---------------------------------------------------------------------------
print_step "4c. A person stops a run, names a conversation, lists them and reads back..."
# The slow workflow is busy for six seconds; stopped, it ends as stopped,
# and the message that waited has its turn.
SENT=$(send "${SLOW}" stopped m-1 "too long")
[ "$(sent_state "${SENT}")" = "started" ] || { print_error "Slow did not start: ${SENT}"; exit 1; }
SENT=$(send "${SLOW}" stopped m-2 "after it")
sleep 1
read -r STATUS BODY <<< "$(api_status POST "$(conversation "${SLOW}" stopped)/stop" '{}')"
[ "${STATUS}" = "200" ] && [ "$(echo "${BODY}" | jq -r '.data.cleared')" = "0" ] && [ -n "$(echo "${BODY}" | jq -r '.data.instanceId // empty')" ] \
    || { print_error "Stop answered ${STATUS} ${BODY}"; exit 1; }
wait_events "${SLOW}" stopped '[.[] | select(.kind == "run_ended")] | length >= 1'
[ "$(events "${SLOW}" stopped | jq -r '[.[] | select(.kind == "run_ended")][0] | [.turn, .body.status] | join(",")')" = "m-1,stopped" ] \
    || { print_error "The stopped run did not end as stopped: $(events "${SLOW}" stopped | jq -c '[.[] | select(.kind == "run_ended")]')"; exit 1; }
wait_events "${SLOW}" stopped "$(ended 2)"
[ "$(replies "${SLOW}" stopped)" = '["after it"]' ] || { print_error "The message that waited did not have its turn: $(replies "${SLOW}" stopped)"; exit 1; }
print_success "A stopped run ends as stopped, and the message that waited has its turn ✓"

read -r STATUS BODY <<< "$(api_status PUT "$(conversation "${SLOW}" stopped)" '{"title":"Stopped once","metadata":{"chat":42}}')"
[ "$(echo "${BODY}" | jq -c '[.data.title, .data.metadata]')" = '["Stopped once",{"chat":42}]' ] || { print_error "Title and metadata were not kept: ${BODY}"; exit 1; }
LISTED=$(curl -sS "${API}/workflows/${SLOW}/conversations" | jq -c '[.data.conversations[] | [.key, .title, .active, .lastSaid.kind, .lastSaid.text]]')
[ "${LISTED}" = '[["stopped","Stopped once",false,"reply","after it"],["busy",null,false,"reply","three"]]' ] \
    || { print_error "The list is not the workflow's conversations by activity: ${LISTED}"; exit 1; }
PAGE=$(curl -sS "${API}/workflows/${SLOW}/conversations?limit=1")
NEXT=$(echo "${PAGE}" | jq -r '.data.next // empty')
[ -n "${NEXT}" ] && [ "$(curl -sS "${API}/workflows/${SLOW}/conversations?limit=1&cursor=${NEXT}" | jq -c '[.data.conversations[].key, .data.next]')" = '["busy",null]' ] \
    || { print_error "The list does not page: ${PAGE}"; exit 1; }
# Read back from the newest, only what was said.
BACK=$(curl -sS "${API}$(conversation "${SLOW}" stopped)/events?order=desc&limit=2&kinds=message,reply" | jq -c '[.data[] | [.seq, .kind, .body.text]]')
[ "$(echo "${BACK}" | jq -c '[.[] | .[1:]]')" = '[["reply","after it"],["message","after it"]]' ] || { print_error "Reading back from the newest gave ${BACK}"; exit 1; }
BEFORE=$(echo "${BACK}" | jq -r '.[-1][0]')
OLDER=$(curl -sS "${API}$(conversation "${SLOW}" stopped)/events?before=${BEFORE}&limit=10&kinds=message,reply" | jq -c '[.data[] | [.kind, .body.text]]')
[ "${OLDER}" = '[["message","too long"]]' ] || { print_error "Reading further back gave ${OLDER}"; exit 1; }
print_success "A conversation has a title and what its client keeps; they are listed by activity, a page at a time, and read back from the newest ✓"

# ---------------------------------------------------------------------------
print_step "4d. A run reads what was said in its conversation; an adapter hands a platform's messages over..."
RESP=$(api_post /triggers "{\"workflow_id\": \"${INBOUND}\", \"trigger_type\": \"HTTP\", \"active\": true}")
INBOUND_TRIGGER=$(echo "${RESP}" | jq -r '.data.id // .id // empty')
[ -n "${INBOUND_TRIGGER}" ] || { print_error "The inbound trigger was not made: ${RESP}"; exit 1; }
platform() { # update text: what the platform's webhook would send
    curl -sS --max-time 60 -X POST -H 'Content-Type: application/json' \
        -d "$(jq -nc --arg u "$1" --arg t "$2" '{key: "telegram:7", update: $u, text: $t, meta: {chat: 7}}')" \
        "${API}/events/http/${INBOUND_TRIGGER}/run"
}
platform update-1 "first thing" >/dev/null
# The platform delivers the same update again, as platforms do.
platform update-1 "first thing" >/dev/null
wait_events "${HISTORIAN}" "telegram:7" "$(ended 1)"
platform update-2 "second thing" >/dev/null
wait_events "${HISTORIAN}" "telegram:7" "$(ended 2)"
[ "$(events "${HISTORIAN}" "telegram:7" | jq -c '[.[] | select(.kind == "message") | .body.id]')" = '["update-1","update-2"]' ] \
    || { print_error "The platform's messages are not each in the conversation once: $(events "${HISTORIAN}" "telegram:7" | jq -c '[.[] | select(.kind == "message")]')"; exit 1; }
# The second run read the whole conversation as a transcript.
TRANSCRIPT=$(events "${HISTORIAN}" "telegram:7" | jq -c '[.[] | select(.kind == "reply")][1].body.data | [.[] | [.role, .text]]')
[ "${TRANSCRIPT}" = '[["user","first thing"],["assistant","remembered"],["user","second thing"]]' ] \
    || { print_error "The run did not read what was said in its conversation: ${TRANSCRIPT}"; exit 1; }
print_success "A run reads what was said in its own conversation, as a transcript ✓"
STATE=$(curl -sS "${API}$(conversation "${HISTORIAN}" "telegram:7")" | jq -c '[.data.replyWorkflowId, .data.metadata]')
[ "${STATE}" = "[\"${OUTBOUND}\",{\"chat\":7}]" ] || { print_error "The inbound half did not open the conversation as asked: ${STATE}"; exit 1; }
OUT=""
for _ in {1..90}; do
    OUT=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(json_agg(convert_from(output, 'UTF8')::json ORDER BY (convert_from(output, 'UTF8')::json->>'seq')::int), '[]') FROM instances \
         WHERE output IS NOT NULL AND convert_from(output, 'UTF8')::json->>'outbound' = 'true'")
    [ "$(echo "${OUT}" | jq 'length')" = "2" ] && break
    sleep 1
done
[ "$(echo "${OUT}" | jq -c '[.[] | [.kind, .text, .chat]]')" = '[["reply","remembered",7],["reply","remembered",7]]' ] \
    || { print_error "The outbound half was not started once for each reply, told the chat: ${OUT}"; exit 1; }
print_success "A stand-in adapter: each platform message in once, each reply out once, in order, with the chat it kept ✓"

# ---------------------------------------------------------------------------
print_step "4e. A model talks to a workflow over MCP..."
MCP_URL="http://127.0.0.1:${TEST_PORT_PUBLIC}/mcp"
MCP_HEADERS="${TEST_DATA_DIR}/mcp_headers"
MCP_SESSION=""
mcp() { # body -> the answer's JSON-RPC message
    curl -sS --max-time 150 -D "${MCP_HEADERS}" -X POST "${MCP_URL}" -H "Content-Type: application/json" \
        -H "Accept: application/json, text/event-stream" ${MCP_SESSION:+-H "Mcp-Session-Id: ${MCP_SESSION}"} -d "$1" \
        | python3 -c 'import sys, json
body = sys.stdin.read()
lines = [l[5:].strip() for l in body.splitlines() if l.startswith("data:")]
print(lines[-1] if lines else body.strip())'
}
mcp '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"conversations-e2e","version":"1"}}}' >/dev/null
MCP_SESSION=$(grep -i '^mcp-session-id:' "${MCP_HEADERS}" | head -1 | cut -d' ' -f2 | tr -d '\r\n' || true)
mcp '{"jsonrpc":"2.0","method":"notifications/initialized"}' >/dev/null
TOOLS=$(mcp '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | jq -c '[.result.tools[].name | select(startswith("conversation_"))] | sort')
[ "${TOOLS}" = '["conversation_list","conversation_read","conversation_send","conversation_stop"]' ] \
    || { print_error "The MCP conversation tools are not offered: ${TOOLS}"; exit 1; }
tool() { # name arguments -> the tool's answer
    mcp "$(jq -nc --arg n "$1" --argjson a "$2" '{jsonrpc: "2.0", id: 2, method: "tools/call", params: {name: $n, arguments: $a}}')" \
        | jq -r '.result.content[0].text // .error.message // .'
}
ONE=$(tool conversation_send "$(jq -nc --arg w "${TALKER}" '{workflow_id: $w, key: "mcp", text: "start"}')")
[ "$(echo "${ONE}" | jq -c '[.state, .turn.settled, [.turn.events[].kind]]')" = '["started",true,["message","run_started","reply","waiting_for_input"]]' ] \
    || { print_error "The first turn over MCP is not what was said: ${ONE}"; exit 1; }
REQUEST=$(echo "${ONE}" | jq -r '.turn.events[-1].body.requestId')
# The second turn answers what the run asked, structured, naming the request.
TWO=$(tool conversation_send "$(jq -nc --arg w "${TALKER}" --arg r "${REQUEST}" '{workflow_id: $w, key: "mcp", payload: {message: "structured"}, in_reply_to: $r}')")
[ "$(echo "${TWO}" | jq -c '[.state, .turn.settled, [.turn.events[] | select(.kind == "reply") | .body.text], .turn.events[-1].kind]')" = '["delivered",true,["structured"],"run_ended"]' ] \
    || { print_error "The second turn over MCP did not answer the run: ${TWO}"; exit 1; }
THREE=$(tool conversation_send "$(jq -nc --arg w "${TALKER}" '{workflow_id: $w, key: "mcp", text: "again"}')")
[ "$(echo "${THREE}" | jq -c '[.state, .turn.settled, .turn.events[-1].kind]')" = '["started",true,"waiting_for_input"]' ] \
    || { print_error "The third turn over MCP did not start the next run: ${THREE}"; exit 1; }
READ=$(tool conversation_read "$(jq -nc --arg w "${TALKER}" '{workflow_id: $w, key: "mcp", kinds: "message"}')")
[ "$(echo "${READ}" | jq -c '[[.events[].body | .text // .payload.message], (.conversation.waitingFor != null)]')" = '[["start","structured","again"],true]' ] \
    || { print_error "Reading the conversation over MCP gave ${READ}"; exit 1; }
LISTED=$(tool conversation_list "$(jq -nc --arg w "${TALKER}" '{workflow_id: $w}')")
echo "${LISTED}" | jq -e '[.conversations[].key] | index("mcp") != null' >/dev/null || { print_error "The MCP list does not have the conversation: ${LISTED}"; exit 1; }
STOPPED=$(tool conversation_stop "$(jq -nc --arg w "${TALKER}" '{workflow_id: $w, key: "mcp"}')")
[ -n "$(echo "${STOPPED}" | jq -r '.instanceId // empty')" ] || { print_error "Stopping over MCP gave ${STOPPED}"; exit 1; }
print_success "Over MCP: three turns, one a structured answer to what the run asked, read back, listed and stopped ✓"

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
