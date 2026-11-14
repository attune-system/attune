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



@pytest.fixture
def webhook_contract_client():
    """Exercise the real wrapper and generated parser on an owned HTTP transport."""
    import httpx
    from generated_client import AuthenticatedClient
    from helpers import AttuneClient

    clients = []

    def create(respond):
        wrapper = AttuneClient(base_url="http://webhook.invalid", auto_login=False)
        wrapper.auth_client = AuthenticatedClient(
            base_url=wrapper.base_url,
            token="test-owned-token",
            httpx_args={"transport": httpx.MockTransport(respond)},
        )
        clients.append(wrapper)
        return wrapper

    yield create
    for wrapper in clients:
        wrapper.auth_client.get_httpx_client().close()
        wrapper.session._session.close()


def _webhook_metadata(enabled=True):
    return {
        "id": 42, "ref": "fixture.hook", "pack_ref": "fixture", "label": "Hook",
        "enabled": True, "is_adhoc": True, "reference_visibility": "public",
        "param_schema": None, "out_schema": None, "event_config_schema": None,
        "webhook_enabled": enabled,
        "created": "2026-11-13T00:00:00Z", "updated": "2026-11-13T00:00:00Z",
    }


@pytest.mark.no_api
@pytest.mark.parametrize("enabled", [True, False])
def test_fire_webhook_uses_only_explicit_management_credentials(webhook_contract_client, enabled, caplog):
    import httpx

    calls = []
    key = "wh_test_owned_credential"

    def respond(request):
        assert request.headers["authorization"] == "Bearer test-owned-token"
        if request.method == "GET" and request.url.path == "/api/v1/triggers/fixture.hook":
            calls.append("metadata")
            return httpx.Response(200, json={"data": _webhook_metadata(enabled)})
        operation = "reveal" if enabled else "enable"
        if request.method == "POST" and request.url.path == f"/api/v1/triggers/fixture.hook/webhooks/{operation}":
            calls.append(operation)
            return httpx.Response(200, json={"data": {**_webhook_metadata(), "webhook_key": key}})
        if request.method == "POST" and request.url.path == f"/api/v1/webhooks/{key}":
            import json
            assert json.loads(request.content) == {"payload": {"message": "hello"}}
            assert key.encode() in request.url.raw_path
            calls.append("receiver")
            return httpx.Response(200, json={"data": {"event_id": 17}})
        pytest.fail("Unexpected webhook route or method")

    client = webhook_contract_client(respond)
    result = client.fire_webhook(trigger_ref="fixture.hook", payload={"message": "hello"})
    assert result == {"event_id": 17}
    assert calls == ["metadata", "reveal" if enabled else "enable", "receiver"]
    assert key not in caplog.text


@pytest.mark.no_api
@pytest.mark.parametrize("status", [401, 403, 404])
def test_fire_webhook_does_not_bypass_denied_reveal(webhook_contract_client, status):
    import httpx

    calls = []
    key = "wh_test_owned_not_for_errors"

    def respond(request):
        if request.method == "GET":
            calls.append("metadata")
            return httpx.Response(200, json={"data": _webhook_metadata()})
        assert request.method == "POST"
        assert request.url.path == "/api/v1/triggers/fixture.hook/webhooks/reveal"
        calls.append("reveal")
        return httpx.Response(status, json={"error": key})

    client = webhook_contract_client(respond)
    with pytest.raises(RuntimeError, match=f"HTTP {status}") as error:
        client.fire_webhook(trigger_ref="fixture.hook", payload={})
    assert key not in str(error.value)
    assert calls == ["metadata", "reveal"]


@pytest.mark.no_api
@pytest.mark.parametrize("management", [None, "disabled", "missing_key", "enable_denied"])
def test_fire_webhook_disabled_or_unavailable_credentials_never_reach_receiver(webhook_contract_client, management):
    import httpx

    calls = []

    def respond(request):
        if request.method == "GET":
            calls.append("metadata")
            enabled = management not in (None, "enable_denied")
            return httpx.Response(200, json={"data": _webhook_metadata(enabled)})
        operation = "enable" if management == "enable_denied" else "reveal"
        assert request.method == "POST"
        assert request.url.path == f"/api/v1/triggers/fixture.hook/webhooks/{operation}"
        calls.append(operation)
        if management == "enable_denied":
            return httpx.Response(403, json={"error": "wh_test_owned_denied"})
        return httpx.Response(200, json={"data": _webhook_metadata(management != "disabled")})

    client = webhook_contract_client(respond)
    reason = "HTTP 403" if management == "enable_denied" else ("unavailable" if management == "missing_key" else "disabled")
    with pytest.raises(RuntimeError, match=reason) as error:
        client.fire_webhook(trigger_ref="fixture.hook", payload={}, auto_enable=management is not None)
    assert "wh_test_owned" not in str(error.value)
    assert calls == (["metadata"] if management is None else ["metadata", "enable" if management == "enable_denied" else "reveal"])


@pytest.mark.no_api
@pytest.mark.parametrize("delivery", ["fire", "post"])
@pytest.mark.parametrize("failure", ["transport", "response"])
def test_webhook_delivery_errors_do_not_expose_credentials(webhook_contract_client, delivery, failure, caplog):
    import httpx
    import traceback

    key = "wh_test_owned_private_url"

    def respond(request):
        if request.method == "GET":
            return httpx.Response(200, json={"data": _webhook_metadata()})
        if request.url.path.endswith("/webhooks/reveal"):
            return httpx.Response(200, json={"data": {**_webhook_metadata(), "webhook_key": key}})
        assert request.url.path == f"/api/v1/webhooks/{key}"
        if failure == "transport":
            raise httpx.ConnectError(f"Cannot send to {request.url}", request=request)
        return httpx.Response(403, text=key)

    client = webhook_contract_client(respond)
    with pytest.raises(RuntimeError) as error:
        if delivery == "fire":
            client.fire_webhook(trigger_ref="fixture.hook", payload={})
        else:
            client.post_webhook(f"/api/v1/webhooks/{key}", payload={})
    assert key not in str(error.value)
    assert key not in "".join(traceback.format_exception(error.value))
    assert key not in caplog.text


@pytest.mark.no_api
def test_webhook_fixture_retains_enable_response_not_ordinary_metadata(webhook_contract_client):
    import httpx
    from helpers.fixtures import create_webhook_trigger

    calls = []
    key = "wh_test_owned_fixture"

    def respond(request):
        assert request.method == "POST"
        if request.url.path == "/api/v1/triggers":
            calls.append("create")
            return httpx.Response(201, json={"data": _webhook_metadata(False)})
        assert request.url.path == "/api/v1/triggers/fixture.hook/webhooks/enable"
        calls.append("enable")
        return httpx.Response(200, json={"data": {**_webhook_metadata(), "webhook_key": key}})

    client = webhook_contract_client(respond)
    trigger = create_webhook_trigger(client, name="hook", pack_ref="fixture")
    assert trigger["webhook_url"] == f"/api/v1/webhooks/{key}"
    assert calls == ["create", "enable"]


@pytest.mark.no_api
def test_raw_trigger_metadata_does_not_synthesize_webhook_key(webhook_contract_client):
    import httpx

    client = webhook_contract_client(lambda request: httpx.Response(200, json={"data": _webhook_metadata(False)}))
    assert "webhook_key" not in client.get("/api/v1/triggers/fixture.hook").json()["data"]

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
