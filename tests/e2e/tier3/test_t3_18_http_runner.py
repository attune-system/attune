"""T3.18: HTTP Request Action Execution Test."""

import json
import pytest
from helpers import AttuneClient
from helpers.fixtures import create_webhook_trigger, unique_ref
from helpers.polling import wait_for_event_count, wait_for_execution_status


def execute_http_request(client: AttuneClient, parameters: dict) -> dict:
    execution = client.execute_action(
        {"action": "core.http_request", "parameters": parameters}
    )
    return wait_for_execution_status(
        client=client,
        execution_id=execution["id"],
        expected_status="completed",
        timeout=30,
    )


@pytest.mark.tier3
@pytest.mark.runner
@pytest.mark.http
def test_http_runner_basic_get(client: AttuneClient, test_pack):
    print("\n" + "=" * 80)
    print("T3.18a: HTTP Request Basic GET Test")
    print("=" * 80)

    execution = execute_http_request(
        client,
        {
            "url": "http://api:8080/health",
            "method": "GET",
            "headers": {"User-Agent": "Attune-Test/1.0"},
            "timeout": 10,
        },
    )

    result = execution.get("result", {})
    assert result.get("status_code") == 200
    assert result.get("success") is True
    print(f"✓ HTTP GET succeeded: execution {execution['id']}")


@pytest.mark.tier3
@pytest.mark.runner
@pytest.mark.http
def test_http_runner_post_with_json(client: AttuneClient, test_pack):
    print("\n" + "=" * 80)
    print("T3.18b: HTTP Request POST with JSON Test")
    print("=" * 80)

    trigger = create_webhook_trigger(
        client=client,
        pack_ref=test_pack["ref"],
        trigger_ref=f"http_post_target_{unique_ref()}",
        description="Webhook target for HTTP request POST test",
    )

    execution = execute_http_request(
        client,
        {
            "url": f"http://api:8080{trigger['webhook_url']}",
            "method": "POST",
            "headers": {"Content-Type": "application/json"},
            "body": json.dumps({"payload": {"source": "http_request", "ok": True}}),
            "timeout": 10,
        },
    )

    result = execution.get("result", {})
    assert result.get("status_code") == 200
    assert result.get("success") is True
    events = wait_for_event_count(
        client,
        expected_count=1,
        trigger_ref=trigger["ref"],
        timeout=10,
        operator=">=",
    )
    assert events[0]["payload"]["source"] == "http_request"
    print(f"✓ HTTP POST created webhook event: {events[0]['id']}")


@pytest.mark.tier3
@pytest.mark.runner
@pytest.mark.http
def test_http_runner_authentication_header(client: AttuneClient, test_pack):
    print("\n" + "=" * 80)
    print("T3.18c: HTTP Request Header Test")
    print("=" * 80)

    execution = execute_http_request(
        client,
        {
            "url": "http://api:8080/health",
            "method": "GET",
            "headers": {
                "Authorization": "Bearer test-token",
                "User-Agent": "Attune-Test/1.0",
            },
            "timeout": 10,
        },
    )

    result = execution.get("result", {})
    assert result.get("status_code") == 200
    assert result.get("success") is True
    print(f"✓ HTTP request with custom headers succeeded: execution {execution['id']}")


@pytest.mark.tier3
@pytest.mark.runner
@pytest.mark.http
def test_http_runner_error_handling(client: AttuneClient, test_pack):
    print("\n" + "=" * 80)
    print("T3.18d: HTTP Request Error Handling Test")
    print("=" * 80)

    execution = client.execute_action(
        {
            "action": "core.http_request",
            "parameters": {
                "url": "http://api:8080/api/v1/does-not-exist",
                "method": "GET",
                "timeout": 10,
            },
        }
    )
    final_exec = wait_for_execution_status(
        client=client,
        execution_id=execution["id"],
        expected_status="failed",
        timeout=30,
    )

    result = final_exec.get("result", {})
    assert result["exit_code"] == 1
    result = json.loads(result["stdout"])
    assert result.get("status_code") == 404
    assert result.get("success") is False
    print(f"✓ HTTP 404 captured as failed execution: {execution['id']}")
