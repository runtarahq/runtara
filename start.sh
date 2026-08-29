#!/bin/bash
# Runtara Development Launcher
# Starts runtara-server, which embeds runtara-environment and runtara-core.

set -e

# Load .env file if it exists
if [ -f .env ]; then
    set -a
    source .env
    set +a
fi

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Default configuration
DATA_DIR="${DATA_DIR:-.data}"
TENANT_ID="${TENANT_ID:-local}"

# Runtara keeps three databases: the server's own, the durable-runtime one
# shared by core and environment, and the tenant object model.
SERVER_DATABASE_URL="${RUNTARA_SERVER_DATABASE_URL:-postgres://localhost/runtara_server}"
DATABASE_URL="${RUNTARA_DATABASE_URL:-postgres://localhost/runtara}"
OBJECT_MODEL_DATABASE_URL="${OBJECT_MODEL_DATABASE_URL:-postgres://localhost/runtara_objects}"

# Valkey backs checkpoint storage; the server refuses to boot without it.
VALKEY_HOST="${VALKEY_HOST:-127.0.0.1}"
VALKEY_PORT="${VALKEY_PORT:-6379}"

# Port configuration
SERVER_PORT="${SERVER_PORT:-7001}"            # API + UI
CORE_PORT="${RUNTARA_CORE_HTTP_PORT:-8003}"   # Core: instance API (see RUNTARA_CORE_HTTP_PORT)
ENV_PORT="${RUNTARA_ENV_PORT:-8002}"          # Environment: Management SDK connects here

# PID file locations
PID_DIR="${DATA_DIR}/pids"
PID_FILE="${PID_DIR}/runtara.pid"

# Log file locations
LOG_DIR="${DATA_DIR}/logs"
LOG_FILE="${LOG_DIR}/runtara.log"

print_header() {
    echo -e "${BLUE}"
    echo "╔════════════════════════════════════════════════════════════╗"
    echo "║               Runtara Development Launcher                 ║"
    echo "╚════════════════════════════════════════════════════════════╝"
    echo -e "${NC}"
}

print_status() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_dependencies() {
    print_status "Checking dependencies..."

    if ! command -v cargo &> /dev/null; then
        print_error "cargo not found. Please install Rust."
        exit 1
    fi

    if ! command -v psql &> /dev/null; then
        print_warning "psql not found. Database connectivity cannot be verified."
    fi
}

setup_directories() {
    print_status "Setting up directories..."
    mkdir -p "${PID_DIR}"
    mkdir -p "${LOG_DIR}"
}

check_database() {
    print_status "Checking database connections..."
    if ! command -v psql &> /dev/null; then
        return
    fi
    for url in "${SERVER_DATABASE_URL}" "${DATABASE_URL}" "${OBJECT_MODEL_DATABASE_URL}"; do
        if psql "${url}" -c "SELECT 1" &> /dev/null; then
            print_status "Database connection OK: ${url}"
        else
            print_warning "Cannot connect to database at ${url}"
            print_warning "Make sure PostgreSQL is running and the database exists"
            echo ""
            echo "  To create the databases:"
            echo "    createdb runtara_server && createdb runtara && createdb runtara_objects"
            echo ""
        fi
    done
}

build_services() {
    print_status "Building runtara-server..."
    cargo build -p runtara-server --release 2>&1 | tail -5
    print_status "Build complete"
}

stop_services() {
    print_status "Stopping existing services..."

    if [ -f "${PID_FILE}" ]; then
        PID=$(cat "${PID_FILE}")
        if kill -0 "$PID" 2>/dev/null; then
            print_status "Stopping runtara (PID: $PID)"
            kill "$PID" 2>/dev/null || true
            sleep 1
        fi
        rm -f "${PID_FILE}"
    fi

    # Also clean up old PID files from previous start.sh version
    rm -f "${PID_DIR}/core.pid" "${PID_DIR}/environment.pid" 2>/dev/null || true
}

start_server() {
    print_status "Starting runtara-server..."
    print_status "  API + UI port:    ${SERVER_PORT}"
    print_status "  Environment port: ${ENV_PORT} (Management SDK)"
    print_status "  Core port:        ${CORE_PORT} (Instance SDK)"

    TENANT_ID="${TENANT_ID}" \
    AUTH_PROVIDER="${AUTH_PROVIDER:-local}" \
    SERVER_PORT="${SERVER_PORT}" \
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DATABASE_URL}" \
    RUNTARA_DATABASE_URL="${DATABASE_URL}" \
    OBJECT_MODEL_DATABASE_URL="${OBJECT_MODEL_DATABASE_URL}" \
    VALKEY_HOST="${VALKEY_HOST}" \
    VALKEY_PORT="${VALKEY_PORT}" \
    RUNTARA_ENV_HTTP_PORT="${ENV_PORT}" \
    RUNTARA_CORE_HTTP_PORT="${CORE_PORT}" \
    DATA_DIR="${DATA_DIR}" \
    RUST_LOG="${RUST_LOG:-runtara_server=info,runtara_environment=info,runtara_core=info}" \
        cargo run -p runtara-server --release > "${LOG_FILE}" 2>&1 &

    PID=$!
    echo $PID > "${PID_FILE}"
    print_status "Runtara started (PID: $PID)"
    print_status "  Log file: ${LOG_FILE}"

    # Wait a moment for server to initialize
    sleep 2

    if ! kill -0 "$PID" 2>/dev/null; then
        print_error "Runtara failed to start. Check ${LOG_FILE}"
        tail -20 "${LOG_FILE}"
        exit 1
    fi
}

show_status() {
    echo ""
    echo -e "${GREEN}════════════════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}  Runtara started successfully!${NC}"
    echo -e "${GREEN}════════════════════════════════════════════════════════════${NC}"
    echo ""
    echo "  Endpoints:"
    echo "    - API + UI:                     http://127.0.0.1:${SERVER_PORT}"
    echo "    - Environment (Management SDK): 127.0.0.1:${ENV_PORT}"
    echo "    - Core (Instance SDK):          127.0.0.1:${CORE_PORT}"
    echo ""
    echo "  Environment Variables for Management SDK:"
    echo "    export RUNTARA_ENVIRONMENT_ADDR=127.0.0.1:${ENV_PORT}"
    echo "    export RUNTARA_SKIP_CERT_VERIFICATION=true"
    echo ""
    echo "  Logs:"
    echo "    tail -f ${LOG_FILE}"
    echo ""
    echo "  To stop:"
    echo "    ./start.sh stop"
    echo ""
}

show_logs() {
    echo "Showing logs (Ctrl+C to exit)..."
    echo ""
    tail -f "${LOG_FILE}"
}

usage() {
    echo "Usage: $0 [command]"
    echo ""
    echo "Commands:"
    echo "  start     Start runtara server (default)"
    echo "  stop      Stop the server"
    echo "  restart   Restart the server"
    echo "  status    Show server status"
    echo "  logs      Follow log output"
    echo "  build     Build only"
    echo "  help      Show this help message"
    echo ""
    echo "Environment Variables:"
    echo "  TENANT_ID                    Tenant identifier (default: local)"
    echo "  AUTH_PROVIDER                Auth mode (default: local)"
    echo "  SERVER_PORT                  API + UI port (default: 7001)"
    echo "  RUNTARA_SERVER_DATABASE_URL  Server database (default: postgres://localhost/runtara_server)"
    echo "  RUNTARA_DATABASE_URL         Durable-runtime database (default: postgres://localhost/runtara)"
    echo "  OBJECT_MODEL_DATABASE_URL    Object model database (default: postgres://localhost/runtara_objects)"
    echo "  VALKEY_HOST / VALKEY_PORT    Valkey for checkpoint storage (default: 127.0.0.1:6379)"
    echo "  DATA_DIR                     Data directory (default: .data)"
    echo "  RUNTARA_CORE_HTTP_PORT       Core instance API port (default: 8003)"
    echo "  RUNTARA_ENV_PORT             Environment HTTP port (default: 8002)"
    echo "  RUST_LOG                     Log level (default: runtara_*=info)"
    echo ""
}

status_command() {
    echo "Service Status:"
    echo ""

    if [ -f "${PID_FILE}" ]; then
        PID=$(cat "${PID_FILE}")
        if kill -0 "$PID" 2>/dev/null; then
            echo -e "  runtara: ${GREEN}Running${NC} (PID: $PID)"
        else
            echo -e "  runtara: ${RED}Stopped${NC} (stale PID file)"
        fi
    else
        echo -e "  runtara: ${YELLOW}Not running${NC}"
    fi
    echo ""
}

# Main script
case "${1:-start}" in
    start)
        print_header
        check_dependencies
        setup_directories
        check_database
        stop_services
        build_services
        start_server
        show_status
        ;;
    stop)
        print_header
        setup_directories
        stop_services
        print_status "Runtara stopped"
        ;;
    restart)
        print_header
        check_dependencies
        setup_directories
        check_database
        stop_services
        build_services
        start_server
        show_status
        ;;
    status)
        print_header
        setup_directories
        status_command
        ;;
    logs)
        setup_directories
        show_logs
        ;;
    build)
        print_header
        check_dependencies
        build_services
        ;;
    help|--help|-h)
        usage
        ;;
    *)
        print_error "Unknown command: $1"
        usage
        exit 1
        ;;
esac
