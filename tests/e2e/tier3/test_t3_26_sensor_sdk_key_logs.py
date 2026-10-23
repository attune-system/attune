"""Managed sensor SDK reads must protect log values with recorded Key origins."""

import os
import json
import uuid

import pytest

from helpers import AttuneClient, unique_ref, wait_for_condition


@pytest.mark.tier3
@pytest.mark.integration
@pytest.mark.security
@pytest.mark.rbac
def test_sensor_sdk_key_reads_require_source_authority_for_logs(
    client: AttuneClient, api_base_url: str, test_timeout: int, pack_ref: str
):
    sensor_ref = f"{pack_ref}.sdk_log_reader"
    sentinel = os.getenv("ATTUNE_E2E_SDK_LOG_SENTINEL") or f"sdk-log-fixture-{uuid.uuid4().hex}"
    key_ref = rule_ref = reader_identity = None
    admin_log_assignment = None
    reader = None

    def assign(permission_ref):
        response = client.post("/api/v1/permissions/assignments", json={
            "identity_id": reader_identity,
            "permission_set_ref": f"{pack_ref}.{permission_ref}",
        })
        assert response.status_code == 201

    try:
        identity = client.get("/auth/me")
        assert identity.status_code == 200
        assignment = client.post("/api/v1/permissions/assignments", json={
            "identity_id": identity.json()["data"]["id"],
            "permission_set_ref": f"{pack_ref}.sdk_log_reader",
        })
        assert assignment.status_code == 201
        admin_log_assignment = assignment.json()["data"]["id"]
        response = client.post("/api/v1/keys", json={
            "local_ref": f"sdk_log_{unique_ref()}",
            "owner_type": "pack", "owner_pack_ref": pack_ref,
            "name": "Synthetic sensor SDK log fixture",
            "encrypted": True, "value": sentinel,
        })
        assert response.status_code == 201
        key_ref = response.json()["data"]["ref"]
        rule = client.create_rule(
            ref=f"{pack_ref}.sdk_log_rule_{unique_ref()}",
            label="SDK Log Probe Rule", pack_ref=pack_ref,
            trigger_ref=f"{pack_ref}.sdk_log_probe", action_ref=f"{pack_ref}.echo",
            trigger_params={"key_ref": key_ref},
            action_params={"message": "Synthetic SDK log probe"},
        )
        rule_ref = rule["ref"]

        def logged():
            response = client.get(f"/api/v1/sensors/{sensor_ref}/logs/stdout")
            return response.status_code == 200 and sentinel in response.text

        wait_for_condition(logged, timeout=90,
            error_message="Managed sensor did not complete its synthetic SDK Key read")
        login = f"sdk-log-{unique_ref()}@attune.local"
        registration = client.register(login=login,
            password="SDK-log-fixture-password-123!", display_name="SDK Log Fixture")
        reader_identity = registration["user"]["id"]
        assign("sdk_log_reader")
        reader = AttuneClient(base_url=api_base_url, timeout=test_timeout, auto_login=False)
        reader.login(login=login, password="SDK-log-fixture-password-123!")

        for stream in ("stdout", "stderr"):
            response = reader.get(f"/api/v1/sensors/{sensor_ref}/logs/{stream}")
            assert response.status_code == 403
            assert sentinel not in response.text
        assign("sdk_key_reader")
        response = reader.get(f"/api/v1/sensors/{sensor_ref}/logs/stdout")
        assert response.status_code == 403
        assert sentinel not in response.text
        assign("sdk_key_decrypt")
        for stream in ("stdout", "stderr"):
            response = reader.get(f"/api/v1/sensors/{sensor_ref}/logs/{stream}")
            assert response.status_code == 200
            assert sentinel in response.text
        audit = client.get("/api/v1/audit-events", params={"per_page": 100})
        assert audit.status_code == 200
        assert sentinel not in audit.text
    finally:
        if admin_log_assignment is not None:
            response = client.delete(f"/api/v1/permissions/assignments/{admin_log_assignment}")
            assert response.status_code in (200, 404)
        if reader is not None:
            reader.logout()
        if reader_identity is not None:
            response = client.delete(f"/api/v1/identities/{reader_identity}")
            assert response.status_code in (200, 404)
        if rule_ref is not None:
            response = client.delete(f"/api/v1/rules/{rule_ref}")
            assert response.status_code in (200, 404)
        if key_ref is not None:
            response = client.delete(f"/api/v1/keys/{key_ref}")
            assert response.status_code in (200, 404)


@pytest.mark.tier3
@pytest.mark.integration
@pytest.mark.security
def test_secret_sensor_stdin_live_delivery_restart_and_historical_disclosure(
    client: AttuneClient, api_base_url: str, test_timeout: int, pack_ref: str
):
    sensor_ref = f"{pack_ref}.secret_lifecycle"
    trigger_ref = f"{pack_ref}.secret_lifecycle_probe"
    sentinel = os.getenv("ATTUNE_E2E_SDK_LOG_SENTINEL") or f"sensor-secret-{uuid.uuid4().hex}"
    keys = []
    assignments = []
    rule_ref = reader_identity = None
    reader = None
    input_refs = [f"{pack_ref}.sdk_key_reader", f"{pack_ref}.sdk_key_decrypt"]

    def create_key(suffix):
        response = client.post("/api/v1/keys", json={
            "local_ref": f"lifecycle_{unique_ref()}", "owner_type": "pack",
            "owner_pack_ref": pack_ref, "name": f"Synthetic lifecycle fixture {suffix}",
            "encrypted": True, "value": f"{sentinel}-{suffix}",
        })
        assert response.status_code == 201
        reference = response.json()["data"]["ref"]
        keys.append(reference)
        return reference

    def selector(reference):
        return "{{ keystore['" + reference + "'] }}"

    def logs(marker=None, value=None):
        response = client.get(f"/api/v1/sensors/{sensor_ref}/logs/stdout")
        if response.status_code != 200:
            return []
        records = []
        for line in response.text.splitlines():
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if isinstance(record, dict) and (marker is None or record.get("marker") == marker):
                if value is None or record.get("config", {}).get("credential") == value:
                    records.append(record)
        return records

    try:
        identity = client.get("/auth/me").json()["data"]["id"]
        response = client.post("/api/v1/permissions/assignments", json={
            "identity_id": identity, "permission_set_ref": f"{pack_ref}.sdk_log_reader",
        })
        assert response.status_code == 201
        assignments.append(response.json()["data"]["id"])
        config_key = create_key("config")
        initial_key = create_key("initial")
        live_key = create_key("live")
        response = client.put(f"/api/v1/sensors/{sensor_ref}", json={"config": {
            "credential": selector(config_key), "input_permission_set_refs": input_refs,
        }})
        assert response.status_code == 200
        rule_ref = f"{pack_ref}.secret_lifecycle_fixture"
        response = client.post("/api/v1/rules", json={
            "ref": rule_ref, "label": "Synthetic secret lifecycle rule", "pack_ref": pack_ref,
            "trigger_ref": trigger_ref, "action_ref": f"{pack_ref}.echo",
            "trigger_params": {"credential": selector(initial_key)},
            "action_params": {"message": "Synthetic lifecycle event"},
            "permission_set_refs": [],
        })
        assert response.status_code == 201
        response = client.post(f"/api/v1/sensors/{sensor_ref}/enable")
        assert response.status_code == 200
        wait_for_condition(lambda: logs("secret-initial-rule", f"{sentinel}-initial"),
            timeout=120, error_message="Secret initial rule configuration was not delivered through stdin")
        bootstrap = logs("secret-bootstrap", f"{sentinel}-config")[-1]
        assert bootstrap["config"]["count"] == 3
        initial = logs("secret-initial-rule", f"{sentinel}-initial")[-1]
        assert initial["config"]["count"] == 7
        original_generation = initial["generation"]
        for path in (f"/api/v1/sensors/{sensor_ref}", f"/api/v1/rules/{rule_ref}"):
            response = client.get(path)
            assert response.status_code == 200
            assert sentinel not in response.text

        summary = client.get(f"/api/v1/sensors/{sensor_ref}/logs").json()
        artifact_id = next(entry["artifact_id"] for entry in summary["logs"] if entry["stream"] == "stdout")
        versions = client.get(f"/api/v1/artifacts/{artifact_id}/versions")
        assert versions.status_code == 200
        original_version = versions.json()["items"][0]["version"]

        response = client.put(f"/api/v1/rules/{rule_ref}", json={
            "trigger_params": {"credential": selector(live_key), "rotate": True},
        })
        assert response.status_code == 200
        wait_for_condition(lambda: logs("secret-live-rule", f"{sentinel}-live"),
            timeout=90, error_message="Updated secret rule configuration was not delivered")
        assert logs("secret-live-rule", f"{sentinel}-live")[-1]["generation"] == original_generation
        if os.getenv("ATTUNE_E2E_SDK_LOG_SENTINEL"):
            wait_for_condition(lambda: len(client.get(f"/api/v1/artifacts/{artifact_id}/versions").json()["items"]) >= 2,
                timeout=30, error_message="Protected sensor logs did not rotate in the admitted generation")
        response = client.put(f"/api/v1/rules/{rule_ref}", json={
            "trigger_params": {"credential": selector(live_key)},
        })
        assert response.status_code == 200
        wait_for_condition(lambda: any("rotate" not in record["config"]
            for record in logs("secret-live-rule", f"{sentinel}-live")),
            timeout=30, error_message="Sensor did not apply the post-rotation rule update")
        response = client.put(f"/api/v1/sensors/{sensor_ref}", json={"config": {
            "credential": selector(config_key), "count": 4, "input_permission_set_refs": input_refs,
        }})
        assert response.status_code == 200
        wait_for_condition(lambda: any(record["generation"] != original_generation
            and record["config"]["count"] == 4 for record in logs("secret-bootstrap", f"{sentinel}-config")),
            timeout=120, error_message="Sensor restart did not deliver a new protected bootstrap")

        login = f"secret-lifecycle-{unique_ref()}@attune.local"
        registration = client.register(login=login, password="Secret-lifecycle-fixture-123!", display_name="Secret Lifecycle Fixture")
        reader_identity = registration["user"]["id"]
        reader = AttuneClient(base_url=api_base_url, timeout=test_timeout, auto_login=False)
        reader.login(login=login, password="Secret-lifecycle-fixture-123!")
        for permission in ("sdk_log_reader", "sdk_key_reader", "sdk_key_decrypt"):
            response = client.post("/api/v1/permissions/assignments", json={
                "identity_id": reader_identity, "permission_set_ref": f"{pack_ref}.{permission}",
            })
            assert response.status_code == 201
            assignments.append(response.json()["data"]["id"])
            historical = reader.get(f"/api/v1/artifacts/{artifact_id}/versions/{original_version}/download")
            if permission != "sdk_key_decrypt":
                assert historical.status_code == 403
                assert sentinel not in historical.text
            else:
                assert historical.status_code == 200
                assert f"{sentinel}-initial" in historical.text
        response = client.get("/api/v1/events", params={"trigger_ref": trigger_ref})
        assert response.status_code == 200
        assert sentinel not in response.text
        audit = client.get("/api/v1/audit-events", params={"per_page": 100})
        assert audit.status_code == 200
        assert sentinel not in audit.text
        # Keep a newer sealed version before lowering retention. Pending active
        # streams do not count as retained ready versions.
        response = client.put(f"/api/v1/rules/{rule_ref}", json={
            "trigger_params": {"credential": selector(live_key), "rotate": True},
        })
        assert response.status_code == 200

        def newer_version_ready():
            response = client.get(f"/api/v1/artifacts/{artifact_id}/versions")
            return response.status_code == 200 and any(
                version["version"] > original_version and version.get("body_state") == "ready"
                for version in response.json()["items"]
            )

        wait_for_condition(newer_version_ready, timeout=60,
            error_message="Rotation did not seal a newer protected sensor log version")
        response = client.put(f"/api/v1/rules/{rule_ref}", json={
            "trigger_params": {"credential": selector(live_key), "count": 19},
        })
        assert response.status_code == 200
        wait_for_condition(lambda: any(record["config"].get("count") == 19
            for record in logs("secret-live-rule", f"{sentinel}-live")),
            timeout=30, error_message="Sensor did not stop the retention fixture rotation")
        response = client.put(f"/api/v1/sensors/{sensor_ref}", json={
            "log_retention_policy": {"op": "set", "value": "versions"},
            "log_retention_limit": {"op": "set", "value": 1},
            "config": {"credential": selector(config_key), "count": 12,
                "input_permission_set_refs": input_refs},
        })
        assert response.status_code == 200
        wait_for_condition(lambda: any(record["config"].get("count") == 12
            for record in logs("secret-bootstrap", f"{sentinel}-config")),
            timeout=120, error_message="Sensor did not restart with the retention override")

        def historical_version_reclaimed():
            response = client.get(f"/api/v1/artifacts/{artifact_id}/versions")
            return response.status_code == 200 and all(
                version["version"] != original_version
                for version in response.json()["items"]
            )

        wait_for_condition(historical_version_reclaimed, timeout=120,
            error_message="Supervisor did not reclaim the protected historical sensor log version")
        historical = reader.get(f"/api/v1/artifacts/{artifact_id}/versions/{original_version}/download")
        assert historical.status_code == 404
        assert sentinel not in historical.text
        wait_for_condition(lambda: reader.get(f"/api/v1/sensors/{sensor_ref}/logs/stdout").status_code == 200,
            timeout=30, error_message="Retention made the current protected sensor logs unreadable to the authorized reader")
    finally:
        response = client.post(f"/api/v1/sensors/{sensor_ref}/disable")
        assert response.status_code in (200, 404)
        if rule_ref is not None:
            response = client.delete(f"/api/v1/rules/{rule_ref}")
            assert response.status_code in (200, 404)
        if reader is not None:
            reader.logout()
        for assignment in reversed(assignments):
            response = client.delete(f"/api/v1/permissions/assignments/{assignment}")
            assert response.status_code in (200, 404)
        if reader_identity is not None:
            response = client.delete(f"/api/v1/identities/{reader_identity}")
            assert response.status_code in (200, 404)
        for key in keys:
            response = client.delete(f"/api/v1/keys/{key}")
            assert response.status_code in (200, 404)
