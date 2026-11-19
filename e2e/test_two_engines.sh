#!/bin/bash
# E2E Test: two engines on one deployment (docs/multiple-engines-plan.md).
#
# The split stack with two engines (docker-compose.engines.yml): an API and
# intake asking either engine, and two executors told of both. Checked:
#
#   1. Both engines are members; each executor registered with each.
#   2. Workflows compile, and runs started through the API are started by
#      both engines.
#   3. A reply written in pieces reaches a stream read through either engine,
#      piece by piece and in order, then whole.
#   4. Cron starts each tick once, with two engines enqueuing it.
#   5. The admission counter agrees with the durable reservations.
#   6. An engine killed mid-run: the run ends completed, once, on the other.
#   7. An engine killed mid-compile: the other compiles it.
#
# Usage:
#   RUNTARA_IMAGE_TAG=<tag> ./e2e/test_two_engines.sh
# brings a stack up as project `runtara-two-engines` and takes it down after.
# To run against a stack already up (with the engines overlay):
#   COMPOSE_PROJECT=<project> START_STACK=0 RUNTARA_API_PORT=<port> ./e2e/test_two_engines.sh
#
# Prereqs: docker, jq, python3, and the image named by RUNTARA_IMAGE_TAG.
# SKIP_CRON=1 leaves out step 4 (it waits two minutes).
set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

COMPOSE_PROJECT="${COMPOSE_PROJECT:-runtara-two-engines}"
START_STACK="${START_STACK:-1}"
export RUNTARA_API_PORT="${RUNTARA_API_PORT:-7301}"
export RUNTARA_PORT="${RUNTARA_PORT:-8301}"
export RUNTARA_EXECUTORS="${RUNTARA_EXECUTORS:-2}"
export RUNTARA_ENGINES=2
TENANT="evaluation"
API="http://127.0.0.1:${RUNTARA_API_PORT}/api/runtime"
WORK="$(mktemp -d -t runtara_two_engines_XXXXXX)"
T0="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

compose() {
    docker compose -p "${COMPOSE_PROJECT}" --project-directory "${PROJECT_ROOT}" \
        -f "${PROJECT_ROOT}/docker-compose.yml" -f "${PROJECT_ROOT}/docker-compose.seal.yml" \
        -f "${PROJECT_ROOT}/docker-compose.executor.yml" -f "${PROJECT_ROOT}/docker-compose.roles.yml" \
        -f "${PROJECT_ROOT}/docker-compose.engines.yml" "$@"
}
# A query of the runtime database ($1) or the server database.
psql_in() {
    local database="$1"; shift
    compose exec -T postgres sh -c "psql -X -U \"\$POSTGRES_USER\" -d ${database} -tA" <<<"$*"
}
valkey() {
    # The evaluation stack's engine user (docker-compose.roles.yml).
    compose exec -T valkey valkey-cli --user default --pass runtara_engine --no-auth-warning "$@"
}
engines() { compose ps -q runtara; }
name_of() { docker inspect -f '{{.Name}}' "$1" | sed 's|^/||'; }

api_post() { curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"; }
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
    if [ "${START_STACK}" = "1" ] && [ "${KEEP_STACK:-0}" != "1" ]; then
        compose down -v >/dev/null 2>&1 || true
    elif [ "${START_STACK}" != "1" ]; then
        # Whatever was killed comes back.
        compose up -d --pull never runtara >/dev/null 2>&1 || true
    fi
    rm -rf "${WORK}"
    exit ${code}
}
trap cleanup EXIT

wait_healthy() {
    for _ in $(seq 1 120); do
        curl -fsS -o /dev/null "http://127.0.0.1:${RUNTARA_API_PORT}/health" 2>/dev/null && return 0
        sleep 2
    done
    print_error "The API did not become healthy"; exit 1
}

# Create, save and compile a workflow from a graph; echo its id.
workflow() {
    local name="$1" graph="$2" resp wf version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"two engines e2e\"}")
    wf=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }
    resp=$(api_post "/workflows/${wf}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update of ${name} failed: ${resp}"; exit 1; }
    version=$(latest_version "${wf}")
    resp=$(api_post "/workflows/${wf}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Compile of ${name} failed: ${resp}"; exit 1; }
    api_post "/workflows/${wf}/versions/${version}/set-current" '{}' >/dev/null
    echo "${wf}"
}
latest_version() {
    curl -sS "${API}/workflows/$1/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1'
}
instance_status() {
    curl -sS --max-time 10 "${API}/workflows/instances/$1" 2>/dev/null | jq -r '.data.status // .status // empty' 2>/dev/null || true
}

echo "==============================================================="
echo "E2E: two engines"
echo "==============================================================="

if [ "${START_STACK}" = "1" ]; then
    print_step "Starting ${COMPOSE_PROJECT} with two engines (image ${RUNTARA_IMAGE_TAG:-latest})..."
    compose up -d --pull never >/dev/null 2>&1
fi
wait_healthy

# --- 1. Members ---------------------------------------------------------------
print_step "Both engines are members, and each executor told each..."
for _ in $(seq 1 30); do
    MEMBERS=$(psql_in runtara "SELECT count(*) FROM engine_members WHERE last_seen > now() - interval '30 seconds'")
    [ "${MEMBERS}" = "2" ] && break
    sleep 2
done
[ "${MEMBERS}" = "2" ] || { print_error "${MEMBERS} live engines, not 2"; exit 1; }
for engine in $(engines); do
    for _ in $(seq 1 30); do
        TOLD=$(docker logs "${engine}" 2>&1 | grep 'an executor registered' | grep -o 'boot=[0-9a-f]*' | sort -u | wc -l | tr -d ' ')
        [ "${TOLD}" -ge "${RUNTARA_EXECUTORS}" ] && break
        sleep 2
    done
    [ "${TOLD}" -ge "${RUNTARA_EXECUTORS}" ] || { print_error "$(name_of "${engine}") heard from ${TOLD} executors"; exit 1; }
done
print_success "2 engines; each heard from ${RUNTARA_EXECUTORS} executors ✓"

# --- 2. Compiles and runs -----------------------------------------------------
print_step "Compiling the workflows..."
ANSWER=$(workflow two-engines-answer "$(jq -n '{
    name: "answer", entryPoint: "finish",
    steps: { finish: { id: "finish", stepType: "Finish", inputMapping: {
        answer: { valueType: "immediate", value: 42 } } } },
    executionPlan: [], variables: {}, inputSchema: {}, outputSchema: {} }')")
SLOW=$(workflow two-engines-slow "$(jq -n '{
    name: "slow", durable: true, entryPoint: "hold",
    steps: { hold: { id: "hold", stepType: "Delay", durationMs: { valueType: "immediate", value: 20000 } },
             finish: { id: "finish", stepType: "Finish", inputMapping: {
                 answer: { valueType: "immediate", value: "held" } } } },
    executionPlan: [ { fromStep: "hold", toStep: "finish" } ],
    variables: {}, inputSchema: {}, outputSchema: {} }')")
piece() {
    jq -n --arg id "$1" --arg text "$2" '{
        id: $id, stepType: "Agent", agentId: "conversation", capabilityId: "reply", maxRetries: 0,
        inputMapping: { text: { valueType: "immediate", value: $text },
                        replyId: { valueType: "immediate", value: "story" },
                        partial: { valueType: "immediate", value: true } } }'
}
short() { jq -n --arg id "$1" '{ id: $id, stepType: "Delay", durationMs: { valueType: "immediate", value: 700 } }'; }
WRITER=$(workflow two-engines-writer "$(jq -n \
    --argjson a "$(piece a "Once")" --argjson b "$(piece b " upon")" --argjson c "$(piece c " a time")" \
    --argjson w1 "$(short w1)" --argjson w2 "$(short w2)" '{
    name: "writer", durable: true, entryPoint: "a",
    steps: { a: $a, w1: $w1, b: $b, w2: $w2, c: $c,
             whole: { id: "whole", stepType: "Agent", agentId: "conversation", capabilityId: "reply", maxRetries: 0,
                      inputMapping: { text: { valueType: "immediate", value: "Once upon a time." },
                                      replyId: { valueType: "immediate", value: "story" } } },
             finish: { id: "finish", stepType: "Finish", inputMapping: {
                 response: { valueType: "immediate", value: "not said" } } } },
    executionPlan: [ { fromStep: "a", toStep: "w1" }, { fromStep: "w1", toStep: "b" },
                     { fromStep: "b", toStep: "w2" }, { fromStep: "w2", toStep: "c" },
                     { fromStep: "c", toStep: "whole" }, { fromStep: "whole", toStep: "finish" } ],
    variables: {}, inputSchema: {}, outputSchema: {} }')")
print_success "Compiled on the executors ✓"

print_step "Runs started through the API are started by both engines..."
SINCE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
for _ in $(seq 1 20); do
    ANSWERED=$(api_post "/events/http-sync/${ANSWER}" '{}' 180 | jq -r '.outputs.answer // empty')
    [ "${ANSWERED}" = "42" ] || { print_error "A run answered '${ANSWERED}'"; exit 1; }
done
for engine in $(engines); do
    STARTED=$(docker logs --since "${SINCE}" "${engine}" 2>&1 | grep -c 'Instance start durably queued' || true)
    [ "${STARTED}" -gt 0 ] || { print_error "$(name_of "${engine}") started none of 20 runs"; exit 1; }
    echo "  $(name_of "${engine}"): ${STARTED} of 20"
done
print_success "20 runs answered 42; both engines started some ✓"

# --- 3. A reply in pieces, across engines --------------------------------------
print_step "A reply written in pieces reaches a stream read through either engine..."
conversation() { echo "/workflows/$1/conversations/$2"; }
api_status PUT "$(conversation "${WRITER}" story)" '{}' >/dev/null
PIECES="${WORK}/pieces.txt"
(curl -sS -N --max-time 60 -H 'Accept: text/event-stream' "${API}$(conversation "${WRITER}" story)/events?kinds=reply_delta,reply,run_ended" 2>/dev/null \
    | python3 -u -c 'import sys, time
for line in sys.stdin:
    print(int(time.time() * 1000), line.rstrip(), flush=True)' > "${PIECES}") &
PIECES_PID=$!
sleep 1
api_status POST "$(conversation "${WRITER}" story)/messages" '{"id": "m-1", "text": "go"}' >/dev/null
for _ in $(seq 1 120); do grep -q 'event: run_ended' "${PIECES}" 2>/dev/null && break; sleep 0.5; done
kill "${PIECES_PID}" 2>/dev/null || true; wait "${PIECES_PID}" 2>/dev/null || true
SEEN=$(python3 - "${PIECES}" <<'PY'
import json, sys
seen, block = [], {}
def done(block):
    kind = block.get("event")
    if kind in ("reply_delta", "reply") and "data" in block:
        body = json.loads(block["data"])["body"]
        seen.append([kind, body.get("offset"), body["text"], block["at"]])
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
)
echo "  on the stream: $(echo "${SEEN}" | jq -c '[.[] | .[0:3]]')"
[ "$(echo "${SEEN}" | jq -c '[.[] | .[0:3]]')" = '[["reply_delta",0,"Once"],["reply_delta",4," upon"],["reply_delta",9," a time"],["reply",null,"Once upon a time."]]' ] \
    || { print_error "The reply did not reach the stream piece by piece, then whole"; exit 1; }
SPREAD=$(echo "${SEEN}" | jq '.[2][3] - .[0][3]')
[ "${SPREAD}" -gt 1000 ] || { print_error "The pieces arrived together (${SPREAD} ms apart)"; exit 1; }
print_success "Piece by piece, in order, then whole (${SPREAD} ms from first to last) ✓"

# --- 4. Cron ------------------------------------------------------------------
if [ "${SKIP_CRON:-0}" != "1" ]; then
    print_step "Cron starts each tick once (two minutes)..."
    TICKED=$(workflow two-engines-cron "$(jq -n '{
        name: "ticked", entryPoint: "finish",
        steps: { finish: { id: "finish", stepType: "Finish", inputMapping: {
            answer: { valueType: "immediate", value: 1 } } } },
        executionPlan: [], variables: {}, inputSchema: {}, outputSchema: {} }')")
    TRIGGER=$(api_post /triggers "{\"workflow_id\": \"${TICKED}\", \"trigger_type\": \"CRON\", \"active\": true, \"configuration\": {\"expression\": \"* * * * *\", \"timezone\": \"UTC\"}}" | jq -r '.data.id // .id // empty')
    [ -n "${TRIGGER}" ] || { print_error "The cron trigger was not made"; exit 1; }
    sleep 150
    curl -sS -X DELETE --max-time 30 "${API}/triggers/${TRIGGER}" >/dev/null || true
    sleep 5
    TICKS=$(psql_in runtara_server "SELECT count(DISTINCT idempotency_key) FROM execution_requests WHERE idempotency_key LIKE 'cron:${TRIGGER}:%'")
    RUNS=$(curl -sS "${API}/workflows/${TICKED}/instances?size=100" | jq '[(.data.content // .data // [])[]] | length')
    [ "${TICKS}" -ge 2 ] || { print_error "${TICKS} ticks in 150 seconds"; exit 1; }
    [ "${RUNS}" = "${TICKS}" ] || { print_error "${RUNS} runs for ${TICKS} ticks"; exit 1; }
    print_success "${TICKS} ticks, ${RUNS} runs ✓"
fi

# --- 5. The admission counter -------------------------------------------------
print_step "The admission counter agrees with the durable reservations..."
for _ in $(seq 1 40); do
    HELD=$(valkey GET "${TENANT}:admission:reserved" | tr -d '\r')
    OWED=$(psql_in runtara_server "SELECT count(*) FROM execution_admission_reservations WHERE tenant_id = '${TENANT}' AND released_at IS NULL")
    [ "${HELD:-0}" = "${OWED}" ] && break
    sleep 3
done
[ "${HELD:-0}" = "${OWED}" ] || { print_error "The counter says ${HELD}, the reservations ${OWED}"; exit 1; }
KEEPERS=$(valkey GET "runtara:singleton:admission-counter:${TENANT}" | tr -d '\r')
[ -n "${KEEPERS}" ] || { print_error "No engine keeps the counter"; exit 1; }
print_success "Counter ${HELD:-0} = reservations ${OWED}; kept by ${KEEPERS} ✓"

# --- 6. An engine killed mid-run ----------------------------------------------
print_step "An engine killed mid-run: its run ends on the other..."
INSTANCE=$(api_post "/workflows/${SLOW}/execute" '{"inputs": {"data": {}}}' | jq -er '.data.instanceId')
OWNER=""
for _ in $(seq 1 60); do
    OWNER=$(psql_in runtara "SELECT lease_owner FROM instance_launches WHERE instance_id = '${INSTANCE}' AND lease_owner IS NOT NULL ORDER BY 1 LIMIT 1")
    [ -n "${OWNER}" ] && break
    sleep 0.5
done
[ -n "${OWNER}" ] || { print_error "No engine took the run"; exit 1; }
VICTIM=""
for engine in $(engines); do
    LOGS=$(docker logs "${engine}" 2>&1)
    case "${LOGS}" in *"owner=${OWNER}"*) VICTIM="${engine}" ;; esac
done
[ -n "${VICTIM}" ] || { print_error "No engine is ${OWNER}"; exit 1; }
sleep 3
docker kill "${VICTIM}" >/dev/null
echo "  killed $(name_of "${VICTIM}"), which ran ${INSTANCE}"
STATUS=""
for _ in $(seq 1 120); do
    STATUS=$(instance_status "${INSTANCE}")
    case "${STATUS}" in completed|failed|cancelled) break ;; esac
    sleep 2
done
[ "${STATUS}" = "completed" ] || { print_error "The run is ${STATUS:-unknown} after its engine was killed"; exit 1; }
LAUNCHES=$(psql_in runtara "SELECT count(*) FROM instance_launches WHERE instance_id = '${INSTANCE}'")
compose up -d --pull never runtara >/dev/null 2>&1
print_success "The run completed on the other engine (${LAUNCHES} launch rows) ✓"

# --- 7. An engine killed mid-compile ------------------------------------------
print_step "An engine killed mid-compile: the other compiles it..."
wait_healthy
for _ in $(seq 1 30); do
    [ "$(psql_in runtara "SELECT count(*) FROM engine_members WHERE last_seen > now() - interval '30 seconds'")" -ge 2 ] && break
    sleep 2
done
GRAPH=$(jq -n '{ name: "answer", entryPoint: "finish",
    steps: { finish: { id: "finish", stepType: "Finish", inputMapping: {
        answer: { valueType: "immediate", value: 43 } } } },
    executionPlan: [], variables: {}, inputSchema: {}, outputSchema: {} }')
SINCE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
api_post "/workflows/${ANSWER}/update" "{\"executionGraph\": ${GRAPH}}" >/dev/null
VERSION=$(latest_version "${ANSWER}")
VICTIM=""
for _ in $(seq 1 200); do
    for engine in $(engines); do
        TAKEN=$(docker logs --since "${SINCE}" "${engine}" 2>&1 | grep 'Processing compilation request' \
            | grep "workflow_id=${ANSWER}" | grep -c "version=${VERSION} " || true)
        if [ "${TAKEN}" -gt 0 ]; then
            VICTIM="${engine}"; break 2
        fi
    done
    sleep 0.1
done
[ -n "${VICTIM}" ] || { print_error "No engine took the compile of version ${VERSION}"; exit 1; }
docker kill "${VICTIM}" >/dev/null
echo "  killed $(name_of "${VICTIM}"), compiling version ${VERSION}"
COMPILED=""
for _ in $(seq 1 60); do
    COMPILED=$(curl -sS --max-time 10 "${API}/workflows/${ANSWER}/versions" 2>/dev/null \
        | jq -r --argjson v "${VERSION}" '.data[]? | select((.version // .versionNumber) == $v) | (.compilationStatus // .status // empty)' 2>/dev/null || true)
    [ "${COMPILED}" = "success" ] && break
    sleep 2
done
[ "${COMPILED}" = "success" ] || { print_error "Version ${VERSION} is '${COMPILED}' after its engine was killed"; exit 1; }
compose up -d --pull never runtara >/dev/null 2>&1
print_success "Version ${VERSION} compiled by the other engine ✓"

echo
print_success "Two engines: all checks passed (since ${T0})"
