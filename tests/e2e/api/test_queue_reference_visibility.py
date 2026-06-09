#!/usr/bin/env python3
"""
End-to-end tests for work queue reference visibility.

These tests exercise queue discovery behavior for public/private/restricted
queues. Item submission visibility and constrained queue-item grants are covered
by Rust API integration tests because e2e environments typically only expose
seeded permission sets.

Run with: pytest tests/e2e/api/test_queue_reference_visibility.py -v -s
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
def viewer_client(admin_client: AttuneClient) -> AttuneClient:
    client = AttuneClient(API_BASE_URL)
    login = f"queue_visibility_viewer_{unique_suffix()}@attune.local"
    password = "TestPassword123!"

    identity = admin_client.post(
        "/api/v1/identities",
        json={
            "login": login,
            "display_name": f"E2E {login}",
            "password": password,
            "attributes": {"test": "queue_reference_visibility"},
        },
    )["data"]
    admin_client.post(
        "/api/v1/permissions/assignments",
        json={
            "identity_id": identity["id"],
            "permission_set_ref": "core.viewer",
        },
    )

    client.login(login, password)
    return client


def create_pack(client: AttuneClient, pack_ref: str) -> Dict[str, Any]:
    return client.post(
        "/api/v1/packs",
        json={
            "ref": pack_ref,
            "label": f"E2E {pack_ref}",
            "description": "Queue visibility e2e pack",
            "version": "1.0.0",
            "conf_schema": {},
            "config": {},
            "meta": {"test": "queue_reference_visibility"},
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
            "description": "Queue visibility e2e dispatch action",
            "entrypoint": "actions/echo.py",
            "param_schema": {},
        },
    )["data"]


def create_queue(
    client: AttuneClient,
    pack_ref: str,
    name: str,
    dispatch_action_ref: str,
    *,
    visibility: str = "public",
    allowed_pack_refs: Optional[list[str]] = None,
) -> Dict[str, Any]:
    return client.post(
        "/api/v1/queues",
        json={
            "ref": f"{pack_ref}.{name}",
            "pack_ref": pack_ref,
            "label": f"E2E {name}",
            "description": "Queue visibility e2e queue",
            "dispatch_action_ref": dispatch_action_ref,
            "item_schema": {},
            "action_params": {},
            "reference_visibility": visibility,
            "reference_allowed_pack_refs": allowed_pack_refs or [],
        },
    )["data"]


def queue_refs_from_list(
    client: AttuneClient, params: Optional[Dict[str, str]] = None
) -> set[str]:
    refs: set[str] = set()
    page = 1
    while True:
        query = {"page": str(page), "per_page": "100", **dict(params or {})}
        body = client.get("/api/v1/queues", params=query)
        refs.update(item["ref"] for item in body["items"])
        if not body["pagination"]["has_next"]:
            return refs
        page += 1


@pytest.fixture
def visibility_resources(admin_client: AttuneClient) -> Dict[str, Any]:
    suffix = unique_suffix()
    owner_pack = f"queue_owner_{suffix}"
    allowed_pack = f"queue_allowed_{suffix}"
    blocked_pack = f"queue_blocked_{suffix}"

    for pack_ref in [owner_pack, allowed_pack, blocked_pack]:
        create_pack(admin_client, pack_ref)

    owner_action = create_action(admin_client, owner_pack, "dispatch")
    public_queue = create_queue(
        admin_client,
        owner_pack,
        "public_queue",
        owner_action["ref"],
    )
    private_queue = create_queue(
        admin_client,
        owner_pack,
        "private_queue",
        owner_action["ref"],
        visibility="private",
    )
    restricted_queue = create_queue(
        admin_client,
        owner_pack,
        "restricted_queue",
        owner_action["ref"],
        visibility="restricted",
        allowed_pack_refs=[allowed_pack],
    )

    return {
        "owner_pack": owner_pack,
        "allowed_pack": allowed_pack,
        "blocked_pack": blocked_pack,
        "public_queue": public_queue,
        "private_queue": private_queue,
        "restricted_queue": restricted_queue,
    }


@pytest.mark.e2e
@pytest.mark.api
@pytest.mark.visibility
def test_queue_visibility_filters_list_discovery(
    viewer_client: AttuneClient, visibility_resources: Dict[str, Any]
):
    default_refs = queue_refs_from_list(viewer_client)
    assert visibility_resources["public_queue"]["ref"] in default_refs
    assert visibility_resources["private_queue"]["ref"] not in default_refs
    assert visibility_resources["restricted_queue"]["ref"] not in default_refs

    allowed_refs = queue_refs_from_list(
        viewer_client,
        {"referencing_pack_ref": visibility_resources["allowed_pack"]},
    )
    assert visibility_resources["public_queue"]["ref"] in allowed_refs
    assert visibility_resources["private_queue"]["ref"] not in allowed_refs
    assert visibility_resources["restricted_queue"]["ref"] in allowed_refs

    blocked_refs = queue_refs_from_list(
        viewer_client,
        {"referencing_pack_ref": visibility_resources["blocked_pack"]},
    )
    assert visibility_resources["public_queue"]["ref"] in blocked_refs
    assert visibility_resources["private_queue"]["ref"] not in blocked_refs
    assert visibility_resources["restricted_queue"]["ref"] not in blocked_refs


@pytest.mark.e2e
@pytest.mark.api
@pytest.mark.visibility
def test_queue_visibility_filters_detail_discovery(
    viewer_client: AttuneClient, visibility_resources: Dict[str, Any]
):
    restricted_ref = visibility_resources["restricted_queue"]["ref"]

    allowed_response = viewer_client.request(
        "GET",
        f"/api/v1/queues/{restricted_ref}",
        params={"referencing_pack_ref": visibility_resources["allowed_pack"]},
    )
    assert allowed_response.status_code == 200, allowed_response.text

    blocked_response = viewer_client.request(
        "GET",
        f"/api/v1/queues/{restricted_ref}",
        params={"referencing_pack_ref": visibility_resources["blocked_pack"]},
    )
    assert blocked_response.status_code == 404, blocked_response.text
