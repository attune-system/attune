"""
Pytest Configuration and Shared Fixtures for E2E Tests

This module provides shared fixtures and configuration for all
end-to-end tests.
"""

import copy
import os
import sys
import time
from typing import Generator

import pytest

# Add project root to path for imports
project_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if project_root not in sys.path:
    sys.path.insert(0, project_root)

from helpers import AttuneClient, create_test_pack, unique_ref

# ============================================================================
# Session-scoped Fixtures
# ============================================================================


@pytest.fixture(scope="session")
def api_base_url() -> str:
    """Get API base URL from environment"""
    return os.getenv("ATTUNE_API_URL", "http://localhost:8080")


@pytest.fixture(scope="session")
def test_timeout() -> int:
    """Get test timeout from environment"""
    return int(os.getenv("TEST_TIMEOUT", "60"))


@pytest.fixture(scope="session")
def test_user_credentials() -> dict:
    """Get test user credentials"""
    return {
        "login": os.getenv("TEST_USER_LOGIN", "test@attune.local"),
        "password": os.getenv("TEST_USER_PASSWORD", "TestPass123!"),
        "display_name": "E2E Test User",
    }


# ============================================================================
# Function-scoped Fixtures
# ============================================================================


@pytest.fixture
def client(api_base_url: str, test_timeout: int) -> Generator[AttuneClient, None, None]:
    """
    Create authenticated Attune API client

    This fixture creates a new client for each test function and automatically
    logs in. The client is cleaned up after the test completes.
    """
    client = AttuneClient(base_url=api_base_url, timeout=test_timeout)

    # Auto-login with test credentials
    try:
        client.login()
    except Exception as e:
        pytest.fail(f"Failed to authenticate client: {e}")

    yield client

    # Cleanup: logout
    client.logout()


@pytest.fixture(scope="session")
def session_client(
    api_base_url: str, test_timeout: int
) -> Generator[AttuneClient, None, None]:
    """Create an authenticated client for session-scoped fixture setup."""
    client = AttuneClient(base_url=api_base_url, timeout=test_timeout)

    try:
        yield client
    finally:
        client.logout()


@pytest.fixture(scope="session")
def cache_e2e_retention_config(session_client: AttuneClient):
    """Use short cache lifecycle windows only while cache E2E scenarios run."""
    response = session_client._request("GET", "/api/v1/retention-config")
    assert response.status_code == 200, response.text
    original = response.json()["data"]
    configured = copy.deepcopy(original)
    configured["check_interval_seconds"] = 2
    cache = configured["cache_retention"]
    cache["min_traversal_window_seconds"] = 5
    cache["staging_expiry_seconds"] = 5
    cache["dry_run"] = False

    update = session_client._request(
        "PUT", "/api/v1/retention-config", json=configured
    )
    assert update.status_code == 200, update.text
    yield

    restore = session_client._request(
        "PUT", "/api/v1/retention-config", json=original
    )
    assert restore.status_code == 200, restore.text


@pytest.fixture
def unique_user_client(
    api_base_url: str, test_timeout: int
) -> Generator[AttuneClient, None, None]:
    """
    Create client with unique test user

    This fixture creates a new user for each test, ensuring complete isolation
    between tests. Useful for multi-tenancy tests.
    """
    client = AttuneClient(base_url=api_base_url, timeout=test_timeout, auto_login=False)

    # Generate unique credentials
    timestamp = int(time.time())
    login = f"test_{timestamp}_{unique_ref()}@attune.local"
    password = "TestPass123!"

    # Register and login
    try:
        client.register(
            login=login, password=password, display_name=f"Test User {timestamp}"
        )
        client.login(login=login, password=password)
    except Exception as e:
        pytest.fail(f"Failed to create unique user: {e}")

    yield client

    # Cleanup
    client.logout()


@pytest.fixture(scope="session")
def e2e_pack_ref(worker_id: str) -> str:
    """Use one isolated fixture pack per pytest-xdist worker."""
    mode = os.getenv("ATTUNE_E2E_PACK_ISOLATION", "worker").lower()
    if mode in {"0", "false", "no", "shared"}:
        return "test_pack"
    suffix = worker_id if worker_id != "master" else "local"
    return f"test_pack_{suffix}"


@pytest.fixture(scope="session")
def test_pack(session_client: AttuneClient, e2e_pack_ref: str) -> dict:
    """
    Create or get test pack

    This fixture ensures the test pack is available for tests.
    """
    try:
        pack = create_test_pack(
            session_client,
            pack_ref=e2e_pack_ref,
            pack_dir="tests/fixtures/packs/test_pack",
        )
        return pack
    except Exception as e:
        pytest.fail(f"Failed to create test pack: {e}")


@pytest.fixture(scope="session")
def pack_ref(test_pack: dict) -> str:
    """Get pack reference from test pack"""
    return test_pack["ref"]


@pytest.fixture(scope="function")
def clean_test_data(request):
    """
    Clean test data after each test to prevent interference with next test

    This fixture runs after each test function and cleans up
    test-related data to ensure isolation between tests.

    Usage: Add 'clean_test_data' to test function parameters to enable cleanup
    """
    # Run the test first
    yield

    # Only clean if running E2E tests (not unit tests)
    if "e2e" not in request.node.nodeid:
        return

    db_url = os.getenv(
        "DATABASE_URL", "postgresql://attune:attune@postgres:5432/attune"
    )

    try:
        import psycopg

        with psycopg.connect(db_url) as conn:
            with conn.cursor() as cur:
                cur.execute("""
                    DELETE FROM event WHERE created > NOW() - INTERVAL '5 minutes';
                    DELETE FROM enforcement WHERE created > NOW() - INTERVAL '5 minutes';
                    DELETE FROM execution WHERE created > NOW() - INTERVAL '5 minutes';
                    DELETE FROM inquiry WHERE created > NOW() - INTERVAL '5 minutes';
                """)
            conn.commit()
    except Exception as e:
        # Don't fail tests if cleanup fails
        print(f"Warning: Test data cleanup failed: {e}")


@pytest.fixture(scope="session", autouse=True)
def setup_database():
    """
    Ensure database is properly set up before running tests

    This runs once per test session to verify runtimes are seeded.
    In Docker environments, init-packs handles seeding so this is a no-op.
    """
    db_url = os.getenv(
        "DATABASE_URL", "postgresql://attune:attune@postgres:5432/attune"
    )

    try:
        import psycopg

        with psycopg.connect(db_url) as conn:
            with conn.cursor() as cur:
                cur.execute("SELECT COUNT(*) FROM runtime WHERE pack_ref = 'core'")
                row = cur.fetchone()
                runtime_count = row[0] if row else 0

        if runtime_count == 0:
            print("\n⚠ No runtimes found — expected init-packs to seed them.")
            print("  If running outside Docker, run: ./scripts/load-core-pack.sh")
        else:
            print(f"\n✓ Database ready ({runtime_count} core runtimes found)")

    except Exception as e:
        print(f"\n⚠ Database check skipped: {e}")

    yield


# ============================================================================
# Helper Fixtures
# ============================================================================


@pytest.fixture
def wait_time() -> dict:
    """
    Standard wait times for various operations

    Returns a dict with common wait times to keep tests consistent.
    """
    return {
        "quick": 2,  # Quick operations (API calls)
        "short": 5,  # Short operations (simple executions)
        "medium": 15,  # Medium operations (workflows)
        "long": 30,  # Long operations (multi-step workflows)
        "extended": 60,  # Extended operations (slow timers)
    }


# ============================================================================
# Pytest Hooks
# ============================================================================


def pytest_configure(config):
    """
    Pytest configuration hook

    Called before test collection starts.
    """
    # Add custom markers
    config.addinivalue_line("markers", "tier1: Tier 1 core tests")
    config.addinivalue_line("markers", "tier2: Tier 2 orchestration tests")
    config.addinivalue_line("markers", "tier3: Tier 3 advanced tests")
    config.addinivalue_line("markers", "api: API integration tests (ported from Rust)")


def pytest_collection_modifyitems(config, items):
    """
    Modify test collection

    Called after test collection to modify or re-order tests.
    """
    # Sort tests by marker priority (tier1 -> tier2 -> tier3)
    tier_order = {"tier1": 0, "tier2": 1, "tier3": 2, None: 3}

    def get_tier_priority(item):
        for marker in item.iter_markers():
            if marker.name in tier_order:
                return tier_order[marker.name]
        return tier_order[None]

    items.sort(key=get_tier_priority)


def pytest_report_header(config):
    """
    Add custom header to test report

    Returns list of strings to display at top of test run.
    """
    api_url = os.getenv("ATTUNE_API_URL", "http://localhost:8080")
    return [
        f"Attune E2E Test Suite",
        f"API URL: {api_url}",
        f"Test Timeout: {os.getenv('TEST_TIMEOUT', '60')}s",
    ]


def pytest_runtest_setup(item):
    """
    Hook called before each test

    Can be used for test-specific setup or to skip tests based on conditions.
    """
    # Supervisor retention tests exercise the maintenance service directly
    # against PostgreSQL and do not require the API process to be running.
    if item.get_closest_marker("supervisor"):
        return

    # Check if API is reachable before running tests
    api_url = os.getenv("ATTUNE_API_URL", "http://localhost:8080")

    # Only check on first test
    if not hasattr(pytest_runtest_setup, "_api_checked"):
        import requests

        try:
            response = requests.get(f"{api_url}/health", timeout=5)
            if response.status_code != 200:
                pytest.exit(f"API health check failed: {response.status_code}")
        except requests.exceptions.RequestException as e:
            pytest.exit(f"Cannot reach Attune API at {api_url}: {e}")

        pytest_runtest_setup._api_checked = True


def pytest_runtest_teardown(item, nextitem):
    """
    Hook called after each test

    Can be used for cleanup or logging.
    """
    pass


# ============================================================================
# Cleanup Helpers
# ============================================================================


@pytest.fixture(autouse=True)
def cleanup_on_failure(request):
    """
    Auto-cleanup fixture that captures test state on failure

    This fixture runs for every test and captures useful debug info
    if the test fails.
    """
    yield

    # If test failed, capture additional debug info
    if request.node.rep_call.failed if hasattr(request.node, "rep_call") else False:
        print("\n=== Test Failed - Debug Info ===")
        print(f"Test: {request.node.name}")
        print(f"Location: {request.node.location}")
        # Add more debug info as needed


@pytest.hookimpl(tryfirst=True, hookwrapper=True)
def pytest_runtest_makereport(item, call):
    """
    Hook to capture test results for use in fixtures

    This allows fixtures to check if test passed/failed.
    """
    outcome = yield
    rep = outcome.get_result()
    setattr(item, f"rep_{rep.when}", rep)
