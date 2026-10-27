#!/bin/bash
# E2E Test: the server runs workflows on a runner from outside
# runtara-environment (M3.1).
#
# Builds `runtara-server-delegating` (crates/runtara-runner-delegate: the stock
# server booted with a runner that delegates to the embedded one), runs the
# given e2e suites against it, and checks that their launches went through
# the delegating runner.
#
# Usage: e2e/test_delegating_runner.sh [suite ...]
#   suites are e2e/test_<suite>.sh names; default: workflow_agent_parity
#   child_run_fan_out child_run_deadline.
# Prerequisites: those of the suites (Postgres with POSTGRES_PASSWORD or
# PGPASSWORD, docker, the built agent components).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SUITES=("$@")
[ ${#SUITES[@]} -gt 0 ] || SUITES=(workflow_agent_parity child_run_fan_out child_run_deadline)

WORK_DIR="$(mktemp -d -t runtara_delegating_runner_e2e_XXXXXX)"
trap 'rm -rf "${WORK_DIR}"' EXIT
LAUNCH_LOG="${WORK_DIR}/launches.log"

echo "Building runtara-server-delegating..."
(cd "${PROJECT_ROOT}" && SQLX_OFFLINE="${SQLX_OFFLINE:-true}" \
    cargo build -p runtara-runner-delegate --bin runtara-server-delegating >&2)

# The suites run whatever RUNTARA_SERVER_BIN names; this one keeps the
# runner's launch lines, which the suites' own logs do not outlive.
WRAPPER="${WORK_DIR}/runtara-server"
cat > "${WRAPPER}" <<WRAP
#!/bin/bash
exec "${PROJECT_ROOT}/target/debug/runtara-server-delegating" "\$@" 2> >(tee -a "${LAUNCH_LOG}" >&2)
WRAP
chmod +x "${WRAPPER}"

for suite in "${SUITES[@]}"; do
    echo "== ${suite}"
    RUNTARA_SERVER_BIN="${WRAPPER}" bash "${SCRIPT_DIR}/test_${suite}.sh"
done

LAUNCHES=$(grep -c "delegating runner: launch" "${LAUNCH_LOG}" || true)
[ "${LAUNCHES}" -gt 0 ] || { echo "no launch went through the delegating runner" >&2; exit 1; }
echo "[SUCCESS] ${#SUITES[@]} suite(s) passed on the delegating runner (${LAUNCHES} launches through it)"
