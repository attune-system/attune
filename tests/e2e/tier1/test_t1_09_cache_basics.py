"""T1.9: Public cache namespace, refresh, and read contract."""

import pytest

from e2e.cache_helpers import (
    CACHE_OWNER_TYPE,
    assertion_id,
    cache_entries,
    cache_namespace,
    high_entropy_sentinel,
    publish_generation,
    scan_all,
)
from helpers import AttuneClient

pytestmark = pytest.mark.usefixtures("cache_e2e_retention_config")


def _entry(response: dict) -> dict | None:
    return response.get("item", response.get("entry"))


@pytest.mark.tier1
@pytest.mark.integration
@pytest.mark.cache
def test_cache_namespace_refresh_exact_multi_and_bytewise_scan(
    client: AttuneClient, pack_ref: str
):
    """Exercise a complete small cache snapshot without direct database access."""
    sentinel = high_entropy_sentinel()
    with cache_namespace(
        client,
        owner_ref=pack_ref,
        prefix="users",
        policy={"max_records_per_generation": 20, "max_generation_bytes": 65536},
    ) as users_namespace:
        records = [
            {"external_id": "Z-user", "value": {"ordinal": 0, "sentinel": sentinel}},
            {"external_id": "a-user", "value": {"ordinal": 1, "sentinel": sentinel}},
            {"external_id": "A-user", "value": {"ordinal": 2, "sentinel": sentinel}},
            {"external_id": "aa-user", "value": {"ordinal": 3, "sentinel": sentinel}},
        ]
        active_generation, refresh_id = publish_generation(
            client,
            owner_ref=pack_ref,
            namespace=users_namespace,
            entries=records,
            expected_active_generation_id=None,
            chunk_size=2,
        )
        assertion = assertion_id(users_namespace, refresh_id)

        exact = client.cache_lookup(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=users_namespace,
            external_id="a-user",
        )
        assert exact.get("generation_id", exact.get("generation")) == active_generation, assertion
        exact_entry = _entry(exact)
        assert exact_entry["external_id"] == records[1]["external_id"], assertion
        assert exact_entry["value"] == records[1]["value"], assertion

        missing = client.cache_lookup(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=users_namespace,
            external_id="does-not-exist",
        )
        assert _entry(missing) is None, assertion

        multi = client.cache_lookup_many(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=users_namespace,
            external_ids=["aa-user", "does-not-exist", "Z-user"],
        )
        returned = multi.get("items", multi.get("entries", []))
        assert {entry["external_id"] for entry in returned} == {"aa-user", "Z-user"}, assertion
        assert multi.get("missing_external_ids", multi.get("missing_ids")) == [
            "does-not-exist"
        ], assertion

        scanned_generation, scanned, _ = scan_all(
            client,
            owner_ref=pack_ref,
            namespace=users_namespace,
            page_size=2,
        )
        scanned_ids = [entry["external_id"] for entry in scanned]
        assert scanned_generation == active_generation, assertion
        assert scanned_ids == sorted(scanned_ids, key=lambda value: value.encode("utf-8")), assertion
        assert len(scanned_ids) == len(set(scanned_ids)) == len(records), assertion
        assert all(entry["value"]["sentinel"] == sentinel for entry in scanned), assertion

        with cache_namespace(
            client,
            owner_ref=pack_ref,
            prefix="locations",
            policy={"max_records_per_generation": 20, "max_generation_bytes": 65536},
        ) as locations_namespace:
            location_records = cache_entries(1, prefix="a-user", revision="locations")
            location_records[0]["external_id"] = "a-user"
            location_generation, location_refresh = publish_generation(
                client,
                owner_ref=pack_ref,
                namespace=locations_namespace,
                entries=location_records,
                expected_active_generation_id=None,
            )
            location_lookup = client.cache_lookup(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=locations_namespace,
                external_id="a-user",
            )
            assert location_lookup.get("generation_id", location_lookup.get("generation")) == location_generation, (
                assertion_id(locations_namespace, location_refresh)
            )
            assert _entry(location_lookup)["value"]["revision"] == "locations", (
                assertion_id(locations_namespace, location_refresh)
            )
            assert _entry(exact)["value"]["ordinal"] == 1, assertion

        namespaces = client.cache_list_namespaces(
            owner_type=CACHE_OWNER_TYPE, owner_ref=pack_ref
        )
        namespace_response = next(
            item for item in namespaces if item.get("namespace") == users_namespace
        )
        assert namespace_response.get("namespace") == users_namespace, assertion
        assert all(
            forbidden not in repr(namespace_response).lower()
            for forbidden in ("credential", "secret_delivery", "api_token", "password")
        ), assertion
