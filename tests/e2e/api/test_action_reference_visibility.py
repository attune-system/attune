#!/usr/bin/env python3
"""
End-to-end tests for action reference visibility.

These tests exercise the public API behavior for action reference visibility:
rules, workflows, and queues must respect private/restricted action settings, and
action discovery must hide actions that are not visible to the requesting pack or
caller.

Run with: pytest tests/e2e/api/test_action_reference_visibility.py -v -s
"""

import os
import time
import uuid
from typing import Any, Dict, Iterable, Optional

import pytest
import requests


API_BASE_URL = os.getenv("ATTUNE_API_URL", "http://localhost:8080")


class AttuneClient:
    def __init__(self, base_url: str):
        self.base_url = base_url.rstrip("/")
        self.session = requests.Session()
        self.token: Optional[str] = None

    def login(self, login: str, password: str) -> str:
        response = self.session.post(
            f"{self.base_url}/auth/login",
            json={"login": login, "password": password},
        )
        response.raise_for_status()
        self.token = response.json()["data"]["access_token"]
        self.session.headers.update({"Authorization": f"Bearer {self.token}"})
        return self.token

    def register(self, login: str, password: str) -> str:
        response = self.session.post(
            f"{self.base_url}/auth/register",
            json={
                "login": login,
                "password": password,
                "display_name": f"E2E {login}",
            },
        )
        response.raise_for_status()
        self.token = response.json()["data"]["access_token"]
        self.session.headers.update({"Authorization": f"Bearer {self.token}"})
        return self.token

    def request(self, method: str, path: str, **kwargs) -> requests.Response:
        response = self.session.request(method, f"{self.base_url}{path}", **kwargs)
        return response

    def get(self, path: str, **kwargs) -> Dict[str, Any]:
        response = self.request("GET", path, **kwargs)
        response.raise_for_status()
        return response.json()

    def post(self, path: str, **kwargs) -> Dict[str, Any]:
        response = self.request("POST", path, **kwargs)
        response.raise_for_status()
        return response.json()

    def put(self, path: str, **kwargs) -> Dict[str, Any]:
        response = self.request("PUT", path, **kwargs)
        response.raise_for_status()
        return response.json()


def unique_suffix() -> str:
    return f"{int(time.time() * 1000)}_{uuid.uuid4().hex[:8]}"


def login_with_first_available(candidates: Iterable[tuple[str, str]]) -> AttuneClient:
    client = AttuneClient(API_BASE_URL)
    last_error: Optional[Exception] = None
    for login, password in candidates:
        try:
            client.login(login, password)
            return client
        except requests.HTTPError as exc:
            last_error = exc

    pytest.skip(f"No configured e2e login succeeded: {last_error}")


@pytest.fixture(scope="session")
def admin_client() -> AttuneClient:
    configured_login = os.getenv("ATTUNE_E2E_LOGIN")
    configured_password = os.getenv("ATTUNE_E2E_PASSWORD")
    candidates = []
    if configured_login and configured_password:
        candidates.append((configured_login, configured_password))
    candidates.extend(
        [
            ("test@attune.local", "TestPass123!"),
            ("admin@attune.local", "AdminPass123!"),
        ]
    )
    return login_with_first_available(candidates)


@pytest.fixture
def unprivileged_client(admin_client: AttuneClient) -> AttuneClient:
    client = AttuneClient(API_BASE_URL)
    login = f"visibility_reader_{unique_suffix()}@attune.local"
    password = "TestPassword123!"
    try:
        client.register(login, password)
    except requests.HTTPError as exc:
        if exc.response is None or exc.response.status_code != 403:
            raise

        admin_client.post(
            "/api/v1/identities",
            json={
                "login": login,
                "display_name": f"E2E {login}",
                "password": password,
                "attributes": {"test": "action_reference_visibility"},
            },
        )
        client.login(login, password)
    return client


def assert_error_contains(response: requests.Response, expected: str) -> None:
    assert response.status_code in {400, 403, 404}, response.text
    assert expected.lower() in response.text.lower()


def create_pack(client: AttuneClient, pack_ref: str) -> Dict[str, Any]:
    return client.post(
        "/api/v1/packs",
        json={
            "ref": pack_ref,
            "label": f"E2E {pack_ref}",
            "description": "Action visibility e2e pack",
            "version": "1.0.0",
            "conf_schema": {},
            "config": {},
            "meta": {"test": "action_reference_visibility"},
            "tags": ["e2e", "visibility"],
        },
    )["data"]


def create_action(
    client: AttuneClient,
    pack_ref: str,
    name: str,
    *,
    visibility: str = "public",
    allowed_pack_refs: Optional[list[str]] = None,
) -> Dict[str, Any]:
    return client.post(
        "/api/v1/actions",
        json={
            "ref": f"{pack_ref}.{name}",
            "pack_ref": pack_ref,
            "label": f"E2E {name}",
            "description": "Action visibility e2e action",
            "entrypoint": "actions/echo.py",
            "param_schema": {
                "message": {
                    "type": "string",
                    "required": False,
                    "description": "Message to echo",
                }
            },
            "reference_visibility": visibility,
            "reference_allowed_pack_refs": allowed_pack_refs or [],
        },
    )["data"]


def create_trigger(client: AttuneClient, pack_ref: str, name: str) -> Dict[str, Any]:
    return client.post(
        "/api/v1/triggers",
        json={
            "ref": f"{pack_ref}.{name}",
            "pack_ref": pack_ref,
            "label": f"E2E {name}",
            "description": "Action visibility e2e trigger",
            "enabled": True,
            "param_schema": {},
            "out_schema": {},
        },
    )["data"]


def create_rule_response(
    client: AttuneClient,
    pack_ref: str,
    name: str,
    action_ref: str,
    trigger_ref: str,
) -> requests.Response:
    return client.request(
        "POST",
        "/api/v1/rules",
        json={
            "ref": f"{pack_ref}.{name}",
            "pack_ref": pack_ref,
            "label": f"E2E {name}",
            "description": "Action visibility e2e rule",
            "action_ref": action_ref,
            "trigger_ref": trigger_ref,
            "conditions": {},
            "action_params": {"message": "hello"},
            "trigger_params": {},
            "enabled": True,
        },
    )


def create_workflow_response(
    client: AttuneClient,
    pack_ref: str,
    name: str,
    action_ref: str,
) -> requests.Response:
    return client.request(
        "POST",
        "/api/v1/workflows",
        json={
            "ref": f"{pack_ref}.{name}",
            "pack_ref": pack_ref,
            "label": f"E2E {name}",
            "description": "Action visibility e2e workflow",
            "version": "1.0.0",
            "param_schema": {},
            "out_schema": {},
            "definition": {
                "version": "1.0.0",
                "tasks": [
                    {
                        "name": "call_target",
                        "action": action_ref,
                        "input": {"message": "hello"},
                    }
                ],
            },
            "tags": ["e2e", "visibility"],
        },
    )


def create_queue_response(
    client: AttuneClient,
    pack_ref: str,
    name: str,
    action_ref: str,
) -> requests.Response:
    return client.request(
        "POST",
        "/api/v1/queues",
        json={
            "ref": f"{pack_ref}.{name}",
            "pack_ref": pack_ref,
            "label": f"E2E {name}",
            "description": "Action visibility e2e queue",
            "enabled": True,
            "accepting_new_items": True,
            "dispatch_action_ref": action_ref,
            "default_priority": 0,
            "allow_pending_update": False,
            "update_strategy": "replace",
            "batch_mode": "single",
            "item_schema": {},
            "action_params": {"message": "{{ item.message }}"},
            "config": {},
        },
    )


def action_refs_from_list(client: AttuneClient, params: Optional[Dict[str, str]] = None) -> set[str]:
    query = dict(params or {})
    body = client.get("/api/v1/actions/search", params=query)
    return {item["ref"] for item in body["items"]}


@pytest.fixture
def visibility_resources(admin_client: AttuneClient) -> Dict[str, Any]:
    suffix = unique_suffix()
    owner_pack = f"vis_owner_{suffix}"
    allowed_pack = f"vis_allowed_{suffix}"
    blocked_pack = f"vis_blocked_{suffix}"

    for pack_ref in [owner_pack, allowed_pack, blocked_pack]:
        create_pack(admin_client, pack_ref)

    private_action = create_action(
        admin_client,
        owner_pack,
        "private_action",
        visibility="private",
    )
    restricted_action = create_action(
        admin_client,
        owner_pack,
        "restricted_action",
        visibility="restricted",
        allowed_pack_refs=[allowed_pack],
    )
    public_action = create_action(admin_client, owner_pack, "public_action")

    allowed_trigger = create_trigger(admin_client, allowed_pack, "visibility_trigger")
    blocked_trigger = create_trigger(admin_client, blocked_pack, "visibility_trigger")

    return {
        "owner_pack": owner_pack,
        "allowed_pack": allowed_pack,
        "blocked_pack": blocked_pack,
        "private_action": private_action["ref"],
        "restricted_action": restricted_action["ref"],
        "public_action": public_action["ref"],
        "allowed_trigger": allowed_trigger["ref"],
        "blocked_trigger": blocked_trigger["ref"],
    }


@pytest.mark.e2e
@pytest.mark.visibility
class TestActionReferenceVisibility:
    def test_rule_rejects_private_cross_pack_action(
        self, admin_client: AttuneClient, visibility_resources: Dict[str, Any]
    ):
        response = create_rule_response(
            admin_client,
            visibility_resources["blocked_pack"],
            "private_rule",
            visibility_resources["private_action"],
            visibility_resources["blocked_trigger"],
        )

        assert_error_contains(response, "cannot reference action")

    def test_restricted_action_allows_allow_listed_rule_and_queue(
        self, admin_client: AttuneClient, visibility_resources: Dict[str, Any]
    ):
        rule_response = create_rule_response(
            admin_client,
            visibility_resources["allowed_pack"],
            "allowed_restricted_rule",
            visibility_resources["restricted_action"],
            visibility_resources["allowed_trigger"],
        )
        assert rule_response.status_code == 201, rule_response.text
        assert rule_response.json()["data"]["action_ref"] == visibility_resources["restricted_action"]

        queue_response = create_queue_response(
            admin_client,
            visibility_resources["allowed_pack"],
            "allowed_restricted_queue",
            visibility_resources["restricted_action"],
        )
        assert queue_response.status_code == 201, queue_response.text
        assert (
            queue_response.json()["data"]["dispatch_action_ref"]
            == visibility_resources["restricted_action"]
        )

    def test_workflow_and_queue_reject_non_allow_listed_restricted_action(
        self, admin_client: AttuneClient, visibility_resources: Dict[str, Any]
    ):
        workflow_response = create_workflow_response(
            admin_client,
            visibility_resources["blocked_pack"],
            "blocked_restricted_workflow",
            visibility_resources["restricted_action"],
        )
        assert_error_contains(workflow_response, "cannot reference action")

        queue_response = create_queue_response(
            admin_client,
            visibility_resources["blocked_pack"],
            "blocked_restricted_queue",
            visibility_resources["restricted_action"],
        )
        assert_error_contains(queue_response, "cannot reference action")

    def test_action_api_filters_private_and_restricted_without_reference_context(
        self,
        unprivileged_client: AttuneClient,
        visibility_resources: Dict[str, Any],
    ):
        no_context_refs = action_refs_from_list(
            unprivileged_client,
            {"packs": visibility_resources["owner_pack"]},
        )
        assert visibility_resources["public_action"] in no_context_refs
        assert visibility_resources["private_action"] not in no_context_refs
        assert visibility_resources["restricted_action"] not in no_context_refs

        allowed_context_refs = action_refs_from_list(
            unprivileged_client,
            {
                "packs": visibility_resources["owner_pack"],
                "referencing_pack_ref": visibility_resources["allowed_pack"],
            },
        )
        assert visibility_resources["restricted_action"] in allowed_context_refs
        assert visibility_resources["private_action"] not in allowed_context_refs

        hidden_get = unprivileged_client.request(
            "GET", f"/api/v1/actions/{visibility_resources['private_action']}"
        )
        assert hidden_get.status_code == 404, hidden_get.text

    def test_visibility_tightening_is_blocked_when_external_references_exist(
        self, admin_client: AttuneClient, visibility_resources: Dict[str, Any]
    ):
        rule_response = create_rule_response(
            admin_client,
            visibility_resources["blocked_pack"],
            "public_rule_before_tightening",
            visibility_resources["public_action"],
            visibility_resources["blocked_trigger"],
        )
        assert rule_response.status_code == 201, rule_response.text

        tighten_response = admin_client.request(
            "PUT",
            f"/api/v1/actions/{visibility_resources['public_action']}",
            json={
                "reference_visibility": "private",
                "reference_allowed_pack_refs": [],
            },
        )
        assert_error_contains(tighten_response, "cannot change action")
