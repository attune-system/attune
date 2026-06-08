"""
T3.21: Secret-safe operational metadata access

Validates that operational read permissions expose useful non-secret metadata while
secret values in events, enforcements, and executions stay redacted unless the
caller has the matching decrypt permission.
"""

import os

import psycopg
import pytest
from psycopg import sql
from psycopg.types.json import Jsonb

from helpers import (
    AttuneClient,
    unique_ref,
    wait_for_condition,
    wait_for_execution_count,
)


SECRET_MARKER_KEY = "$attune_secret"


def _data(response):
    assert response.status_code in (200, 201), response.text
    body = response.json()
    return body.get("data", body) if isinstance(body, dict) else body


def _register_user_with_grants(
    admin_client: AttuneClient,
    *,
    prefix: str,
    grants: list[dict],
    api_base_url: str,
    test_timeout: int,
) -> AttuneClient:
    login = f"{prefix}_{unique_ref()}@attune.local"
    password = f"{prefix}_Password123!"
    permset_ref = f"e2e.{prefix}_{unique_ref()}"

    registration = admin_client.register(
        login=login,
        password=password,
        display_name=f"E2E {prefix.title()}",
    )
    identity_id = registration["user"]["id"]
    _assign_direct_permission_set(identity_id, permset_ref, grants)

    user_client = AttuneClient(
        base_url=api_base_url,
        timeout=test_timeout,
        auto_login=False,
    )
    user_client.login(login=login, password=password)
    return user_client


def _assign_direct_permission_set(identity_id: int, permset_ref: str, grants: list[dict]) -> None:
    db_url = os.getenv(
        "DATABASE_URL", "postgresql://attune:attune@postgres:5432/attune"
    )
    schema = os.getenv("ATTUNE__DATABASE__SCHEMA", "attune")
    with psycopg.connect(db_url) as conn:
        with conn.cursor() as cur:
            cur.execute(
                sql.SQL("SET search_path TO {}, public").format(sql.Identifier(schema))
            )
            cur.execute(
                """
                INSERT INTO permission_set (ref, label, description, grants)
                VALUES (%s, %s, %s, %s)
                RETURNING id
                """,
                (
                    permset_ref,
                    f"E2E {permset_ref}",
                    "Created by secret-safe metadata e2e tests",
                    Jsonb(grants),
                ),
            )
            permission_set_id = cur.fetchone()[0]
            cur.execute(
                """
                INSERT INTO permission_assignment (identity, permset)
                VALUES (%s, %s)
                """,
                (identity_id, permission_set_id),
            )
        conn.commit()


def _get_event(
    client: AttuneClient,
    event_id: int,
    *,
    include_secret_values: bool = False,
) -> dict:
    params = {"include_secret_values": "true"} if include_secret_values else None
    return _data(client._request("GET", f"/api/v1/events/{event_id}", params=params))


def _get_enforcement(
    client: AttuneClient,
    enforcement_id: int,
    *,
    include_secret_values: bool = False,
) -> dict:
    params = {"include_secret_values": "true"} if include_secret_values else None
    return _data(
        client._request("GET", f"/api/v1/enforcements/{enforcement_id}", params=params)
    )


def _get_execution(
    client: AttuneClient,
    execution_id: int,
    *,
    include_secret_values: bool = False,
) -> dict:
    params = {"include_secret_values": "true"} if include_secret_values else None
    return _data(
        client._request("GET", f"/api/v1/executions/{execution_id}", params=params)
    )


def _is_secret_marker(value) -> bool:
    return isinstance(value, dict) and value.get(SECRET_MARKER_KEY) is True


def _wait_for_event(client: AttuneClient, trigger_ref: str) -> dict:
    event = {}

    def found_event() -> bool:
        nonlocal event
        events = client.list_events(trigger_ref=trigger_ref, limit=20, enrich=False)
        if not events:
            return False
        event = events[0]
        return True

    wait_for_condition(
        found_event,
        timeout=15,
        error_message=f"Event for trigger {trigger_ref} was not created",
    )
    return _get_event(client, event["id"])


@pytest.mark.tier3
@pytest.mark.security
@pytest.mark.secrets
@pytest.mark.rbac
@pytest.mark.webhook
def test_event_payload_secrets_require_event_decrypt(
    client: AttuneClient,
    test_pack,
    api_base_url: str,
    test_timeout: int,
):
    pack_ref = test_pack["ref"]
    trigger_name = f"secret_event_{unique_ref()}"
    trigger_ref = f"{pack_ref}.{trigger_name}"
    trigger = client.create_trigger(
        ref=trigger_ref,
        label="Secret Event Payload",
        pack_ref=pack_ref,
        description="E2E trigger with a secret payload field",
        param_schema={
            "account": {"type": "string"},
            "token": {"type": "string", "secret": True},
        },
    )
    trigger = client.enable_webhook(trigger_ref=trigger["ref"])
    webhook_url = f"/api/v1/webhooks/{trigger['webhook_key']}"

    client.post_webhook(
        webhook_url,
        payload={
            "account": "customer-123",
            "token": "event-token-secret",
        },
    )
    created_event = _wait_for_event(client, trigger_ref)

    reader = _register_user_with_grants(
        client,
        prefix="operational_reader",
        api_base_url=api_base_url,
        test_timeout=test_timeout,
        grants=[
            {
                "resource": "events",
                "actions": ["read"],
            }
        ],
    )
    decrypt_reader = _register_user_with_grants(
        client,
        prefix="event_decrypt_reader",
        api_base_url=api_base_url,
        test_timeout=test_timeout,
        grants=[
            {
                "resource": "events",
                "actions": ["read", "decrypt"],
            }
        ],
    )

    redacted_event = _get_event(reader, created_event["id"])
    assert redacted_event["payload"]["account"] == "customer-123"
    assert _is_secret_marker(redacted_event["payload"]["token"])

    forbidden = reader._request(
        "GET",
        f"/api/v1/events/{created_event['id']}",
        params={"include_secret_values": "true"},
    )
    assert forbidden.status_code == 403, forbidden.text

    revealed_event = _get_event(
        decrypt_reader,
        created_event["id"],
        include_secret_values=True,
    )
    assert revealed_event["payload"] == {
        "account": "customer-123",
        "token": "event-token-secret",
    }


@pytest.mark.tier3
@pytest.mark.security
@pytest.mark.secrets
@pytest.mark.rbac
@pytest.mark.webhook
def test_event_to_enforcement_to_execution_keeps_readable_non_secret_mappings_redacted(
    client: AttuneClient,
    test_pack,
    api_base_url: str,
    test_timeout: int,
):
    pack_ref = test_pack["ref"]
    trigger_name = f"secret_mapping_{unique_ref()}"
    trigger_ref = f"{pack_ref}.{trigger_name}"
    action_ref = f"{pack_ref}.secret_mapping_action_{unique_ref()}"

    trigger = client.create_trigger(
        ref=trigger_ref,
        label="Secret Mapping Trigger",
        pack_ref=pack_ref,
        description="E2E trigger with a mapped secret payload field",
        param_schema={
            "service": {"type": "string"},
            "api_key": {"type": "string", "secret": True},
        },
    )
    trigger = client.enable_webhook(trigger_ref=trigger["ref"])
    webhook_url = f"/api/v1/webhooks/{trigger['webhook_key']}"

    action = client.create_action(
        ref=action_ref,
        label="Secret Mapping Action",
        pack_ref=pack_ref,
        description="E2E action with a secret input parameter",
        runtime_ref="core.shell",
        entrypoint='echo "service=$service"',
        param_schema={
            "service": {"type": "string"},
            "api_key": {"type": "string", "secret": True},
        },
    )

    rule = client.create_rule(
        pack_ref=pack_ref,
        name=f"secret_mapping_rule_{unique_ref()}",
        trigger_ref=trigger_ref,
        action_ref=action["ref"],
        enabled=True,
        action_params={
            "service": "{{ event.payload.service }}",
            "api_key": "{{ event.payload.api_key }}",
        },
    )

    reader = _register_user_with_grants(
        client,
        prefix="mapping_reader",
        api_base_url=api_base_url,
        test_timeout=test_timeout,
        grants=[
            {
                "resource": "events",
                "actions": ["read"],
            },
            {
                "resource": "enforcements",
                "actions": ["read"],
            },
            {
                "resource": "executions",
                "actions": ["read"],
            },
        ],
    )
    decrypt_reader = _register_user_with_grants(
        client,
        prefix="mapping_decrypt_reader",
        api_base_url=api_base_url,
        test_timeout=test_timeout,
        grants=[
            {
                "resource": "events",
                "actions": ["read", "decrypt"],
            },
            {
                "resource": "enforcements",
                "actions": ["read", "decrypt"],
            },
            {
                "resource": "executions",
                "actions": ["read", "decrypt"],
            },
        ],
    )

    client.post_webhook(
        webhook_url,
        payload={
            "service": "billing",
            "api_key": "mapped-secret-key",
        },
    )
    event = _wait_for_event(client, trigger_ref)

    enforcements = []

    def found_enforcement() -> bool:
        nonlocal enforcements
        enforcements = client.list_enforcements(rule_ref=rule["ref"], limit=20)
        return len(enforcements) >= 1

    wait_for_condition(
        found_enforcement,
        timeout=20,
        error_message="Enforcement was not created for secret mapping rule",
    )
    enforcement = enforcements[0]

    executions = wait_for_execution_count(
        client=client,
        expected_count=1,
        action_ref=action["ref"],
        timeout=20,
        operator=">=",
    )
    execution = executions[0]

    redacted_event = _get_event(reader, event["id"])
    assert redacted_event["payload"]["service"] == "billing"
    assert _is_secret_marker(redacted_event["payload"]["api_key"])

    redacted_enforcement = _get_enforcement(reader, enforcement["id"])
    assert redacted_enforcement["payload"]["service"] == "billing"
    assert _is_secret_marker(redacted_enforcement["payload"]["api_key"])
    assert redacted_enforcement["config"]["service"] == "billing"
    assert _is_secret_marker(redacted_enforcement["config"]["api_key"])

    redacted_execution = _get_execution(reader, execution["id"])
    assert redacted_execution["config"]["service"] == "billing"
    assert _is_secret_marker(redacted_execution["config"]["api_key"])

    for path in (
        f"/api/v1/enforcements/{enforcement['id']}",
        f"/api/v1/executions/{execution['id']}",
    ):
        forbidden = reader._request(
            "GET",
            path,
            params={"include_secret_values": "true"},
        )
        assert forbidden.status_code == 403, forbidden.text

    revealed_event = _get_event(decrypt_reader, event["id"], include_secret_values=True)
    assert revealed_event["payload"]["api_key"] == "mapped-secret-key"

    revealed_enforcement = _get_enforcement(
        decrypt_reader,
        enforcement["id"],
        include_secret_values=True,
    )
    assert revealed_enforcement["payload"]["service"] == "billing"
    assert _is_secret_marker(revealed_enforcement["payload"]["api_key"])
    assert revealed_enforcement["config"]["api_key"] == "mapped-secret-key"

    revealed_execution = _get_execution(
        decrypt_reader,
        execution["id"],
        include_secret_values=True,
    )
    assert revealed_execution["config"]["api_key"] == "mapped-secret-key"
