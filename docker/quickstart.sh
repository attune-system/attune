#!/bin/bash
# quickstart.sh - Quick start script for Attune Docker deployment
# This script helps you get Attune up and running with Docker in minutes

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Helper functions
print_header() {
    echo -e "\n${BLUE}=================================================="
    echo -e "$1"
    echo -e "==================================================${NC}\n"
}

print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠ $1${NC}"
}

print_error() {
    echo -e "${RED}✗ $1${NC}"
}

print_info() {
    echo -e "${BLUE}ℹ $1${NC}"
}

check_command() {
    if ! command -v $1 &> /dev/null; then
        print_error "$1 is not installed"
        return 1
    else
        print_success "$1 is installed"
        return 0
    fi
}

# Main script
print_header "Attune Docker Quick Start"

# Change to project root
cd "$(dirname "$0")/.."

# Check prerequisites
print_info "Checking prerequisites..."

if ! check_command docker; then
    print_error "Docker is required. Install from: https://docs.docker.com/get-docker/"
    exit 1
fi

if ! check_command docker-compose && ! docker compose version &> /dev/null; then
    print_error "Docker Compose is required. Install from: https://docs.docker.com/compose/install/"
    exit 1
fi

# Detect docker-compose command
if command -v docker-compose &> /dev/null; then
    DOCKER_COMPOSE="docker-compose"
else
    DOCKER_COMPOSE="docker compose"
fi

# Check if Docker daemon is running
if ! docker info &> /dev/null; then
    print_error "Docker daemon is not running. Please start Docker and try again."
    exit 1
fi

print_success "All prerequisites met"

# Enable BuildKit for faster incremental builds
print_info "Enabling Docker BuildKit for faster builds..."
export DOCKER_BUILDKIT=1
export COMPOSE_DOCKER_CLI_BUILD=1
print_success "BuildKit enabled for this session"

# Check for .env file
print_info "Checking configuration..."

if [ ! -f .env ]; then
    print_warning ".env file not found. Creating from template..."

    if [ -f env.docker.example ]; then
        cp env.docker.example .env
        print_success "Created .env file from env.docker.example"

        # Generate secure secrets
        print_info "Generating secure secrets..."

        JWT_SECRET=$(openssl rand -base64 32 2>/dev/null || head -c 32 /dev/urandom | base64)
        ENCRYPTION_KEY=$(openssl rand -base64 32 2>/dev/null || head -c 32 /dev/urandom | base64)

        # Update .env file with generated secrets
        if [[ "$OSTYPE" == "darwin"* ]]; then
            # macOS
            sed -i '' "s/JWT_SECRET=.*/JWT_SECRET=${JWT_SECRET}/" .env
            sed -i '' "s/ENCRYPTION_KEY=.*/ENCRYPTION_KEY=${ENCRYPTION_KEY}/" .env
        else
            # Linux
            sed -i "s/JWT_SECRET=.*/JWT_SECRET=${JWT_SECRET}/" .env
            sed -i "s/ENCRYPTION_KEY=.*/ENCRYPTION_KEY=${ENCRYPTION_KEY}/" .env
        fi

        print_success "Generated and saved secure secrets"
    else
        print_error "env.docker.example not found. Please create .env file manually."
        exit 1
    fi
else
    print_success ".env file exists"

    # Check if secrets are still default values
    if grep -q "docker-dev-secret-change-in-production" .env || grep -q "docker-dev-encryption-key-32ch" .env; then
        print_warning "Default secrets detected in .env file"

        read -p "Would you like to generate new secure secrets? (y/n) " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            JWT_SECRET=$(openssl rand -base64 32 2>/dev/null || head -c 32 /dev/urandom | base64)
            ENCRYPTION_KEY=$(openssl rand -base64 32 2>/dev/null || head -c 32 /dev/urandom | base64)

            if [[ "$OSTYPE" == "darwin"* ]]; then
                sed -i '' "s/JWT_SECRET=.*/JWT_SECRET=${JWT_SECRET}/" .env
                sed -i '' "s/ENCRYPTION_KEY=.*/ENCRYPTION_KEY=${ENCRYPTION_KEY}/" .env
            else
                sed -i "s/JWT_SECRET=.*/JWT_SECRET=${JWT_SECRET}/" .env
                sed -i "s/ENCRYPTION_KEY=.*/ENCRYPTION_KEY=${ENCRYPTION_KEY}/" .env
            fi

            print_success "Generated and saved secure secrets"
        fi
    fi
fi

chmod 600 .env

# Pull or build images
print_header "Building Docker Images"
print_info "This may take 5-6 minutes on first run with BuildKit..."
print_info "Subsequent builds will be much faster (~30-60 seconds)"

if ! $DOCKER_COMPOSE build; then
    print_error "Failed to build Docker images"
    exit 1
fi

print_success "Docker images built successfully"

# Start services
print_header "Starting Services"

if ! $DOCKER_COMPOSE up -d; then
    print_error "Failed to start services"
    exit 1
fi

print_success "Services started"

# Wait for services to be healthy
print_info "Waiting for services to be healthy..."
sleep 5

MAX_WAIT=120
WAITED=0
ALL_HEALTHY=false

while [ $WAITED -lt $MAX_WAIT ]; do
    UNHEALTHY=$($DOCKER_COMPOSE ps --format json 2>/dev/null | grep -c '"Health":"unhealthy"' || true)
    STARTING=$($DOCKER_COMPOSE ps --format json 2>/dev/null | grep -c '"Health":"starting"' || true)

    if [ "$UNHEALTHY" -eq 0 ] && [ "$STARTING" -eq 0 ]; then
        ALL_HEALTHY=true
        break
    fi

    echo -n "."
    sleep 5
    WAITED=$((WAITED + 5))
done

echo ""

if [ "$ALL_HEALTHY" = true ]; then
    print_success "All services are healthy"
else
    print_warning "Some services may not be fully ready yet"
    print_info "Check status with: $DOCKER_COMPOSE ps"
fi

# Display service status
print_header "Service Status"
$DOCKER_COMPOSE ps

# Display access information
print_header "Access Information"

echo -e "${GREEN}Attune is now running!${NC}\n"

print_info "Web UI:              http://localhost:3000"
print_info "API:                 http://localhost:8080"
print_info "API Documentation:   http://localhost:8080/api-spec/swagger-ui/"
print_info "RabbitMQ Management: http://localhost:15672"
print_info "                     (username: attune, password: attune)"

echo ""
print_info "View logs:           $DOCKER_COMPOSE logs -f"
print_info "Stop services:       $DOCKER_COMPOSE down"
print_info "View status:         $DOCKER_COMPOSE ps"

# Check if we need to run migrations
print_header "Database Setup"
print_info "Checking database migrations..."

if $DOCKER_COMPOSE exec -T api sh -c "sqlx migrate info" &> /dev/null; then
    print_success "Database migrations are up to date"
else
    print_warning "Database migrations may need to be run manually"
    print_info "Run: $DOCKER_COMPOSE exec api sqlx migrate run"
fi

# Offer to create admin user
print_header "Admin User Setup"

read -p "Would you like to create an admin user now? (y/n) " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    echo ""
    read -p "Enter admin username: " ADMIN_USER
    read -p "Enter admin email: " ADMIN_EMAIL
    read -s -p "Enter admin password: " ADMIN_PASSWORD
    echo ""

    # Note: This requires the CLI or API endpoint to be available
    print_info "Creating admin user..."
    print_warning "Manual user creation may be required via API or database"
    print_info "Use the API at http://localhost:8080/auth/register or the CLI tool"
fi

# Final message
print_header "Setup Complete!"

echo -e "${GREEN}Attune is ready to use!${NC}\n"
print_info "Next steps:"
echo "  1. Open http://localhost:3000 in your browser"
echo "  2. Create a user account"
echo "  3. Explore the documentation: http://localhost:8080/api-spec/swagger-ui/"
echo ""
print_info "For help:"
echo "  - View logs: $DOCKER_COMPOSE logs -f [service]"
echo "  - Documentation: docs/docker-deployment.md"
echo "  - Troubleshooting: docker/README.md"
echo ""
print_info "BuildKit is enabled for this session"
echo "  To enable globally, run: ./docker/enable-buildkit.sh"
echo ""

print_success "Happy automating!"
