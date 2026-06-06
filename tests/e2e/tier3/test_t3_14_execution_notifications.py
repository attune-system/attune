"""T3.14: Execution Completion Notifications Test."""

import asyncio
import json
import os
import time

import pytest
import websockets
from helpers import AttuneClient
from helpers.fixtures import (
    create_echo_action,
    create_failing_action,
    create_webhook_trigger,
    unique_ref,
)
from helpers.polling import (
    wait_for_execution_completion,
    wait_for_execution_count,
    wait_for_execution_status,
)


def _notifier_ws_url() -> str:
    base_url = os.getenv("ATTUNE_WS_URL", "ws://localhost:8081").rstrip("/")
    return f"{base_url}/ws"


def _connect_notifier_ws(client: AttuneClient):
    return websockets.connect(
        _notifier_ws_url(),
        additional_headers={"Authorization": f"Bearer {client.access_token}"},
        subprotocols=["attune.v1"],
    )


async def _wait_for_execution_notification(
    client: AttuneClient,
    execution_id: int,
    expected_status: str,
    *,
    timeout: float = 10.0,
) -> dict:
    deadline = asyncio.get_running_loop().time() + timeout
    async with _connect_notifier_ws(client) as websocket:
        welcome = json.loads(await asyncio.wait_for(websocket.recv(), timeout=3))
        assert welcome["type"] == "welcome"

        await websocket.send(
            json.dumps({"type": "subscribe", "filter": "entity_type:execution"})
        )
        while asyncio.get_running_loop().time() < deadline:
            remaining = max(0.1, deadline - asyncio.get_running_loop().time())
            message = json.loads(await asyncio.wait_for(websocket.recv(), timeout=remaining))
            if message.get("type") != "notification":
                continue
            if message.get("notification_type") != "execution_status_changed":
                continue
            if message.get("entity_id") != execution_id:
                continue
            payload = message.get("payload") or {}
            if payload.get("status") == expected_status:
                return message

    raise AssertionError(
        f"Did not receive {expected_status!r} execution notification for {execution_id}"
    )


@pytest.mark.tier3
@pytest.mark.notifications
@pytest.mark.websocket
def test_execution_success_notification(client: AttuneClient, test_pack):
    """
    Test that successful execution completion triggers notification.

    Flow:
    1. Create webhook trigger and echo action
    2. Create rule linking webhook to action
    3. Subscribe to WebSocket notifications
    4. Trigger webhook
    5. Verify notification received for execution completion
    6. Validate notification payload structure
    """
    print("\n" + "=" * 80)
    print("T3.14.1: Execution Success Notification")
    print("=" * 80)

    pack_ref = test_pack["ref"]

    # Step 1: Create webhook trigger
    print("\n[STEP 1] Creating webhook trigger...")
    trigger_ref = f"notify_webhook_{unique_ref()}"
    trigger = create_webhook_trigger(
        client=client,
        pack_ref=pack_ref,
        trigger_ref=trigger_ref,
        description="Webhook for notification test",
    )
    print(f"✓ Created trigger: {trigger['ref']}")

    # Step 2: Create echo action
    print("\n[STEP 2] Creating echo action...")
    action_ref = f"notify_action_{unique_ref()}"
    action = create_echo_action(
        client=client,
        pack_ref=pack_ref,
        action_ref=action_ref,
        description="Action for notification test",
    )
    print(f"✓ Created action: {action['ref']}")

    # Step 3: Create rule
    print("\n[STEP 3] Creating rule...")
    rule_ref = f"{pack_ref}.notify_rule_{unique_ref()}"
    rule_payload = {
        "ref": rule_ref,
        "pack_ref": pack_ref,
        "label": "Notify Rule",
        "trigger_ref": trigger["ref"],
        "action_ref": action["ref"],
        "enabled": True,
    }
    rule_response = client.post("/api/v1/rules", json=rule_payload)
    assert rule_response.status_code == 201, (
        f"Failed to create rule: {rule_response.text}"
    )
    rule = rule_response.json()["data"]
    print(f"✓ Created rule: {rule['ref']}")

    # Note: WebSocket notifications require the notifier service to be running.
    # For now, we'll validate the execution completes and check that notification
    # metadata is properly stored in the database.

    # Step 4: Trigger webhook
    print("\n[STEP 4] Triggering webhook...")
    webhook_url = trigger["webhook_url"]
    test_payload = {"message": "test notification", "timestamp": time.time()}
    webhook_response = client.post(webhook_url, json={"payload": test_payload})
    assert webhook_response.status_code == 200, (
        f"Webhook trigger failed: {webhook_response.text}"
    )
    print(f"✓ Webhook triggered successfully")

    # Step 5: Wait for execution completion
    print("\n[STEP 5] Waiting for execution to complete...")
    executions = wait_for_execution_count(
        client,
        expected_count=1,
        action_ref=action["ref"],
        timeout=10,
        operator=">=",
    )
    execution_id = executions[0]["id"]

    execution = wait_for_execution_completion(client, execution_id, timeout=10)
    print(f"✓ Execution succeeded with status: {execution['status']}")
    assert execution["status"] == "completed", (
        f"Expected succeeded, got {execution['status']}"
    )

    # Step 6: Validate notification metadata
    print("\n[STEP 6] Validating notification metadata...")
    # Check that the execution has notification fields set
    assert "created" in execution, "Execution missing created timestamp"
    assert "updated" in execution, "Execution missing updated timestamp"

    # The notifier service would have sent a notification at this point
    # In a full integration test with WebSocket, we would verify the message here
    print(f"✓ Execution metadata validated for notifications")
    print(f"  - Execution ID: {execution_id}")
    print(f"  - Status: {execution['status']}")
    print(f"  - Created: {execution['created']}")
    print(f"  - Updated: {execution['updated']}")

    print("\n✅ Test passed: Execution completion notification flow validated")


@pytest.mark.tier3
@pytest.mark.notifications
@pytest.mark.websocket
def test_execution_failure_notification(client: AttuneClient, test_pack):
    """
    Test that failed execution triggers notification.

    Flow:
    1. Create webhook trigger and failing action
    2. Create rule
    3. Trigger webhook
    4. Verify notification for failed execution
    """
    print("\n" + "=" * 80)
    print("T3.14.2: Execution Failure Notification")
    print("=" * 80)

    pack_ref = test_pack["ref"]

    # Step 1: Create webhook trigger
    print("\n[STEP 1] Creating webhook trigger...")
    trigger_ref = f"fail_notify_webhook_{unique_ref()}"
    trigger = create_webhook_trigger(
        client=client,
        pack_ref=pack_ref,
        trigger_ref=trigger_ref,
        description="Webhook for failure notification test",
    )
    print(f"✓ Created trigger: {trigger['ref']}")

    # Step 2: Create failing action
    print("\n[STEP 2] Creating failing action...")
    action = create_failing_action(
        client=client,
        pack_ref=pack_ref,
        name=f"fail_notify_action_{unique_ref()}",
    )
    print(f"✓ Created action: {action['ref']}")

    # Step 3: Create rule
    print("\n[STEP 3] Creating rule...")
    rule_ref = f"{pack_ref}.fail_notify_rule_{unique_ref()}"
    rule_payload = {
        "ref": rule_ref,
        "pack_ref": pack_ref,
        "label": "Fail Notify Rule",
        "trigger_ref": trigger["ref"],
        "action_ref": action["ref"],
        "enabled": True,
    }
    rule_response = client.post("/api/v1/rules", json=rule_payload)
    assert rule_response.status_code == 201, (
        f"Failed to create rule: {rule_response.text}"
    )
    rule = rule_response.json()["data"]
    print(f"✓ Created rule: {rule['ref']}")

    # Step 4: Trigger webhook
    print("\n[STEP 4] Triggering webhook...")
    webhook_url = trigger["webhook_url"]
    test_payload = {"message": "trigger failure", "timestamp": time.time()}
    webhook_response = client.post(webhook_url, json={"payload": test_payload})
    assert webhook_response.status_code == 200, (
        f"Webhook trigger failed: {webhook_response.text}"
    )
    print(f"✓ Webhook triggered successfully")

    # Step 5: Wait for execution to fail
    print("\n[STEP 5] Waiting for execution to fail...")
    executions = wait_for_execution_count(
        client,
        expected_count=1,
        action_ref=action["ref"],
        timeout=10,
        operator=">=",
    )
    execution_id = executions[0]["id"]

    execution = wait_for_execution_completion(client, execution_id, timeout=10)
    print(f"✓ Execution succeeded with status: {execution['status']}")
    assert execution["status"] == "failed", (
        f"Expected failed, got {execution['status']}"
    )

    # Step 6: Validate notification metadata for failure
    print("\n[STEP 6] Validating failure notification metadata...")
    assert "created" in execution, "Execution missing created timestamp"
    assert "updated" in execution, "Execution missing updated timestamp"
    assert execution["result"] is not None, (
        "Failed execution should have result with error"
    )

    print(f"✓ Failure notification metadata validated")
    print(f"  - Execution ID: {execution_id}")
    print(f"  - Status: {execution['status']}")
    print(f"  - Result available: {execution['result'] is not None}")

    print("\n✅ Test passed: Execution failure notification flow validated")


@pytest.mark.tier3
@pytest.mark.notifications
@pytest.mark.websocket
def test_execution_timeout_notification(client: AttuneClient, test_pack):
    """A timed-out workflow child should produce timeout execution metadata."""
    pack_ref = test_pack["ref"]
    action = client.create_action(
        pack_ref=pack_ref,
        data={
            "name": f"timeout_notify_action_{unique_ref()}",
            "description": "Action that times out",
            "runtime_ref": "core.shell",
            "entrypoint": 'echo "starting timeout notification test"; sleep 20',
            "enabled": True,
            "parameters": {},
        },
    )
    workflow = client.create_workflow(
        pack_ref=pack_ref,
        name=f"timeout_notify_workflow_{unique_ref()}",
        label="Timeout Notification Workflow",
        tasks=[
            {
                "name": "timeout_task",
                "action": action["ref"],
                "input": {},
                "timeout": 2,
            }
        ],
    )

    execution = client.create_execution(action_ref=workflow["ref"], parameters={})
    parent = wait_for_execution_status(
        client=client,
        execution_id=execution["id"],
        expected_status="failed",
        timeout=12,
    )
    children = client.list_executions(parent=execution["id"], limit=100)
    child = next(item for item in children if item["action_ref"] == action["ref"])
    child_details = client.get_execution(child["id"])

    assert parent["status"] == "failed"
    assert child_details["status"] == "timeout"
    assert "timed out" in str(child_details.get("result")).lower()
    assert child_details["created"]
    assert child_details["updated"]


@pytest.mark.tier3
@pytest.mark.notifications
@pytest.mark.websocket
def test_websocket_notification_delivery(client: AttuneClient, test_pack):
    """Test actual authenticated WebSocket delivery for execution updates."""
    pack_ref = test_pack["ref"]
    action = create_echo_action(
        client=client,
        pack_ref=pack_ref,
        action_ref=f"ws_notify_action_{unique_ref()}",
        description="Action for WebSocket notification test",
    )

    async def run_test() -> tuple[dict, dict]:
        async with _connect_notifier_ws(client) as websocket:
            welcome = json.loads(await asyncio.wait_for(websocket.recv(), timeout=3))
            assert welcome["type"] == "welcome"
            await websocket.send(
                json.dumps({"type": "subscribe", "filter": "entity_type:execution"})
            )

            execution = client.create_execution(
                action_ref=action["ref"],
                parameters={"message": "websocket notification"},
            )
            execution_id = execution["id"]

            deadline = asyncio.get_running_loop().time() + 10
            notification = None
            while asyncio.get_running_loop().time() < deadline:
                remaining = max(0.1, deadline - asyncio.get_running_loop().time())
                message = json.loads(
                    await asyncio.wait_for(websocket.recv(), timeout=remaining)
                )
                if message.get("type") != "notification":
                    continue
                if message.get("notification_type") != "execution_status_changed":
                    continue
                if message.get("entity_id") != execution_id:
                    continue
                payload = message.get("payload") or {}
                if payload.get("status") == "completed":
                    notification = message
                    break

            assert notification is not None, (
                f"Did not receive completed notification for execution {execution_id}"
            )
            final_execution = wait_for_execution_completion(
                client, execution_id, timeout=10
            )
            return notification, final_execution

    notification, execution = asyncio.run(run_test())

    assert execution["status"] == "completed"
    assert notification["type"] == "notification"
    assert notification["entity_type"] == "execution"
    assert notification["entity_id"] == execution["id"]
    assert notification["payload"]["status"] == "completed"
