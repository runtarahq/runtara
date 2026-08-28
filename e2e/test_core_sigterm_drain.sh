#!/bin/bash
# E2E Test: the standalone runtara-core binary must handle SIGTERM, drain
# before it shuts down, and actually apply the concurrency cap it logs.
#
# Docker, Kubernetes and systemd all stop a process with SIGTERM, then SIGKILL
# after their own grace period. A binary that awaits only Ctrl+C leaves
# SIGTERM's default disposition in place: the kernel kills it, the drain never
# runs, and every running instance is severed mid-flight.
#
# Asserts, against a real process:
#   * SIGTERM produces a clean exit (status 0) — unpatched it is 143,
#   * the drain flag is flipped BEFORE the server stops accepting, i.e. the log
#     shows signal -> "CoreRuntime draining" -> "CoreRuntime shutting down" in
#     that order. Ordering is the property: refusing new registrations only
#     helps if it happens while the server is still serving,
#   * RUNTARA_MAX_CONCURRENT_INSTANCES is applied, not merely logged — the
#     registration past the cap gets 429, not 200,
#   * and the cap counts running work only: suspending an instance frees its
#     slot, so workflows parked in a durable sleep cannot wedge it shut.
#
# Needs cargo, curl and a reachable Postgres; it creates and drops its own
# throwaway database. No docker, no ./start.sh. Everything else runs in this
# one shell invocation, including the signal, so nothing races a tool
# round-trip.

set -uo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

CAP="${CAP:-2}"                       # RUNTARA_MAX_CONCURRENT_INSTANCES
GRACE_MS="${GRACE_MS:-5000}"          # RUNTARA_CORE_SHUTDOWN_GRACE_MS
EXIT_WAIT_S="${EXIT_WAIT_S:-30}"      # how long to wait for the process to exit

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

# A throwaway database per run, not a shared one. The concurrency cap this test
# asserts is enforced by `SELECT COUNT(*) FROM instances WHERE status='running'`
# with no tenant filter, so any pre-existing running row would move the cap and
# fail the test for the wrong reason. `$$` keeps two concurrent runs apart.
TEST_DB="runtara_e2e_core_sigterm_$$"

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql \
        -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" \
        -v ON_ERROR_STOP=1 -tA "$@"
}

DB_CREATED=0

WORK_DIR="$(mktemp -d)"
LOG_FILE="${WORK_DIR}/core.log"
CORE_PID=""

cleanup() {
    if [ -n "${CORE_PID}" ] && kill -0 "${CORE_PID}" 2>/dev/null; then
        kill -KILL "${CORE_PID}" 2>/dev/null
        wait "${CORE_PID}" 2>/dev/null
    fi
    if [ "${DB_CREATED}" = "1" ]; then
        # WITH (FORCE) is required, not tidiness: every fail() path SIGKILLs the
        # core, and a plain DROP loses to "database is being accessed by other
        # users", leaking one database per aborted run.
        psql_quiet -d postgres \
            -c "DROP DATABASE IF EXISTS \"${TEST_DB}\" WITH (FORCE);" >/dev/null 2>&1
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

print_step "Checking Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"
# Before the build, not after: a missing database should cost a second, not a
# full debug compile of runtara-core.
if ! psql_quiet -d postgres -c 'SELECT 1;' >/dev/null 2>&1; then
    fail "cannot reach Postgres as ${POSTGRES_USER}@${POSTGRES_HOST}:${POSTGRES_PORT} (set POSTGRES_HOST/PORT/USER/PASSWORD)"
fi

print_step "Building runtara-core"

# Build from the repo root, not the caller's cwd. `--manifest-path` alone is
# not enough: rustup resolves rust-toolchain.toml from the working directory,
# so invoking this from elsewhere builds with whatever toolchain happens to be
# default and trips the crate's rust-version floor.
if ! (cd "${PROJECT_ROOT}" && cargo build -p runtara-core --bin runtara-core) >/dev/null 2>&1; then
    (cd "${PROJECT_ROOT}" && cargo build -p runtara-core --bin runtara-core)
    fail "cargo build failed"
fi

# Ask cargo where the binary landed rather than assuming ./target — a shared
# CI cache sets CARGO_TARGET_DIR, and guessing turns a build that actually
# succeeded into "binary not found".
TARGET_DIR="$(cd "${PROJECT_ROOT}" && cargo metadata --format-version 1 --no-deps |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
[ -n "${TARGET_DIR}" ] || fail "could not resolve cargo target directory"
CORE_BIN="${TARGET_DIR}/debug/runtara-core"
[ -x "${CORE_BIN}" ] || fail "runtara-core binary not found at ${CORE_BIN}"

print_step "Creating throwaway database ${TEST_DB}"
psql_quiet -d postgres -c "DROP DATABASE IF EXISTS \"${TEST_DB}\" WITH (FORCE);" >/dev/null 2>&1
psql_quiet -d postgres -c "CREATE DATABASE \"${TEST_DB}\";" >/dev/null || fail "could not create ${TEST_DB}"
DB_CREATED=1
# The core migrations use gen_random_uuid() (built in since PG13), but the
# crate's own parity harness provisions pgcrypto before running the same
# migrator; mirror it so the two provisioning paths cannot disagree.
psql_quiet -d "${TEST_DB}" -c 'CREATE EXTENSION IF NOT EXISTS pgcrypto;' >/dev/null \
    || fail "could not create pgcrypto extension in ${TEST_DB}"

PORT="$(pick_port)"
print_step "Starting runtara-core on 127.0.0.1:${PORT} (Postgres, cap=${CAP}, grace=${GRACE_MS}ms)"

# Run from the temp dir so `dotenvy`, which walks up to the repo root, cannot
# apply the developer's .env. RUNTARA_DATABASE_URL is passed explicitly and
# dotenvy does not override the environment, so the database is never at risk;
# what this guards is RUST_LOG and the OTEL exporter settings, which would
# otherwise replace the log filter the assertions below read.
(
    cd "${WORK_DIR}" || exit 1
    RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB}" \
    RUNTARA_HTTP_PORT="${PORT}" \
    RUNTARA_MAX_CONCURRENT_INSTANCES="${CAP}" \
    RUNTARA_CORE_SHUTDOWN_GRACE_MS="${GRACE_MS}" \
    RUST_LOG="runtara_core=info" \
    exec "${CORE_BIN}"
) >"${LOG_FILE}" 2>&1 &
CORE_PID=$!

print_step "Waiting for /health"
READY=0
for _ in $(seq 1 200); do
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

# ---------------------------------------------------------------------------
# 1. The concurrency cap is applied, not just logged.
#
# `register` inserts the row then flips it to `running`, and the cap counts
# rows in `running`/`suspended` — so sequential registrations really do walk
# the counter up to the limit.
# ---------------------------------------------------------------------------
register() {
    curl -s -o /dev/null -w '%{http_code}' \
        -X POST "http://127.0.0.1:${PORT}/api/v1/instances/$1/register" \
        -H 'Content-Type: application/json' \
        -d '{"tenant_id":"e2e-sigterm"}'
}

print_step "Registering ${CAP} instances, then one past the cap"
for i in $(seq 1 "${CAP}"); do
    code="$(register "live-${i}")"
    [ "${code}" = "200" ] || fail "register live-${i} returned ${code}, expected 200"
    echo "  register live-${i} -> ${code} ok"
done

over_code="$(register "live-over")"
echo "  register live-over -> ${over_code}"
if [ "${over_code}" != "429" ]; then
    fail "registration past the cap returned ${over_code}, expected 429 (RUNTARA_MAX_CONCURRENT_INSTANCES is not wired into the builder)"
fi
print_success "cap enforced: fresh registration past ${CAP} rejected with 429"

# ---------------------------------------------------------------------------
# 2. Parked work does not hold a slot.
#
# Durable sleep and signal-waits both park an instance in `suspended`, and a
# workflow can sit there for days. If the cap counted those rows, a handful of
# long-parked workflows would hold it closed forever — there is no reaper in
# this crate to open it again. Suspending one instance must free exactly one
# slot.
# ---------------------------------------------------------------------------
print_step "Suspending live-1, then registering past the cap again"
susp_code="$(curl -s -o /dev/null -w '%{http_code}' \
    -X POST "http://127.0.0.1:${PORT}/api/v1/instances/live-1/suspended")"
[ "${susp_code}" = "200" ] || fail "suspending live-1 returned ${susp_code}, expected 200"

after_code="$(register "live-after-park")"
echo "  register live-after-park -> ${after_code}"
if [ "${after_code}" != "200" ]; then
    fail "registration after a suspend returned ${after_code}, expected 200 (a parked instance is holding a concurrency slot)"
fi
print_success "suspended instance freed its slot"

# ---------------------------------------------------------------------------
# 3. SIGTERM is handled, and the drain happens before the shutdown.
# ---------------------------------------------------------------------------
print_step "Sending SIGTERM to pid ${CORE_PID}"
kill -TERM "${CORE_PID}"

EXITED=0
for _ in $(seq 1 $((EXIT_WAIT_S * 5))); do
    if ! kill -0 "${CORE_PID}" 2>/dev/null; then
        EXITED=1
        break
    fi
    sleep 0.2
done
if [ "${EXITED}" != "1" ]; then
    fail "runtara-core did not exit within ${EXIT_WAIT_S}s of SIGTERM"
fi

wait "${CORE_PID}"
STATUS=$?
CORE_PID=""

echo "  exit status: ${STATUS}"
if [ "${STATUS}" != "0" ]; then
    # 143 = 128 + SIGTERM: the default disposition ran, so the binary never
    # installed a handler and nothing drained.
    fail "runtara-core exited ${STATUS} on SIGTERM, expected 0 (143 means SIGTERM was never handled)"
fi
print_success "clean exit on SIGTERM"

# ---------------------------------------------------------------------------
# 4. Ordering. Presence alone would also pass for a binary that shut the
#    server down first and only then flipped the drain flag, which is the bug
#    this ticket is about.
# ---------------------------------------------------------------------------
print_step "Checking drain-before-shutdown ordering in the log"

line_of() {
    grep -n -- "$1" "${LOG_FILE}" | head -1 | cut -d: -f1
}

SIGNAL_LINE="$(line_of 'Shutdown signal received')"
DRAIN_LINE="$(line_of 'CoreRuntime draining')"
SHUTDOWN_LINE="$(line_of 'CoreRuntime shutting down')"

[ -n "${SIGNAL_LINE}" ]   || fail "log has no 'Shutdown signal received' line"
[ -n "${DRAIN_LINE}" ]    || fail "log has no 'CoreRuntime draining' line — set_draining() was never called"
[ -n "${SHUTDOWN_LINE}" ] || fail "log has no 'CoreRuntime shutting down' line"

if ! grep -q 'SIGTERM' "${LOG_FILE}"; then
    fail "log does not name SIGTERM as the signal received"
fi

if [ "${SIGNAL_LINE}" -ge "${DRAIN_LINE}" ] || [ "${DRAIN_LINE}" -ge "${SHUTDOWN_LINE}" ]; then
    fail "expected signal(${SIGNAL_LINE}) < draining(${DRAIN_LINE}) < shutting down(${SHUTDOWN_LINE})"
fi

sed -n "${SIGNAL_LINE},${SHUTDOWN_LINE}p" "${LOG_FILE}" | sed 's/^/  /'
print_success "drained before shutting down, in that order"

echo ""
print_success "SIGTERM drain e2e passed"
