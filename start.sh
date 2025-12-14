#!/bin/bash
# Runtara Development Launcher
# Starts runtara-core and runtara-environment for local testing

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
CORE_DATABASE_URL="${RUNTARA_DATABASE_URL:-postgres://localhost/runtara}"
ENV_DATABASE_URL="${RUNTARA_ENVIRONMENT_DATABASE_URL:-postgres://localhost/runtara_environment}"

# Port configuration
CORE_INSTANCE_PORT="${RUNTARA_QUIC_PORT:-8001}"      # Instances connect here (SDK)
CORE_INTERNAL_PORT="${RUNTARA_ADMIN_PORT:-8003}"     # Environment connects here (internal)
ENV_PORT="${RUNTARA_ENV_QUIC_PORT:-8002}"            # Management SDK connects here

# PID file locations
PID_DIR="${DATA_DIR}/pids"
CORE_PID_FILE="${PID_DIR}/core.pid"
ENV_PID_FILE="${PID_DIR}/environment.pid"

# Log file locations
LOG_DIR="${DATA_DIR}/logs"
CORE_LOG_FILE="${LOG_DIR}/core.log"
ENV_LOG_FILE="${LOG_DIR}/environment.log"

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
    print_status "Checking database connections..."
    if command -v psql &> /dev/null; then
        # Check Core database
        if psql "${CORE_DATABASE_URL}" -c "SELECT 1" &> /dev/null; then
            print_status "Core database connection OK"
        else
            print_warning "Cannot connect to Core database at ${CORE_DATABASE_URL}"
            print_warning "Make sure PostgreSQL is running and the database exists"
            echo ""
            echo "  To create the database:"
            echo "    createdb runtara"
            echo ""
        fi
        # Check Environment database
        if psql "${ENV_DATABASE_URL}" -c "SELECT 1" &> /dev/null; then
            print_status "Environment database connection OK"
        else
            print_warning "Cannot connect to Environment database at ${ENV_DATABASE_URL}"
            print_warning "Make sure PostgreSQL is running and the database exists"
            echo ""
            echo "  To create the database:"
            echo "    createdb runtara_environment"
            echo ""
        fi
    fi
}

build_services() {
    print_status "Building runtara-core and runtara-environment..."
    cargo build -p runtara-core -p runtara-environment --release 2>&1 | tail -5
    print_status "Build complete"
}

stop_services() {
    print_status "Stopping existing services..."

    if [ -f "${CORE_PID_FILE}" ]; then
        PID=$(cat "${CORE_PID_FILE}")
        if kill -0 "$PID" 2>/dev/null; then
            print_status "Stopping runtara-core (PID: $PID)"
            kill "$PID" 2>/dev/null || true
            sleep 1
        fi
        rm -f "${CORE_PID_FILE}"
    fi

    if [ -f "${ENV_PID_FILE}" ]; then
        PID=$(cat "${ENV_PID_FILE}")
        if kill -0 "$PID" 2>/dev/null; then
            print_status "Stopping runtara-environment (PID: $PID)"
            kill "$PID" 2>/dev/null || true
            sleep 1
        fi
        rm -f "${ENV_PID_FILE}"
    fi
}

start_core() {
    print_status "Starting runtara-core..."
    print_status "  Instance port: ${CORE_INSTANCE_PORT} (SDK connects here)"
    print_status "  Internal port: ${CORE_INTERNAL_PORT} (Environment connects here)"

    RUNTARA_DATABASE_URL="${CORE_DATABASE_URL}" \
    RUNTARA_QUIC_PORT="${CORE_INSTANCE_PORT}" \
    RUNTARA_ADMIN_PORT="${CORE_INTERNAL_PORT}" \
    RUST_LOG="${RUST_LOG:-runtara_core=info}" \
        cargo run -p runtara-core --release > "${CORE_LOG_FILE}" 2>&1 &

    CORE_PID=$!
    echo $CORE_PID > "${CORE_PID_FILE}"
    print_status "runtara-core started (PID: $CORE_PID)"
    print_status "  Log file: ${CORE_LOG_FILE}"

    # Wait a moment for core to initialize
    sleep 2

    if ! kill -0 "$CORE_PID" 2>/dev/null; then
        print_error "runtara-core failed to start. Check ${CORE_LOG_FILE}"
        tail -20 "${CORE_LOG_FILE}"
        exit 1
    fi
}

start_environment() {
    print_status "Starting runtara-environment..."
    print_status "  Management port: ${ENV_PORT} (Management SDK connects here)"
    print_status "  Core instance port: 127.0.0.1:${CORE_INSTANCE_PORT} (passed to instances)"

    RUNTARA_ENVIRONMENT_DATABASE_URL="${ENV_DATABASE_URL}" \
    RUNTARA_ENV_QUIC_PORT="${ENV_PORT}" \
    RUNTARA_CORE_ADDR="127.0.0.1:${CORE_INSTANCE_PORT}" \
    DATA_DIR="${DATA_DIR}" \
    RUST_LOG="${RUST_LOG:-runtara_environment=info}" \
        cargo run -p runtara-environment --release > "${ENV_LOG_FILE}" 2>&1 &

    ENV_PID=$!
    echo $ENV_PID > "${ENV_PID_FILE}"
    print_status "runtara-environment started (PID: $ENV_PID)"
    print_status "  Log file: ${ENV_LOG_FILE}"

    # Wait a moment for environment to initialize
    sleep 2

    if ! kill -0 "$ENV_PID" 2>/dev/null; then
        print_error "runtara-environment failed to start. Check ${ENV_LOG_FILE}"
        tail -20 "${ENV_LOG_FILE}"
        exit 1
    fi
}

show_status() {
    echo ""
    echo -e "${GREEN}════════════════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}  Services started successfully!${NC}"
    echo -e "${GREEN}════════════════════════════════════════════════════════════${NC}"
    echo ""
    echo "  Endpoints:"
    echo "    - Environment (Management SDK): 127.0.0.1:${ENV_PORT}"
    echo "    - Core (Instance SDK):          127.0.0.1:${CORE_INSTANCE_PORT}"
    echo ""
    echo "  Environment Variables for Management SDK:"
    echo "    export RUNTARA_ENVIRONMENT_ADDR=127.0.0.1:${ENV_PORT}"
    echo "    export RUNTARA_SKIP_CERT_VERIFICATION=true"
    echo ""
    echo "  Environment Variables for Instance SDK:"
    echo "    export RUNTARA_SERVER_ADDR=127.0.0.1:${CORE_INSTANCE_PORT}"
    echo "    export RUNTARA_SKIP_CERT_VERIFICATION=true"
    echo ""
    echo "  Logs:"
    echo "    tail -f ${CORE_LOG_FILE}"
    echo "    tail -f ${ENV_LOG_FILE}"
    echo ""
    echo "  To stop services:"
    echo "    ./start.sh stop"
    echo ""
}

show_logs() {
    echo "Showing logs (Ctrl+C to exit)..."
    echo ""
    tail -f "${CORE_LOG_FILE}" "${ENV_LOG_FILE}"
}

usage() {
    echo "Usage: $0 [command]"
    echo ""
    echo "Commands:"
    echo "  start     Start runtara-core and runtara-environment (default)"
    echo "  stop      Stop all services"
    echo "  restart   Restart all services"
    echo "  status    Show service status"
    echo "  logs      Follow log output"
    echo "  build     Build services only"
    echo "  help      Show this help message"
    echo ""
    echo "Environment Variables:"
    echo "  RUNTARA_DATABASE_URL    PostgreSQL connection string (default: postgres://localhost/runtara)"
    echo "  DATA_DIR                Data directory (default: .data)"
    echo "  RUNTARA_QUIC_PORT       Core instance QUIC port (default: 8001)"
    echo "  RUNTARA_ADMIN_PORT      Core management port (default: 8003)"
    echo "  RUNTARA_ENV_QUIC_PORT   Environment QUIC port (default: 8002)"
    echo "  RUST_LOG                Log level (default: runtara_*=info)"
    echo ""
}

status_command() {
    echo "Service Status:"
    echo ""

    if [ -f "${CORE_PID_FILE}" ]; then
        PID=$(cat "${CORE_PID_FILE}")
        if kill -0 "$PID" 2>/dev/null; then
            echo -e "  runtara-core:        ${GREEN}Running${NC} (PID: $PID)"
        else
            echo -e "  runtara-core:        ${RED}Stopped${NC} (stale PID file)"
        fi
    else
        echo -e "  runtara-core:        ${YELLOW}Not running${NC}"
    fi

    if [ -f "${ENV_PID_FILE}" ]; then
        PID=$(cat "${ENV_PID_FILE}")
        if kill -0 "$PID" 2>/dev/null; then
            echo -e "  runtara-environment: ${GREEN}Running${NC} (PID: $PID)"
        else
            echo -e "  runtara-environment: ${RED}Stopped${NC} (stale PID file)"
        fi
    else
        echo -e "  runtara-environment: ${YELLOW}Not running${NC}"
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
        start_core
        start_environment
        show_status
        ;;
    stop)
        print_header
        setup_directories
        stop_services
        print_status "All services stopped"
        ;;
    restart)
        print_header
        check_dependencies
        setup_directories
        check_database
        stop_services
        build_services
        start_core
        start_environment
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
