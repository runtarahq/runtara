#!/bin/bash
# E2E Test: an AI Agent step's answer reaches its conversation as the model
# writes it (docs/ai-agent-streaming.md).
#
# A stand-in OpenAI provider on this machine answers a streamed request with
# server-sent events a while apart, and records whether each request asked
# to stream. Through a running server, with seal attaching the key:
#
#   1. An AI Agent step in a run that carries a conversation streams: its
#      pieces reach an open stream of the conversation as the provider sends
#      them, under one reply id, then the whole answer as the reply under the
#      same id, said once (not again from the run's output).
#   2. A step whose name begins with `_` does not stream: no pieces, and the
#      run's output is the reply.
#   3. A run that carries no conversation calls its model unstreamed.
#
# Usage:  POSTGRES_PORT=55432 POSTGRES_USER=postgres ./e2e/test_ai_streaming.sh
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

TEST_DB_SERVER="${TEST_DB_SERVER:-ai_streaming_server_$$}"
TEST_DB_RUNTIME="${TEST_DB_RUNTIME:-ai_streaming_runtime_$$}"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17980}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18981}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18982}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18983}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18984}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16421}"
TEST_DATA_DIR="$(mktemp -d -t runtara_ai_streaming_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
PROVIDER_PID=""
PROVIDER_PORT="${PROVIDER_PORT:-17989}"
PROVIDER_LOG="${TEST_DATA_DIR}/provider.jsonl"
VALKEY_CONTAINER=""
TENANT="${TENANT_ID_OVERRIDE:-ai_streaming_$$}"

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

# A connection is saved with its secrets sealed: the server takes none
# readable. $1 is the create request, written as it was before that.
connection_post() {
    python3 "${SCRIPT_DIR}/sealed_connection.py" --api "${API}" "$1"
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
    [ -n "${PROVIDER_PID}" ] && kill "${PROVIDER_PID}" 2>/dev/null || true
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
        RUNTARA_PROXY_ALLOWED_HOSTS="127.0.0.1:${PROVIDER_PORT}" \
        RUNTARA_PROXY_ALLOW_HTTP_HOSTS=127.0.0.1 \
        RUNTARA_SEAL_PRIVATE_CIDRS="127.0.0.0/8" \
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


# The stand-in provider: a streamed request is answered with three pieces of
# text 400 ms apart; any other, whole. Each request's `stream` is recorded.
PROVIDER="${SCRIPT_DIR}/stand_in_model_provider.py"

echo "==============================================================="
echo "E2E: an AI Agent step's answer, streamed to its conversation"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN}"; exit 1; }
[ -f "${COMPONENTS_DIR}/runtara_agent_ai_tools.wasm" ] || { print_error "Missing ai-tools component: run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Starting the stand-in provider on :${PROVIDER_PORT}..."
python3 "${PROVIDER}" "${PROVIDER_PORT}" "${PROVIDER_LOG}" &
PROVIDER_PID=$!
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${PROVIDER_PORT}) 2>/dev/null && break; sleep 0.25; done

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "127.0.0.1:${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases ${TEST_DB_SERVER}, ${TEST_DB_RUNTIME}..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC} (tenant ${TENANT})..."
start_server

print_step "Creating an OpenAI connection to the stand-in..."
RESP=$(connection_post "$(jq -nc --arg base "http://127.0.0.1:${PROVIDER_PORT}/v1" '{
    title: "stand-in model", integrationId: "openai_api_key",
    connectionParameters: { api_key: "sk-stand-in", base_url: $base } }')")
CONN=$(echo "${RESP}" | jq -r '.connectionId // .data.id // .id // empty')
[ -n "${CONN}" ] || { print_error "Connection create failed: ${RESP}"; exit 1; }

# A workflow of one AI Agent step named $2, answering the message; its
# output is the step's answer.
ai_workflow() {
    workflow "$1" "$(jq -n --arg name "$2" --arg conn "${CONN}" '{
        name: "ai", entryPoint: "answer",
        steps: {
            answer: { id: "answer", name: $name, stepType: "AiAgent", connectionId: $conn, config: {
                systemPrompt: { valueType: "immediate", value: "Tell a story." },
                userPrompt: { valueType: "reference", value: "data.message" },
                model: { valueType: "immediate", value: "gpt-4o-mini" } } },
            finish: { id: "finish", stepType: "Finish", inputMapping: {
                response: { valueType: "reference", value: "steps.answer.outputs.response" } } } },
        executionPlan: [ { fromStep: "answer", toStep: "finish", label: "next" } ],
        variables: {}, inputSchema: { message: { type: "string" } }, outputSchema: {} }')"
}
SHOWN=$(ai_workflow ai-shown "Answer")
HIDDEN=$(ai_workflow ai-hidden "_think")

# Listen to conversation $2 of workflow $1 into file $3, each line with the
# millisecond it arrived; echo the listener's pid.
listen() {
    (curl -sS -N --max-time 60 -H 'Accept: text/event-stream' "${API}$(conversation "$1" "$2")/events?kinds=reply_delta,reply,run_ended" 2>/dev/null \
        | python3 -u -c 'import sys, time
for line in sys.stdin:
    print(int(time.time() * 1000), line.rstrip(), flush=True)') > "$3" &
    echo $!
}
# What a listener heard: [kind, replyId, text, ms] of each piece and reply.
heard() {
    python3 - "$1" <<'PY'
import json, sys
seen, block = [], {}
def done(block):
    kind = block.get("event")
    if kind in ("reply_delta", "reply") and "data" in block:
        body = json.loads(block["data"])["body"]
        seen.append([kind, body.get("replyId"), body.get("text"), block["at"]])
for line in open(sys.argv[1]):
    at, _, text = line.rstrip("\n").partition(" ")
    if not text:
        done(block)
        block = {}
        continue
    name, _, value = text.partition(": ")
    block.setdefault("at", int(at))
    block[name] = value
done(block)
print(json.dumps(seen))
PY
}
# Send message $3 to conversation $2 of workflow $1, and hear it answered.
converse() {
    local heard_file="${TEST_DATA_DIR}/$2.txt" pid
    api_status PUT "$(conversation "$1" "$2")" '{}' >/dev/null
    pid=$(listen "$1" "$2" "${heard_file}")
    sleep 1
    send "$1" "$2" m-1 "$3" >/dev/null
    for _ in {1..90}; do grep -q 'event: run_ended' "${heard_file}" 2>/dev/null && break; sleep 0.5; done
    kill "${pid}" 2>/dev/null || true; wait "${pid}" 2>/dev/null || true
    heard "${heard_file}"
}

# ---------------------------------------------------------------------------
print_step "1. A shown step's answer reaches the conversation as it is written, then whole..."
SEEN=$(converse "${SHOWN}" story "Tell me one.")
echo "  on the stream: $(echo "${SEEN}" | jq -c '[.[] | [.[0], .[2]]]')"
[ "$(echo "${SEEN}" | jq -c '[.[] | select(.[0] == "reply_delta") | .[2]] | add')" = '"Once upon a time."' ] \
    || { print_error "The pieces were not the answer: ${SEEN}"; events "${SHOWN}" story | jq -c '.[] | {kind, body}' >&2; cat "${PROVIDER_LOG}" >&2 || true; cat "${TEST_DATA_DIR}/story.txt" >&2 || true; exit 1; }
[ "$(echo "${SEEN}" | jq '[.[] | select(.[0] == "reply_delta")] | length')" -ge 2 ] \
    || { print_error "The answer did not arrive in pieces: ${SEEN}"; exit 1; }
[ "$(echo "${SEEN}" | jq -c '[.[] | select(.[0] == "reply") | .[2]]')" = '["Once upon a time."]' ] \
    || { print_error "The answer was not said once, whole, as the reply: ${SEEN}"; exit 1; }
[ "$(echo "${SEEN}" | jq -c '.[-1][0]')" = '"reply"' ] || { print_error "The reply did not come last: ${SEEN}"; exit 1; }
[ "$(echo "${SEEN}" | jq '[.[] | .[1]] | unique | length')" = "1" ] && [ "$(echo "${SEEN}" | jq -r '.[0][1]')" != "null" ] \
    || { print_error "The pieces and the reply do not share one reply id: ${SEEN}"; exit 1; }
SPREAD=$(echo "${SEEN}" | jq '[.[] | select(.[0] == "reply_delta") | .[3]] | (max - min)')
[ "${SPREAD}" -ge 600 ] || { print_error "The pieces arrived together (${SPREAD} ms apart), not as the provider sent them"; exit 1; }
[ "$(sed -n 1p "${PROVIDER_LOG}" | jq -c '[.stream, .authorized]')" = '[true,true]' ] \
    || { print_error "The provider was not asked to stream, or not with the key: $(cat "${PROVIDER_LOG}")"; exit 1; }
[ "$(events "${SHOWN}" story | jq '[.[] | select(.kind == "reply_delta")] | length')" = "0" ] || { print_error "A piece was kept"; exit 1; }
[ "$(events "${SHOWN}" story | jq '[.[] | select(.kind == "reply")] | length')" = "1" ] || { print_error "More than one reply was kept: $(events "${SHOWN}" story)"; exit 1; }
print_success "Streamed in pieces over ${SPREAD} ms under one reply id, then said once, whole; the key attached by seal ✓"

# ---------------------------------------------------------------------------
print_step "2. A step named with _ does not stream; the run's output is the reply..."
SEEN=$(converse "${HIDDEN}" quiet "Tell me one.")
echo "  on the stream: $(echo "${SEEN}" | jq -c '[.[] | [.[0], .[2]]]')"
[ "$(echo "${SEEN}" | jq -c '[.[] | [.[0], .[2]]]')" = '[["reply","Once upon a time."]]' ] \
    || { print_error "A step named with _ was shown: ${SEEN}"; exit 1; }
[ "$(sed -n 2p "${PROVIDER_LOG}" | jq '.stream')" = "false" ] || { print_error "A step named with _ asked to stream: $(cat "${PROVIDER_LOG}")"; exit 1; }
print_success "No pieces; the reply is the run's output ✓"

# ---------------------------------------------------------------------------
print_step "3. A run that carries no conversation calls its model unstreamed..."
RESP=$(api_post "/workflows/${SHOWN}/execute" '{"inputs": {"data": {"message": "Tell me one."}, "variables": {}}}')
RUN=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${RUN}" ] || { print_error "Execute failed: ${RESP}"; exit 1; }
for _ in {1..90}; do
    STATUS=$(curl -sS "${API}/workflows/instances/${RUN}" | jq -r '.data.status // empty')
    case "${STATUS}" in completed|failed|cancelled) break ;; esac
    sleep 1
done
[ "${STATUS}" = "completed" ] || { print_error "The run ended ${STATUS}"; exit 1; }
[ "$(sed -n 3p "${PROVIDER_LOG}" | jq '.stream')" = "false" ] || { print_error "A run with no conversation asked to stream: $(cat "${PROVIDER_LOG}")"; exit 1; }
print_success "Unstreamed, as before ✓"

# ---------------------------------------------------------------------------
print_step "4. A tool is shown its arguments; what its step maps is fixed, and not shown..."
TOOLED=$(workflow ai-tooled "$(jq -n --arg conn "${CONN}" '{
    name: "ai-tooled", entryPoint: "answer",
    steps: {
        answer: { id: "answer", name: "_answer", stepType: "AiAgent", connectionId: $conn, config: {
            systemPrompt: { valueType: "immediate", value: "Use the tools." },
            userPrompt: { valueType: "reference", value: "data.message" },
            model: { valueType: "immediate", value: "gpt-4o-mini" } } },
        calc: { id: "calc", name: "Work out an arithmetic expression", stepType: "Agent",
            agentId: "utils", capabilityId: "calculate" },
        date: { id: "date", name: "Format a date", stepType: "Agent",
            agentId: "utils", capabilityId: "format-date-from-iso", inputMapping: {
                target_format: { valueType: "immediate", value: "%Y-%m-%d" } } },
        finish: { id: "finish", stepType: "Finish", inputMapping: {
            response: { valueType: "reference", value: "steps.answer.outputs.response" } } } },
    executionPlan: [
        { fromStep: "answer", toStep: "calc", label: "calculate" },
        { fromStep: "answer", toStep: "date", label: "format_date" },
        { fromStep: "answer", toStep: "finish", label: "next" } ],
    variables: {}, inputSchema: { message: { type: "string" } }, outputSchema: {} }')")
RESP=$(api_post "/workflows/${TOOLED}/execute" '{"inputs": {"data": {"message": "What is 2+2?"}, "variables": {}}}')
RUN=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${RUN}" ] || { print_error "Execute failed: ${RESP}"; exit 1; }
for _ in {1..90}; do
    STATUS=$(curl -sS "${API}/workflows/instances/${RUN}" | jq -r '.data.status // empty')
    case "${STATUS}" in completed|failed|cancelled) break ;; esac
    sleep 1
done
[ "${STATUS}" = "completed" ] || { print_error "The run ended ${STATUS}"; exit 1; }
OFFERED=$(tail -n 1 "${PROVIDER_LOG}" | jq -c '.tools')
echo "  offered: ${OFFERED}"
[ "$(echo "${OFFERED}" | jq -c '.[] | select(.name == "calculate") | .parameters.required')" = '["expression","variables"]' ] \
    || { print_error "calculate was not shown its inputs: ${OFFERED}"; exit 1; }
[ "$(echo "${OFFERED}" | jq -r '.[] | select(.name == "calculate") | .parameters.properties.expression.type')" = "string" ] \
    || { print_error "calculate's expression is not a string: ${OFFERED}"; exit 1; }
[ "$(echo "${OFFERED}" | jq -c '.[] | select(.name == "format_date") | .parameters.properties | keys')" = '["iso_date"]' ] \
    || { print_error "format_date's fixed target_format was shown, or iso_date was not: ${OFFERED}"; exit 1; }
print_success "calculate shows expression and variables; format_date shows iso_date and not its fixed format ✓"

echo ""
print_success "AI Agent streaming: shown as written, said once, and only where a conversation listens; tools shown their arguments ✓"
