#!/usr/bin/env python3
"""
Quick Test Script for E2E Testing
Tests basic connectivity and authentication without full pytest setup
"""

import re
import sys

import requests
from requests.adapters import HTTPAdapter
from urllib3.util.retry import Retry

API_URL = "http://localhost:8080"


def print_response_diagnostic(response):
    """Print response metadata without exposing the response body."""
    print(f"     HTTP status: {response.status_code}")
    request_id = next(
        (
            response.headers.get(name)
            for name in ("x-request-id", "x-attune-request-id", "request-id")
            if response.headers.get(name)
        ),
        None,
    )
    if request_id and re.fullmatch(r"[A-Za-z0-9._:-]{1,128}", request_id):
        print(f"     Request ID: {request_id}")
    print("     Response body omitted")


def test_health():
    """Test health endpoint"""
    print("Testing /health endpoint...")
    try:
        response = requests.get(f"{API_URL}/health", timeout=5)
        response.raise_for_status()
        data = response.json()
        if data.get("status") != "ok":
            print("✗ Health check returned an unexpected status field")
            return False
        print(f"✓ Health check passed (HTTP {response.status_code})")
        return True
    except Exception as e:
        print(f"✗ Health check failed ({type(e).__name__})")
        if getattr(e, "response", None) is not None:
            print_response_diagnostic(e.response)
        return False


def test_register_and_login():
    """Test user registration and login"""
    print("\nTesting authentication...")

    session = requests.Session()
    retry = Retry(total=3, backoff_factor=1, status_forcelist=[429, 500, 502, 503, 504])
    adapter = HTTPAdapter(max_retries=retry)
    session.mount("http://", adapter)
    session.mount("https://", adapter)

    # Try to register
    try:
        print("  Attempting registration...")
        reg_response = session.post(
            f"{API_URL}/auth/register",
            json={
                "login": "test@attune.local",
                "password": "TestPass123!",
                "display_name": "Test User",
            },
            timeout=5,
        )
        if reg_response.status_code == 201:
            print("  ✓ User registered successfully")
        elif reg_response.status_code == 409:
            print("  ℹ User already exists (conflict)")
        else:
            print(f"  ⚠ Registration returned: {reg_response.status_code}")
    except Exception as e:
        print(f"  ⚠ Registration failed ({type(e).__name__})")
        if getattr(e, "response", None) is not None:
            print_response_diagnostic(e.response)

    # Try to login
    try:
        print("  Attempting login...")
        login_response = session.post(
            f"{API_URL}/auth/login",
            json={"login": "test@attune.local", "password": "TestPass123!"},
            timeout=5,
        )
        login_response.raise_for_status()
        data = login_response.json()
        token = data["data"]["access_token"]
        print(f"  ✓ Login successful, token length: {len(token)}")

        # Test authenticated request
        session.headers.update({"Authorization": f"Bearer {token}"})
        me_response = session.get(f"{API_URL}/auth/me", timeout=5)
        me_response.raise_for_status()
        user_data = me_response.json()
        print(f"  ✓ Authenticated as: {user_data['data']['login']}")

        return True
    except Exception as e:
        print(f"  ✗ Login failed ({type(e).__name__})")
        if getattr(e, "response", None) is not None:
            print_response_diagnostic(e.response)
        return False


def test_pack_endpoints():
    """Test pack list endpoint"""
    print("\nTesting pack endpoints...")

    session = requests.Session()

    # Login first
    try:
        login_response = session.post(
            f"{API_URL}/auth/login",
            json={"login": "test@attune.local", "password": "TestPass123!"},
            timeout=5,
        )
        login_response.raise_for_status()
        token = login_response.json()["data"]["access_token"]
        session.headers.update({"Authorization": f"Bearer {token}"})
    except Exception as e:
        print(f"  ⚠ Could not authenticate ({type(e).__name__})")
        if getattr(e, "response", None) is not None:
            print_response_diagnostic(e.response)
        return False

    # Test pack list
    try:
        print("  Fetching pack list...")
        response = session.get(f"{API_URL}/api/v1/packs", timeout=5)
        response.raise_for_status()
        data = response.json()
        count = len(data.get("data", []))
        print(f"  ✓ Pack list retrieved: {count} packs found")
        return True
    except Exception as e:
        print(f"  ✗ Pack list failed ({type(e).__name__})")
        if getattr(e, "response", None) is not None:
            print_response_diagnostic(e.response)
        return False


def test_trigger_creation():
    """Test creating a trigger"""
    print("\nTesting trigger creation...")

    session = requests.Session()

    # Login first
    try:
        login_response = session.post(
            f"{API_URL}/auth/login",
            json={"login": "test@attune.local", "password": "TestPass123!"},
            timeout=5,
        )
        login_response.raise_for_status()
        token = login_response.json()["data"]["access_token"]
        session.headers.update({"Authorization": f"Bearer {token}"})
    except Exception as e:
        print(f"  ⚠ Could not authenticate ({type(e).__name__})")
        if getattr(e, "response", None) is not None:
            print_response_diagnostic(e.response)
        return False

    # First ensure test_pack exists
    try:
        print("  Checking for test_pack...")
        import os

        test_pack_paths = [
            "tests/fixtures/packs/test_pack",
            os.path.join(os.path.dirname(__file__), "fixtures/packs/test_pack"),
        ]

        pack_path = None
        for path in test_pack_paths:
            abs_path = os.path.abspath(path)
            if os.path.exists(abs_path):
                pack_path = abs_path
                break

        if pack_path:
            # Register pack
            reg_response = session.post(
                f"{API_URL}/api/v1/packs/register",
                json={"path": pack_path, "force": True, "skip_tests": True},
                timeout=10,
            )
            if reg_response.status_code in [200, 201, 409]:
                print(f"  ✓ Pack registered/exists")
            else:
                print(f"  ⚠ Pack registration returned: {reg_response.status_code}")
        else:
            print(f"  ⚠ Test pack not found, skipping pack registration")
    except Exception as e:
        print(f"  ⚠ Pack check failed ({type(e).__name__})")
        if getattr(e, "response", None) is not None:
            print_response_diagnostic(e.response)

    # Create trigger
    try:
        print("  Creating webhook trigger...")
        import time

        unique_id = int(time.time() * 1000) % 1000000
        trigger_ref = f"test_pack.quick_test_trigger_{unique_id}"

        trigger_data = {
            "ref": trigger_ref,
            "pack_ref": "test_pack",
            "label": "Quick Test Webhook Trigger",
            "description": "Webhook trigger for quick E2E testing",
            "enabled": True,
            "param_schema": {
                "type": "object",
                "properties": {
                    "url_path": {"type": "string"},
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "DELETE"],
                    },
                },
                "required": ["url_path"],
            },
            "out_schema": {
                "type": "object",
                "properties": {
                    "headers": {"type": "object"},
                    "body": {"type": "object"},
                },
            },
        }

        response = session.post(
            f"{API_URL}/api/v1/triggers",
            json=trigger_data,
            timeout=5,
        )
        response.raise_for_status()
        data = response.json()
        print(f"  ✓ Trigger created: {data['data']['ref']}")
        return True
    except Exception as e:
        print(f"  ✗ Trigger creation failed ({type(e).__name__})")
        if getattr(e, "response", None) is not None:
            print_response_diagnostic(e.response)
        return False


def test_rule_creation():
    """Test creating a complete automation rule"""
    print("\nTesting rule creation...")

    session = requests.Session()

    # Login first
    try:
        login_response = session.post(
            f"{API_URL}/auth/login",
            json={"login": "test@attune.local", "password": "TestPass123!"},
            timeout=5,
        )
        login_response.raise_for_status()
        token = login_response.json()["data"]["access_token"]
        session.headers.update({"Authorization": f"Bearer {token}"})
    except Exception as e:
        print(f"  ⚠ Could not authenticate ({type(e).__name__})")
        if getattr(e, "response", None) is not None:
            print_response_diagnostic(e.response)
        return False

    try:
        import time

        unique_id = int(time.time() * 1000) % 1000000

        # Step 1: Create trigger
        print("  Creating trigger...")
        trigger_ref = f"test_pack.quick_rule_trigger_{unique_id}"
        trigger_response = session.post(
            f"{API_URL}/api/v1/triggers",
            json={
                "ref": trigger_ref,
                "pack_ref": "test_pack",
                "label": "Quick Rule Test Trigger",
                "description": "Trigger for rule test",
                "enabled": True,
                "param_schema": {"type": "object"},
                "out_schema": {"type": "object"},
            },
            timeout=5,
        )
        trigger_response.raise_for_status()
        print(f"    ✓ Trigger: {trigger_ref}")

        # Step 2: Create action
        print("  Creating action...")
        action_ref = f"test_pack.quick_rule_action_{unique_id}"
        action_response = session.post(
            f"{API_URL}/api/v1/actions",
            json={
                "ref": action_ref,
                "pack_ref": "test_pack",
                "label": "Quick Rule Test Action",
                "description": "Action for rule test",
                "entrypoint": "actions/echo.py",
                "param_schema": {
                    "type": "object",
                    "properties": {"message": {"type": "string"}},
                },
            },
            timeout=5,
        )
        action_response.raise_for_status()
        print(f"    ✓ Action: {action_ref}")

        # Step 3: Create rule
        print("  Creating rule...")
        rule_ref = f"test_pack.quick_test_rule_{unique_id}"
        rule_response = session.post(
            f"{API_URL}/api/v1/rules",
            json={
                "ref": rule_ref,
                "pack_ref": "test_pack",
                "label": "Quick Test Rule",
                "description": "Complete rule for quick testing",
                "action_ref": action_ref,
                "trigger_ref": trigger_ref,
                "conditions": {},
                "action_params": {"message": "Test message"},
                "enabled": True,
            },
            timeout=5,
        )
        rule_response.raise_for_status()
        rule_data = rule_response.json()
        print(f"    ✓ Rule: {rule_data['data']['ref']}")
        print(f"  ✓ Complete automation rule created successfully")
        return True

    except Exception as e:
        print(f"  ✗ Rule creation failed ({type(e).__name__})")
        if getattr(e, "response", None) is not None:
            print_response_diagnostic(e.response)
        return False


def main():
    """Run all quick tests"""
    print("=" * 60)
    print("Attune E2E Quick Test")
    print("=" * 60)
    print(f"API URL: {API_URL}")
    print()

    results = []

    # Test health
    results.append(("Health Check", test_health()))

    # Test auth
    results.append(("Authentication", test_register_and_login()))

    # Test pack endpoints
    results.append(("Pack Endpoints", test_pack_endpoints()))

    # Test trigger creation
    results.append(("Trigger Creation", test_trigger_creation()))

    # Test rule creation (full automation flow)
    results.append(("Rule Creation", test_rule_creation()))

    # Summary
    print("\n" + "=" * 60)
    print("Test Summary")
    print("=" * 60)

    passed = sum(1 for _, result in results if result)
    total = len(results)

    for name, result in results:
        status = "✓ PASS" if result else "✗ FAIL"
        print(f"{status:8} {name}")

    print("-" * 60)
    print(f"Total: {passed}/{total} passed")
    print("=" * 60)

    if passed == total:
        print("\n✓ All tests passed! E2E environment is ready.")
        sys.exit(0)
    else:
        print(f"\n✗ {total - passed} test(s) failed. Check API service.")
        sys.exit(1)


if __name__ == "__main__":
    main()
