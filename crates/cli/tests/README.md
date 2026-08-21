# Attune CLI Integration Tests

This directory contains comprehensive integration tests for the Attune CLI tool. These tests verify that the CLI correctly interacts with the Attune API server by mocking API responses and testing real CLI command execution.

## Overview

The integration tests are organized into several test files:

- **`test_auth.rs`** - Authentication commands (login, logout, whoami)
- **`test_packs.rs`** - Pack management commands (list, get)
- **`test_actions.rs`** - Action commands (list, get, execute)
- **`test_executions.rs`** - Execution monitoring (list, get, result filtering)
- **`test_config.rs`** - Configuration and profile management
- **`test_rules_triggers_sensors.rs`** - Rules, triggers, and sensors commands
- **`common/mod.rs`** - Shared test utilities and mock fixtures

## Test Architecture

### Test Fixtures

The tests use `TestFixture` from the `common` module, which provides:

- **Mock API Server**: Uses `wiremock` to simulate the Attune API
- **Temporary Config**: Creates isolated config directories for each test
- **Helper Functions**: Pre-configured mock responses for common API endpoints

### Test Strategy

Each test:

1. Creates a fresh test fixture with an isolated config directory
2. Writes a test configuration (with or without authentication tokens)
3. Mounts mock API responses on the mock server
4. Executes the CLI binary with specific arguments
5. Asserts on exit status, stdout, and stderr content
6. Verifies config file changes (if applicable)

## Running the Tests

### Run All Integration Tests

```bash
cargo test --package attune-cli --tests
```

### Run Specific Test File

```bash
# Authentication tests only
cargo test --package attune-cli --test test_auth

# Pack tests only
cargo test --package attune-cli --test test_packs

# Execution tests only
cargo test --package attune-cli --test test_executions
```

### Run Specific Test

```bash
cargo test --package attune-cli --test test_auth test_login_success
```

### Run with Output

```bash
cargo test --package attune-cli --tests -- --nocapture
```

### Run in Parallel (default) or Serial

```bash
# Parallel (faster)
cargo test --package attune-cli --tests

# Serial (for debugging)
cargo test --package attune-cli --tests -- --test-threads=1
```

## Test Coverage

### Authentication (test_auth.rs)

- ✅ Login with valid credentials
- ✅ Login with invalid credentials
- ✅ Whoami when authenticated
- ✅ Whoami when unauthenticated
- ✅ Logout and token removal
- ✅ Profile override with --profile flag
- ✅ Missing required arguments
- ✅ JSON/YAML output formats

### Packs (test_packs.rs)

- ✅ List packs when authenticated
- ✅ List packs when unauthenticated
- ✅ Get pack by reference
- ✅ Pack not found (404)
- ✅ Empty pack list
- ✅ JSON/YAML output formats
- ✅ Profile and API URL overrides

### Actions (test_actions.rs)

- ✅ List actions
- ✅ Get action details
- ✅ Execute action with parameters
- ✅ Execute with multiple parameters
- ✅ Execute with JSON parameters
- ✅ Execute without parameters
- ✅ Execute with --watch flag
- ✅ Execute with --async flag
- ✅ List actions by pack
- ✅ Invalid parameter formats
- ✅ JSON/YAML output formats

### Executions (test_executions.rs)

- ✅ List executions
- ✅ Get execution by ID
- ✅ Get execution result (raw output)
- ✅ Filter by status
- ✅ Filter by pack name
- ✅ Filter by action
- ✅ Multiple filters combined
- ✅ Empty execution list
- ✅ Invalid execution ID
- ✅ JSON/YAML output formats

### Configuration (test_config.rs)

- ✅ Show current configuration
- ✅ Get specific config key
- ✅ Set config values (api_url, output_format)
- ✅ List all profiles
- ✅ Show specific profile
- ✅ Add new profile
- ✅ Switch profile (use command)
- ✅ Remove profile
- ✅ Cannot remove default profile
- ✅ Cannot remove active profile
- ✅ Profile override with --profile flag
- ✅ Profile override with ATTUNE_PROFILE env var
- ✅ Sensitive data masking
- ✅ JSON/YAML output formats

### Rules, Triggers, Sensors (test_rules_triggers_sensors.rs)

- ✅ List rules/triggers/sensors
- ✅ Get by reference
- ✅ Not found (404)
- ✅ List by pack
- ✅ Empty results
- ✅ JSON/YAML output formats
- ✅ Cross-feature profile usage

## Writing New Tests

### Basic Test Structure

```rust
#[tokio::test]
async fn test_my_feature() {
    // 1. Create test fixture
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("token", "refresh");

    // 2. Mock API response
    mock_some_endpoint(&fixture.mock_server).await;

    // 3. Execute CLI command
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("subcommand")
        .arg("action");

    // 4. Assert results
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("expected output"));
}
```

### Adding Custom Mock Responses

```rust
use wiremock::{Mock, ResponseTemplate, matchers::{method, path}};
use serde_json::json;

Mock::given(method("GET"))
    .and(path("/api/v1/custom-endpoint"))
    .respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "data": {"key": "value"}
    })))
    .mount(&fixture.mock_server)
    .await;
```

### Testing Error Cases

```rust
#[tokio::test]
async fn test_error_case() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    // Mock error response
    Mock::given(method("GET"))
        .and(path("/api/v1/endpoint"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": "Internal server error"
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .arg("command");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error"));
}
```

## Dependencies

The integration tests use:

- **`assert_cmd`** - For testing CLI binaries
- **`predicates`** - For flexible assertions
- **`wiremock`** - For mocking HTTP API responses
- **`tempfile`** - For temporary test directories
- **`tokio-test`** - For async test utilities

## Continuous Integration

These tests should be run in CI/CD pipelines:

```yaml
# Example GitHub Actions workflow
- name: Run CLI Integration Tests
  run: cargo test --package attune-cli --tests
```

## Troubleshooting

### Tests Hanging

If tests hang, it's likely due to:
- Missing mock responses for API endpoints
- The CLI waiting for user input (use appropriate flags to avoid interactive prompts)

### Flaky Tests

If tests are flaky:
- Ensure proper cleanup between tests (fixtures are automatically cleaned up)
- Check for race conditions in parallel test execution
- Run with `--test-threads=1` to isolate the issue

### Config File Conflicts

Each test uses isolated temporary directories, so config conflicts should not occur. If they do:
- Verify `XDG_CONFIG_HOME` and `HOME` environment variables are set correctly
- Check that the test is using `fixture.config_dir_path()`

## Future Enhancements

Potential improvements for the test suite:

- [ ] Add performance benchmarks for CLI commands
- [x] Test shell completion generation
- [ ] Test CLI with real API server (optional integration mode)
- [ ] Add tests for interactive prompts using `dialoguer`
- [ ] Test error recovery and retry logic
- [ ] Add tests for verbose/debug logging output
- [ ] Test handling of network timeouts and connection errors
- [ ] Add property-based tests with `proptest`

## Documentation

For more information:
- [CLI Usage Guide](../README.md)
- [CLI Profile Management](../../../docs/cli-profiles.md)
- [API Documentation](../../../docs/api-*.md)
- [Main Project README](../../../README.md)
