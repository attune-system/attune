#!/usr/bin/env python3
"""
End-to-end tests for trigger reference visibility.

These tests exercise public/private/restricted trigger subscription behavior:
rules may subscribe to public triggers from any pack, private triggers only from
the same pack, and restricted triggers from the same pack or allow-listed packs.

Run with: pytest tests/e2e/api/test_trigger_reference_visibility.py -v -s
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
            timeout=10,
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
            timeout=10,
        )
        response.raise_for_status()
        self.token = response.json()["data"]["access_token"]
        self.session.headers.update({"Authorization": f"Bearer {self.token}"})
        return self.token

    def request(self, method: str, path: str, **kwargs) -> requests.Response:
        kwargs.setdefault("timeout", 10)
        return self.session.request(method, f"{self.base_url}{path}", **kwargs)

    def get(self, path: str, **kwargs) -> Dict[str, Any]:
        response = self.request("GET", path, **kwargs)
        response.raise_for_status()
        return response.json()

    def post(self, path: str, **kwargs) -> Dict[str, Any]:
        response = self.request("POST", path, **kwargs)
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
    login = f"trigger_visibility_reader_{unique_suffix()}@attune.local"
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
                "attributes": {"test": "trigger_reference_visibility"},
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
            "description": "Trigger visibility e2e pack",
            "version": "1.0.0",
            "conf_schema": {},
            "config": {},
            "meta": {"test": "trigger_reference_visibility"},
            "tags": ["e2e", "visibility"],
        },
    )["data"]


def create_action(client: AttuneClient, pack_ref: str, name: str) -> Dict[str, Any]:
    return client.post(
        "/api/v1/actions",
        json={
            "ref": f"{pack_ref}.{name}",
            "pack_ref": pack_ref,
            "label": f"E2E {name}",
            "description": "Trigger visibility e2e action",
            "entrypoint": "actions/echo.py",
            "param_schema": {
                "message": {
                    "type": "string",
                    "required": False,
                    "description": "Message to echo",
                }
            },
        },
    )["data"]


def create_trigger(
    client: AttuneClient,
    pack_ref: str,
    name: str,
    *,
    visibility: str = "public",
    allowed_pack_refs: Optional[list[str]] = None,
) -> Dict[str, Any]:
    return client.post(
        "/api/v1/triggers",
        json={
            "ref": f"{pack_ref}.{name}",
            "pack_ref": pack_ref,
            "label": f"E2E {name}",
            "description": "Trigger visibility e2e trigger",
            "enabled": True,
            "param_schema": {},
            "out_schema": {},
            "reference_visibility": visibility,
            "reference_allowed_pack_refs": allowed_pack_refs or [],
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
            "description": "Trigger visibility e2e rule",
            "action_ref": action_ref,
            "trigger_ref": trigger_ref,
            "conditions": {},
            "action_params": {"message": "hello"},
            "trigger_params": {},
            "enabled": True,
        },
    )


def trigger_refs_from_list(
    client: AttuneClient, params: Optional[Dict[str, str]] = None
) -> set[str]:
    refs: set[str] = set()
    page = 1
    while True:
        query = {"page": str(page), "page_size": "100", **dict(params or {})}
        body = client.get("/api/v1/triggers", params=query)
        refs.update(item["ref"] for item in body["items"])
        if not body["pagination"]["has_next"]:
            return refs
        page += 1


@pytest.fixture
def visibility_resources(admin_client: AttuneClient) -> Dict[str, Any]:
    suffix = unique_suffix()
    owner_pack = f"trig_owner_{suffix}"
    allowed_pack = f"trig_allowed_{suffix}"
    blocked_pack = f"trig_blocked_{suffix}"

    for pack_ref in [owner_pack, allowed_pack, blocked_pack]:
        create_pack(admin_client, pack_ref)

    owner_action = create_action(admin_client, owner_pack, "rule_action")
    allowed_action = create_action(admin_client, allowed_pack, "rule_action")
    blocked_action = create_action(admin_client, blocked_pack, "rule_action")

    public_trigger = create_trigger(admin_client, owner_pack, "public_trigger")
    private_trigger = create_trigger(
        admin_client,
        owner_pack,
        "private_trigger",
        visibility="private",
    )
    restricted_trigger = create_trigger(
        admin_client,
        owner_pack,
        "restricted_trigger",
        visibility="restricted",
        allowed_pack_refs=[allowed_pack],
    )

    return {
        "owner_pack": owner_pack,
        "allowed_pack": allowed_pack,
        "blocked_pack": blocked_pack,
        "owner_action": owner_action["ref"],
        "allowed_action": allowed_action["ref"],
        "blocked_action": blocked_action["ref"],
        "public_trigger": public_trigger["ref"],
        "private_trigger": private_trigger["ref"],
        "restricted_trigger": restricted_trigger["ref"],
    }


@pytest.mark.e2e
@pytest.mark.api
@pytest.mark.visibility
class TestTriggerReferenceVisibility:
    def test_public_trigger_allows_cross_pack_rule(
        self, admin_client: AttuneClient, visibility_resources: Dict[str, Any]
    ):
        response = create_rule_response(
            admin_client,
            visibility_resources["blocked_pack"],
            "public_trigger_rule",
            visibility_resources["blocked_action"],
            visibility_resources["public_trigger"],
        )

        assert response.status_code == 201, response.text
        assert response.json()["data"]["trigger_ref"] == visibility_resources["public_trigger"]

    def test_private_trigger_rejects_cross_pack_rule(
        self, admin_client: AttuneClient, visibility_resources: Dict[str, Any]
    ):
        response = create_rule_response(
            admin_client,
            visibility_resources["blocked_pack"],
            "private_trigger_rule",
            visibility_resources["blocked_action"],
            visibility_resources["private_trigger"],
        )

        assert_error_contains(response, "cannot subscribe to trigger")

    def test_private_trigger_allows_same_pack_rule(
        self, admin_client: AttuneClient, visibility_resources: Dict[str, Any]
    ):
        response = create_rule_response(
            admin_client,
            visibility_resources["owner_pack"],
            "private_same_pack_rule",
            visibility_resources["owner_action"],
            visibility_resources["private_trigger"],
        )

        assert response.status_code == 201, response.text
        assert response.json()["data"]["trigger_ref"] == visibility_resources["private_trigger"]

    def test_restricted_trigger_allows_allow_listed_pack(
        self, admin_client: AttuneClient, visibility_resources: Dict[str, Any]
    ):
        response = create_rule_response(
            admin_client,
            visibility_resources["allowed_pack"],
            "restricted_allowed_rule",
            visibility_resources["allowed_action"],
            visibility_resources["restricted_trigger"],
        )

        assert response.status_code == 201, response.text
        assert response.json()["data"]["trigger_ref"] == visibility_resources["restricted_trigger"]

    def test_restricted_trigger_rejects_non_allow_listed_pack(
        self, admin_client: AttuneClient, visibility_resources: Dict[str, Any]
    ):
        response = create_rule_response(
            admin_client,
            visibility_resources["blocked_pack"],
            "restricted_blocked_rule",
            visibility_resources["blocked_action"],
            visibility_resources["restricted_trigger"],
        )

        assert_error_contains(response, "cannot subscribe to trigger")

    def test_trigger_api_filters_private_and_restricted_without_reference_context(
        self,
        unprivileged_client: AttuneClient,
        visibility_resources: Dict[str, Any],
    ):
        no_context_refs = trigger_refs_from_list(unprivileged_client)
        assert visibility_resources["public_trigger"] in no_context_refs
        assert visibility_resources["private_trigger"] not in no_context_refs
        assert visibility_resources["restricted_trigger"] not in no_context_refs

        allowed_context_refs = trigger_refs_from_list(
            unprivileged_client,
            {"referencing_pack_ref": visibility_resources["allowed_pack"]},
        )
        assert visibility_resources["restricted_trigger"] in allowed_context_refs
        assert visibility_resources["private_trigger"] not in allowed_context_refs

        hidden_get = unprivileged_client.request(
            "GET", f"/api/v1/triggers/{visibility_resources['private_trigger']}"
        )
        assert hidden_get.status_code == 404, hidden_get.text

        visible_get = unprivileged_client.request(
            "GET",
            f"/api/v1/triggers/{visibility_resources['restricted_trigger']}",
            params={"referencing_pack_ref": visibility_resources["allowed_pack"]},
        )
        assert visible_get.status_code == 200, visible_get.text

    def test_visibility_tightening_is_blocked_when_external_rules_exist(
        self, admin_client: AttuneClient, visibility_resources: Dict[str, Any]
    ):
        rule_response = create_rule_response(
            admin_client,
            visibility_resources["blocked_pack"],
            "public_rule_before_tightening",
            visibility_resources["blocked_action"],
            visibility_resources["public_trigger"],
        )
        assert rule_response.status_code == 201, rule_response.text

        tighten_response = admin_client.request(
            "PUT",
            f"/api/v1/triggers/{visibility_resources['public_trigger']}",
            json={
                "reference_visibility": "private",
                "reference_allowed_pack_refs": [],
            },
        )
        assert_error_contains(tighten_response, "cannot change trigger")
