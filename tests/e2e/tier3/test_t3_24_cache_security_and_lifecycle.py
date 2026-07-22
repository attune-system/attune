"""T3.24: Cache authorization at action, sensor, audit, and lifecycle boundaries."""

import json
import os

import pytest

from e2e.cache_helpers import (
    CACHE_OWNER_TYPE,
    assert_error_status,
    assertion_id,
    cache_entries,
    cache_namespace,
    cache_namespace_ref,
    cache_refresh_ref,
    generation_id,
    high_entropy_sentinel,
    publish_generation,
    response_status,
    wait_for_cursor_expiry,
)
from helpers import AttuneClient, unique_ref, wait_for_condition, wait_for_execution_status
from helpers.client_wrapper import CacheApiError

pytestmark = pytest.mark.usefixtures("cache_e2e_retention_config")


def _new_user(
    admin: AttuneClient,
    *,
    api_base_url: str,
    test_timeout: int,
    prefix: str,
    permission_set_ref: str | None = None,
) -> tuple[AttuneClient, int]:
    login = f"{prefix}_{unique_ref()}@attune.local"
    password = f"{prefix}-E2E-password-123!"
    registration = admin.register(login=login, password=password, display_name=prefix)
    identity_id = registration["user"]["id"]
    if permission_set_ref:
        assignment = admin.post(
            "/api/v1/permissions/assignments",
            json={
                "identity_id": identity_id,
                "permission_set_ref": permission_set_ref,
            },
        )
        assert assignment.status_code == 201, assignment.text
    user = AttuneClient(base_url=api_base_url, timeout=test_timeout, auto_login=False)
    user.login(login=login, password=password)
    return user, identity_id


def _delete_identity(admin: AttuneClient, identity_id: int) -> None:
    response = admin.delete(f"/api/v1/identities/{identity_id}")
    assert response.status_code in (200, 404), response.text


def _execution_text(execution: dict) -> str:
    return json.dumps(execution, sort_keys=True, default=str)


def _list_audit_text(client: AttuneClient) -> str:
    response = client.get("/api/v1/audit-events", params={"per_page": 100})
    assert response.status_code == 200, response.text
    return response.text


@pytest.mark.tier3
@pytest.mark.integration
@pytest.mark.cache
@pytest.mark.security
@pytest.mark.rbac
def test_cache_identity_execution_and_sensor_scopes_do_not_disclose_payloads(
    client: AttuneClient,
    api_base_url: str,
    test_timeout: int,
    pack_ref: str,
):
    """Use real identity, execution, and sensor tokens rather than test-minted JWTs."""
    shared_namespace = cache_namespace_ref("shared-scope")
    other_pack_ref = cache_namespace_ref("cache-owner-pack")
    other_pack = client.create_pack(
        ref=other_pack_ref,
        label="Cache scope isolation owner",
        description="E2E owner for cache authorization isolation",
    )
    assert other_pack["ref"] == other_pack_ref
    restricted = authorized = None
    restricted_identity = authorized_identity = None
    sentinel = high_entropy_sentinel()
    try:
        with cache_namespace(
            client,
            owner_ref=pack_ref,
            prefix="shared-scope",
            namespace=shared_namespace,
        ), cache_namespace(
            client,
            owner_ref=other_pack_ref,
            prefix="shared-scope",
            namespace=shared_namespace,
        ):
            owner_generation, owner_refresh = publish_generation(
                client,
                owner_ref=pack_ref,
                namespace=shared_namespace,
                entries=cache_entries(
                    3, prefix="visible", revision="owner", sentinel=sentinel
                ),
                expected_active_generation_id=None,
            )
            other_generation, other_refresh = publish_generation(
                client,
                owner_ref=other_pack_ref,
                namespace=shared_namespace,
                entries=cache_entries(
                    3, prefix="visible", revision="other", sentinel=sentinel
                ),
                expected_active_generation_id=None,
            )
            owner_assertion = assertion_id(shared_namespace, owner_refresh)
            other_assertion = assertion_id(shared_namespace, other_refresh)

            restricted, restricted_identity = _new_user(
                client,
                api_base_url=api_base_url,
                test_timeout=test_timeout,
                prefix="cache_restricted",
            )
            authorized, authorized_identity = _new_user(
                client,
                api_base_url=api_base_url,
                test_timeout=test_timeout,
                prefix="cache_authorized",
                permission_set_ref=f"{pack_ref}.cache_reader",
            )

            for operation_name, operation in (
                (
                    "lookup",
                    lambda: restricted.cache_lookup(
                        owner_type=CACHE_OWNER_TYPE,
                        owner_ref=pack_ref,
                        namespace=shared_namespace,
                        external_id="visible-000000",
                    ),
                ),
                (
                    "count-via-scan",
                    lambda: restricted.cache_scan(
                        owner_type=CACHE_OWNER_TYPE,
                        owner_ref=pack_ref,
                        namespace=shared_namespace,
                        page_size=1,
                    ),
                ),
                (
                    "generation-metadata",
                    lambda: restricted.cache_list_generations(
                        owner_type=CACHE_OWNER_TYPE,
                        owner_ref=pack_ref,
                        namespace=shared_namespace,
                    ),
                ),
                (
                    "namespace-metadata",
                    lambda: restricted.cache_get_namespace(
                        owner_type=CACHE_OWNER_TYPE,
                        owner_ref=pack_ref,
                        namespace=shared_namespace,
                    ),
                ),
            ):
                assert_error_status(
                    operation,
                    expected={403, 404},
                    assertion=f"restricted {operation_name} must not reveal {owner_assertion}",
                )

            authorized_lookup = authorized.cache_lookup(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=shared_namespace,
                external_id="visible-000000",
            )
            assert generation_id(authorized_lookup) == owner_generation, owner_assertion
            admin_lookup = client.cache_lookup(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=other_pack_ref,
                namespace=shared_namespace,
                external_id="visible-000000",
            )
            assert generation_id(admin_lookup) == other_generation, other_assertion
            assert (
                authorized_lookup.get("item", authorized_lookup.get("entry"))["value"][
                    "revision"
                ]
                == "owner"
            ), owner_assertion

            action_ref = f"{pack_ref}.cache_read"
            action_execution = client.create_execution(
                action_ref=action_ref,
                parameters={
                    "namespace": shared_namespace,
                    "external_id": "visible-000000",
                },
                permission_set_refs=["standard"],
            )
            completed = wait_for_execution_status(
                client,
                action_execution["id"],
                expected_status="completed",
                timeout=30,
            )
            action_text = _execution_text(completed)
            assert "cache-read-ok" in action_text, (
                f"standard execution cache read failed: execution={action_execution['id']} "
                f"{owner_assertion}"
            )
            assert "stdin_cache_payload\": false" in action_text, (
                f"cache data was injected into action stdin: execution={action_execution['id']}"
            )
            assert sentinel not in action_text, (
                f"cache payload leaked into action output/metadata: execution={action_execution['id']}"
            )

            with cache_namespace(
                client,
                owner_ref=action_ref,
                owner_type="action",
                prefix="action-scope",
            ) as action_namespace:
                action_generation, action_refresh = publish_generation(
                    client,
                    owner_ref=action_ref,
                    owner_type="action",
                    namespace=action_namespace,
                    entries=cache_entries(1, prefix="action-owned", revision="action"),
                    expected_active_generation_id=None,
                )
                action_scoped_execution = client.create_execution(
                    action_ref=action_ref,
                    parameters={
                        "namespace": action_namespace,
                        "external_id": "action-owned-000000",
                        "owner_type": "action",
                        "owner_ref": action_ref,
                    },
                    permission_set_refs=["standard"],
                )
                action_scoped_completed = wait_for_execution_status(
                    client,
                    action_scoped_execution["id"],
                    expected_status="completed",
                    timeout=30,
                )
                action_scoped_text = _execution_text(action_scoped_completed)
                assert "cache-read-ok" in action_scoped_text, (
                    "standard execution token could not read its action scope: "
                    f"execution={action_scoped_execution['id']} "
                    f"{assertion_id(action_namespace, action_refresh)}"
                )
                assert str(action_generation) in action_scoped_text

            denied_execution = client.create_execution(
                action_ref=action_ref,
                parameters={
                    "namespace": shared_namespace,
                    "external_id": "visible-000000",
                    "owner_ref": other_pack_ref,
                },
                permission_set_refs=["standard"],
            )
            denied_completed = wait_for_execution_status(
                client,
                denied_execution["id"],
                expected_status="completed",
                timeout=30,
            )
            assert "cache-read-denied" in _execution_text(denied_completed), (
                f"standard token crossed pack scope: execution={denied_execution['id']} "
                f"{other_assertion}"
            )

            no_token_execution = client.create_execution(
                action_ref=action_ref,
                parameters={
                    "namespace": shared_namespace,
                    "external_id": "visible-000000",
                },
                permission_set_refs=[],
            )
            no_token_completed = wait_for_execution_status(
                client,
                no_token_execution["id"],
                expected_status="completed",
                timeout=30,
            )
            no_token_text = _execution_text(no_token_completed)
            assert "cache-token-missing" in no_token_text, (
                f"empty permission_set_refs unexpectedly received a token: "
                f"execution={no_token_execution['id']}"
            )
            assert sentinel not in no_token_text

            sensor_rule = client.create_rule(
                ref=f"{pack_ref}.cache_sensor_rule_{unique_ref()}",
                label="Cache Sensor Probe Rule",
                pack_ref=pack_ref,
                trigger_ref=f"{pack_ref}.cache_probe",
                action_ref=action_ref,
                enabled=True,
                trigger_params={
                    "namespace": shared_namespace,
                    "external_id": "visible-000000",
                    "denied_owner_ref": other_pack_ref,
                },
                action_params={
                    "namespace": shared_namespace,
                    "external_id": "visible-000000",
                },
            )
            assert sensor_rule["enabled"] is True
            sensor_ref = f"{pack_ref}.cache_reader"
            sensor_log = ""

            def sensor_probe_logged() -> bool:
                nonlocal sensor_log
                sensor_log = client.get_sensor_log(sensor_ref)
                return "cache-sensor-probe" in sensor_log

            wait_for_condition(
                sensor_probe_logged,
                timeout=45,
                error_message=f"Managed sensor did not log cache probe: sensor={sensor_ref}",
            )
            sensor_line = next(
                line for line in sensor_log.splitlines() if "cache-sensor-probe" in line
            )
            sensor_probe = json.loads(sensor_line)
            assert sensor_probe["read_status"] == 200, f"sensor read scope {owner_assertion}"
            assert sensor_probe["other_scope_status"] in {403, 404}, (
                f"sensor crossed pack scope {other_assertion}: {sensor_probe}"
            )
            assert sensor_probe["write_status"] == 403, (
                f"sensor standard token wrote cache: {sensor_probe}"
            )
            assert sentinel not in sensor_log, "cache payload leaked into sensor log metadata"

            audit_text = _list_audit_text(client)
            assert sentinel not in audit_text, (
                f"cache payload leaked into audit/service error metadata: {owner_assertion}"
            )
    finally:
        if restricted is not None:
            restricted.logout()
        if authorized is not None:
            authorized.logout()
        if restricted_identity is not None:
            _delete_identity(client, restricted_identity)
        if authorized_identity is not None:
            _delete_identity(client, authorized_identity)
        pack_delete = None

        def pack_deleted_after_cache_cleanup() -> bool:
            nonlocal pack_delete
            pack_delete = client._request("DELETE", f"/api/v1/packs/{other_pack_ref}")
            if pack_delete.status_code in (200, 204, 404):
                return True
            if pack_delete.status_code in (409, 422):
                return False
            raise AssertionError(pack_delete.text)

        wait_for_condition(
            pack_deleted_after_cache_cleanup,
            timeout=45,
            error_message=(
                "Cache owner pack remained referenced after namespace cleanup: "
                f"pack={other_pack_ref} response={pack_delete.text if pack_delete else '-'}"
            ),
        )


@pytest.mark.tier3
@pytest.mark.integration
@pytest.mark.cache
@pytest.mark.security
@pytest.mark.slow
def test_cache_lifecycle_retains_pinned_snapshot_then_expires_and_cleans(
    client: AttuneClient, pack_ref: str
):
    """Poll the deployed supervisor lifecycle; never inspect or clean cache SQL directly."""
    with cache_namespace(
        client,
        owner_ref=pack_ref,
        prefix="lifecycle",
        policy={
            "freshness_target_seconds": 1,
            "max_retained_generations": 2,
            "max_records_per_generation": 20,
        },
    ) as namespace:
        first_generation, first_refresh = publish_generation(
            client,
            owner_ref=pack_ref,
            namespace=namespace,
            entries=cache_entries(4, prefix="lifecycle", revision="first"),
            expected_active_generation_id=None,
            chunk_size=2,
        )
        first_page = client.cache_scan(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            page_size=1,
        )
        cursor = first_page.get("next_cursor")
        assert cursor, f"pinned scan setup {assertion_id(namespace, first_refresh)}"

        second_generation, second_refresh = publish_generation(
            client,
            owner_ref=pack_ref,
            namespace=namespace,
            entries=cache_entries(4, prefix="lifecycle", revision="second"),
            expected_active_generation_id=first_generation,
            chunk_size=2,
        )
        pinned = client.cache_scan(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            page_size=1,
            generation_id=first_generation,
            cursor=cursor,
        )
        assert generation_id(pinned) == first_generation, (
            f"retired generation was not pinned-readable "
            f"{assertion_id(namespace, first_refresh)}"
        )
        assert pinned.get("items", pinned.get("entries"))[0]["value"]["revision"] == "first"

        abandoned_refresh = cache_refresh_ref("expired-staging")
        abandoned_generation = generation_id(
            client.cache_create_generation(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                client_refresh_id=abandoned_refresh,
                expected_active_generation_id=second_generation,
                expected_chunk_count=1,
                expected_record_count=1,
            )
        )
        client.cache_abandon_generation(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            generation_id=abandoned_generation,
        )

        wait_for_cursor_expiry(
            client,
            owner_ref=pack_ref,
            namespace=namespace,
            generation=first_generation,
            cursor=cursor,
        )

        def expired_generations_cleaned_in_batches() -> bool:
            generations = client.cache_list_generations(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
            )
            by_id = {
                generation_id(item): item
                for item in generations
                if item.get("generation_id", item.get("id")) is not None
            }
            active = by_id.get(second_generation)
            return (
                active is not None
                and active.get("state", active.get("status")) == "active"
                and first_generation not in by_id
                and abandoned_generation not in by_id
            )

        wait_for_condition(
            expired_generations_cleaned_in_batches,
            timeout=float(
                os.getenv("ATTUNE_E2E_CACHE_MAINTENANCE_TIMEOUT_SECONDS", "45")
            ),
            error_message=(
                "Supervisor did not retain the active generation and clean expired "
                f"retired/abandoned generations: namespace={namespace} "
                f"refresh={second_refresh}"
            ),
        )
