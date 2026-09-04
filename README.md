# Attune

An event-driven automation and orchestration platform built in Rust.

## Documentation

The maintained Attune documentation is at [docs.attunedev.org](https://docs.attunedev.org/).

## Overview

Attune is a comprehensive automation platform similar to StackStorm or Apache Airflow, designed for building event-driven workflows with built-in multi-tenancy, RBAC (Role-Based Access Control), and human-in-the-loop capabilities.

### Key Features

- **Event-Driven Architecture**: Sensors monitor for triggers, which fire events that activate rules
- **Flexible Automation**: Pack-based system for organizing and distributing automation components
- **Workflow Orchestration**: Support for complex workflows with parent-child execution relationships
- **Human-in-the-Loop**: Inquiry system for async user interactions and approvals
- **Multi-Runtime Support**: Execute actions in different runtime environments (Python, Node.js, containers)
- **RBAC & Multi-Tenancy**: Comprehensive permission system with identity-based access control
- **Real-Time Notifications**: PostgreSQL-based pub/sub for real-time event streaming
- **Secure Secrets Management**: Encrypted key-value storage with ownership scoping
- **Execution Policies**: Rate limiting and concurrency control for action executions

## Architecture

Attune is built as a distributed system with multiple specialized services:

### Services

1. **API Service** (`attune-api`): REST API gateway for all client interactions
2. **Executor Service** (`attune-executor`): Manages action execution lifecycle and scheduling
3. **Worker Service** (`attune-worker`): Executes actions in various runtime environments
4. **Sensor Service** (`attune-sensor`): Monitors for trigger conditions and generates events
5. **Notifier Service** (`attune-notifier`): Handles real-time notifications and pub/sub

### Core Concepts

- **Pack**: A bundle of related automation components (actions, sensors, rules, triggers)
- **Trigger**: An event type that can activate rules (e.g., "webhook_received")
- **Sensor**: Monitors for trigger conditions and creates events
- **Event**: An instance of a trigger firing with payload data
- **Action**: An executable task (e.g., "send_email", "deploy_service")
- **Rule**: Connects triggers to actions with conditional logic
- **Execution**: A single action run, supports nested workflows
- **Inquiry**: Async user interaction within a workflow (approvals, input requests)

## Project Structure

```
attune/
├── Cargo.toml              # Workspace root configuration
├── crates/
│   ├── common/             # Shared library
│   │   ├── src/
│   │   │   ├── config.rs   # Configuration management
│   │   │   ├── db.rs       # Database connection pooling
│   │   │   ├── error.rs    # Error types
│   │   │   ├── models.rs   # Data models
│   │   │   ├── schema.rs   # Schema utilities
│   │   │   └── utils.rs    # Common utilities
│   │   └── Cargo.toml
│   ├── api/                # API service
│   ├── executor/           # Execution service
│   ├── worker/             # Worker service
│   ├── sensor/             # Sensor service
│   ├── notifier/           # Notification service
│   └── cli/                # CLI tool
└── reference/
    ├── models.py           # Python SQLAlchemy models (reference)
    └── models.md           # Data model documentation
```

## Prerequisites

### Local Development
- **Rust**: 1.75 or later
- **PostgreSQL**: 14 or later
- **RabbitMQ**: 3.12 or later (for message queue)
- **Redis**: 7.0 or later (optional, for caching)

### Docker Deployment (Recommended)
- **Docker**: 20.10 or later
- **Docker Compose**: 2.0 or later

## Getting Started

### Option 1: Docker (Recommended)

The fastest way to get Attune running is with Docker:

```bash
# Clone the repository
git clone https://github.com/yourusername/attune.git
cd attune

# Run the quick start script
./docker/quickstart.sh
```

This will:
- Generate secure secrets
- Build all Docker images
- Start all services (API, Executor, Worker, Sensor, Notifier, Web UI)
- Start infrastructure (PostgreSQL, RabbitMQ, Redis)
- Set up the database with migrations

Access the application:
- **Web UI**: http://localhost:3000
- **API**: http://localhost:8080
- **API Docs**: http://localhost:8080/api-spec/swagger-ui/

For more details, see [Docker Deployment Guide](docs/docker-deployment.md).

### Option 2: Local Development Setup

#### 1. Clone the Repository

```bash
git clone https://github.com/yourusername/attune.git
cd attune
```

#### 2. Set Up Database

```bash
# Create PostgreSQL database
createdb attune

# Run migrations
sqlx migrate run
```

For an existing v0.2.1-or-earlier SQLx database, use `attune-api --migrate` once
instead. Its embedded runner bridges legacy migration checksums before SQLx
validation; the standalone SQLx CLI cannot perform that upgrade bridge.

#### 3. Load the Core Pack

The core pack provides essential built-in automation components (timers, HTTP actions, etc.):

```bash
# Install Python dependencies for the loader
pip install psycopg2-binary pyyaml

# Load the core pack into the database
./scripts/load-core-pack.sh

# Or use the Python script directly
python3 scripts/load_core_pack.py
```

**Verify the core pack is loaded:**
```bash
# Using CLI (after starting API)
attune pack show core

# Using database
psql attune -c "SELECT * FROM attune.pack WHERE ref = 'core';"
```

See [Core Pack Setup Guide](packs/core/SETUP.md) for detailed instructions.

### 4. Configure Application

Create a configuration file from the example:

```bash
cp config.example.yaml config.yaml
```

Edit `config.yaml` with your settings:

```yaml
# Attune Configuration
service_name: attune
environment: development

database:
  url: postgresql://postgres:postgres@localhost:5432/attune

server:
  host: 0.0.0.0
  port: 8080
  cors_origins:
    - http://localhost:3000
    - http://localhost:5173

security:
  jwt_secret: your-secret-key-change-this
  jwt_access_expiration: 3600
  encryption_key: your-32-char-encryption-key-here

log:
  level: info
  format: json
```

**Generate secure secrets:**
```bash
# JWT secret
openssl rand -base64 64

# Encryption key
openssl rand -base64 32
```

### 5. Build All Services

```bash
cargo build --release
```

### 6. Run Services

Each service can be run independently:

```bash
# API Service
cargo run --bin attune-api --release

# Executor Service
cargo run --bin attune-executor --release

# Worker Service
cargo run --bin attune-worker --release

# Sensor Service
cargo run --bin attune-sensor --release

# Notifier Service
cargo run --bin attune-notifier --release
```

### 7. Using the CLI

Install and use the Attune CLI to interact with the API:

```bash
# Build and install CLI
cargo install --path crates/cli

# Login to API
attune auth login --username admin

# List packs
attune pack list

# List packs as JSON (shorthand)
attune pack list -j

# Execute an action
attune action execute core.echo --param message="Hello World"

# Monitor executions
attune execution list

# Get raw execution result for piping
attune execution result 123 | jq '.data'
```

See [CLI Documentation](crates/cli/README.md) for comprehensive usage guide.

## Development

### Web UI Development (Quick Start)

For rapid frontend development with hot-module reloading:

```bash
# Terminal 1: Start backend services in Docker
docker compose up -d postgres rabbitmq redis api executor worker-shell sensor

# Terminal 2: Start Vite dev server
cd web
npm install  # First time only
npm run dev

# Browser: Open http://localhost:3001
```

The Vite dev server provides:
- ⚡ **Instant hot-module reloading** - changes appear immediately
- 🚀 **Fast iteration** - no Docker rebuild needed for frontend changes
- 🔧 **Full API access** - properly configured CORS with backend services
- 🎯 **Source maps** - easy debugging

**Why port 3001?** The Docker web container uses port 3000. Vite automatically uses 3001 to avoid conflicts.

**Documentation:**
- **Quick Start**: [`docs/development/QUICKSTART-vite.md`](docs/development/QUICKSTART-vite.md)
- **Full Guide**: [`docs/development/vite-dev-setup.md`](docs/development/vite-dev-setup.md)

**Default test user:**
- Email: `test@attune.local`
- Password: `TestPass123!`

### Building

```bash
# Build all crates
cargo build

# Build specific service
cargo build -p attune-api

# Build with optimizations
cargo build --release
```

### Testing

```bash
# Run all tests
cargo test

# Run tests for specific crate
cargo test -p attune-common

# Run tests with output
cargo test -- --nocapture

# Run tests in parallel (recommended - uses schema-per-test isolation)
cargo test -- --test-threads=4
```

### SQLx Compile-Time Query Checking

Attune uses SQLx macros for type-safe database queries. These macros verify queries at compile time using cached metadata.

**Setup for Development:**

1. Copy the example environment file:
   ```bash
   cp .env.example .env
   ```

2. The `.env` file enables SQLx offline mode by default:
   ```bash
   SQLX_OFFLINE=true
   DATABASE_URL=postgresql://postgres:postgres@localhost:5432/attune?options=-c%20search_path%3Dattune%2Cpublic
   ```

**Regenerating Query Metadata:**

When you modify SQLx queries (in `query!`, `query_as!`, or `query_scalar!` macros), regenerate the cached metadata:

```bash
# Ensure database is running and up-to-date
sqlx database setup

# Regenerate offline query data
cargo sqlx prepare --workspace
```

This creates/updates `.sqlx/` directory with query metadata. **Commit these files to version control** so other developers and CI/CD can build without a database connection.

**Benefits of Offline Mode:**
- ✅ Fast compilation without database connection
- ✅ Works in CI/CD environments
- ✅ Type-safe queries verified at compile time
- ✅ Consistent query validation across all environments

### Code Quality

```bash
# Check code without building
cargo check

# Run linter
cargo clippy

# Format code
cargo fmt
```

## Configuration

Attune uses YAML configuration files with environment variable overrides.

### Configuration Loading Priority

1. **Base configuration file** (`config.yaml` or path from `ATTUNE_CONFIG` environment variable)
2. **Environment-specific file** (e.g., `config.development.yaml`, `config.production.yaml`)
3. **Environment variables** (prefix: `ATTUNE__`, separator: `__`)
   - Example: `ATTUNE__DATABASE__URL`, `ATTUNE__SERVER__PORT`

### Quick Setup

```bash
# Copy example configuration
cp config.example.yaml config.yaml

# Edit configuration
nano config.yaml

# Or use environment-specific config
cp config.example.yaml config.development.yaml
```

### Environment Variable Overrides

You can override any YAML setting with environment variables:

```bash
export ATTUNE__DATABASE__URL=postgresql://localhost/attune
export ATTUNE__SERVER__PORT=3000
export ATTUNE__LOG__LEVEL=debug
export ATTUNE__SECURITY__JWT_SECRET=$(openssl rand -base64 64)
```

### Configuration Structure

See [Configuration Guide](docs/configuration.md) for detailed documentation.

Main configuration sections:

- `database`: PostgreSQL connection settings
- `redis`: Redis connection (optional)
- `message_queue`: RabbitMQ settings
- `server`: HTTP server configuration
- `log`: Logging settings
- `security`: JWT and encryption settings
- `worker`: Worker-specific settings

## Data Models

See `reference/models.md` for comprehensive documentation of all data models.

Key models include:
- Pack, Runtime, Worker
- Trigger, Sensor, Event
- Action, Rule, Enforcement
- Execution, Inquiry
- Identity, PermissionSet
- Key (secrets), Notification

## CLI Tool

Attune includes a comprehensive command-line interface for interacting with the platform.

### Installation

```bash
cargo install --path crates/cli
```

### Quick Start

```bash
# Login
attune auth login --username admin

# Install a pack
attune pack install https://github.com/example/attune-pack-monitoring

# List actions
attune action list --pack monitoring

# Execute an action
attune action execute monitoring.check_health --param endpoint=https://api.example.com

# Monitor executions
attune execution list --limit 20

# Search executions
attune execution list --pack monitoring --status failed
attune execution list --result "error"

# Get raw execution result
attune execution result 123 | jq '.field'
```

### Features

- **Pack Management**: Install, list, and manage automation packs
- **Action Execution**: Run actions with parameters, wait for completion
- **Rule Management**: Create, enable, disable, and configure rules
- **Execution Monitoring**: View execution status, logs, and results with advanced filtering
- **Result Extraction**: Get raw execution results for piping to other tools
- **Multiple Output Formats**: Table (default), JSON (`-j`), and YAML (`-y`) output
- **Configuration Management**: Persistent config with token storage

See the [CLI README](crates/cli/README.md) for detailed documentation and examples.

## API Documentation

API documentation will be available at `/docs` when running the API service (OpenAPI/Swagger).

## Deployment

### Docker (Recommended)

**🚀 New to Docker deployment? Start here**: [Docker Quick Start Guide](docker/QUICK_START.md)

**Quick Setup**:

```bash
# Stop conflicting system services (if needed)
./scripts/stop-system-services.sh

# Start all services (migrations run automatically)
docker compose up -d

# Check status
docker compose ps

# Access Web UI
open http://localhost:3000
```

**Building Images** (only needed if you modify code):

```bash
# Pre-warm build cache (prevents race conditions)
make docker-cache-warm

# Build all services
make docker-build
```

**Documentation**:
- [Docker Quick Start Guide](docker/QUICK_START.md) - Get started in 5 minutes
- [Port Conflicts Resolution](docker/PORT_CONFLICTS.md) - Fix "address already in use" errors
- [Build Optimization Guide](docker/DOCKER_BUILD_RACE_CONDITIONS.md) - Build performance tips
- [Docker Configuration Reference](docker/README.md) - Complete Docker documentation

### Kubernetes

Kubernetes manifests are located in the `deploy/kubernetes/` directory.

```bash
kubectl apply -f deploy/kubernetes/
```

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

### Code Style

- Follow Rust standard conventions
- Use `cargo fmt` before committing
- Ensure `cargo clippy` passes without warnings
- Write tests for new functionality

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Acknowledgments

Inspired by:
- [StackStorm](https://stackstorm.com/) - Event-driven automation platform
- [Apache Airflow](https://airflow.apache.org/) - Workflow orchestration
- [Temporal](https://temporal.io/) - Durable execution

## Roadmap

### Phase 1: Core Infrastructure (Current)
- [x] Project structure and workspace setup
- [x] Common library with models and utilities
- [ ] Database migrations
- [ ] Service stubs and configuration

### Phase 2: Basic Services
- [ ] API service with REST endpoints
- [ ] Executor service for managing executions
- [ ] Worker service for running actions
- [ ] Basic pack management

### Phase 3: Event System
- [ ] Sensor service implementation
- [ ] Event generation and processing
- [ ] Rule evaluation engine
- [ ] Enforcement creation

### Phase 4: Advanced Features
- [ ] Inquiry system for human-in-the-loop
- [ ] Workflow orchestration (parent-child executions)
- [ ] Execution policies (rate limiting, concurrency)
- [ ] Real-time notifications

### Phase 5: Production Ready
- [ ] Comprehensive testing
- [ ] Performance optimization
- [ ] Documentation and examples
- [ ] Deployment tooling
- [ ] Monitoring and observability

## Support

For questions, issues, or contributions:
- Open an issue on GitHub
- Check the documentation in `reference/models.md`
- Review code examples in the `examples/` directory (coming soon)

## Status

**Current Status**: Early Development

The project structure and core models are in place. Service implementation is ongoing.
