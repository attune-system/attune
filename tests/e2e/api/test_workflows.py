"""
API Tests: Workflow CRUD & Filtering

Ported from crates/api/tests/workflow_tests.rs
Tests workflow creation, retrieval, listing, update, delete, and validation.
"""

import uuid

import pytest


def _uid():
    return uuid.uuid4().hex[:8]


def _create_pack(client, suffix=None):
    """Create a unique pack and return its data."""
    ref = f"wftest_{suffix or _uid()}"
    resp = client.session.post(
        f"{client.base_url}/api/v1/packs",
        json={
            "ref": ref,
            "label": f"WF Test Pack {ref}",
            "version": "1.0.0",
            "description": "Pack for workflow tests",
        },
        timeout=client.timeout,
    )
    resp.raise_for_status()
    return resp.json()["data"]


def _create_workflow(client, pack_ref, wf_ref, **overrides):
    """Create a workflow via API and return response object (raw)."""
    payload = {
        "ref": wf_ref,
        "pack_ref": pack_ref,
        "label": overrides.pop("label", "Test Workflow"),
        "description": overrides.pop("description", "A test workflow"),
        "version": overrides.pop("version", "1.0.0"),
        "definition": overrides.pop(
            "definition",
            {
                "version": "1.0.0",
                "tasks": [
                    {
                        "name": "task1",
                        "action": "core.echo",
                        "input": {"message": "Hello"},
                    }
                ],
            },
        ),
        "tags": overrides.pop("tags", []),
    }
    payload.update(overrides)
    return client.session.post(
        f"{client.base_url}/api/v1/workflows",
        json=payload,
        timeout=client.timeout,
    )


@pytest.mark.api
class TestCreateWorkflow:
    """Workflow creation endpoint tests."""

    def test_create_workflow_success(self, client):
        pack = _create_pack(client)
        uid = _uid()
        wf_ref = f"{pack['ref']}.wf_{uid}"

        resp = _create_workflow(
            client,
            pack["ref"],
            wf_ref,
            label="Test Workflow",
            tags=["test", "automation"],
        )

        assert resp.status_code == 201
        body = resp.json()
        assert body["data"]["ref"] == wf_ref
        assert body["data"]["label"] == "Test Workflow"
        assert body["data"]["version"] == "1.0.0"
        assert len(body["data"]["tags"]) == 2

    def test_create_workflow_duplicate_ref(self, client):
        pack = _create_pack(client)
        uid = _uid()
        wf_ref = f"{pack['ref']}.dup_{uid}"

        # Create first workflow
        resp1 = _create_workflow(client, pack["ref"], wf_ref)
        assert resp1.status_code == 201

        # Attempt duplicate
        resp2 = _create_workflow(client, pack["ref"], wf_ref)
        assert resp2.status_code == 409

    def test_create_workflow_pack_not_found(self, client):
        resp = _create_workflow(
            client,
            "nonexistent_pack_xyz",
            "nonexistent_pack_xyz.workflow",
        )
        assert resp.status_code == 404

    def test_create_workflow_requires_auth(self, api_base_url):
        """Unauthenticated requests should be rejected."""
        import requests

        s = requests.Session()
        resp = s.post(
            f"{api_base_url}/api/v1/workflows",
            json={
                "ref": "test.workflow",
                "pack_ref": "test",
                "label": "Test",
                "version": "1.0.0",
                "definition": {"tasks": []},
            },
            timeout=10,
        )
        # Should be 401 (auth required)
        assert resp.status_code == 401


@pytest.mark.api
class TestGetWorkflow:
    """Workflow retrieval endpoint tests."""

    def test_get_workflow_by_ref(self, client):
        pack = _create_pack(client)
        uid = _uid()
        wf_ref = f"{pack['ref']}.get_{uid}"

        _create_workflow(client, pack["ref"], wf_ref, label="My Workflow")

        resp = client.session.get(
            f"{client.base_url}/api/v1/workflows/{wf_ref}",
            timeout=client.timeout,
        )
        assert resp.status_code == 200
        body = resp.json()
        assert body["data"]["ref"] == wf_ref
        assert body["data"]["label"] == "My Workflow"
        assert body["data"]["version"] == "1.0.0"

    def test_get_workflow_not_found(self, client):
        resp = client.session.get(
            f"{client.base_url}/api/v1/workflows/nonexistent.workflow_{_uid()}",
            timeout=client.timeout,
        )
        assert resp.status_code == 404


@pytest.mark.api
class TestListWorkflows:
    """Workflow listing and filtering tests."""

    def test_list_workflows(self, client):
        pack = _create_pack(client)

        # Create 3 workflows
        for i in range(1, 4):
            wf_ref = f"{pack['ref']}.list_{_uid()}_{i}"
            resp = _create_workflow(client, pack["ref"], wf_ref, label=f"Workflow {i}")
            assert resp.status_code == 201

        # List filtered by pack_ref
        resp = client.session.get(
            f"{client.base_url}/api/v1/workflows",
            params={"pack_ref": pack["ref"], "page": 1, "per_page": 10},
            timeout=client.timeout,
        )
        assert resp.status_code == 200
        body = resp.json()
        assert len(body["data"]) == 3
        assert body["pagination"]["total_items"] == 3

    def test_list_workflows_by_pack(self, client):
        pack1 = _create_pack(client)
        pack2 = _create_pack(client)

        # 2 workflows for pack1
        for i in range(1, 3):
            _create_workflow(client, pack1["ref"], f"{pack1['ref']}.bp_{_uid()}_{i}")
        # 1 workflow for pack2
        _create_workflow(client, pack2["ref"], f"{pack2['ref']}.bp_{_uid()}")

        # List for pack1 only
        resp = client.session.get(
            f"{client.base_url}/api/v1/workflows",
            params={"pack_ref": pack1["ref"]},
            timeout=client.timeout,
        )
        assert resp.status_code == 200
        body = resp.json()
        workflows = body["data"]
        assert len(workflows) == 2
        assert all(w["pack_ref"] == pack1["ref"] for w in workflows)

    def test_list_workflows_filter_by_tag(self, client):
        pack = _create_pack(client)

        _create_workflow(client, pack["ref"], f"{pack['ref']}.tag1_{_uid()}", tags=["incident", "approval"])
        _create_workflow(client, pack["ref"], f"{pack['ref']}.tag2_{_uid()}", tags=["incident"])
        _create_workflow(client, pack["ref"], f"{pack['ref']}.tag3_{_uid()}", tags=["automation"])

        # Filter by tag "incident"
        resp = client.session.get(
            f"{client.base_url}/api/v1/workflows",
            params={"tags": "incident", "pack_ref": pack["ref"]},
            timeout=client.timeout,
        )
        assert resp.status_code == 200
        body = resp.json()
        assert len(body["data"]) == 2


@pytest.mark.api
class TestUpdateWorkflow:
    """Workflow update endpoint tests."""

    def test_update_workflow(self, client):
        pack = _create_pack(client)
        uid = _uid()
        wf_ref = f"{pack['ref']}.upd_{uid}"

        _create_workflow(client, pack["ref"], wf_ref, label="Original Label")

        # Update
        resp = client.session.put(
            f"{client.base_url}/api/v1/workflows/{wf_ref}",
            json={
                "label": "Updated Label",
                "description": "Updated description",
                "version": "1.1.0",
            },
            timeout=client.timeout,
        )
        assert resp.status_code == 200
        body = resp.json()
        assert body["data"]["label"] == "Updated Label"
        assert body["data"]["description"] == "Updated description"
        assert body["data"]["version"] == "1.1.0"

    def test_update_workflow_not_found(self, client):
        resp = client.session.put(
            f"{client.base_url}/api/v1/workflows/nonexistent.wf_{_uid()}",
            json={"label": "Updated Label"},
            timeout=client.timeout,
        )
        assert resp.status_code == 404


@pytest.mark.api
class TestDeleteWorkflow:
    """Workflow delete endpoint tests."""

    def test_delete_workflow(self, client):
        pack = _create_pack(client)
        uid = _uid()
        wf_ref = f"{pack['ref']}.del_{uid}"

        _create_workflow(client, pack["ref"], wf_ref)

        # Delete
        resp = client.session.delete(
            f"{client.base_url}/api/v1/workflows/{wf_ref}",
            timeout=client.timeout,
        )
        assert resp.status_code == 200

        # Verify gone
        resp = client.session.get(
            f"{client.base_url}/api/v1/workflows/{wf_ref}",
            timeout=client.timeout,
        )
        assert resp.status_code == 404

    def test_delete_workflow_not_found(self, client):
        resp = client.session.delete(
            f"{client.base_url}/api/v1/workflows/nonexistent.wf_{_uid()}",
            timeout=client.timeout,
        )
        assert resp.status_code == 404


@pytest.mark.api
class TestWorkflowValidation:
    """Workflow input validation tests."""

    def test_create_workflow_empty_ref(self, client):
        pack = _create_pack(client)
        resp = _create_workflow(client, pack["ref"], "", label="Test")
        assert 400 <= resp.status_code < 500

    def test_create_workflow_empty_label(self, client):
        pack = _create_pack(client)
        wf_ref = f"{pack['ref']}.val_{_uid()}"
        resp = _create_workflow(client, pack["ref"], wf_ref, label="")
        assert 400 <= resp.status_code < 500
