#!/usr/bin/env python3
"""Validation for the generated client and Attune wrapper client."""

import os
import sys

import pytest

# Add tests directory to path when this file is executed directly.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))


@pytest.mark.no_api
def test_imports():
    """Generated and wrapper client imports must succeed."""
    from generated_client import AuthenticatedClient, Client
    from helpers import AttuneClient

    assert AuthenticatedClient is not None
    assert Client is not None
    assert AttuneClient is not None


@pytest.mark.no_api
def test_retry_policy_excludes_non_idempotent_methods():
    """Automatic retries must never replay POST/PATCH writes."""
    from helpers.client import AttuneClient

    client = AttuneClient(base_url="http://localhost:8080", auto_login=False)
    retries = client.session.get_adapter("http://").max_retries
    assert "POST" not in retries.allowed_methods
    assert "PATCH" not in retries.allowed_methods
    assert "GET" in retries.allowed_methods


@pytest.mark.no_api
def test_client_initialization():
    """The wrapper must preserve caller configuration without auto-login."""
    from helpers import AttuneClient

    client = AttuneClient(
        base_url="http://localhost:8080", timeout=30, auto_login=False
    )
    assert client.base_url == "http://localhost:8080"
    assert client.timeout == 30
    assert client.auth_client is None


@pytest.mark.no_api
def test_models():
    """Generated models must construct and serialize correctly."""
    from generated_client.models.login_request import LoginRequest

    request = LoginRequest(login="test@example.com", password="password123")
    data = request.to_dict()
    assert data["login"] == "test@example.com"
    assert data["password"] == "password123"


@pytest.mark.api
def test_health_check(api_url="http://localhost:8080"):
    """The configured live API must answer its unauthenticated health check."""
    from helpers import AttuneClient

    client = AttuneClient(base_url=api_url, timeout=5, auto_login=False)
    health = client.health()
    assert health is not None


@pytest.mark.no_api
def test_to_dict_helper():
    """The wrapper conversion helper must retain plain Python values."""
    from helpers.client_wrapper import to_dict

    value = {"key": "value"}
    assert to_dict(value) == value
    assert to_dict(None) is None
    items = [{"a": 1}, {"b": 2}]
    assert to_dict(items) == items


def main():
    """Run the checks without pytest for developer smoke validation."""
    checks = [
        ("Imports", test_imports, ()),
        ("Client Init", test_client_initialization, ()),
        ("Models", test_models, ()),
        ("to_dict Helper", test_to_dict_helper, ()),
    ]
    api_url = os.getenv("ATTUNE_API_URL")
    if api_url:
        checks.append(("Health Check", test_health_check, (api_url,)))

    failures = 0
    for name, check, args in checks:
        try:
            check(*args)
            print(f"✓ PASS: {name}")
        except Exception as error:
            failures += 1
            print(f"✗ FAIL: {name}: {error}")

    print(f"\nResults: {len(checks) - failures}/{len(checks)} tests passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
