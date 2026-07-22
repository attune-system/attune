.PHONY: help build test clean run-api run-executor run-worker run-sensor run-notifier \
        run-supervisor run-supervisor-release \
        check fmt clippy install-tools db-create db-migrate db-reset docker-build \
        docker-up docker-down docker-cache-warm docker-stop-system-services dev watch generate-agents-index \
        docker-build-workers docker-build-worker-base docker-build-worker-python \
        docker-build-worker-node docker-build-worker-full deny ci-rust ci-web-blocking ci-web-advisory \
        ci-security-blocking ci-security-advisory ci-blocking ci-advisory \
        fmt-check pre-commit install-git-hooks \
        build-agent docker-build-agent docker-build-agent-arm64 docker-build-agent-all \
        run-agent run-agent-release \
        docker-up-agent docker-down-agent \
        docker-build-pack-binaries docker-build-pack-binaries-arm64 docker-build-pack-binaries-all \
        docker-build-mcp docker-up-mcp docker-down-mcp \
        e2e-test e2e-test-debug e2e-test-tier1 e2e-test-tier2 e2e-test-tier3 e2e-test-standalone \
        e2e-test-cache-load

TEST_DB_ADMIN_URL ?= postgresql://attune:attune@localhost:5432/postgres
TEST_DB_URL ?= postgresql://attune:attune@localhost:5432/attune_test

# Default target
help:
	@echo "Attune Development Commands"
	@echo "==========================="
	@echo ""
	@echo "Building:"
	@echo "  make build          - Build all services"
	@echo "  make build-release  - Build all services in release mode"
	@echo "  make clean          - Clean build artifacts"
	@echo ""
	@echo "Testing:"
	@echo "  make test           - Run all tests"
	@echo "  make test-common    - Run tests for common library"
	@echo "  make test-api       - Run tests for API service"
	@echo "  make test-integration     - Run integration tests (common + API)"
	@echo "  make test-integration-api - Run API integration tests (requires DB)"
	@echo "  make e2e-test       - Run E2E tests (Docker Compose lifecycle)"
	@echo "  make e2e-test-debug - Run E2E tests, keep stack running"
	@echo "  make e2e-test-tier1 - Run E2E tier 1 tests only"
	@echo "  make e2e-test-cache-load - Run the opt-in 200,000-record cache load test"
	@echo "  make check          - Check code without building"
	@echo ""
	@echo "Code Quality:"
	@echo "  make fmt            - Format all code"
	@echo "  make fmt-check      - Verify formatting without changing files"
	@echo "  make clippy         - Run linter"
	@echo "  make lint           - Run both fmt and clippy"
	@echo "  make deny           - Run cargo-deny checks"
	@echo "  make pre-commit     - Run the git pre-commit checks locally"
	@echo "  make install-git-hooks - Configure git to use the repo hook scripts"
	@echo ""
	@echo "Running Services:"
	@echo "  make run-api        - Run API service"
	@echo "  make run-executor   - Run executor service"
	@echo "  make run-worker     - Run worker service"
	@echo "  make run-sensor     - Run sensor service"
	@echo "  make run-notifier   - Run notifier service"
	@echo "  make run-supervisor - Run supervisor service"
	@echo "  make dev            - Run all services in development mode"
	@echo ""
	@echo "Database:"
	@echo "  make db-create      - Create database"
	@echo "  make db-migrate     - Run migrations"
	@echo "  make db-reset       - Drop and recreate database"
	@echo "  make db-test-setup  - Setup test database"
	@echo "  make db-test-reset  - Reset test database"
	@echo ""
	@echo "Docker (Port conflicts? Run 'make docker-stop-system-services' first):"
	@echo "  make docker-stop-system-services - Stop system PostgreSQL/RabbitMQ/Redis"
	@echo "  make docker-cache-warm           - Pre-load build cache (prevents races)"
	@echo "  make docker-build                - Build Docker images"
	@echo "  make docker-build-workers        - Build all worker variants"
	@echo "  make docker-build-worker-base    - Build base worker (shell only)"
	@echo "  make docker-build-worker-python  - Build Python worker"
	@echo "  make docker-build-worker-node    - Build Node.js worker"
	@echo "  make docker-build-worker-full    - Build full worker (all runtimes)"
	@echo "  make docker-up                   - Start services with docker compose"
	@echo "  make docker-down                 - Stop services"
	@echo "  make docker-build-mcp            - Build MCP service image"
	@echo "  make docker-up-mcp               - Start optional MCP service profile"
	@echo "  make docker-down-mcp             - Stop optional MCP service profile"
	@echo ""
	@echo "Agent (Universal Worker):"
	@echo "  make build-agent              - Build statically-linked agent binary (musl)"
	@echo "  make docker-build-agent       - Build agent Docker image (native arch by default)"
	@echo "  make docker-build-agent-arm64 - Build agent Docker image (arm64)"
	@echo "  make docker-build-agent-all   - Build agent Docker images (amd64 + arm64)"
	@echo "  make run-agent                - Run agent in development mode"
	@echo "  make run-agent-release        - Run agent in release mode"
	@echo "  make docker-up-agent     - Start all services + agent workers (ruby, etc.)"
	@echo "  make docker-down-agent   - Stop agent stack"
	@echo ""
	@echo "Pack Binaries:"
	@echo "  make docker-build-pack-binaries       - Build pack binaries Docker image (native arch by default)"
	@echo "  make docker-build-pack-binaries-arm64  - Build pack binaries Docker image (arm64)"
	@echo "  make docker-build-pack-binaries-all    - Build pack binaries Docker images (amd64 + arm64)"
	@echo ""
	@echo "Development:"
	@echo "  make watch          - Watch and rebuild on changes"
	@echo "  make install-tools  - Install development tools"
	@echo ""
	@echo "Documentation:"
	@echo "  make generate-agents-index - Generate AGENTS.md index for AI agents"
	@echo ""

# Increase rustc stack size to prevent SIGSEGV during compilation
export RUST_MIN_STACK:=67108864

# Building
build:
	cargo build

build-release:
	cargo build --release

clean:
	cargo clean

# Testing
test:
	cargo test

test-common:
	cargo test -p attune-common

test-api:
	cargo test -p attune-api

test-verbose:
	cargo test -- --nocapture --test-threads=1

test-integration: db-test-setup test-integration-api test-integration-common
	@echo "Integration tests complete"

test-integration-api:
	@echo "Running API integration tests..."
	cargo test -p attune-api --test agent_tests -- --ignored --test-threads=1
	cargo test -p attune-api --test execution_token_permissions_e2e -- --ignored --test-threads=1
	cargo test -p attune-api --test inquiry_authz_tests -- --ignored --test-threads=1
	cargo test -p attune-api --test pack_registry_tests -- --ignored --test-threads=1
	cargo test -p attune-api --test pack_workflow_tests -- --ignored --test-threads=1
	cargo test -p attune-api --test permissions_api_tests -- --ignored --test-threads=1
	cargo test -p attune-api --test rbac_scoped_resources_api_tests -- --ignored --test-threads=1
	cargo test -p attune-api --test workflow_tests -- --ignored --test-threads=1
	@echo "API integration tests complete"

test-integration-common:
	@echo "Running common integration tests..."
	cargo test -p attune-common --test action_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test cache_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test enforcement_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test event_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test execution_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test identity_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test inquiry_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test key_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test maintenance_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test migration_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test notification_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test pack_environment_coordination_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test pack_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test permission_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test queue_stats_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test repository_artifact_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test repository_runtime_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test repository_worker_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test rule_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test sensor_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test trigger_repository_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test webhook_tests -- --ignored --test-threads=1
	cargo test -p attune-common --test work_queue_repository_tests -- --ignored --test-threads=1
	@echo "Common integration tests complete"

test-with-db: test test-integration
	@echo "All tests with database complete"

# Code quality
check:
	cargo check --all-features

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-features -- -D warnings

lint: fmt clippy

# Running services
run-api:
	cargo run --bin attune-api

run-api-release:
	cargo run --bin attune-api --release

run-executor:
	cargo run --bin attune-executor

run-executor-release:
	cargo run --bin attune-executor --release

run-worker:
	cargo run --bin attune-worker

run-worker-release:
	cargo run --bin attune-worker --release

run-sensor:
	cargo run --bin attune-sensor

run-sensor-release:
	cargo run --bin attune-sensor --release

run-notifier:
	cargo run --bin attune-notifier

run-notifier-release:
	cargo run --bin attune-notifier --release

run-supervisor:
	cargo run --bin attune-supervisor

run-supervisor-release:
	cargo run --bin attune-supervisor --release

# Development mode (run all services)
dev:
	@echo "Starting all services in development mode..."
	@echo "Note: Run each service in a separate terminal or use docker compose"
	@echo ""
	@echo "Terminal 1: make run-api"
	@echo "Terminal 2: make run-executor"
	@echo "Terminal 3: make run-worker"
	@echo "Terminal 4: make run-sensor"
	@echo "Terminal 5: make run-notifier"
	@echo "Terminal 6: make run-supervisor"

# Watch for changes and rebuild
watch:
	cargo watch -x check -x test -x build

# Database operations
db-create:
	createdb attune || true
	psql -d attune -c "CREATE SCHEMA IF NOT EXISTS attune; ALTER DATABASE attune SET search_path TO attune, public;" || true

db-migrate:
	sqlx migrate run

db-drop:
	dropdb attune || true

db-reset: db-drop db-create db-migrate
	@echo "Database reset complete"

# Test database operations
db-test-create:
	psql $(TEST_DB_ADMIN_URL) -c "CREATE DATABASE attune_test" || true
	psql $(TEST_DB_URL) -c "CREATE SCHEMA IF NOT EXISTS attune; ALTER DATABASE attune_test SET search_path TO attune, public;" || true

db-test-migrate:
	DATABASE_URL=$(TEST_DB_URL) sqlx migrate run

db-test-drop:
	psql $(TEST_DB_ADMIN_URL) -c "DROP DATABASE attune_test"

db-test-reset: db-test-drop db-test-create db-test-migrate
	@echo "Test database reset complete"

db-test-setup: db-test-create db-test-migrate
	@echo "Test database setup complete"

# Docker operations

# Stop system services that conflict with Docker Compose
# This resolves "address already in use" errors for PostgreSQL (5432), RabbitMQ (5672), Redis (6379)
docker-stop-system-services:
	@echo "Stopping system services that conflict with Docker..."
	@./scripts/stop-system-services.sh

# Pre-warm the build cache by building one service first
# This prevents race conditions when building multiple services in parallel
# The first build populates the shared cargo registry/git cache
docker-cache-warm:
	@echo "Warming up build cache (building API service first)..."
	@echo "This prevents race conditions during parallel builds."
	docker compose build api
	@echo ""
	@echo "Cache warmed! Now you can safely run 'make docker-build' for parallel builds."

docker-build:
	@echo "Building Docker images..."
	docker compose build

docker-build-api:
	docker compose build api

docker-build-web:
	docker compose build web

docker-build-mcp:
	docker compose build mcp

docker-up-mcp:
	docker compose --profile mcp up -d mcp

docker-down-mcp:
	docker compose --profile mcp stop mcp

# Native Linux musl target for the current host architecture.
NATIVE_RUST_TARGET := $(shell ARCH="$$(uname -m)"; \
	if [ "$$ARCH" = "x86_64" ] || [ "$$ARCH" = "amd64" ]; then \
		echo x86_64-unknown-linux-musl; \
	elif [ "$$ARCH" = "arm64" ] || [ "$$ARCH" = "aarch64" ]; then \
		echo aarch64-unknown-linux-musl; \
	else \
		echo x86_64-unknown-linux-musl; \
	fi)

# Agent binary (statically-linked for injection into any container)
AGENT_RUST_TARGET ?= $(NATIVE_RUST_TARGET)

# Pack binaries (statically-linked for packs volume)
PACK_BINARIES_RUST_TARGET ?= $(NATIVE_RUST_TARGET)

build-agent:
	@echo "Installing musl target (if not already installed)..."
	rustup target add $(AGENT_RUST_TARGET) 2>/dev/null || true
	@echo "Building statically-linked worker and sensor agent binaries..."
	SQLX_OFFLINE=true cargo build --release --target $(AGENT_RUST_TARGET) --bin attune-agent --bin attune-sensor-agent
	strip target/$(AGENT_RUST_TARGET)/release/attune-agent
	strip target/$(AGENT_RUST_TARGET)/release/attune-sensor-agent
	@echo "✅ Agent binaries built:"
	@echo "   - target/$(AGENT_RUST_TARGET)/release/attune-agent"
	@echo "   - target/$(AGENT_RUST_TARGET)/release/attune-sensor-agent"
	@ls -lh target/$(AGENT_RUST_TARGET)/release/attune-agent
	@ls -lh target/$(AGENT_RUST_TARGET)/release/attune-sensor-agent

docker-build-agent:
	@echo "Building agent Docker image ($(AGENT_RUST_TARGET))..."
	DOCKER_BUILDKIT=1 docker buildx build --build-arg RUST_TARGET=$(AGENT_RUST_TARGET) --target agent-init -f docker/Dockerfile.agent -t attune-agent:latest .
	@echo "✅ Agent image built: attune-agent:latest ($(AGENT_RUST_TARGET))"

docker-build-agent-arm64:
	@echo "Building arm64 agent Docker image..."
	DOCKER_BUILDKIT=1 docker buildx build --build-arg RUST_TARGET=aarch64-unknown-linux-musl --target agent-init -f docker/Dockerfile.agent -t attune-agent:arm64 .
	@echo "✅ Agent image built: attune-agent:arm64"

docker-build-agent-all:
	@echo "Building agent Docker images for all architectures..."
	$(MAKE) docker-build-agent
	$(MAKE) docker-build-agent-arm64
	@echo "✅ All agent images built: attune-agent:latest (amd64), attune-agent:arm64"

run-agent:
	cargo run --bin attune-agent

run-agent-release:
	cargo run --bin attune-agent --release

# Pack binaries (statically-linked for packs volume)
docker-build-pack-binaries:
	@echo "Building pack binaries Docker image ($(PACK_BINARIES_RUST_TARGET))..."
	DOCKER_BUILDKIT=1 docker buildx build --build-arg RUST_TARGET=$(PACK_BINARIES_RUST_TARGET) --target pack-binaries-init -f docker/Dockerfile.pack-binaries -t attune-pack-builder:latest .
	@echo "✅ Pack binaries image built: attune-pack-builder:latest ($(PACK_BINARIES_RUST_TARGET))"

docker-build-pack-binaries-arm64:
	@echo "Building arm64 pack binaries Docker image..."
	DOCKER_BUILDKIT=1 docker buildx build --build-arg RUST_TARGET=aarch64-unknown-linux-musl --target pack-binaries-init -f docker/Dockerfile.pack-binaries -t attune-pack-builder:arm64 .
	@echo "✅ Pack binaries image built: attune-pack-builder:arm64"

docker-build-pack-binaries-all:
	@echo "Building pack binaries Docker images for all architectures..."
	$(MAKE) docker-build-pack-binaries
	$(MAKE) docker-build-pack-binaries-arm64
	@echo "✅ All pack binary images built: attune-pack-builder:latest (amd64), attune-pack-builder:arm64"

run-sensor-agent:
	cargo run --bin attune-sensor-agent

run-sensor-agent-release:
	cargo run --bin attune-sensor-agent --release

docker-up:
	@echo "Starting all services with Docker Compose..."
	docker compose up -d

docker-up-agent:
	@echo "Starting all services + agent-based workers..."
	docker compose -f docker-compose.yaml -f docker-compose.agent.yaml up -d

docker-down:
	@echo "Stopping all services..."
	docker compose down

docker-down-agent:
	@echo "Stopping all services (including agent workers)..."
	docker compose -f docker-compose.yaml -f docker-compose.agent.yaml down

docker-down-volumes:
	@echo "Stopping all services and removing volumes (WARNING: deletes data)..."
	docker compose down -v

docker-restart:
	docker compose restart

docker-logs:
	docker compose logs -f

docker-logs-api:
	docker compose logs -f api

docker-ps:
	docker compose ps

docker-shell-api:
	docker compose exec api /bin/sh

docker-shell-db:
	docker compose exec postgres psql -U attune

docker-clean:
	@echo "Cleaning up Docker resources..."
	docker compose down -v --rmi local
	docker system prune -f

# Install development tools
install-tools:
	@echo "Installing development tools..."
	cargo install cargo-watch
	cargo install cargo-expand
	cargo install sqlx-cli --no-default-features --features postgres
	@echo "Tools installed successfully"

# Setup environment
setup: install-tools
	@echo "Setting up development environment..."
	@if [ ! -f .env ]; then \
		echo "Creating .env file from .env.example..."; \
		cp .env.example .env; \
		echo "⚠️  Please edit .env and update configuration values"; \
	fi
	@if [ ! -f .env.test ]; then \
		echo ".env.test already exists"; \
	fi
	@echo "Setup complete! Run 'make db-create && make db-migrate' to initialize the database."
	@echo "For testing, run 'make db-test-setup' to initialize the test database."

# Documentation
docs:
	cargo doc --no-deps --open

# Generate AGENTS.md index
generate-agents-index:
	@echo "Generating AGENTS.md index..."
	python3 scripts/generate_agents_md_index.py
	@echo "✅ AGENTS.md generated successfully"

# Benchmarks
bench:
	cargo bench

# Coverage
coverage:
	cargo tarpaulin --out Html --output-dir coverage

# Update dependencies
update:
	cargo update

# Audit dependencies for security issues (ignores configured in deny.toml)
audit:
	cargo deny check advisories

deny:
	cargo deny check

ci-rust:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets --all-features -- -D warnings
	cargo test --workspace --all-features
	cargo deny check

ci-web-blocking:
	cd web && npm ci
	cd web && npm run lint
	cd web && npm run typecheck
	cd web && npm run build

ci-web-pre-commit:
	cd web && npm ci
	cd web && npm run lint
	cd web && npm run typecheck

ci-web-advisory:
	cd web && npm ci
	cd web && npm run knip
	cd web && npm audit --omit=dev

ci-security-blocking:
	mkdir -p $$HOME/bin
	GITLEAKS_VERSION="8.24.2"; \
	ARCH="$$(uname -m)"; \
	case "$$ARCH" in \
		x86_64) ARCH="x64" ;; \
		aarch64|arm64) ARCH="arm64" ;; \
		*) echo "Unsupported architecture: $$ARCH"; exit 1 ;; \
	esac; \
	curl -sSfL \
		-o /tmp/gitleaks.tar.gz \
		"https://github.com/gitleaks/gitleaks/releases/download/v$$GITLEAKS_VERSION/gitleaks_$$GITLEAKS_VERSION"_linux_"$$ARCH".tar.gz; \
	tar -xzf /tmp/gitleaks.tar.gz -C $$HOME/bin gitleaks; \
	chmod +x $$HOME/bin/gitleaks
	$$HOME/bin/gitleaks git --report-format sarif --report-path gitleaks.sarif --config .gitleaks.toml

ci-security-advisory:
	pip install semgrep
	semgrep scan --config p/default --error

ci-blocking: ci-rust ci-web-blocking ci-security-blocking
	@echo "✅ Blocking CI checks passed!"

ci-advisory: ci-web-advisory ci-security-advisory
	@echo "Advisory CI checks complete."

# Check dependency tree
tree:
	cargo tree

# Generate licenses list
licenses:
	cargo license --json > licenses.json
	@echo "License information saved to licenses.json"

# Blocking checks run by the git pre-commit hook after formatting.
# Keep the local web step fast; full production builds stay in CI.
pre-commit: deny ci-web-pre-commit ci-security-blocking
	@echo "✅ Pre-commit checks passed."

install-git-hooks:
	git config core.hooksPath .githooks
	chmod +x .githooks/pre-commit
	@echo "✅ Git hooks configured to use .githooks/"

# CI simulation
ci: ci-blocking ci-advisory
	@echo "✅ CI checks passed!"

# ============================================================================
# E2E Integration Tests (Docker Compose)
# ============================================================================

# Full lifecycle: build → start stack → run all tests → tear down
e2e-test:
	@./scripts/run-integration-tests.sh $(ARGS)

# Run tests but keep the stack running for debugging
e2e-test-debug:
	@./scripts/run-integration-tests.sh --no-teardown $(ARGS)

# Tier-specific shortcuts
e2e-test-tier1:
	@./scripts/run-integration-tests.sh --tier 1 $(ARGS)

e2e-test-tier2:
	@./scripts/run-integration-tests.sh --tier 2 $(ARGS)

e2e-test-tier3:
	@./scripts/run-integration-tests.sh --tier 3 $(ARGS)

# Run the scheduled/manual cache performance scenario. The general E2E runner
# excludes performance tests unless its marker expression explicitly requests them.
e2e-test-cache-load:
	@./scripts/run-integration-tests.sh -m "cache and performance" $(ARGS)

# Run standalone transport tests (includes standalone worker/sensor services)
e2e-test-standalone:
	@./scripts/run-integration-tests.sh --standalone -k standalone $(ARGS)
