"""
API Tests: Webhook Management & Receiving

Ported from crates/api/tests/webhook_api_tests.rs
Tests webhook enable/disable/regenerate and webhook receive endpoints.
"""

import uuid

import pytest
import requests


def _uid():
    return uuid.uuid4().hex[:8]


@pytest.mark.api
class TestWebhookEnable:
    """Test enabling webhooks on triggers."""

    def test_enable_webhook(self, client):
        """Enable webhook on a trigger and verify response structure."""
        uid = _uid()
        pack_ref = f"whtest_{uid}"
        trigger_ref = f"{pack_ref}.trigger"

        # Create pack and trigger
        client.create_pack(ref=pack_ref, label=f"Webhook Test {uid}")
        client.create_trigger(pack_ref=pack_ref, ref=trigger_ref, label="WH Trigger")

        # Enable webhook
        resp = client.post(f"/api/v1/triggers/{trigger_ref}/webhooks/enable")
        data = resp["data"]

        assert data["webhook_enabled"] is True
        assert isinstance(data["webhook_key"], str)
        assert data["webhook_key"].startswith("wh_")

    def test_enable_webhook_requires_auth(self, api_base_url, client):
        """Webhook management requires authentication."""
        uid = _uid()
        pack_ref = f"whauth_{uid}"
        trigger_ref = f"{pack_ref}.trigger"

        client.create_pack(ref=pack_ref, label=f"Auth Test {uid}")
        client.create_trigger(pack_ref=pack_ref, ref=trigger_ref, label="Auth Trigger")

        # Try without auth
        s = requests.Session()
        resp = s.post(
            f"{api_base_url}/api/v1/triggers/{trigger_ref}/webhooks/enable",
            timeout=10,
        )
        assert resp.status_code == 401


@pytest.mark.api
class TestWebhookDisable:
    """Test disabling webhooks on triggers."""

    def test_disable_webhook(self, client):
        """Disable an enabled webhook."""
        uid = _uid()
        pack_ref = f"whdis_{uid}"
        trigger_ref = f"{pack_ref}.trigger"

        client.create_pack(ref=pack_ref, label=f"Disable Test {uid}")
        client.create_trigger(pack_ref=pack_ref, ref=trigger_ref, label="Dis Trigger")

        # Enable first
        client.post(f"/api/v1/triggers/{trigger_ref}/webhooks/enable")

        # Disable
        resp = client.post(f"/api/v1/triggers/{trigger_ref}/webhooks/disable")
        data = resp["data"]

        assert data["webhook_enabled"] is False
        assert data.get("webhook_key") is None


@pytest.mark.api
class TestWebhookRegenerate:
    """Test regenerating webhook keys."""

    def test_regenerate_webhook_key(self, client):
        """Regenerate key produces a new key different from original."""
        uid = _uid()
        pack_ref = f"whregen_{uid}"
        trigger_ref = f"{pack_ref}.trigger"

        client.create_pack(ref=pack_ref, label=f"Regen Test {uid}")
        client.create_trigger(pack_ref=pack_ref, ref=trigger_ref, label="Regen Trigger")

        # Enable and capture original key
        enable_resp = client.post(f"/api/v1/triggers/{trigger_ref}/webhooks/enable")
        original_key = enable_resp["data"]["webhook_key"]

        # Regenerate
        regen_resp = client.post(f"/api/v1/triggers/{trigger_ref}/webhooks/regenerate")
        new_key = regen_resp["data"]["webhook_key"]

        assert new_key != original_key
        assert new_key.startswith("wh_")

    def test_regenerate_webhook_key_not_enabled(self, client):
        """Regenerating without enabling first returns 400."""
        uid = _uid()
        pack_ref = f"whnotenabled_{uid}"
        trigger_ref = f"{pack_ref}.trigger"

        client.create_pack(ref=pack_ref, label=f"Not Enabled Test {uid}")
        client.create_trigger(
            pack_ref=pack_ref, ref=trigger_ref, label="Not Enabled Trigger"
        )

        # Try to regenerate without enabling
        resp = client.session.post(
            f"{client.base_url}/api/v1/triggers/{trigger_ref}/webhooks/regenerate",
            timeout=10,
        )
        assert resp.status_code == 400


@pytest.mark.api
class TestWebhookReceive:
    """Test receiving webhooks."""

    def test_receive_webhook(self, client):
        """Send a webhook and verify event creation."""
        uid = _uid()
        pack_ref = f"whrecv_{uid}"
        trigger_ref = f"{pack_ref}.trigger"

        client.create_pack(ref=pack_ref, label=f"Receive Test {uid}")
        client.create_trigger(pack_ref=pack_ref, ref=trigger_ref, label="Recv Trigger")

        # Enable webhook
        enable_resp = client.post(f"/api/v1/triggers/{trigger_ref}/webhooks/enable")
        webhook_key = enable_resp["data"]["webhook_key"]

        # Send webhook (no auth required)
        payload = {
            "payload": {
                "event": "test_event",
                "data": {"foo": "bar", "number": 42},
            },
            "headers": {"X-Test-Header": "test-value"},
            "source_ip": "192.168.1.1",
            "user_agent": "Test Agent/1.0",
        }

        s = requests.Session()
        resp = s.post(
            f"{client.base_url}/api/v1/webhooks/{webhook_key}",
            json=payload,
            timeout=10,
        )
        assert resp.status_code == 200

        body = resp.json()
        assert body["data"]["event_id"] is not None
        assert body["data"]["trigger_ref"] == trigger_ref
        assert isinstance(body["data"]["received_at"], str)
        assert body["data"]["message"] == "Webhook received successfully"

    def test_receive_webhook_invalid_key(self, api_base_url):
        """Invalid webhook key returns 404."""
        s = requests.Session()
        resp = s.post(
            f"{api_base_url}/api/v1/webhooks/wh_invalid_key_12345",
            json={"payload": {"event": "test_event"}},
            timeout=10,
        )
        assert resp.status_code == 404

    def test_receive_webhook_disabled(self, client):
        """Sending to a disabled webhook returns 404."""
        uid = _uid()
        pack_ref = f"whdisrecv_{uid}"
        trigger_ref = f"{pack_ref}.trigger"

        client.create_pack(ref=pack_ref, label=f"Disabled Recv Test {uid}")
        client.create_trigger(
            pack_ref=pack_ref, ref=trigger_ref, label="Disabled Recv Trigger"
        )

        # Enable then disable
        enable_resp = client.post(f"/api/v1/triggers/{trigger_ref}/webhooks/enable")
        webhook_key = enable_resp["data"]["webhook_key"]
        client.post(f"/api/v1/triggers/{trigger_ref}/webhooks/disable")

        # Try to send
        s = requests.Session()
        resp = s.post(
            f"{client.base_url}/api/v1/webhooks/{webhook_key}",
            json={"payload": {"event": "test_event"}},
            timeout=10,
        )
        assert resp.status_code == 404

    def test_receive_webhook_minimal_payload(self, client):
        """Webhook with minimal payload succeeds."""
        uid = _uid()
        pack_ref = f"whmin_{uid}"
        trigger_ref = f"{pack_ref}.trigger"

        client.create_pack(ref=pack_ref, label=f"Minimal Test {uid}")
        client.create_trigger(pack_ref=pack_ref, ref=trigger_ref, label="Min Trigger")

        enable_resp = client.post(f"/api/v1/triggers/{trigger_ref}/webhooks/enable")
        webhook_key = enable_resp["data"]["webhook_key"]

        s = requests.Session()
        resp = s.post(
            f"{client.base_url}/api/v1/webhooks/{webhook_key}",
            json={"payload": {"message": "minimal test"}},
            timeout=10,
        )
        assert resp.status_code == 200
