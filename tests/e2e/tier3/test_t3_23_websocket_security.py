"""T3.23: Notifier websocket security checks."""

import asyncio
import json
import os

import pytest
import websockets
from websockets.exceptions import InvalidStatus

from helpers import AttuneClient


pytestmark = [
    pytest.mark.tier3,
    pytest.mark.security,
    pytest.mark.websocket,
]


def _notifier_ws_url() -> str:
    base_url = os.getenv("ATTUNE_WS_URL", "ws://localhost:8081").rstrip("/")
    return f"{base_url}/ws"


def _status_code(exc: InvalidStatus) -> int | None:
    response = getattr(exc, "response", None)
    return getattr(response, "status_code", None)


async def _recv_json(websocket, timeout: float = 3.0) -> dict:
    return json.loads(await asyncio.wait_for(websocket.recv(), timeout=timeout))


@pytest.mark.notifications
def test_websocket_upgrade_rejects_missing_token():
    """Notifier websocket upgrades must reject unauthenticated clients."""

    async def run_test() -> None:
        with pytest.raises(InvalidStatus) as excinfo:
            async with websockets.connect(
                _notifier_ws_url(),
                subprotocols=["attune.v1"],
            ):
                pass

        assert _status_code(excinfo.value) == 401
        assert "server rejected WebSocket connection" in str(excinfo.value)

    asyncio.run(run_test())


@pytest.mark.notifications
def test_websocket_upgrade_rejects_invalid_token():
    """Notifier websocket upgrades must reject invalid bearer tokens."""

    async def run_test() -> None:
        with pytest.raises(InvalidStatus) as excinfo:
            async with websockets.connect(
                _notifier_ws_url(),
                additional_headers={"Authorization": "Bearer invalid.token.value"},
                subprotocols=["attune.v1"],
            ):
                pass

        assert _status_code(excinfo.value) == 401
        assert "server rejected WebSocket connection" in str(excinfo.value)

    asyncio.run(run_test())


@pytest.mark.notifications
def test_websocket_accepts_browser_subprotocol_auth(client: AttuneClient):
    """Browser-style subprotocol auth should work without an Authorization header."""

    async def run_test() -> None:
        async with websockets.connect(
            _notifier_ws_url(),
            subprotocols=["attune.v1", f"attune.jwt.{client.access_token}"],
        ) as websocket:
            assert websocket.subprotocol == "attune.v1"
            welcome = await _recv_json(websocket)
            assert welcome["type"] == "welcome"

    asyncio.run(run_test())


@pytest.mark.notifications
def test_websocket_denies_other_user_subscription(unique_user_client: AttuneClient):
    """Authenticated clients may not subscribe to another user's feed."""

    async def run_test() -> None:
        own_id = unique_user_client.user_info["id"]
        forbidden_user_id = own_id + 10_000

        async with websockets.connect(
            _notifier_ws_url(),
            additional_headers={"Authorization": f"Bearer {unique_user_client.access_token}"},
            subprotocols=["attune.v1"],
        ) as websocket:
            welcome = await _recv_json(websocket)
            assert welcome["type"] == "welcome"

            await websocket.send(
                json.dumps(
                    {"type": "subscribe", "filter": f"user:{forbidden_user_id}"}
                )
            )
            error = await _recv_json(websocket)
            assert error == {
                "type": "error",
                "message": "Unauthorized to subscribe to requested filter",
            }

    asyncio.run(run_test())


@pytest.mark.notifications
def test_websocket_allows_authenticated_execution_subscription(
    unique_user_client: AttuneClient,
):
    """Authenticated clients may subscribe to execution feeds; row visibility is enforced at delivery."""

    async def run_test() -> None:
        async with websockets.connect(
            _notifier_ws_url(),
            additional_headers={"Authorization": f"Bearer {unique_user_client.access_token}"},
            subprotocols=["attune.v1"],
        ) as websocket:
            welcome = await _recv_json(websocket)
            assert welcome["type"] == "welcome"

            await websocket.send(
                json.dumps({"type": "subscribe", "filter": "entity_type:execution"})
            )
            # Successful subscriptions don't emit an ack frame. We only fail if the
            # server returns an explicit error for this allowed collection filter.
            try:
                response = await _recv_json(websocket, timeout=1.0)
            except asyncio.TimeoutError:
                return

            assert response.get("type") != "error", response

    asyncio.run(run_test())
