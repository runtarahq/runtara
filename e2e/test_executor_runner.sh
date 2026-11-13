#!/bin/bash
# E2E Test: the server runs workflows on an executor (docs/executor.md).
#
# Builds `runtara-server`, `runtara-executor` and `runtara-run`, makes the
# deployment's certificates, starts an executor, and runs the given e2e
# suites against a server with RUNTARA_RUNNER=executor. Every workflow of the
# suites is then compiled by the server, staged with the executor over mutual
# TLS and run in a `runtara-run` process of its own (sandboxed on Linux),
# its workflow API calls coming back over the run channel. Checks that the
# suites' launches went through the executor.
#
# Usage: e2e/test_executor_runner.sh [suite ...]
#   suites are e2e/test_<suite>.sh names; default: workflow_agent_parity
#   child_run_fan_out child_run_deadline.
# Prerequisites: those of the suites (Postgres with POSTGRES_PASSWORD or
# PGPASSWORD, docker, the built agent components).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# runtara-server needs a credential encryption key; a throwaway one per run.
# shellcheck source=../scripts/credential-key.sh
source "${PROJECT_ROOT}/scripts/credential-key.sh"
ensure_credential_key
SUITES=("$@")
[ ${#SUITES[@]} -gt 0 ] || SUITES=(workflow_agent_parity child_run_fan_out child_run_deadline)

EXECUTOR_PORT="${TEST_EXECUTOR_PORT:-17443}"
RUN_CHANNEL_PORT="${TEST_RUN_CHANNEL_PORT:-17444}"
TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"

WORK_DIR="$(mktemp -d -t runtara_executor_runner_e2e_XXXXXX)"
EXECUTOR_PID=""
KEEP_LOGS=0
cleanup() {
    [ -z "${EXECUTOR_PID}" ] || kill "${EXECUTOR_PID}" 2>/dev/null || true
    if [ "${KEEP_LOGS}" = 1 ]; then
        # The certificates' keys go; the logs stay for whoever reads the failure.
        rm -rf "${WORK_DIR}/pki-server" "${WORK_DIR}/pki-executor" "${WORK_DIR}/executor-state"
        echo "logs kept in ${WORK_DIR}" >&2
    else
        rm -rf "${WORK_DIR}"
    fi
}
trap cleanup EXIT
SERVER_LOG="${WORK_DIR}/server.log"
EXECUTOR_LOG="${WORK_DIR}/executor.log"

echo "Building runtara-server, runtara-executor and runtara-run..."
(cd "${PROJECT_ROOT}" && SQLX_OFFLINE="${SQLX_OFFLINE:-true}" \
    cargo build -p runtara-server -p runtara-executor -p runtara-run >&2)

"${TARGET_DIR}/debug/runtara-executor" init-pki \
    --server-dir "${WORK_DIR}/pki-server" --executor-dir "${WORK_DIR}/pki-executor" >&2

# Only Linux has the sandbox; elsewhere the runs are plain processes, which
# still exercises everything between the server and the guest.
UNSANDBOXED=false
[ "$(uname -s)" = "Linux" ] || UNSANDBOXED=true

RUNTARA_EXECUTOR_LISTEN="127.0.0.1:${EXECUTOR_PORT}" \
RUNTARA_EXECUTOR_TLS_DIR="${WORK_DIR}/pki-executor" \
RUNTARA_EXECUTOR_STATE_DIR="${WORK_DIR}/executor-state" \
RUNTARA_EXECUTOR_UNSANDBOXED="${UNSANDBOXED}" \
RUNTARA_EXECUTOR_NO_LANDLOCK="${RUNTARA_EXECUTOR_NO_LANDLOCK:-false}" \
    "${TARGET_DIR}/debug/runtara-executor" >"${EXECUTOR_LOG}" 2>&1 &
EXECUTOR_PID=$!
for _ in $(seq 1 50); do
    grep -q "executor listening" "${EXECUTOR_LOG}" && break
    kill -0 "${EXECUTOR_PID}" 2>/dev/null || { cat "${EXECUTOR_LOG}" >&2; echo "the executor did not start" >&2; exit 1; }
    sleep 0.2
done
grep -q "executor listening" "${EXECUTOR_LOG}" || { cat "${EXECUTOR_LOG}" >&2; echo "the executor did not start" >&2; exit 1; }

# The suites run whatever RUNTARA_SERVER_BIN names; this one is the stock
# server told to use the executor, and keeps the runner's launch lines, which
# the suites' own logs do not outlive.
WRAPPER="${WORK_DIR}/runtara-server"
cat > "${WRAPPER}" <<WRAP
#!/bin/bash
export RUNTARA_RUNNER=executor
export RUNTARA_EXECUTOR_URL="https://127.0.0.1:${EXECUTOR_PORT}"
export RUNTARA_EXECUTOR_TLS_DIR="${WORK_DIR}/pki-server"
export RUNTARA_RUN_CHANNEL_LISTEN="127.0.0.1:${RUN_CHANNEL_PORT}"
export RUNTARA_RUN_CHANNEL_ENDPOINT="https://127.0.0.1:${RUN_CHANNEL_PORT}"
exec "${TARGET_DIR}/debug/runtara-server" "\$@" 2> >(tee -a "${SERVER_LOG}" >&2)
WRAP
chmod +x "${WRAPPER}"

for suite in "${SUITES[@]}"; do
    echo "== ${suite}"
    RUNTARA_SERVER_BIN="${WRAPPER}" bash "${SCRIPT_DIR}/test_${suite}.sh" \
        || { KEEP_LOGS=1; echo "--- executor log" >&2; tail -50 "${EXECUTOR_LOG}" >&2; exit 1; }
done

LAUNCHES=$(grep -c "executor runner: launch" "${SERVER_LOG}" || true)
PLACED=$(grep -c "run placed" "${EXECUTOR_LOG}" || true)
[ "${LAUNCHES}" -gt 0 ] && [ "${PLACED}" -gt 0 ] \
    || { echo "no launch went through the executor" >&2; exit 1; }
echo "[SUCCESS] ${#SUITES[@]} suite(s) passed on the executor (${LAUNCHES} launches, ${PLACED} runs placed)"
