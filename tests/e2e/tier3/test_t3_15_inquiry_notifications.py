"""T3.15: inquiry notifications from production-path workflow creation."""

import asyncio
import json
import os

import pytest
import websockets

from helpers import AttuneClient, start_inquiry_workflow, unique_ref


pytestmark = [pytest.mark.tier3, pytest.mark.notifications, pytest.mark.inquiry]


def _notifier_ws_url() -> str:
    base_url = os.getenv("ATTUNE_WS_URL", "ws://localhost:8081").rstrip("/")
    return f"{base_url}/ws"


def _connect_notifier_ws(client: AttuneClient):
    return websockets.connect(
        _notifier_ws_url(),
        additional_headers={"Authorization": f"Bearer {client.access_token}"},
        subprotocols=["attune.v1"],
    )


async def _subscribe_to_inquiries(websocket) -> None:
    """Wait until the server has processed the inquiry subscription."""
    await websocket.send(
        json.dumps({"type": "subscribe", "filter": "entity_type:inquiry"})
    )
    # The protocol has no subscription acknowledgement. Incoming messages are
    # processed in order, so this error confirms the preceding subscribe ran.
    await websocket.send(
        json.dumps({"type": "subscribe", "filter": "invalid-filter"})
    )
    deadline = asyncio.get_running_loop().time() + 3
    while asyncio.get_running_loop().time() < deadline:
        remaining = max(0.1, deadline - asyncio.get_running_loop().time())
        response = json.loads(
            await asyncio.wait_for(websocket.recv(), timeout=remaining)
        )
        if response.get("type") == "error":
            return
    raise AssertionError("Notifier did not confirm subscription processing")


async def _wait_for_inquiry_notification(
    websocket,
    *,
    notification_type: str,
    inquiry_id: int,
    timeout: float = 10.0,
) -> dict:
    deadline = asyncio.get_running_loop().time() + timeout
    while asyncio.get_running_loop().time() < deadline:
        remaining = max(0.1, deadline - asyncio.get_running_loop().time())
        message = json.loads(await asyncio.wait_for(websocket.recv(), timeout=remaining))
        if (
            message.get("type") == "notification"
            and message.get("notification_type") == notification_type
            and message.get("entity_type") == "inquiry"
            and message.get("entity_id") == inquiry_id
        ):
            return message

    raise AssertionError(
        f"Did not receive {notification_type!r} notification for inquiry {inquiry_id}"
    )


def test_inquiry_creation_has_workflow_metadata(
    client: AttuneClient, test_pack: dict
):
    run = start_inquiry_workflow(
        client,
        test_pack["ref"],
        purpose=f"notification-metadata-{unique_ref()}",
        prompt="Approve the notification metadata test?",
    )

    assert run.inquiry["created_by_execution"] == run.creator_execution["id"]
    assert run.inquiry["workflow_execution"] is not None
    assert run.inquiry["workflow_task_name"] == "request_inquiry"
    assert run.inquiry["status"] == "pending"
    assert run.inquiry["created"]
    assert run.inquiry["updated"]


@pytest.mark.websocket
def test_websocket_delivers_inquiry_created_and_responded_notifications(
    client: AttuneClient, test_pack: dict
):
    async def run_test() -> tuple[dict, dict, dict]:
        async with _connect_notifier_ws(client) as websocket:
            welcome = json.loads(await asyncio.wait_for(websocket.recv(), timeout=3))
            assert welcome["type"] == "welcome"
            await _subscribe_to_inquiries(websocket)

            run = start_inquiry_workflow(
                client,
                test_pack["ref"],
                purpose=f"websocket-response-{unique_ref()}",
                prompt="Approve the WebSocket notification test?",
            )
            created = await _wait_for_inquiry_notification(
                websocket,
                notification_type="inquiry_created",
                inquiry_id=run.inquiry["id"],
            )

            client.respond_to_inquiry(
                run.inquiry["id"], response={"approved": True}
            )
            responded = await _wait_for_inquiry_notification(
                websocket,
                notification_type="inquiry_responded",
                inquiry_id=run.inquiry["id"],
            )
            return run.inquiry, created, responded

    inquiry, created, responded = asyncio.run(run_test())

    assert created["payload"]["status"] == "pending"
    assert created["payload"]["created_by_execution"] == inquiry[
        "created_by_execution"
    ]
    assert responded["payload"]["status"] == "responded"
    assert responded["payload"]["created_by_execution"] == inquiry[
        "created_by_execution"
    ]


@pytest.mark.websocket
def test_websocket_delivers_inquiry_timeout_notification(
    client: AttuneClient, test_pack: dict
):
    async def run_test() -> tuple[dict, dict]:
        async with _connect_notifier_ws(client) as websocket:
            welcome = json.loads(await asyncio.wait_for(websocket.recv(), timeout=3))
            assert welcome["type"] == "welcome"
            await _subscribe_to_inquiries(websocket)

            run = start_inquiry_workflow(
                client,
                test_pack["ref"],
                purpose=f"websocket-timeout-{unique_ref()}",
                prompt="This notification inquiry should time out",
                timeout_seconds=2,
            )
            notification = await _wait_for_inquiry_notification(
                websocket,
                notification_type="inquiry_timeout",
                inquiry_id=run.inquiry["id"],
                timeout=15,
            )
            return run.inquiry, notification

    inquiry, notification = asyncio.run(run_test())
    timed_out = client.get_inquiry(inquiry["id"])

    assert timed_out["status"] == "timeout"
    assert notification["payload"]["status"] == "timeout"
    assert notification["payload"]["created_by_execution"] == inquiry[
        "created_by_execution"
    ]
    with pytest.raises(Exception, match="(timeout|409|400|responded|terminal)"):
        client.respond_to_inquiry(inquiry["id"], response={"approved": True})
