#!/bin/bash
# Runtara Development Launcher
# Starts runtara-environment with embedded runtara-core

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
DATABASE_URL="${RUNTARA_DATABASE_URL:-postgres://localhost/runtara}"

# Port configuration
CORE_PORT="${RUNTARA_QUIC_PORT:-8001}"        # Core: instances connect here (SDK)
ENV_PORT="${RUNTARA_ENV_QUIC_PORT:-8002}"     # Environment: Management SDK connects here

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
    mkdir -p "${DATA_DIR}/bundles"
}

check_database() {
    print_status "Checking database connection..."
    if command -v psql &> /dev/null; then
        if psql "${DATABASE_URL}" -c "SELECT 1" &> /dev/null; then
            print_status "Database connection OK"
        else
            print_warning "Cannot connect to database at ${DATABASE_URL}"
            print_warning "Make sure PostgreSQL is running and the database exists"
            echo ""
            echo "  To create the database:"
            echo "    createdb runtara"
            echo ""
        fi
    fi
}

build_services() {
    print_status "Building runtara-environment (with embedded core)..."
    cargo build -p runtara-environment --release 2>&1 | tail -5
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
    print_status "Starting runtara-environment with embedded core..."
    print_status "  Environment port: ${ENV_PORT} (Management SDK)"
    print_status "  Core port: ${CORE_PORT} (Instance SDK)"

    RUNTARA_DATABASE_URL="${DATABASE_URL}" \
    RUNTARA_ENV_QUIC_PORT="${ENV_PORT}" \
    RUNTARA_CORE_ADDR="127.0.0.1:${CORE_PORT}" \
    DATA_DIR="${DATA_DIR}" \
    RUST_LOG="${RUST_LOG:-runtara_environment=info,runtara_core=info}" \
        cargo run -p runtara-environment --release > "${LOG_FILE}" 2>&1 &

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
    echo "    - Environment (Management SDK): 127.0.0.1:${ENV_PORT}"
    echo "    - Core (Instance SDK):          127.0.0.1:${CORE_PORT}"
    echo ""
    echo "  Environment Variables for Management SDK:"
    echo "    export RUNTARA_ENVIRONMENT_ADDR=127.0.0.1:${ENV_PORT}"
    echo "    export RUNTARA_SKIP_CERT_VERIFICATION=true"
    echo ""
    echo "  Environment Variables for Instance SDK:"
    echo "    export RUNTARA_SERVER_ADDR=127.0.0.1:${CORE_PORT}"
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
    echo "  RUNTARA_DATABASE_URL    PostgreSQL connection string (default: postgres://localhost/runtara)"
    echo "  DATA_DIR                Data directory (default: .data)"
    echo "  RUNTARA_QUIC_PORT       Core instance QUIC port (default: 8001)"
    echo "  RUNTARA_ENV_QUIC_PORT   Environment QUIC port (default: 8002)"
    echo "  RUST_LOG                Log level (default: runtara_*=info)"
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
