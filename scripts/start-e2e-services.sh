#!/bin/bash
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
CONFIG_FILE="${CONFIG_FILE:-config.e2e.yaml}"
LOG_DIR="./tests/logs"
PID_DIR="./tests/pids"

# Detect database schema from config file if possible
DETECTED_SCHEMA=""
if [ -f "$CONFIG_FILE" ]; then
    DETECTED_SCHEMA=$(grep -E '^\s*schema:' "$CONFIG_FILE" | sed -E 's/^\s*schema:\s*"?([^"]+)"?.*/\1/' | tr -d ' ')
fi

# Service ports (from config.e2e.yaml)
API_PORT=8080
NOTIFIER_WS_PORT=8081

echo -e "${GREEN}=== Attune E2E Services Startup ===${NC}\n"

# Display configuration info
echo -e "${BLUE}Configuration:${NC}"
echo -e "  • Config file: ${YELLOW}$CONFIG_FILE${NC}"
if [ -n "$DETECTED_SCHEMA" ]; then
    echo -e "  • Database schema: ${YELLOW}$DETECTED_SCHEMA${NC}"
else
    echo -e "  • Database schema: ${YELLOW}attune${NC} (default)"
fi
echo ""

# Create necessary directories
mkdir -p "$LOG_DIR"
mkdir -p "$PID_DIR"
mkdir -p "./tests/artifacts"
mkdir -p "./tests/venvs"

# Function to check if a service is running
is_service_running() {
    local pid_file="$PID_DIR/$1.pid"
    if [ -f "$pid_file" ]; then
        local pid=$(cat "$pid_file")
        if ps -p $pid > /dev/null 2>&1; then
            return 0
        else
            rm -f "$pid_file"
            return 1
        fi
    fi
    return 1
}

# Function to stop a service
stop_service() {
    local service_name=$1
    local pid_file="$PID_DIR/$service_name.pid"

    if [ -f "$pid_file" ]; then
        local pid=$(cat "$pid_file")
        echo -e "${YELLOW}→${NC} Stopping $service_name (PID: $pid)..."
        kill $pid 2>/dev/null || true
        sleep 2
        if ps -p $pid > /dev/null 2>&1; then
            echo -e "${YELLOW}!${NC} Forcefully killing $service_name..."
            kill -9 $pid 2>/dev/null || true
        fi
        rm -f "$pid_file"
        echo -e "${GREEN}✓${NC} $service_name stopped"
    fi
}

# Function to start a service
start_service() {
    local service_name=$1
    local binary_name=$2
    local log_file="$LOG_DIR/$service_name.log"
    local pid_file="$PID_DIR/$service_name.pid"

    echo -e "${YELLOW}→${NC} Starting $service_name..."

    # Build the service if not already built
    if [ ! -f "./target/debug/$binary_name" ]; then
        echo -e "${BLUE}  Building $binary_name...${NC}"
        cargo build --bin $binary_name 2>&1 | tee "$LOG_DIR/$service_name-build.log"
    fi

    # Start the service
    # Only set ATTUNE__ENVIRONMENT if using default e2e config
    # Otherwise, let the config file determine the environment
    if [ "$CONFIG_FILE" = "config.e2e.yaml" ]; then
        ATTUNE__ENVIRONMENT=e2e ATTUNE_CONFIG="$CONFIG_FILE" ./target/debug/$binary_name > "$log_file" 2>&1 &
    else
        ATTUNE_CONFIG="$CONFIG_FILE" ./target/debug/$binary_name > "$log_file" 2>&1 &
    fi
    local pid=$!
    echo $pid > "$pid_file"

    # Wait a moment and check if it's still running
    sleep 2
    if ps -p $pid > /dev/null 2>&1; then
        echo -e "${GREEN}✓${NC} $service_name started (PID: $pid)"
        echo -e "   Log: ${BLUE}$log_file${NC}"
        return 0
    else
        echo -e "${RED}✗${NC} $service_name failed to start"
        echo -e "   Check log: ${RED}$log_file${NC}"
        tail -20 "$log_file"
        return 1
    fi
}

# Function to check service health
check_service_health() {
    local service_name=$1
    local health_url=$2
    local max_attempts=${3:-30}
    local attempt=0

    echo -e "${YELLOW}→${NC} Checking $service_name health..."

    while [ $attempt -lt $max_attempts ]; do
        if curl -s -f "$health_url" > /dev/null 2>&1; then
            echo -e "${GREEN}✓${NC} $service_name is healthy"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done

    echo -e "${RED}✗${NC} $service_name health check failed after $max_attempts attempts"
    return 1
}

# Check for existing services and stop them
echo -e "${YELLOW}Checking for running E2E services...${NC}"
for service in api executor worker sensor notifier; do
    if is_service_running $service; then
        stop_service $service
    fi
done
echo ""

# Check dependencies
echo -e "${YELLOW}Checking dependencies...${NC}"

# Check PostgreSQL
echo -e "${YELLOW}→${NC} Checking PostgreSQL..."
echo -e "   Attempting connection to: ${BLUE}postgresql://localhost:5432/attune_e2e${NC}"

PGPASSWORD=postgres psql -h localhost -p 5432 -U postgres -d attune_e2e -c '\q' 2>/tmp/pg_check_error.txt
PG_EXIT=$?

if [ $PG_EXIT -eq 0 ]; then
    echo -e "${GREEN}✓${NC} PostgreSQL connection successful"
else
    echo -e "${RED}✗${NC} Cannot connect to PostgreSQL database 'attune_e2e'"
    echo ""
    echo -e "${YELLOW}Diagnostic Information:${NC}"

    # Check if PostgreSQL is running at all
    if ! pg_isready -h localhost -p 5432 > /dev/null 2>&1; then
        echo -e "  ${RED}✗${NC} PostgreSQL server is not running on localhost:5432"
        echo -e "    Start it with: ${YELLOW}sudo systemctl start postgresql${NC}"
    else
        echo -e "  ${GREEN}✓${NC} PostgreSQL server is running"

        # Check if the database exists
        if ! PGPASSWORD=postgres psql -h localhost -p 5432 -U postgres -lqt 2>/dev/null | cut -d \| -f 1 | grep -qw attune_e2e; then
            echo -e "  ${RED}✗${NC} Database 'attune_e2e' does not exist"
            echo -e "    Create it with: ${YELLOW}./scripts/setup-e2e-db.sh${NC}"
        else
            echo -e "  ${GREEN}✓${NC} Database 'attune_e2e' exists"
            echo -e "  ${RED}✗${NC} Connection failed for another reason"

            # Show the actual error
            if [ -f /tmp/pg_check_error.txt ] && [ -s /tmp/pg_check_error.txt ]; then
                echo -e "\n${YELLOW}Error details:${NC}"
                cat /tmp/pg_check_error.txt | sed 's/^/    /'
            fi
        fi
    fi

    rm -f /tmp/pg_check_error.txt
    exit 1
fi

# Check RabbitMQ
echo -e "${YELLOW}→${NC} Checking RabbitMQ..."
echo -e "   Testing connection to: ${BLUE}localhost:5672 (AMQP)${NC}"

if nc -z localhost 5672 2>/dev/null || timeout 1 bash -c 'cat < /dev/null > /dev/tcp/localhost/5672' 2>/dev/null; then
    echo -e "${GREEN}✓${NC} RabbitMQ is running (AMQP port 5672)"
else
    echo -e "${RED}✗${NC} RabbitMQ is not running on port 5672"
    echo ""
    echo -e "${YELLOW}Diagnostic Information:${NC}"

    # Check if RabbitMQ process is running
    if pgrep -x rabbitmq-server > /dev/null || pgrep -x beam.smp > /dev/null; then
        echo -e "  ${YELLOW}!${NC} RabbitMQ process is running but port 5672 is not accessible"
        echo -e "    The service may still be starting up"
        echo -e "    Check status: ${YELLOW}sudo rabbitmq-diagnostics status${NC}"
        echo -e "    View logs: ${YELLOW}sudo journalctl -u rabbitmq-server -n 50${NC}"
    else
        echo -e "  ${RED}✗${NC} RabbitMQ process is not running"
        echo -e "    Start it with: ${YELLOW}sudo systemctl start rabbitmq-server${NC}"
        echo -e "    Or: ${YELLOW}sudo service rabbitmq-server start${NC}"
    fi

    # Check if port might be in use by something else
    if netstat -tuln 2>/dev/null | grep -q :5672 || ss -tuln 2>/dev/null | grep -q :5672; then
        echo -e "  ${YELLOW}!${NC} Port 5672 appears to be in use"
        echo -e "    Check what's using it: ${YELLOW}sudo lsof -i :5672${NC}"
    fi

    exit 1
fi

echo ""

# Build all services first
echo -e "${YELLOW}Building all services...${NC}"
echo -e "   Building binaries: ${BLUE}api, executor, worker, sensor, notifier${NC}"
echo -e "   This may take a few moments..."

cargo build --bins 2>&1 | tee "$LOG_DIR/build.log"
BUILD_EXIT=${PIPESTATUS[0]}

if [ $BUILD_EXIT -ne 0 ]; then
    echo -e "${RED}✗${NC} Build failed"
    echo ""
    echo -e "${YELLOW}Last 30 lines of build output:${NC}"
    tail -30 "$LOG_DIR/build.log" | sed 's/^/  /'
    echo ""
    echo -e "Full build log: ${RED}$LOG_DIR/build.log${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} All services built successfully\n"

# Start services in order
echo -e "${GREEN}=== Starting Services ===${NC}\n"

# 1. Start API service
if ! start_service "api" "attune-api"; then
    echo -e "${RED}Failed to start API service${NC}"
    exit 1
fi
sleep 2

# Check API health
if ! check_service_health "API" "http://127.0.0.1:$API_PORT/health"; then
    echo -e "${RED}API service is not healthy${NC}"
    echo ""
    echo -e "${YELLOW}Last 20 lines of API log:${NC}"
    tail -20 "$LOG_DIR/api.log" | sed 's/^/  /'
    echo ""
    echo -e "Full log: ${RED}$LOG_DIR/api.log${NC}"
    exit 1
fi
echo ""

# 2. Start Executor service
if ! start_service "executor" "attune-executor"; then
    echo -e "${RED}Failed to start Executor service${NC}"
    echo ""
    echo -e "${YELLOW}Last 20 lines of Executor log:${NC}"
    tail -20 "$LOG_DIR/executor.log" | sed 's/^/  /'
    echo ""
    echo -e "Full log: ${RED}$LOG_DIR/executor.log${NC}"
    exit 1
fi
sleep 2
echo ""

# 3. Start Worker service
if ! start_service "worker" "attune-worker"; then
    echo -e "${RED}Failed to start Worker service${NC}"
    echo ""
    echo -e "${YELLOW}Last 20 lines of Worker log:${NC}"
    tail -20 "$LOG_DIR/worker.log" | sed 's/^/  /'
    echo ""
    echo -e "Full log: ${RED}$LOG_DIR/worker.log${NC}"
    exit 1
fi
sleep 2
echo ""

# 4. Start Sensor service
if ! start_service "sensor" "attune-sensor"; then
    echo -e "${RED}Failed to start Sensor service${NC}"
    echo ""
    echo -e "${YELLOW}Last 20 lines of Sensor log:${NC}"
    tail -20 "$LOG_DIR/sensor.log" | sed 's/^/  /'
    echo ""
    echo -e "Full log: ${RED}$LOG_DIR/sensor.log${NC}"
    exit 1
fi
sleep 2
echo ""

# 5. Start Notifier service
if ! start_service "notifier" "attune-notifier"; then
    echo -e "${RED}Failed to start Notifier service${NC}"
    echo ""
    echo -e "${YELLOW}Last 20 lines of Notifier log:${NC}"
    tail -20 "$LOG_DIR/notifier.log" | sed 's/^/  /'
    echo ""
    echo -e "Full log: ${RED}$LOG_DIR/notifier.log${NC}"
    exit 1
fi
sleep 2
echo ""

# Display running services
echo -e "${GREEN}=== All Services Started ===${NC}\n"
echo -e "${GREEN}Running Services:${NC}"
for service in api executor worker sensor notifier; do
    pid_file="$PID_DIR/$service.pid"
    if [ -f "$pid_file" ]; then
        pid=$(cat "$pid_file")
        echo -e "  • ${GREEN}$service${NC} (PID: $pid)"
    fi
done

echo -e "\n${GREEN}Service Endpoints:${NC}"
echo -e "  • API:       ${BLUE}http://127.0.0.1:$API_PORT${NC}"
echo -e "  • Health:    ${BLUE}http://127.0.0.1:$API_PORT/health${NC}"
echo -e "  • Docs:      ${BLUE}http://127.0.0.1:$API_PORT/docs${NC}"
echo -e "  • WebSocket: ${BLUE}ws://127.0.0.1:$NOTIFIER_WS_PORT${NC}"

echo -e "\n${GREEN}Logs:${NC}"
for service in api executor worker sensor notifier; do
    echo -e "  • $service: ${BLUE}$LOG_DIR/$service.log${NC}"
done

echo -e "\n${GREEN}Management:${NC}"
echo -e "  • View logs:  ${YELLOW}tail -f $LOG_DIR/<service>.log${NC}"
echo -e "  • Stop all:   ${YELLOW}./scripts/stop-e2e-services.sh${NC}"
echo -e "  • Run tests:  ${YELLOW}cargo test --test integration${NC}"

echo -e "\n${GREEN}Test User:${NC}"
echo -e "  • Login:    ${YELLOW}test@attune.local${NC}"
echo -e "  • Password: ${YELLOW}TestPass123!${NC}"

echo -e "\n${GREEN}Quick Test:${NC}"
echo -e "  ${YELLOW}curl http://127.0.0.1:$API_PORT/health"
echo -e "  ${YELLOW}curl -X POST http://127.0.0.1:$API_PORT/auth/login \\"
echo -e "  ${YELLOW}  -H 'Content-Type: application/json' \\"
echo -e "  ${YELLOW}  -d '{\"login\":\"test@attune.local\",\"password\":\"TestPass123!\"}'${NC}"

echo ""
