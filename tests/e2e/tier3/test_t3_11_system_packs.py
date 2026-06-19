"""T3.11: system pack visibility and future pack ownership scoping."""

import pytest
from helpers import AttuneClient
from helpers.fixtures import unique_ref


pytestmark = [
    pytest.mark.tier3,
    pytest.mark.security,
    pytest.mark.packs,
]


def _pack_refs(client: AttuneClient) -> set[str]:
    return {pack["ref"] for pack in client.list_packs()}


def _data(response):
    assert response.status_code in (200, 201), response.text
    body = response.json()
    return body.get("data", body) if isinstance(body, dict) else body


def _permission_set_id(client: AttuneClient, permission_set_ref: str) -> int:
    permission_sets = _data(client._request("GET", "/api/v1/permissions/sets"))
    for permission_set in permission_sets:
        if permission_set["ref"] == permission_set_ref:
            return permission_set["id"]
    raise AssertionError(f"Permission set {permission_set_ref!r} not found")


def _register_user_with_permission_role(
    client: AttuneClient,
    *,
    role_prefix: str,
    permission_set_ref: str,
) -> AttuneClient:
    login = f"{role_prefix}_{unique_ref()}@example.com"
    password = f"{role_prefix}_password_123"
    role = f"e2e_{role_prefix}_{unique_ref()}"

    registration = client.register(
        login=login,
        password=password,
        display_name=f"E2E {role_prefix.title()}",
    )
    identity_id = registration["user"]["id"]
    permission_set_id = _permission_set_id(client, permission_set_ref)

    _data(
        client._request(
            "POST",
            f"/api/v1/permissions/sets/{permission_set_id}/roles",
            json={"role": role},
        )
    )
    _data(
        client._request(
            "POST",
            f"/api/v1/identities/{identity_id}/roles",
            json={"role": role},
        )
    )

    role_client = AttuneClient(base_url=client.base_url)
    role_client.login(login=login, password=password)
    return role_client


def test_system_pack_visible_to_all_identities(
    client: AttuneClient, unique_user_client: AttuneClient
):
    """The built-in core pack should be visible to every authenticated identity."""
    user1_refs = _pack_refs(client)
    user2_refs = _pack_refs(unique_user_client)

    assert "core" in user1_refs
    assert "core" in user2_refs

    user1_core = client.get_pack_by_ref("core")
    user2_core = unique_user_client.get_pack_by_ref("core")
    assert user1_core is not None
    assert user2_core is not None
    assert user1_core["ref"] == "core"
    assert user2_core["ref"] == "core"


def test_user_pack_ownership_scopes_visibility(client: AttuneClient):
    """User-created packs should be visible only to their owner or constrained grantees."""
    user2_client = _register_user_with_permission_role(
        client,
        role_prefix="pack_owner",
        permission_set_ref="core.admin",
    )
    user1_pack_ref = f"user1_pack_{unique_ref()}"
    user2_pack_ref = f"user2_pack_{unique_ref()}"

    try:
        user1_pack = client.create_pack(
            {
                "ref": user1_pack_ref,
                "label": "User 1 Private Pack",
                "version": "1.0.0",
                "description": "Should only be visible to the creating owner",
            }
        )
        user2_pack = user2_client.create_pack(
            {
                "ref": user2_pack_ref,
                "label": "User 2 Private Pack",
                "version": "1.0.0",
                "description": "Should only be visible to the creating owner",
            }
        )

        assert user1_pack["ref"] == user1_pack_ref
        assert user2_pack["ref"] == user2_pack_ref
        assert client.get_pack_by_ref(user1_pack_ref) is not None
        assert user2_client.get_pack_by_ref(user2_pack_ref) is not None

        user2_direct = user2_client.get(f"/api/v1/packs/{user1_pack_ref}")
        user1_direct = client.get(f"/api/v1/packs/{user2_pack_ref}")
        assert user2_direct.status_code in (403, 404)
        assert user1_direct.status_code in (403, 404)
    finally:
        for owner, pack_ref in (
            (client, user1_pack_ref),
            (user2_client, user2_pack_ref),
        ):
            response = owner.delete(f"/api/v1/packs/{pack_ref}")
            assert response.status_code in (200, 204, 404)


def test_system_pack_actions_available_to_all(
    client: AttuneClient,
):
    """Core pack actions should be executable by authorized identities across tenants."""
    user2_client = _register_user_with_permission_role(
        client,
        role_prefix="system_pack_executor",
        permission_set_ref="core.admin",
    )
    user1_actions = {action["ref"] for action in client.list_actions(limit=500)}
    user2_actions = {action["ref"] for action in user2_client.list_actions(limit=500)}

    assert "core.echo" in user1_actions
    assert "core.echo" in user2_actions

    user1_execution = client.execute_action(
        {"action": "core.echo", "parameters": {"message": "User 1 system pack test"}}
    )
    user2_execution = user2_client.execute_action(
        {"action": "core.echo", "parameters": {"message": "User 2 system pack test"}}
    )

    assert user1_execution["action_ref"] == "core.echo"
    assert user2_execution["action_ref"] == "core.echo"
