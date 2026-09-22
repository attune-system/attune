# E2E Testing Quick Start Guide

**Last Updated**: 2026-01-22  
**Status**: ✅ Infrastructure Ready - Quick Test Passing (3/3)

---

## Prerequisites

- **Attune API service running** on `http://localhost:8080` (or set `ATTUNE_API_URL`)
- Python 3.8+ installed
- Internet connection (for downloading test dependencies)

---

## Quick Validation (No Setup Required)

Test basic connectivity without installing dependencies:

```bash
cd tests
python3 quick_test.py
```

**Expected Output:**
```
============================================================
Attune E2E Quick Test
============================================================
API URL: http://localhost:8080

Testing /health endpoint...
✓ Health check passed: {'status': 'ok'}

Testing authentication...
  Attempting registration...
  ⚠ Registration returned: 200
  Attempting login...
  ✓ Login successful, got token: eyJ0eXAiOiJKV1QiLCJh...
  ✓ Authenticated as: test@attune.local

Testing pack endpoints...
  Fetching pack list...
  ✓ Pack list retrieved: 0 packs found

============================================================
Test Summary
============================================================
✓ PASS   Health Check
✓ PASS   Authentication
✓ PASS   Pack Endpoints
------------------------------------------------------------
Total: 3/3 passed
============================================================

✓ All tests passed! E2E environment is ready.
```

---

## Full Test Suite

### 1. Setup (First Time Only)

```bash
cd tests
./run_e2e_tests.sh --setup
```

This will:
- Create a Python virtual environment at `tests/venvs/e2e`
- Install all test dependencies (pytest, requests, etc.)
- Verify dependencies are installed correctly

### 2. Run Tests

**Basic run:**
```bash
./run_e2e_tests.sh
```

**Verbose output (recommended):**
```bash
./run_e2e_tests.sh -v
```

**Run specific test:**
```bash
./run_e2e_tests.sh -k "test_api_health"
```

**Stop on first failure:**
```bash
./run_e2e_tests.sh -s
```

**With coverage report:**
```bash
./run_e2e_tests.sh --coverage
```

**All options:**
```bash
./run_e2e_tests.sh -h
```

### 3. Cleanup

```bash
./run_e2e_tests.sh --teardown
```

This removes:
- Test artifacts
- Log files
- Pytest cache
- Coverage reports

---

## Manual Test Execution (Advanced)

If you prefer to run pytest directly:

```bash
# Activate virtual environment
source tests/venvs/e2e/bin/activate

# Run tests
cd tests
pytest test_e2e_basic.py -v

# Deactivate when done
deactivate
```

---

## Environment Variables

Configure test behavior via environment variables:

```bash
# API endpoint (default: http://localhost:8080)
export ATTUNE_API_URL="http://localhost:8080"

# Test timeout in seconds (default: 60)
export TEST_TIMEOUT="60"

# Then run tests
./run_e2e_tests.sh -v
```

---

## Troubleshooting

### API Service Not Running

**Error:**
```
✗ API service is not reachable at http://localhost:8080
```

**Solution:**
```bash
# Start API service
cd crates/api
cargo run --release
```

### Authentication Fails

**Error:**
```
✗ Login failed: 422 Client Error: Unprocessable Entity
```

**Common Causes:**
1. **Wrong field names**: Must use `"login"` not `"username"`
2. **Password too short**: Minimum 8 characters required
3. **Missing fields**: Both `login` and `password` are required

**Test credentials:**
- Login: `test@attune.local`
- Password: `TestPass123!` (min 8 chars)

### Import Errors

**Error:**
```
ModuleNotFoundError: No module named 'pytest'
```

**Solution:**
```bash
# Run setup first
./run_e2e_tests.sh --setup

# Or manually install dependencies
pip install -r tests/requirements.txt
```

### Pack Registration Fails

**Error:**
```
FileNotFoundError: [Errno 2] No such file or directory: 'tests/fixtures/packs/test_pack'
```

**Solution:**
```bash
# Verify you're in the project root
pwd  # Should end with /attune

# Check test pack exists
ls -la tests/fixtures/packs/test_pack/

# If missing, the repository may be incomplete
```

---

## Test Scenarios

### Currently Implemented

1. ✅ **Health Check** - Validates API is responding
2. ✅ **Authentication** - User registration and login
3. ✅ **Pack Registration** - Register test pack from local directory
4. ✅ **Action Creation** - Create simple echo action
5. ✅ **Timer Trigger Flow** - Create trigger, action, and rule (infrastructure only)
6. 🔄 **Manual Execution** - Direct action execution (pending endpoint)

### Planned (Phase 3)

- Timer automation flow (sensor → event → rule → execution)
- Workflow execution (3-task sequential workflow)
- FIFO queue ordering (concurrency limits)
- Inquiry (human-in-the-loop) flows
- Secret management across services
- Error handling and retry logic
- WebSocket notifications
- Dependency isolation (per-pack venvs)

---

## API Endpoint Reference

### Health Endpoints (No Auth)
- `GET /health` - Basic health check
- `GET /health/detailed` - Health with database status
- `GET /health/ready` - Readiness probe
- `GET /health/live` - Liveness probe

### Authentication Endpoints (No Auth)
- `POST /auth/register` - Register new user
- `POST /auth/login` - Login and get JWT token
- `POST /auth/refresh` - Refresh access token

### Protected Endpoints (Auth Required)
- `GET /auth/me` - Get current user info
- `POST /auth/change-password` - Change password
- `GET /api/v1/packs` - List packs
- `POST /api/v1/packs/register` - Register pack
- `GET /api/v1/actions` - List actions
- `POST /api/v1/actions` - Create action
- And all other `/api/v1/*` endpoints...

---

## Authentication Schema

### Register Request
```json
{
  "login": "newuser@example.com",   // Min 3 chars (NOT "username")
  "password": "SecurePass123!",     // Min 8 chars, max 128
  "display_name": "New User"        // Optional (NOT "full_name")
}
```

### Login Request
```json
{
  "login": "user@example.com",      // NOT "username"
  "password": "SecurePass123!"      // Min 8 chars
}
```

### Login Response
```json
{
  "data": {
    "access_token": "eyJ0eXAiOiJKV1QiLCJh...",
    "refresh_token": "eyJ0eXAiOiJKV1QiLCJh...",
    "token_type": "Bearer",
    "expires_in": 3600,
    "user": {
      "id": 1,
      "login": "user@example.com",
      "display_name": "User Name"
    }
  }
}
```

---

## CI/CD Integration

### GitHub Actions Example

```yaml
name: E2E Tests

on: [push, pull_request]

jobs:
  e2e-tests:
    runs-on: ubuntu-latest
    
    services:
      postgres:
        image: postgres:14
        env:
          POSTGRES_PASSWORD: postgres
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
      
      rabbitmq:
        image: rabbitmq:4.3.6-management-alpine
        options: >-
          --health-cmd "rabbitmq-diagnostics ping"
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
    
    steps:
      - uses: actions/checkout@v3
      
      - name: Setup Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      
      - name: Start API Service
        run: |
          cd crates/api
          cargo run --release &
          sleep 5
      
      - name: Run E2E Tests
        run: |
          ./tests/run_e2e_tests.sh --setup -v
      
      - name: Upload Test Reports
        if: always()
        uses: actions/upload-artifact@v3
        with:
          name: e2e-test-reports
          path: tests/htmlcov/
```

---

## Getting Help

- **Documentation**: See `tests/README.md` for detailed test scenarios
- **Work Summary**: `work-summary/2026-01-22-e2e-testing-phase2.md`
- **Issues**: Check service logs in `tests/logs/` (if running via scripts)
- **Quick Test**: Use `python3 tests/quick_test.py` to isolate API connectivity issues

---

## Status Summary

| Component | Status | Notes |
|-----------|--------|-------|
| Test Infrastructure | ✅ Complete | AttuneClient, fixtures, runner |
| Quick Test | ✅ Passing | 3/3 tests passing |
| Basic Tests | 🔄 Partial | 5 scenarios implemented |
| Advanced Tests | 📋 Planned | Timer flow, workflows, FIFO |
| CI/CD Integration | 📋 Planned | GitHub Actions workflow |

**Last Validation**: 2026-01-22 - Quick test confirmed: health ✓, auth ✓, pack endpoints ✓

---

**Ready to test? Start here:** `./tests/run_e2e_tests.sh --setup -v`
