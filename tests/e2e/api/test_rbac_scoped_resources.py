"""
API Tests: RBAC Scoped Resources

Ported from crates/api/tests/rbac_scoped_resources_api_tests.rs
Tests RBAC enforcement for pack-scoped keys, artifacts, queues, and
artifact visibility/scope authorization (including execution tokens).
"""

import json as json_mod
import os
import time
import uuid

import jwt
import psycopg
import pytest
import requests


def _uid():
    return uuid.uuid4().hex[:8]


def _db_url():
    return os.environ.get(
        "DATABASE_URL", "postgresql://attune:attune@localhost:5432/attune"
    )


def _jwt_secret():
    return os.environ.get(
        "ATTUNE__SECURITY__JWT_SECRET",
        os.environ.get("JWT_SECRET", "docker-dev-secret-change-in-production"),
    )


def _make_execution_token(identity_id: int, execution_id: int, action_ref: str) -> str:
    now = int(time.time())
    standard_pack_refs = []
    if "." in action_ref:
        pack_ref = action_ref.split(".", 1)[0]
        if pack_ref:
            standard_pack_refs.append(pack_ref)
    return jwt.encode(
        {
            "sub": str(identity_id),
            "login": f"execution:{execution_id}",
            "token_type": "execution",
            "scope": "execution",
            "metadata": {
                "execution_id": execution_id,
                "action_ref": action_ref,
                "permission_set_refs": ["standard"],
                "standard_access_action_refs": [action_ref] if action_ref else [],
                "standard_access_pack_refs": standard_pack_refs,
            },
            "iat": now,
            "exp": now + 300,
        },
        _jwt_secret(),
        algorithm="HS256",
    )


def _register_scoped_user(base_url: str, login: str, grants: list) -> str:
    """Register a user and assign scoped permissions via direct DB. Returns access token."""
    resp = requests.post(
        f"{base_url}/auth/register",
        json={
            "login": login,
            "password": "TestPassword123!",
            "display_name": f"Scoped User {login}",
        },
        timeout=10,
    )
    assert resp.status_code in (200, 201), f"register failed: {resp.text}"
    token = resp.json()["data"]["access_token"]

    with psycopg.connect(_db_url()) as conn:
        with conn.cursor() as cur:
            cur.execute("SELECT id FROM identity WHERE login = %s", (login,))
            identity_id = cur.fetchone()[0]

            permset_ref = f"test.scoped_{_uid()}"
            cur.execute(
                "INSERT INTO permission_set (ref, label, description, grants) "
                "VALUES (%s, %s, %s, %s::jsonb) RETURNING id",
                (
                    permset_ref,
                    "Scoped Test Permission Set",
                    "Scoped test grants",
                    json_mod.dumps(grants),
                ),
            )
            permset_id = cur.fetchone()[0]

            cur.execute(
                "INSERT INTO permission_assignment (identity, permset) VALUES (%s, %s)",
                (identity_id, permset_id),
            )
        conn.commit()

    return token


def _get_identity_id(login: str) -> int:
    """Look up identity id by login."""
    with psycopg.connect(_db_url()) as conn:
        with conn.cursor() as cur:
            cur.execute("SELECT id FROM identity WHERE login = %s", (login,))
            row = cur.fetchone()
            assert row is not None, f"identity {login} not found"
    return row[0]


def _response_data(resp: requests.Response):
    body = resp.json()
    if "data" in body:
        return body["data"]
    if "items" in body:
        return body["items"]
    return body


def _create_key_via_db(
    local_ref: str,
    owner_type: str,
    owner: str,
    owner_pack_ref: str = None,
    value="allowed",
) -> int:
    """Insert a key row directly via DB. Returns id."""
    with psycopg.connect(_db_url()) as conn:
        with conn.cursor() as cur:
            owner_identity = None
            owner_pack = None
            owner_action = None
            owner_sensor = None

            if owner_type == "identity":
                owner_identity = int(owner)
            elif owner_type == "pack":
                pack_ref = owner_pack_ref or owner
                cur.execute(
                    "INSERT INTO pack (ref, label, version, conf_schema, config, meta, tags, "
                    "runtime_deps, dependencies, installers) "
                    "VALUES (%s, %s, '1.0.0', '{}', '{}', '{}', '{}', '{}', '{}', '{}') "
                    "ON CONFLICT (ref) DO UPDATE SET ref = EXCLUDED.ref RETURNING id",
                    (pack_ref, f"Pack {pack_ref}"),
                )
                owner_pack = cur.fetchone()[0]

            cur.execute(
                "INSERT INTO key (local_ref, owner_type, owner_identity, owner_pack, owner_pack_ref, "
                "owner_action, owner_sensor, name, encrypted, value) "
                "VALUES (%s, %s, %s, %s, %s, %s, %s, %s, false, %s::jsonb) RETURNING id",
                (
                    local_ref,
                    owner_type,
                    owner_identity,
                    owner_pack,
                    owner_pack_ref,
                    owner_action,
                    owner_sensor,
                    f"Key {local_ref}",
                    json_mod.dumps(value),
                ),
            )
            key_id = cur.fetchone()[0]
        conn.commit()
    return key_id


def _create_artifact_via_db(
    ref: str,
    scope: str,
    owner: str,
    visibility: str,
    art_type: str = "file_text",
) -> int:
    """Insert an artifact row directly via DB. Returns id."""
    with psycopg.connect(_db_url()) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "INSERT INTO artifact (ref, scope, owner, type, visibility, "
                "retention_policy, retention_limit, name, content_type) "
                "VALUES (%s, %s, %s, %s, %s, 'versions', 5, 'test artifact', 'text/plain') "
                "RETURNING id",
                (ref, scope, owner, art_type, visibility),
            )
            art_id = cur.fetchone()[0]
        conn.commit()
    return art_id


def _create_pack_with_action(pack_ref: str, action_ref: str) -> tuple:
    """Create a pack and action via DB. Returns (pack_id, action_id)."""
    with psycopg.connect(_db_url()) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "INSERT INTO pack (ref, label, version, conf_schema, config, meta, tags, "
                "runtime_deps, dependencies, installers) "
                "VALUES (%s, %s, '1.0.0', '{}', '{}', '{}', '{}', '{}', '{}', '{}') "
                "RETURNING id",
                (pack_ref, f"Pack {pack_ref}"),
            )
            pack_id = cur.fetchone()[0]
            cur.execute(
                "INSERT INTO action (ref, pack, pack_ref, label, entrypoint, required_worker_runtimes) "
                "VALUES (%s, %s, %s, %s, 'main.py', '{}') RETURNING id",
                (action_ref, pack_id, pack_ref, f"Action {action_ref}"),
            )
            action_id = cur.fetchone()[0]
        conn.commit()
    return pack_id, action_id


def _create_work_queue_via_db(
    ref: str,
    pack_id: int = None,
    pack_ref: str = None,
    is_adhoc: bool = False,
    action_id: int = None,
    action_ref: str = "",
) -> int:
    """Insert a work queue row directly via DB. Returns id."""
    with psycopg.connect(_db_url()) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "INSERT INTO work_queue (ref, pack, pack_ref, is_adhoc, label, "
                "dispatch_action, dispatch_action_ref, default_priority, "
                "allow_pending_update, update_strategy, batch_mode, "
                "item_schema, action_params, config) "
                "VALUES (%s, %s, %s, %s, %s, %s, %s, 0, true, 'replace', 'single', "
                "'{}', '{}', '{}') RETURNING id",
                (
                    ref,
                    pack_id,
                    pack_ref,
                    is_adhoc,
                    f"Queue {ref}",
                    action_id,
                    action_ref,
                ),
            )
            queue_id = cur.fetchone()[0]
        conn.commit()
    return queue_id


def _force_queue_item_status(item_id: int, status: str):
    """Force a queue item to a given status via DB."""
    with psycopg.connect(_db_url()) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "UPDATE work_queue_item SET status = %s WHERE id = %s",
                (status, item_id),
            )
        conn.commit()


# ============================================================================
# Pack-Scoped Permission Tests
# ============================================================================


@pytest.mark.api
class TestPackScopedPermissions:
    """Tests that pack-scoped RBAC constraints enforce owner_refs for keys and artifacts."""

    def test_pack_scoped_key_permissions_enforce_owner_refs(self, api_base_url):
        """
        User with keys:read scoped to pack 'python_example' can only see keys
        owned by that pack. Other keys return 404.
        """
        uid = _uid()
        login = f"scoped_keys_{uid}"

        token = _register_scoped_user(
            api_base_url,
            login,
            [
                {
                    "resource": "keys",
                    "actions": ["read"],
                    "constraints": {
                        "owner_types": ["pack"],
                        "owner_refs": ["python_example"],
                    },
                }
            ],
        )

        # Create allowed key (python_example pack)
        allowed_local_ref = f"python_example_key_{_uid()}"
        allowed_ref = f"pack.python_example.{allowed_local_ref}"
        _create_key_via_db(
            local_ref=allowed_local_ref,
            owner_type="pack",
            owner="python_example",
            owner_pack_ref="python_example",
            value="allowed",
        )

        # Create blocked key (other_pack)
        blocked_local_ref = f"other_pack_key_{_uid()}"
        blocked_ref = f"pack.other_pack.{blocked_local_ref}"
        _create_key_via_db(
            local_ref=blocked_local_ref,
            owner_type="pack",
            owner="other_pack",
            owner_pack_ref="other_pack",
            value="blocked",
        )

        headers = {"Authorization": f"Bearer {token}"}

        # List keys — should include the allowed one and exclude blocked refs.
        resp = requests.get(
            f"{api_base_url}/api/v1/keys", headers=headers, timeout=10
        )
        assert resp.status_code == 200
        data = _response_data(resp)
        refs = {item["ref"] for item in data}
        assert allowed_ref in refs
        assert blocked_ref not in refs

        # GET blocked key returns 404
        resp = requests.get(
            f"{api_base_url}/api/v1/keys/{blocked_ref}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 404

    def test_pack_scoped_artifact_permissions_enforce_owner_refs(self, api_base_url):
        """
        User with artifacts:read+create scoped to 'python_example'.
        GET allowed artifact succeeds, blocked returns 404.
        POST create in allowed pack succeeds, blocked returns 403.
        """
        uid = _uid()
        login = f"scoped_artifacts_{uid}"

        token = _register_scoped_user(
            api_base_url,
            login,
            [
                {
                    "resource": "artifacts",
                    "actions": ["read", "create"],
                    "constraints": {
                        "owner_types": ["pack"],
                        "owner_refs": ["python_example"],
                    },
                }
            ],
        )

        allowed_ref = f"python_example.allowed_{_uid()}"
        allowed_id = _create_artifact_via_db(
            ref=allowed_ref,
            scope="pack",
            owner="python_example",
            visibility="private",
        )

        blocked_ref = f"other_pack.blocked_{_uid()}"
        blocked_id = _create_artifact_via_db(
            ref=blocked_ref,
            scope="pack",
            owner="other_pack",
            visibility="private",
        )

        headers = {"Authorization": f"Bearer {token}"}

        # GET allowed artifact
        resp = requests.get(
            f"{api_base_url}/api/v1/artifacts/{allowed_id}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 200

        # GET blocked artifact
        resp = requests.get(
            f"{api_base_url}/api/v1/artifacts/{blocked_id}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 404

        # POST create in allowed pack
        resp = requests.post(
            f"{api_base_url}/api/v1/artifacts",
            headers=headers,
            json={
                "ref": f"python_example.created_{_uid()}",
                "scope": "pack",
                "owner": "python_example",
                "type": "file_text",
                "name": "Created Artifact",
            },
            timeout=10,
        )
        assert resp.status_code == 201

        # POST create in blocked pack
        resp = requests.post(
            f"{api_base_url}/api/v1/artifacts",
            headers=headers,
            json={
                "ref": f"other_pack.created_{_uid()}",
                "scope": "pack",
                "owner": "other_pack",
                "type": "file_text",
                "name": "Blocked Artifact",
            },
            timeout=10,
        )
        assert resp.status_code == 403


# ============================================================================
# Queue Admin CRUD Tests
# ============================================================================


@pytest.mark.api
class TestQueueAdminCrud:
    """Tests queue CRUD with broad grants and pending-item guards."""

    def test_queue_admin_crud_and_pending_item_guards(self, api_base_url):
        """
        Full CRUD lifecycle: create queue, enqueue, merge-patch duplicate,
        update item, list items, force to succeeded via DB, verify update/delete
        on succeeded item returns 409 CONFLICT.
        """
        uid = _uid()

        # Create pack + action for the queue
        pack_ref = f"queue_admin_pack_{uid}"
        action_ref = f"{pack_ref}.dispatch_{uid}"
        _create_pack_with_action(pack_ref, action_ref)

        login = f"queue_admin_{uid}"
        token = _register_scoped_user(
            api_base_url,
            login,
            [
                {
                    "resource": "queues",
                    "actions": ["read", "create", "update", "delete"],
                }
            ],
        )

        headers = {"Authorization": f"Bearer {token}"}
        queue_ref = f"adhoc_queue_{_uid()}"

        # Create queue
        resp = requests.post(
            f"{api_base_url}/api/v1/queues",
            headers=headers,
            json={
                "ref": queue_ref,
                "label": "Adhoc Queue",
                "dispatch_action_ref": action_ref,
                "enabled": False,
                "allow_pending_update": True,
                "update_strategy": "merge_patch",
                "batch_mode": "single",
            },
            timeout=10,
        )
        assert resp.status_code == 201, resp.text

        # Enqueue item
        resp = requests.post(
            f"{api_base_url}/api/v1/queues/{queue_ref}/items",
            headers=headers,
            json={
                "item_key": "order-123",
                "payload": {"state": "queued"},
                "metadata": {"source": "api"},
            },
            timeout=10,
        )
        assert resp.status_code == 201, resp.text
        item_id = _response_data(resp)["id"]

        # Merge-patch duplicate key
        resp = requests.post(
            f"{api_base_url}/api/v1/queues/{queue_ref}/items",
            headers=headers,
            json={
                "item_key": "order-123",
                "payload": {"extra": True},
                "metadata": {"attempt": 2},
            },
            timeout=10,
        )
        assert resp.status_code == 200, resp.text

        # Update pending item
        resp = requests.put(
            f"{api_base_url}/api/v1/queues/{queue_ref}/items/{item_id}",
            headers=headers,
            json={"priority": 9, "metadata": {"attempt": 3}},
            timeout=10,
        )
        assert resp.status_code == 200, resp.text

        # List items
        resp = requests.get(
            f"{api_base_url}/api/v1/queues/{queue_ref}/items",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 200
        items = _response_data(resp)
        assert len(items) == 1
        assert items[0]["payload"]["state"] == "queued"
        assert items[0]["payload"]["extra"] is True
        assert items[0]["priority"] == 9

        # Force item to succeeded via DB
        _force_queue_item_status(item_id, "completed")

        # Update succeeded item — should return 409
        resp = requests.put(
            f"{api_base_url}/api/v1/queues/{queue_ref}/items/{item_id}",
            headers=headers,
            json={"priority": 5},
            timeout=10,
        )
        assert resp.status_code == 409

        # Delete succeeded item — should return 409
        resp = requests.delete(
            f"{api_base_url}/api/v1/queues/{queue_ref}/items/{item_id}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 409

        # Delete queue itself
        resp = requests.delete(
            f"{api_base_url}/api/v1/queues/{queue_ref}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 200

    def test_pack_scoped_queue_permissions(self, api_base_url):
        """
        User with queues:* scoped to 'python_example'. Create blocked queue in
        other_pack via DB. Verify allowed operations succeed and blocked ones
        return 403/404.
        """
        uid = _uid()

        # Create allowed pack + action
        allowed_pack_ref = f"python_example_{uid}"
        allowed_action_ref = f"{allowed_pack_ref}.dispatch_queue"
        allowed_pack_id, allowed_action_id = _create_pack_with_action(
            allowed_pack_ref, allowed_action_ref
        )

        # Create blocked pack + action + queue
        blocked_pack_ref = f"other_pack_{uid}"
        blocked_action_ref = f"{blocked_pack_ref}.dispatch_queue"
        blocked_pack_id, blocked_action_id = _create_pack_with_action(
            blocked_pack_ref, blocked_action_ref
        )

        blocked_queue_ref = f"{blocked_pack_ref}.blocked_queue"
        _create_work_queue_via_db(
            ref=blocked_queue_ref,
            pack_id=blocked_pack_id,
            pack_ref=blocked_pack_ref,
            is_adhoc=False,
            action_id=blocked_action_id,
            action_ref=blocked_action_ref,
        )

        login = f"queue_scoped_{uid}"
        token = _register_scoped_user(
            api_base_url,
            login,
            [
                {
                    "resource": "queues",
                    "actions": ["read", "create", "update", "delete"],
                    "constraints": {"pack_refs": [allowed_pack_ref]},
                }
            ],
        )

        headers = {"Authorization": f"Bearer {token}"}

        # Create queue in allowed pack
        allowed_queue_ref = f"{allowed_pack_ref}.scoped_queue"
        resp = requests.post(
            f"{api_base_url}/api/v1/queues",
            headers=headers,
            json={
                "ref": allowed_queue_ref,
                "pack_ref": allowed_pack_ref,
                "label": "Scoped Queue",
                "dispatch_action_ref": allowed_action_ref,
                "allow_pending_update": True,
                "update_strategy": "replace",
            },
            timeout=10,
        )
        assert resp.status_code == 201, resp.text

        # Create queue in blocked pack — FORBIDDEN
        resp = requests.post(
            f"{api_base_url}/api/v1/queues",
            headers=headers,
            json={
                "ref": f"{blocked_pack_ref}.denied_queue",
                "pack_ref": blocked_pack_ref,
                "label": "Denied Queue",
                "dispatch_action_ref": blocked_action_ref,
            },
            timeout=10,
        )
        assert resp.status_code == 403

        # List queues for allowed pack
        resp = requests.get(
            f"{api_base_url}/api/v1/packs/{allowed_pack_ref}/queues",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 200
        queue_refs = [q["ref"] for q in _response_data(resp)]
        assert allowed_queue_ref in queue_refs

        # GET allowed queue
        resp = requests.get(
            f"{api_base_url}/api/v1/queues/{allowed_queue_ref}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 200

        # GET blocked queue — NOT FOUND
        resp = requests.get(
            f"{api_base_url}/api/v1/queues/{blocked_queue_ref}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 404

        # Enqueue in allowed queue
        resp = requests.post(
            f"{api_base_url}/api/v1/queues/{allowed_queue_ref}/items",
            headers=headers,
            json={"item_key": "job-1", "payload": {"hello": "world"}},
            timeout=10,
        )
        assert resp.status_code == 201
        item_id = _response_data(resp)["id"]

        # Update allowed queue item
        resp = requests.put(
            f"{api_base_url}/api/v1/queues/{allowed_queue_ref}/items/{item_id}",
            headers=headers,
            json={"priority": 11},
            timeout=10,
        )
        assert resp.status_code == 200

        # Delete allowed queue item
        resp = requests.delete(
            f"{api_base_url}/api/v1/queues/{allowed_queue_ref}/items/{item_id}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 200

        # Enqueue in blocked queue — FORBIDDEN
        resp = requests.post(
            f"{api_base_url}/api/v1/queues/{blocked_queue_ref}/items",
            headers=headers,
            json={"payload": {"blocked": True}},
            timeout=10,
        )
        assert resp.status_code == 403


# ============================================================================
# Artifact Authorization Tests (visibility × scope)
# ============================================================================


@pytest.mark.api
class TestArtifactAuthz:
    """
    Tests artifact visibility and scope-based authorization, including
    execution token cross-pack guards.
    """

    def test_public_artifact_readable_by_any_user(self, api_base_url):
        """Public artifact visible to any user with artifacts:read."""
        uid = _uid()
        login = f"public_reader_{uid}"
        token = _register_scoped_user(
            api_base_url,
            login,
            [{"resource": "artifacts", "actions": ["read"]}],
        )

        art_ref = f"some_pack.public_{_uid()}"
        art_id = _create_artifact_via_db(
            ref=art_ref, scope="pack", owner="some_pack", visibility="public"
        )

        headers = {"Authorization": f"Bearer {token}"}
        resp = requests.get(
            f"{api_base_url}/api/v1/artifacts/{art_id}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 200

    def test_private_identity_scoped_owner_vs_other(self, api_base_url):
        """
        Private identity-scoped artifact: owner can read, other user cannot.
        """
        uid = _uid()

        # Register owner
        owner_login = f"owner_{uid}"
        owner_token = _register_scoped_user(
            api_base_url,
            owner_login,
            [{"resource": "artifacts", "actions": ["read"]}],
        )
        owner_id = _get_identity_id(owner_login)

        # Register other user
        other_login = f"other_{uid}"
        other_token = _register_scoped_user(
            api_base_url,
            other_login,
            [{"resource": "artifacts", "actions": ["read"]}],
        )

        art_ref = f"identity_artifact_{_uid()}"
        art_id = _create_artifact_via_db(
            ref=art_ref,
            scope="identity",
            owner=str(owner_id),
            visibility="private",
        )

        # Owner can read
        resp = requests.get(
            f"{api_base_url}/api/v1/artifacts/{art_id}",
            headers={"Authorization": f"Bearer {owner_token}"},
            timeout=10,
        )
        assert resp.status_code == 200

        # Other user cannot
        resp = requests.get(
            f"{api_base_url}/api/v1/artifacts/{art_id}",
            headers={"Authorization": f"Bearer {other_token}"},
            timeout=10,
        )
        assert resp.status_code == 404

    def test_private_action_scoped_artifact_uses_derived_pack(self, api_base_url):
        """
        Private action-scoped artifact: user with packs:read on 'python_example'
        can read artifacts owned by 'python_example.deploy', not 'other_pack.deploy'.
        """
        uid = _uid()
        login = f"pack_reader_{uid}"
        token = _register_scoped_user(
            api_base_url,
            login,
            [
                {
                    "resource": "packs",
                    "actions": ["read"],
                    "constraints": {"pack_refs": ["python_example"]},
                }
            ],
        )

        allowed_id = _create_artifact_via_db(
            ref=f"python_example.deploy_log_{_uid()}",
            scope="action",
            owner="python_example.deploy",
            visibility="private",
        )
        blocked_id = _create_artifact_via_db(
            ref=f"other_pack.deploy_log_{_uid()}",
            scope="action",
            owner="other_pack.deploy",
            visibility="private",
        )

        headers = {"Authorization": f"Bearer {token}"}

        resp = requests.get(
            f"{api_base_url}/api/v1/artifacts/{allowed_id}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 200

        resp = requests.get(
            f"{api_base_url}/api/v1/artifacts/{blocked_id}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 404

    def test_private_sensor_scoped_artifact_uses_derived_pack(self, api_base_url):
        """
        Private sensor-scoped artifact: same pack-derivation rule as action scope.
        """
        uid = _uid()
        login = f"sensor_reader_{uid}"
        token = _register_scoped_user(
            api_base_url,
            login,
            [
                {
                    "resource": "packs",
                    "actions": ["read"],
                    "constraints": {"pack_refs": ["sensor_pack"]},
                }
            ],
        )

        allowed_id = _create_artifact_via_db(
            ref=f"sensor_pack.heartbeat_{_uid()}",
            scope="sensor",
            owner="sensor_pack.heartbeat",
            visibility="private",
        )
        blocked_id = _create_artifact_via_db(
            ref=f"foreign.heartbeat_{_uid()}",
            scope="sensor",
            owner="foreign.heartbeat",
            visibility="private",
        )

        headers = {"Authorization": f"Bearer {token}"}

        resp = requests.get(
            f"{api_base_url}/api/v1/artifacts/{allowed_id}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 200

        resp = requests.get(
            f"{api_base_url}/api/v1/artifacts/{blocked_id}",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 404

    def test_list_endpoint_filters_private_artifacts(self, api_base_url):
        """
        List endpoint hides private artifacts the user cannot access but shows
        public ones and private ones the user has pack access to.
        """
        uid = _uid()
        login = f"listing_user_{uid}"
        token = _register_scoped_user(
            api_base_url,
            login,
            [
                {"resource": "artifacts", "actions": ["read"]},
                {
                    "resource": "packs",
                    "actions": ["read"],
                    "constraints": {"pack_refs": ["mine"]},
                },
            ],
        )

        # Public artifact — should be visible
        public_id = _create_artifact_via_db(
            ref=f"mine.public_{_uid()}",
            scope="pack",
            owner="mine",
            visibility="public",
        )
        # Private in user's pack — visible via packs:read
        private_mine_id = _create_artifact_via_db(
            ref=f"mine.private_{_uid()}",
            scope="pack",
            owner="mine",
            visibility="private",
        )
        # Private in foreign pack — must be hidden
        private_foreign_id = _create_artifact_via_db(
            ref=f"yours.private_{_uid()}",
            scope="pack",
            owner="yours",
            visibility="private",
        )

        headers = {"Authorization": f"Bearer {token}"}
        resp = requests.get(
            f"{api_base_url}/api/v1/artifacts?per_page=100",
            headers=headers,
            timeout=10,
        )
        assert resp.status_code == 200
        ids = [item["id"] for item in _response_data(resp)]
        assert public_id in ids
        assert private_mine_id in ids
        assert private_foreign_id not in ids

    def test_execution_token_cannot_cross_pack_mutate(self, api_base_url):
        """
        Execution token from pack_x cannot mutate artifact owned by pack_y,
        but can create in pack_x.
        """
        uid = _uid()
        login = f"exec_user_{uid}"
        _register_scoped_user(
            api_base_url,
            login,
            [{"resource": "artifacts", "actions": ["read", "update", "create"]}],
        )
        identity_id = _get_identity_id(login)

        # Mint execution token in pack_x
        exec_token = _make_execution_token(identity_id, 424242, "pack_x.deploy")

        # Create private artifact in pack_y
        art_id = _create_artifact_via_db(
            ref=f"pack_y.build_log_{_uid()}",
            scope="pack",
            owner="pack_y",
            visibility="private",
        )

        headers = {"Authorization": f"Bearer {exec_token}"}

        # Cross-pack progress append — FORBIDDEN
        resp = requests.post(
            f"{api_base_url}/api/v1/artifacts/{art_id}/progress",
            headers=headers,
            json={"entry": {"msg": "hi"}},
            timeout=10,
        )
        assert resp.status_code == 403

        # Cross-pack create — FORBIDDEN
        resp = requests.post(
            f"{api_base_url}/api/v1/artifacts",
            headers=headers,
            json={
                "ref": f"pack_y.created_{_uid()}",
                "scope": "pack",
                "owner": "pack_y",
                "type": "file_text",
                "name": "x",
            },
            timeout=10,
        )
        assert resp.status_code == 403

        # Same-pack create — OK
        resp = requests.post(
            f"{api_base_url}/api/v1/artifacts",
            headers=headers,
            json={
                "ref": f"pack_x.created_{_uid()}",
                "scope": "pack",
                "owner": "pack_x",
                "type": "file_text",
                "name": "x",
            },
            timeout=10,
        )
        assert resp.status_code == 201

    def test_dotless_action_owner_is_treated_as_malformed(self, api_base_url):
        """
        Artifact with owner 'action' (no dot) is malformed. Execution token
        from pack_x cannot mutate it — the cross-pack guard refuses.
        """
        uid = _uid()
        login = f"dotless_user_{uid}"
        _register_scoped_user(
            api_base_url,
            login,
            [{"resource": "artifacts", "actions": ["read", "update", "create"]}],
        )
        identity_id = _get_identity_id(login)

        exec_token = _make_execution_token(identity_id, 12345, "pack_x.deploy")

        # Malformed owner (no dot separator)
        art_id = _create_artifact_via_db(
            ref=f"malformed_owner_{_uid()}",
            scope="action",
            owner="action",
            visibility="private",
        )

        headers = {"Authorization": f"Bearer {exec_token}"}
        resp = requests.post(
            f"{api_base_url}/api/v1/artifacts/{art_id}/progress",
            headers=headers,
            json={"entry": {"msg": "hi"}},
            timeout=10,
        )
        assert resp.status_code == 403

    def test_execution_token_with_empty_action_ref_is_refused(self, api_base_url):
        """
        Execution token with empty action_ref cannot derive a token pack;
        cross-pack writes are refused with 403.
        """
        uid = _uid()
        login = f"empty_ref_user_{uid}"
        _register_scoped_user(
            api_base_url,
            login,
            [{"resource": "artifacts", "actions": ["read", "update", "create"]}],
        )
        identity_id = _get_identity_id(login)

        # Empty action_ref
        exec_token = _make_execution_token(identity_id, 99999, "")

        # Pack-scoped artifact
        art_id = _create_artifact_via_db(
            ref=f"pack_z.log_{_uid()}",
            scope="pack",
            owner="pack_z",
            visibility="private",
        )

        headers = {"Authorization": f"Bearer {exec_token}"}

        # Progress append — FORBIDDEN
        resp = requests.post(
            f"{api_base_url}/api/v1/artifacts/{art_id}/progress",
            headers=headers,
            json={"entry": {"msg": "hi"}},
            timeout=10,
        )
        assert resp.status_code == 403

        # Create in pack — FORBIDDEN
        resp = requests.post(
            f"{api_base_url}/api/v1/artifacts",
            headers=headers,
            json={
                "ref": f"pack_z.created_{_uid()}",
                "scope": "pack",
                "owner": "pack_z",
                "type": "file_text",
                "name": "x",
            },
            timeout=10,
        )
        assert resp.status_code == 403
