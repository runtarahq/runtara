#!/bin/bash
# E2E Test: the instance protocol must answer with the status code that
# describes what actually went wrong.
#
# `CoreError` already classifies its failures, but the HTTP layer used to throw
# that away and answer 500 for all of it. The cost is diagnosability: a
# checkpoint landing on an instance that already finished — an ordinary race
# during drain, the caller's problem, never fixable by retrying — looked
# identical to the database being down.
#
# Asserts, against a real process over real HTTP:
#   * a checkpoint against an unknown instance is 404, not 500,
#   * a checkpoint against a terminal instance is 409, not 500,
#   * an unknown instance's input is 404 (already true — pinned so the
#     error-arm rewrite cannot regress it),
#   * the per-route `code` in the body is unchanged, since clients read it,
#   * and those 4xx answers are logged at WARN, not ERROR. That is the other
#     half of the fix: a drain race should stop paging whoever reads the logs.
#
# Self-contained: needs cargo, curl and SQLite only. No docker, no Postgres, no
# ./start.sh.

set -uo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

WORK_DIR="$(mktemp -d)"
LOG_FILE="${WORK_DIR}/core.log"
CORE_PID=""

cleanup() {
    if [ -n "${CORE_PID}" ] && kill -0 "${CORE_PID}" 2>/dev/null; then
        kill -KILL "${CORE_PID}" 2>/dev/null
        wait "${CORE_PID}" 2>/dev/null
    fi
    rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

fail() {
    print_error "$1"
    echo "----- core log -----"
    cat "${LOG_FILE}" 2>/dev/null
    echo "--------------------"
    exit 1
}

# Pick a free port by binding and releasing it.
pick_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

print_step "Building runtara-core"

# Build from the repo root, not the caller's cwd: rustup resolves
# rust-toolchain.toml from the working directory, so invoking this from
# elsewhere builds with whatever toolchain happens to be default.
if ! (cd "${PROJECT_ROOT}" && cargo build -p runtara-core --bin runtara-core) >/dev/null 2>&1; then
    (cd "${PROJECT_ROOT}" && cargo build -p runtara-core --bin runtara-core)
    fail "cargo build failed"
fi

# Ask cargo where the binary landed rather than assuming ./target — a shared CI
# cache sets CARGO_TARGET_DIR.
TARGET_DIR="$(cd "${PROJECT_ROOT}" && cargo metadata --format-version 1 --no-deps |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
[ -n "${TARGET_DIR}" ] || fail "could not resolve cargo target directory"
CORE_BIN="${TARGET_DIR}/debug/runtara-core"
[ -x "${CORE_BIN}" ] || fail "runtara-core binary not found at ${CORE_BIN}"

PORT="$(pick_port)"
print_step "Starting runtara-core on 127.0.0.1:${PORT} (SQLite)"

# `dotenvy` walks up to the repo root, so run from the temp dir to keep a
# developer's own .env out of this test's configuration.
(
    cd "${WORK_DIR}" || exit 1
    RUNTARA_DATABASE_URL="sqlite://${WORK_DIR}/core.db?mode=rwc" \
    RUNTARA_HTTP_PORT="${PORT}" \
    RUST_LOG="runtara_core=info" \
    exec "${CORE_BIN}"
) >"${LOG_FILE}" 2>&1 &
CORE_PID=$!

print_step "Waiting for /health"
READY=0
for _ in $(seq 1 100); do
    if ! kill -0 "${CORE_PID}" 2>/dev/null; then
        fail "runtara-core exited during startup"
    fi
    if curl -sf -o /dev/null "http://127.0.0.1:${PORT}/health"; then
        READY=1
        break
    fi
    sleep 0.2
done
[ "${READY}" = "1" ] || fail "runtara-core never became healthy on port ${PORT}"
print_success "runtara-core is up (pid ${CORE_PID})"

API="http://127.0.0.1:${PORT}/api/v1/instances"
BODY_FILE="${WORK_DIR}/response.json"

# Issue a request, leaving the response body in BODY_FILE and echoing the status.
call() {
    curl -sS -o "${BODY_FILE}" -w '%{http_code}' "$@"
}

# Assert status, and (when a third argument is given) the body's `code` field.
expect() {
    local label="$1" want_status="$2" want_code="${3:-}"
    local got_status="$4"
    local body; body="$(cat "${BODY_FILE}")"

    echo "  ${label} -> ${got_status} ${body}"
    [ "${got_status}" = "${want_status}" ] || \
        fail "${label}: got ${got_status}, expected ${want_status} (body: ${body})"

    if [ -n "${want_code}" ]; then
        local got_code; got_code="$(python3 -c \
            'import json,sys; print(json.load(sys.stdin).get("code",""))' <"${BODY_FILE}")"
        [ "${got_code}" = "${want_code}" ] || \
            fail "${label}: body code was '${got_code}', expected '${want_code}' — clients read this field"
    fi
}

CHECKPOINT='{"checkpoint_id":"cp-1","state":"aGk="}'

# ---------------------------------------------------------------------------
# 1. A checkpoint against an instance that was never registered.
#    The caller named something that does not exist: 404, not 500.
# ---------------------------------------------------------------------------
print_step "Checkpoint against an unknown instance"
status="$(call -X POST "${API}/ghost/checkpoint" -H 'Content-Type: application/json' -d "${CHECKPOINT}")"
expect "checkpoint on unknown instance" 404 CHECKPOINT_ERROR "${status}"

# ---------------------------------------------------------------------------
# 2. A checkpoint against an instance that already finished — the drain race
#    this test exists for. The instance is real but is past accepting work:
#    409, and emphatically not a 5xx the client would retry forever.
# ---------------------------------------------------------------------------
print_step "Checkpoint against a completed instance"
call -X POST "${API}/inst-done/register" -H 'Content-Type: application/json' \
    -d '{"tenant_id":"e2e-status"}' >/dev/null
status="$(call -X POST "${API}/inst-done/completed" -H 'Content-Type: application/json' -d '{"output":"b2s="}')"
expect "completing inst-done" 200 "" "${status}"

status="$(call -X POST "${API}/inst-done/checkpoint" -H 'Content-Type: application/json' -d "${CHECKPOINT}")"
expect "checkpoint on completed instance" 409 CHECKPOINT_ERROR "${status}"

# ---------------------------------------------------------------------------
# 3. An unknown instance's input. Already 404 before the error-arm rewrite;
#    asserted here so it stays that way.
# ---------------------------------------------------------------------------
print_step "Input of an unknown instance"
status="$(call "${API}/ghost/input")"
expect "input of unknown instance" 404 "" "${status}"

# ---------------------------------------------------------------------------
# 4. The happy path is undisturbed by the rewrite.
# ---------------------------------------------------------------------------
print_step "A checkpoint on a running instance still succeeds"
call -X POST "${API}/inst-live/register" -H 'Content-Type: application/json' \
    -d '{"tenant_id":"e2e-status"}' >/dev/null
status="$(call -X POST "${API}/inst-live/checkpoint" -H 'Content-Type: application/json' -d "${CHECKPOINT}")"
expect "checkpoint on running instance" 200 "" "${status}"

# ---------------------------------------------------------------------------
# 5. Registering a resume against a checkpoint that does not exist. The same
#    fact as a missing checkpoint anywhere else, so it must get the same 404 —
#    /register used to answer 400 for it, because it reports refusals through
#    its own response shape rather than through the error type.
# ---------------------------------------------------------------------------
print_step "Resuming from a checkpoint that does not exist"
status="$(call -X POST "${API}/inst-live/register" -H 'Content-Type: application/json' \
    -d '{"tenant_id":"e2e-status","checkpoint_id":"no-such-checkpoint"}')"
expect "register resuming from a missing checkpoint" 404 REGISTER_ERROR "${status}"

# ---------------------------------------------------------------------------
# 6. The logging half: a caller's mistake is a WARN, not an ERROR. Without this
#    the status codes could be right while the logs still read like an outage.
# ---------------------------------------------------------------------------
print_step "Caller errors are logged at WARN, not ERROR"
if ! grep -q "WARN.*Checkpoint handler error" "${LOG_FILE}"; then
    fail "expected a WARN for the 4xx checkpoint failures; none found"
fi
if grep -q "ERROR.*Checkpoint handler error" "${LOG_FILE}"; then
    fail "a 4xx checkpoint failure was logged at ERROR — that is the noise this change removes"
fi
echo "  $(grep -c "WARN.*Checkpoint handler error" "${LOG_FILE}") WARN, 0 ERROR for checkpoint failures"

# ---------------------------------------------------------------------------
# 7. The drain refusal keeps its Retry-After. The header is shared with the new
#    5xx answers, so a regression in one would show up here.
# ---------------------------------------------------------------------------
print_step "A refusal still tells the client when to come back"
HEADERS="$(curl -sS -D - -o /dev/null -X POST "${API}/ghost-2/checkpoint" \
    -H 'Content-Type: application/json' -d "${CHECKPOINT}")"
if grep -qi "^retry-after:" <<<"${HEADERS}"; then
    fail "a 404 carried Retry-After — that header belongs to \"come back later\", not \"you were wrong\""
fi
echo "  404 carries no Retry-After, as intended"

print_success "Instance protocol status codes and log levels are correct"
